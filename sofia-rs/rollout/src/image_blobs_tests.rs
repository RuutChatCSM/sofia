#![allow(warnings, clippy::all)]

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use sofia_protocol::models::ContentItem;
use sofia_protocol::models::ResponseItem;
use sofia_protocol::protocol::ThreadHistoryMode;
use tempfile::TempDir;

use super::externalize_images_in_value;
use super::resolve_images_in_value;
use crate::RolloutItem;
use crate::is_image_blob_error;

fn rollout_path(home: &Path) -> PathBuf {
    home.join("sessions/2026/01/01/rollout-image-blob-test.jsonl")
}

fn line_with_image(image_url: &str) -> Value {
    json!({
        "timestamp": "2026-01-01T00:00:00Z",
        "ordinal": 0,
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_image",
                "image_url": image_url,
            }],
        },
    })
}

fn image_url_from_line(value: &Value) -> String {
    let decoded = crate::decode_rollout_line(value.clone()).expect("decode rollout line");
    let RolloutItem::ResponseItem(envelope) = decoded.item else {
        panic!("expected response item");
    };
    let ResponseItem::Message { content, .. } = envelope.item else {
        panic!("expected message");
    };
    content
        .into_iter()
        .find_map(|item| match item {
            ContentItem::InputImage { image_url, .. } => Some(image_url),
            _ => None,
        })
        .expect("expected input image")
}

fn large_image_data_url() -> String {
    format!("data:image/png;base64,{}", "A".repeat(20 * 1024))
}

fn blob_files(home: &Path) -> Vec<PathBuf> {
    let root = home.join("blobs/sha256");
    let Ok(directories) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for directory in directories {
        let directory = directory.expect("blob shard entry");
        for file in fs::read_dir(directory.path()).expect("blob shard") {
            files.push(file.expect("blob file").path());
        }
    }
    files
}

#[test]
fn large_image_round_trips_through_one_shared_blob() {
    let home = TempDir::new().expect("temp home");
    let store = super::ImageBlobStore::for_home(home.path());
    let data_url = large_image_data_url();
    let original = line_with_image(data_url.as_str());

    let mut first_line = original.clone();
    let mut second_line = original.clone();
    externalize_images_in_value(&mut first_line, &store).expect("externalize first line");
    externalize_images_in_value(&mut second_line, &store).expect("externalize second line");

    assert_eq!(
        first_line["payload"]["content"][0]["image_url"],
        second_line["payload"]["content"][0]["image_url"],
        "duplicate images should share one reference"
    );
    let reference = first_line["payload"]["content"][0]["image_url"]
        .as_str()
        .expect("reference string");
    assert!(reference.starts_with(super::BLOB_REFERENCE_PREFIX));
    assert_eq!(reference.len(), super::BLOB_REFERENCE_PREFIX.len() + 64);

    let files = blob_files(home.path());
    assert_eq!(
        files.len(),
        1,
        "duplicate images should write one blob file"
    );
    let file_name = files[0]
        .file_name()
        .and_then(|name| name.to_str())
        .expect("blob file name");
    assert_eq!(file_name.len(), 64);
    assert!(
        file_name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    let shard = files[0]
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .expect("blob shard name");
    assert_eq!(shard, &file_name[..2]);
    assert_eq!(
        fs::read_to_string(&files[0]).expect("read blob"),
        data_url,
        "blobs store the exact data URL bytes"
    );

    let path = rollout_path(home.path());
    let first = crate::decode_rollout_line_for_rollout(first_line, path.as_path())
        .expect("decode first line");
    let second = crate::decode_rollout_line_for_rollout(second_line, path.as_path())
        .expect("decode second line");
    assert_eq!(
        serde_json::to_value(&first.item).expect("serialize first"),
        serde_json::to_value(&second.item).expect("serialize second")
    );
    assert_eq!(
        serde_json::to_value(&first.item).expect("serialize first"),
        json!({
            "type": "response_item",
            "payload": original["payload"].clone(),
        })
    );
}

#[test]
fn small_images_stay_inline() {
    let home = TempDir::new().expect("temp home");
    let store = super::ImageBlobStore::for_home(home.path());
    let data_url = format!("data:image/png;base64,{}", "A".repeat(1024));
    let mut line = line_with_image(data_url.as_str());

    externalize_images_in_value(&mut line, &store).expect("externalize small image");

    assert_eq!(line["payload"]["content"][0]["image_url"], json!(data_url));
    assert!(blob_files(home.path()).is_empty());
}

#[test]
fn missing_blob_is_a_clear_decode_error() {
    let home = TempDir::new().expect("temp home");
    let store = super::ImageBlobStore::for_home(home.path());
    let mut line = line_with_image(large_image_data_url().as_str());
    externalize_images_in_value(&mut line, &store).expect("externalize line");
    let files = blob_files(home.path());
    assert_eq!(files.len(), 1);
    fs::remove_file(&files[0]).expect("remove blob");

    let error =
        match crate::decode_rollout_line_for_rollout(line, rollout_path(home.path()).as_path()) {
            Ok(_) => panic!("missing blob must fail"),
            Err(error) => error,
        };
    assert!(is_image_blob_error(&error));
    assert!(
        error.to_string().contains("missing image blob"),
        "unexpected error: {error}"
    );
}

#[test]
fn inline_old_format_lines_decode_unchanged() {
    let data_url = large_image_data_url();
    let line = line_with_image(data_url.as_str());

    assert_eq!(image_url_from_line(&line), data_url);
}

#[test]
fn malformed_reference_is_rejected() {
    let home = TempDir::new().expect("temp home");
    let mut line = line_with_image("sofia-blob:sha256:not-a-hash");
    let error = resolve_images_in_value(&mut line, &super::ImageBlobStore::for_home(home.path()))
        .expect_err("malformed reference must fail");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn history_mode_guard_round_trips_and_still_rejects_unknown_future_modes() {
    assert!(ThreadHistoryMode::is_image_blob_guard("legacy_blob_v1"));
    assert!(ThreadHistoryMode::is_image_blob_guard("paginated_blob_v1"));
    assert!(!ThreadHistoryMode::is_image_blob_guard("paginated_blob_v2"));

    let guarded = json!({"type": "session_meta", "payload": {"history_mode": "paginated_blob_v1"}});
    crate::recorder::reject_unknown_thread_history_mode(&guarded)
        .expect("current binary accepts the guard");
    assert_eq!(
        serde_json::from_value::<ThreadHistoryMode>(guarded["payload"]["history_mode"].clone())
            .expect("guard decodes to the underlying mode"),
        ThreadHistoryMode::Paginated
    );

    let future = json!({"type": "session_meta", "payload": {"history_mode": "paginated_blob_v2"}});
    assert!(
        crate::recorder::reject_unknown_thread_history_mode(&future).is_err(),
        "an older binary rejects the guard; a future value must stay rejected too"
    );
}
