//! Durable backend process identity, independent of application registration.
//! Unknown or replaced start intents never authorize numeric-PID termination.
use crate::{node_runtime_trial::NodeRuntimeTrial, ApiError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;
type Result<T> = std::result::Result<T, ApiError>;
const START_INTENT_CLEANUP_PREFIX: &str = ".start-intent-cleanup-";
fn fail(message: impl Into<String>) -> ApiError {
    ApiError::new("APP_RUN_IDENTITY_INVALID", message)
}
#[cfg(unix)]
#[path = "backend_run_marker/unix.rs"]
mod file_io;
#[cfg(windows)]
#[path = "backend_run_marker/windows.rs"]
mod file_io;
use file_io::{ordinary_path, read_small, StartIntentOwnership};
#[path = "backend_run_marker/inspection.rs"]
mod run_marker;

pub(crate) struct RunGuard {
    slot: PathBuf,
    marker: run_marker::RunMarker,
    owner: StartIntentOwnership,
}
impl RunGuard {
    /// Must be persisted before the first backend process may be spawned.
    pub(crate) fn begin(slot: &Path, id: &str, run_directory: PathBuf) -> Result<Self> {
        if !matches!(
            run_marker::inspect(slot, id, run_marker::boot_time_or_unknown())?.state,
            run_marker::RunState::Absent
        ) {
            return Err(run_marker::unknown());
        }
        let marker = run_marker::RunMarker::launching(id, run_directory);
        let owner = StartIntentOwnership::create(&slot.join("active.json"), &marker)?;
        Ok(Self {
            slot: slot.into(),
            marker,
            owner,
        })
    }
    pub(crate) fn bind(&mut self, runtime: &NodeRuntimeTrial) -> Result<()> {
        self.marker.bind(runtime.process_identity()?);
        self.owner
            .publish(&self.slot.join("active.json"), &self.marker)
    }
    /// Caller must first positively observe closure of its actual owned process.
    pub(crate) fn retire(self) -> Result<()> {
        self.owner.remove_if_owned(&self.slot.join("active.json"))
    }
}
/// Used before a new launch and before deleting a closed application's files.
/// This verifies OS identity; a PID or the absence of an in-memory owner is insufficient.
pub(crate) async fn recover(slot: &Path, id: &str) -> Result<()> {
    run_marker::inspect(slot, id, run_marker::boot_time_or_unknown())?
        .stop_and_retire(slot)
        .await
}
pub(crate) fn status(slot: &Path, id: &str) -> Result<Option<Value>> {
    Ok(run_marker::inspect(slot, id, run_marker::boot_time_or_unknown())?.view())
}
/// Workspace switching without a native host must not move a live backend tree.
pub(crate) fn require_data_drained(data: &Path) -> Result<()> {
    let apps = data.join("apps");
    crate::projects::check_no_symlinks(&apps, true)?;
    if !apps.try_exists()? {
        return Ok(());
    }
    for entry in fs::read_dir(apps)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if uuid::Uuid::parse_str(&id).is_err() {
            continue;
        }
        ordinary_path(&entry.path(), true)?;
        match run_marker::inspect(&entry.path(), &id, run_marker::boot_time_or_unknown())?.state {
            run_marker::RunState::Absent | run_marker::RunState::Stopped => {}
            _ => return Err(run_marker::unknown()),
        }
    }
    Ok(())
}
