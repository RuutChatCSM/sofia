//! Lossless, content-addressed storage for large rollout images.
//!
//! Rollout JSONL records can contain image data URLs (computer-use screenshots, pasted images,
//! MCP image results, and the copies retained inside compaction and projection records). Writing
//! those bytes once and replacing each large occurrence with a reference keeps rollouts small
//! without changing what any reader observes.
//!
//! # On-disk contract
//!
//! * Blob file: `<sofia_home>/blobs/sha256/<first 2 hex chars>/<64 lowercase hex chars>`.
//! * Blob contents: the exact UTF-8 data URL bytes (`data:image/...;base64,...`), so resolving a
//!   reference reproduces the original string byte-for-byte.
//! * Reference string: `sofia-blob:sha256:<64 lowercase hex chars>`, where the digest is the
//!   SHA-256 of the blob file contents.
//!
//! The reference format and path layout are part of the cross-language contract with the desktop
//! app, which reads rollout lines directly. Do not change either without updating that reader.
//!
//! Only strings longer than [`MIN_EXTERNAL_IMAGE_BYTES`] that are `data:image/...;base64,` URLs
//! are externalized; small images stay inline. Externalization happens only while serializing a
//! rollout line, so live events, model requests, and the SQLite projection continue to see the
//! original data URLs.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::RwLock;

use serde_json::Value;
use sha2::Digest;
use tempfile::NamedTempFile;

use crate::ARCHIVED_SESSIONS_SUBDIR;
use crate::SESSIONS_SUBDIR;

/// Prefix of every image blob reference.
pub const BLOB_REFERENCE_PREFIX: &str = "sofia-blob:sha256:";
/// Marker used to distinguish image-blob decode failures from ordinary parse errors.
pub const IMAGE_BLOB_ERROR_PREFIX: &str = "image blob: ";
/// Images at or below this length (including the data URL prefix) stay inline.
pub const MIN_EXTERNAL_IMAGE_BYTES: usize = 16 * 1024;

const HASH_ALGORITHM: &str = "sha256";
const HASH_HEX_LEN: usize = 64;
const BLOBS_SUBDIR: &str = "blobs";

static REGISTERED_HOMES: LazyLock<RwLock<Vec<PathBuf>>> = LazyLock::new(|| RwLock::new(Vec::new()));

/// Resolves image blob references against one or more Sofia homes.
///
/// Writers use [`Self::for_home`] so blobs land under the configured home. Readers use
/// [`Self::for_rollout_path`], which derives the home from the rollout location. The default store
/// additionally consults homes registered by live recorders in this process, which keeps in-process
/// tests and tooling working when they parse a rollout line without a path.
#[derive(Clone, Debug)]
pub(crate) struct ImageBlobStore {
    roots: Vec<PathBuf>,
}

impl ImageBlobStore {
    /// Store blobs under `sofia_home/blobs/sha256`.
    pub(crate) fn for_home(sofia_home: &Path) -> Self {
        Self {
            roots: vec![blobs_root(sofia_home)],
        }
    }

    /// Resolve blobs next to a rollout file by deriving its Sofia home.
    pub(crate) fn for_rollout_path(rollout_path: &Path) -> Self {
        match sofia_home_for_rollout_path(rollout_path) {
            Some(home) => Self::for_home(home.as_path()),
            None => Self::default_roots(),
        }
    }

    /// Homes registered by live recorders, falling back to the environment-derived home.
    pub(crate) fn default_roots() -> Self {
        let mut homes = registered_homes();
        if homes.is_empty()
            && let Ok(home) = sofia_utils_home_dir::find_codex_home()
        {
            homes.push(home.to_path_buf());
        }
        Self {
            roots: homes
                .into_iter()
                .map(|home| blobs_root(home.as_path()))
                .collect(),
        }
    }

    /// Store `data_url` if it is large enough and return its blob reference.
    fn store_data_url(&self, data_url: &str) -> io::Result<String> {
        let root = self.roots.first().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "cannot locate a Sofia home for image blobs",
            )
        })?;
        let hash = sha256_hex(data_url.as_bytes());
        let path = root.join(&hash[..2]).join(&hash);
        if !path.exists() {
            let parent = path.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "image blob path has no parent")
            })?;
            fs::create_dir_all(parent)?;
            let mut temp = NamedTempFile::new_in(parent)?;
            temp.write_all(data_url.as_bytes())?;
            temp.flush()?;
            match temp.persist_noclobber(path.as_path()) {
                Ok(_) => {}
                Err(err) if err.error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err.error),
            }
        }
        Ok(format!("{BLOB_REFERENCE_PREFIX}{hash}"))
    }

    /// Read the data URL for `reference`, verifying its content hash.
    fn resolve_reference(&self, reference: &str) -> io::Result<String> {
        let hash = parse_reference(reference)?;
        let mut searched = Vec::new();
        for root in &self.roots {
            let path = root.join(&hash[..2]).join(hash);
            searched.push(path.clone());
            let bytes = match fs::read(path.as_path()) {
                Ok(bytes) => bytes,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            if sha256_hex(bytes.as_slice()) != hash {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "image blob {reference} has unexpected contents at {}",
                        path.display()
                    ),
                ));
            }
            let data_url = String::from_utf8(bytes).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("image blob {reference} is not valid UTF-8: {err}"),
                )
            })?;
            if !is_image_data_url(data_url.as_str()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("image blob {reference} does not contain an image data URL"),
                ));
            }
            return Ok(data_url);
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "missing image blob {reference}; searched {}",
                searched
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ))
    }
}

