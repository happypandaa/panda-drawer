//! Cache names are selected by the original storage policy. Native identity
//! checks bind one actual file; no parent DACL or single-link admission is added.
use crate::{projects::check_no_symlinks, ApiError};
use platform_native::windows::{files as native, removal};
use std::{fs, path::Path, time::Duration};

fn require_absent(path: &Path) -> Result<(), ApiError> {
    for delay in [0, 20, 40, 80, 160, 320, 640, 1000] {
        if delay != 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if !matches!(error.raw_os_error(), Some(5 | 32 | 33)) => {
                return Err(error.into())
            }
            _ => (),
        }
    }
    Err(ApiError::new(
        "STORAGE_CACHE_REMOVE_PENDING",
        "空间缓存仍被占用或无法确认已移除，请关闭占用后重试。",
    )
    .detail("remainingPath", serde_json::json!(path)))
}

pub(super) fn remove_storage_cache(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    match native::open_no_reparse(path) {
        Ok(opened) if opened.file.metadata()?.is_file() => {
            let parent = native::pin_directory(
                path.parent()
                    .ok_or_else(|| ApiError::new("UNSAFE_PATH", "空间缓存没有父目录。"))?,
            )?;
            removal::remove(path, &opened.identity)?;
            // A successful disposition is only delete-pending. Close our own
            // HANDLE before waiting for the actual name to disappear.
            drop(opened);
            require_absent(path)?;
            native::sync_directory(&parent.file)?;
            Ok(())
        }
        Ok(_) => Err(ApiError::new("UNSAFE_PATH", "空间缓存不是普通文件。")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
