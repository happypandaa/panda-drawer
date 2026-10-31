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

/// Complete only a journal bound to this exact pair. False means no active
/// exchange journal exists; it does not prove any outer build has committed.
pub(crate) fn resume_pending(staged: &Path, current: &Path) -> Result<bool, ApiError> {
    #[cfg(windows)]
    {
        platform::resume_pending(staged, current)
    }
    #[cfg(not(windows))]
    {
        let _ = (staged, current);
        Ok(false)
    }
}

/// Read-only query. Invalid, locked or conflicting material returns an error,
/// never false. Callers must preserve that state and must not start rollback.
pub(crate) fn pending(staged: &Path, current: &Path) -> Result<bool, ApiError> {
    #[cfg(windows)]
    {
        platform::pending(staged, current)
    }
    #[cfg(not(windows))]
    {
        let _ = (staged, current);
        Ok(false)
    }
}
