//! Filesystem boundary for lifecycle operations. No workspace or delivery is
//! adopted by a directory type check; persistent identity remains separate.
use crate::{file_identity::PersistedIdentity, path_checks::check_no_symlinks, ApiError};
use std::path::Path;

#[cfg(unix)]
#[path = "lifecycle_file_io/unix.rs"]
mod unix;
#[cfg(windows)]
#[path = "lifecycle_file_io/windows.rs"]
mod windows;

pub(crate) fn require_directory(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, false)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ApiError::new(
            "UNSAFE_PATH",
            "Expected an ordinary project directory",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportPublicationPoint {
    BeforeRename,
    Renamed,
}

pub(crate) fn publish_export_file_with(
    stage: &Path,
    destination: &Path,
    expected: &PersistedIdentity,
    checkpoint: impl FnMut(ExportPublicationPoint) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    #[cfg(unix)]
    {
        unix::publish_export_file_with(stage, destination, expected, checkpoint)
    }
    #[cfg(windows)]
    {
        windows::publish_export_file_with(stage, destination, expected, checkpoint)
    }
}

#[cfg(all(test, windows))]
#[path = "lifecycle_file_io/windows_tests.rs"]
mod tests;
