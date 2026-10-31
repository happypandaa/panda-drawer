//! Native preparation handle lifetimes; Unix retains its original descriptors.
use super::*;
pub(super) fn require_supported() -> Result<(), ApiError> {
    if cfg!(any(target_os = "macos", windows)) {
        Ok(())
    } else {
        Err(error("UNSUPPORTED_PLATFORM", "数据取回目前仅验证 macOS。"))
    }
}
pub(super) fn release_preparation_handles(
    handles: (File, File, File, File),
) -> Option<(File, File, File, File)> {
    #[cfg(unix)]
    {
        Some(handles)
    }
    #[cfg(windows)]
    {
        drop(handles);
        None
    }
}
