//! Bound native directory removal and current migration absence checks.
use crate::{
    file_identity::PersistedIdentity,
    projects::{check_no_symlinks, valid_id},
    ApiError, RuntimePaths,
};
use platform_native::windows::files as native;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[path = "windows/removal.rs"]
mod removal;
fn error(message: impl std::fmt::Display) -> ApiError {
    ApiError::new("WORKSPACE_CLEANUP_BLOCKED", message.to_string())
}

pub(super) fn read_json(path: &Path) -> Result<Value, ApiError> {
    check_no_symlinks(path, false)?;
    let file = native::open_read_file_allowing_hard_links(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > 64_000_000 {
        return Err(error("Invalid cleanup metadata"));
    }
    Ok(serde_json::from_reader(file)?)
}
pub(crate) fn retired_cleanup_status(
    paths: &RuntimePaths,
    migration_id: &str,
) -> Result<Option<Value>, ApiError> {
    if !valid_id(migration_id) {
        return Err(error("Invalid migration identity"));
    }
    let journal = read_json(
        &paths
            .migrations_dir()
            .join(migration_id)
            .join("journal.json"),
    )?;
    let Some(source) = journal["sourcePath"].as_str().map(PathBuf::from) else {
        return Ok(None);
    };
    if !matches!(fs::symlink_metadata(&source), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(None);
    }
    let expected: PersistedIdentity = serde_json::from_value(journal["sourceIdentity"].clone())?;
    if !expected
        .absent_on_expected_volume(&source)
        .map_err(|failure| error(failure.to_string()))?
    {
        return Ok(None);
    }
    Ok(Some(
        json!({"completed":false,"status":"absent","accountingComplete":false}),
    ))
}
pub(crate) fn remove_owned_directory(
    path: &Path,
    expected: &PersistedIdentity,
) -> Result<Value, ApiError> {
    removal::remove(path, expected)
}
pub(crate) fn remove_owned_directory_checked(
    path: &Path,
    expected: &PersistedIdentity,
    verify: impl FnOnce(&Value) -> Result<(), ApiError>,
) -> Result<Value, ApiError> {
    removal::remove_checked(path, expected, verify)
}
