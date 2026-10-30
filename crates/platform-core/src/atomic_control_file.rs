//! Atomic private control metadata. This adapter only uses the supplied path;
//! it never discovers an application data root or grants project ownership.
use crate::{path_checks::check_no_symlinks, ApiError};
use serde::Serialize;
use std::{fs, io::Write, path::Path};
use uuid::Uuid;

pub(crate) fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::new("INVALID_PATH", "Missing parent directory"))?;
    crate::project_file_io::ensure_directory(parent)?;
    let temp = parent.join(format!(".write-{}", Uuid::new_v4().simple()));
    #[cfg(windows)]
    let mut unknown = false;
    #[cfg(unix)]
    let unknown = false;
    let result = (|| {
        let mut file = platform_native::filesystem::create_new_private_file(&temp, false)?;
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            fs::rename(&temp, path)?;
            fs::File::open(parent)?.sync_all()?;
        }
        #[cfg(windows)]
        {
            platform_native::windows::files::rename_file(&temp, path, true).map_err(|error| {
                unknown = error.kind() == std::io::ErrorKind::InvalidData;
                if unknown {
                    ApiError::from(error).outcome("unknown")
                } else {
                    error.into()
                }
            })?;
            // Windows cannot fsync a directory. The file was flushed above;
            // namespace recovery remains the caller's journal responsibility.
            platform_native::filesystem::sync_directory(parent)?;
        }
        Ok(())
    })();
    if result.is_err() && !unknown {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn control_publication_replaces_metadata_and_leaves_only_the_current_file() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-atomic-control-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let path = root.join("private").join("nested").join("control.json");
        atomic_json(&path, &json!({"disposable":1})).unwrap();
        atomic_json(&path, &json!({"disposable":2})).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap()).unwrap(),
            json!({"disposable":2})
        );
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        #[cfg(windows)]
        for item in [&path, &path.parent().unwrap().to_path_buf()] {
            let opened = platform_native::windows::files::open_no_reparse(item).unwrap();
            platform_native::windows::files::verify_private(&opened.file).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn occupied_control_file_preserves_old_metadata_then_retries_after_release() {
        use std::os::windows::fs::OpenOptionsExt;
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-atomic-control-occupied-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let path = root.join("control.json");
        atomic_json(&path, &json!({"disposable":"old"})).unwrap();
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(atomic_json(&path, &json!({"disposable":"new"})).is_err());
        drop(held);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap()).unwrap(),
            json!({"disposable":"old"})
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        atomic_json(&path, &json!({"disposable":"new"})).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap(),
            json!({"disposable":"new"})
        );
    }
}
