use dirs::home_dir;
use sofia_utils_absolute_path::AbsolutePathBuf;
use std::path::PathBuf;

/// Returns the path to the Sofia configuration directory, which can be
/// specified by the `SOFIA_HOME` environment variable. If not set, defaults to
/// `~/.sofia`.
///
/// Sofia is intentionally isolated from OpenAI's Sofia/`~/.sofia` so the two
/// never share (or clobber) state.
///
/// - If `SOFIA_HOME` is set, the value must exist and be a directory. The
///   value will be canonicalized and this function will Err otherwise.
/// - If `SOFIA_HOME` is not set, this function does not verify that the
///   directory exists.
pub fn find_codex_home() -> std::io::Result<AbsolutePathBuf> {
    let sofia_home_env = std::env::var("SOFIA_HOME")
        .ok()
        .filter(|val| !val.is_empty());
    find_codex_home_from_env(sofia_home_env.as_deref())
}

/// Returns the Sofia home directory as a plain `String`, suitable for
/// string interpolation in path construction. Falls back to `~/.sofia`
/// when the canonical resolution fails (e.g. SOFIA_HOME unset and the
/// home directory is unavailable). Prefer `find_codex_home()` when you
/// need an absolute, canonicalized path.
pub fn sofia_home_string() -> String {
    find_codex_home()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_else(|_| "/tmp".to_string());
            format!("{home}/.sofia")
        })
}

fn find_codex_home_from_env(sofia_home_env: Option<&str>) -> std::io::Result<AbsolutePathBuf> {
    // Honor the `SOFIA_HOME` environment variable when it is set to allow users
    // (and tests) to override the default location.
    match sofia_home_env {
        Some(val) => {
            let path = PathBuf::from(val);
            let metadata = std::fs::metadata(&path).map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("SOFIA_HOME points to {val:?}, but that path does not exist"),
                ),
                _ => std::io::Error::new(
                    err.kind(),
                    format!("failed to read SOFIA_HOME {val:?}: {err}"),
                ),
            })?;

            if !metadata.is_dir() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("SOFIA_HOME points to {val:?}, but that path is not a directory"),
                ))
            } else {
                let canonical = path.canonicalize().map_err(|err| {
                    std::io::Error::new(
                        err.kind(),
                        format!("failed to canonicalize SOFIA_HOME {val:?}: {err}"),
                    )
                })?;
                AbsolutePathBuf::from_absolute_path(canonical)
            }
        }
        None => {
            let mut p = home_dir().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Could not find home directory",
                )
            })?;
            p.push(".sofia");
            AbsolutePathBuf::from_absolute_path(p)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::find_codex_home_from_env;
    use dirs::home_dir;
    use pretty_assertions::assert_eq;
    use sofia_utils_absolute_path::AbsolutePathBuf;
    use std::fs;
    use std::io::ErrorKind;
    use tempfile::TempDir;

    #[test]
    fn find_codex_home_env_missing_path_is_fatal() {
        let temp_home = TempDir::new().expect("temp home");
        let missing = temp_home.path().join("missing-sofia-home");
        let missing_str = missing
            .to_str()
            .expect("missing sofia home path should be valid utf-8");

        let err = find_codex_home_from_env(Some(missing_str)).expect_err("missing SOFIA_HOME");
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(
            err.to_string().contains("SOFIA_HOME"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_codex_home_env_file_path_is_fatal() {
        let temp_home = TempDir::new().expect("temp home");
        let file_path = temp_home.path().join("sofia-home.txt");
        fs::write(&file_path, "not a directory").expect("write temp file");
        let file_str = file_path
            .to_str()
            .expect("file sofia home path should be valid utf-8");

        let err = find_codex_home_from_env(Some(file_str)).expect_err("file SOFIA_HOME");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("not a directory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_codex_home_env_valid_directory_canonicalizes() {
        let temp_home = TempDir::new().expect("temp home");
        let temp_str = temp_home
            .path()
            .to_str()
            .expect("temp sofia home path should be valid utf-8");

        let resolved = find_codex_home_from_env(Some(temp_str)).expect("valid SOFIA_HOME");
        let expected = temp_home
            .path()
            .canonicalize()
            .expect("canonicalize temp home");
        let expected = AbsolutePathBuf::from_absolute_path(expected).expect("absolute home");
        assert_eq!(resolved, expected);
    }

    #[test]
    fn find_codex_home_without_env_uses_default_home_dir() {
        let resolved =
            find_codex_home_from_env(/*sofia_home_env*/ None).expect("default SOFIA_HOME");
        let mut expected = home_dir().expect("home dir");
        expected.push(".sofia");
        let expected = AbsolutePathBuf::from_absolute_path(expected).expect("absolute home");
        assert_eq!(resolved, expected);
    }
}
