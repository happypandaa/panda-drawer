//! Artifact directory exchange. Only the caller's exact staged/current pair is
//! authorized here; this module neither selects a project nor performs cleanup.
use crate::ApiError;
use std::path::Path;

#[cfg(not(windows))]
#[path = "build_directory_exchange/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "build_directory_exchange/windows.rs"]
mod platform;

pub(crate) fn replace_directory(staged: &Path, current: &Path) -> Result<(), ApiError> {
    platform::replace_directory(staged, current)
}
