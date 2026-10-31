//! Workspace filesystem policy, copy inventory and native publication boundary.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    pub(crate) path: String,
    pub(crate) kind: String,
    pub(crate) bytes: u64,
    pub(crate) digest: Option<String>,
    pub(crate) target: Option<PathBuf>,
    #[cfg(unix)]
    pub(crate) mode: u32,
    #[cfg(windows)]
    pub(crate) read_only: bool,
}

#[cfg(unix)]
#[path = "workspace_file_io/unix.rs"]
mod unix;
#[cfg(windows)]
#[path = "workspace_file_io/windows.rs"]
mod windows;

pub(crate) fn filesystem(path: &Path) -> Result<Value> {
    #[cfg(unix)]
    {
        unix::filesystem(path)
    }
    #[cfg(windows)]
    {
        let info = platform_native::windows::volume::inspect(path)?;
        Ok(serde_json::json!({
            "type": info.filesystem,
            "local": info.local(),
            "availableBytes": info.available_bytes,
            "totalBytes": info.total_bytes,
            "driveType": info.drive_type,
            "internal": info.internal,
            "internalDiskNumbers": info.internal_disk_numbers,
            "internalDiagnostic": info.internal_diagnostic,
        }))
    }
}
pub(crate) fn require_unsynced(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        unix::require_unsynced(path)
    }
    #[cfg(windows)]
    {
        platform_native::windows::workspace::require_unsynced(path).map_err(Into::into)
    }
}
pub(crate) fn require_supported_filesystem(info: &Value) -> Result<()> {
    #[cfg(unix)]
    {
        unix::require_supported_filesystem(info)
    }
    #[cfg(windows)]
    {
        windows::require_supported_filesystem(info)
    }
}
pub(crate) fn collect(root: &Path, excluded: &BTreeSet<PathBuf>) -> Result<Vec<Entry>> {
    #[cfg(unix)]
    {
        unix::collect(root, excluded)
    }
    #[cfg(windows)]
    {
        windows::collect(root, excluded)
    }
}
pub(crate) fn copy_entries(source: &Path, target: &Path, entries: &[Entry]) -> Result<()> {
    #[cfg(unix)]
    {
        unix::copy_entries(source, target, entries)
    }
    #[cfg(windows)]
    {
        windows::copy_entries(source, target, entries)
    }
}
pub(crate) fn append_phase_log(path: &Path, phase: &str) -> Result<(), crate::ApiError> {
    #[cfg(unix)]
    {
        unix::append_phase_log(path, phase)
    }
    #[cfg(windows)]
    {
        windows::append_phase_log(path, phase)
    }
}
#[cfg(all(test, windows))]
#[path = "workspace_file_io/windows_tests.rs"]
mod tests;