/// Remember a configured Sofia home so path-less decoders in this process can find its blobs.
pub(crate) fn register_sofia_home(sofia_home: &Path) {
    let mut homes = REGISTERED_HOMES
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let home = sofia_home.to_path_buf();
    if !homes.contains(&home) {
        homes.push(home);
    }
}

fn registered_homes() -> Vec<PathBuf> {
    REGISTERED_HOMES
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn blobs_root(sofia_home: &Path) -> PathBuf {
    sofia_home.join(BLOBS_SUBDIR).join(HASH_ALGORITHM)
}

/// Derive the home from `<home>/sessions/...` or `<home>/archived_sessions/...` rollout paths.
fn sofia_home_for_rollout_path(rollout_path: &Path) -> Option<PathBuf> {
    let sessions = OsStr::new(SESSIONS_SUBDIR);
    let archived = OsStr::new(ARCHIVED_SESSIONS_SUBDIR);
    rollout_path.ancestors().find_map(|ancestor| {
        let name = ancestor.file_name()?;
        if name == sessions || name == archived {
            ancestor.parent().map(Path::to_path_buf)
        } else {
            None
        }
    })
}

fn parse_reference(reference: &str) -> io::Result<&str> {
    let Some(rest) = reference.strip_prefix("sofia-blob:") else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported image blob reference {reference}"),
        ));
    };
    let Some(hash) = rest.strip_prefix("sha256:") else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported image blob reference {reference}"),
        ));
    };
    if hash.len() != HASH_HEX_LEN
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("malformed image blob reference {reference}"),
        ));
    }
    Ok(hash)
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = sha2::Sha256::digest(bytes);
    let mut hash = String::with_capacity(HASH_HEX_LEN);
    for byte in digest {
        hash.push(HEX[usize::from(byte >> 4)] as char);
        hash.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    hash
}

fn is_image_data_url(value: &str) -> bool {
    value
        .strip_prefix("data:image/")
        .is_some_and(|rest| rest.contains(";base64,"))
}

fn is_externalizable_image_data_url(value: &str) -> bool {
    value.len() > MIN_EXTERNAL_IMAGE_BYTES && is_image_data_url(value)
}

/// Replace large image data URLs in `value` with blob references.
///
/// This mutates only the serialized JSON copy. In-memory rollout items are never changed.
pub(crate) fn externalize_images_in_value(
    value: &mut Value,
    blobs: &ImageBlobStore,
) -> io::Result<()> {
    match value {
        Value::String(text) if is_externalizable_image_data_url(text) => {
            let reference = blobs.store_data_url(text.as_str())?;
            *text = reference;
        }
        Value::Array(items) => {
            for item in items {
                externalize_images_in_value(item, blobs)?;
            }
        }
        Value::Object(fields) => {
            for field in fields.values_mut() {
                externalize_images_in_value(field, blobs)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

/// Replace every image blob reference in `value` with its original data URL.
pub(crate) fn resolve_images_in_value(value: &mut Value, blobs: &ImageBlobStore) -> io::Result<()> {
    match value {
        Value::String(text) if text.starts_with("sofia-blob:") => {
            *text = blobs.resolve_reference(text.as_str())?;
        }
        Value::Array(items) => {
            for item in items {
                resolve_images_in_value(item, blobs)?;
            }
        }
        Value::Object(fields) => {
            for field in fields.values_mut() {
                resolve_images_in_value(field, blobs)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

/// Returns true when a decode error came from resolving an image blob reference.
pub fn is_image_blob_error(error: &serde_json::Error) -> bool {
    error.to_string().starts_with(IMAGE_BLOB_ERROR_PREFIX)
}

/// Returns true when a stringified decode error came from resolving an image blob reference.
pub fn is_image_blob_error_message(message: &str) -> bool {
    message.starts_with(IMAGE_BLOB_ERROR_PREFIX)
}

#[cfg(test)]
#[path = "image_blobs_tests.rs"]
mod tests;
