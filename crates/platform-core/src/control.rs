//! Durable maintenance control lives outside replaceable project roots.
use crate::{
    builds::atomic_json,
    projects::{check_no_symlinks, valid_id},
    ApiError,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ControlState {
    pub mode: String,
    pub maintenance_id: Option<String>,
    pub reason: Option<String>,
    pub started_at: Option<u64>,
    pub baseline_backup_id: Option<String>,
    pub protected_backup_ids: Vec<String>,
    pub ready_for_changes: bool,
    pub recovery_required: bool,
    pub baseline_input_digest: Option<String>,
    pub pending_build: bool,
    pub pending_build_reason: Option<String>,
    pub had_artifact: bool,
    pub last_error: Option<ApiError>,
    pub last_task_id: Option<String>,
}
impl Default for ControlState {
    fn default() -> Self {
        Self {
            mode: "ready".into(),
            maintenance_id: None,
            reason: None,
            started_at: None,
            baseline_backup_id: None,
            protected_backup_ids: vec![],
            ready_for_changes: false,
            recovery_required: false,
            baseline_input_digest: None,
            pending_build: false,
            pending_build_reason: None,
            had_artifact: false,
            last_error: None,
            last_task_id: None,
        }
    }
}
pub(crate) struct ProjectControl {
    path: PathBuf,
    state: Mutex<ControlState>,
    persist: bool,
    pub frozen: AtomicBool,
}
impl ProjectControl {
    pub fn load(workspace: &Path, id: &str) -> Result<Arc<Self>, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let path = workspace
            .join(".platform/project-state/maintenance")
            .join(format!("{id}.json"));
        check_no_symlinks(&path, true)?;
        let state = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ControlState::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Arc::new(Self {
            path,
            state: Mutex::new(state),
            persist: true,
            frozen: AtomicBool::new(false),
        }))
    }
    pub(crate) fn volatile_for_deletion(workspace: &Path, id: &str) -> Result<Arc<Self>, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        Ok(Arc::new(Self {
            path: workspace
                .join(".platform/project-state/maintenance")
                .join(format!("{id}.json")),
            state: Mutex::new(ControlState {
                mode: "unknown".into(),
                recovery_required: true,
                ..ControlState::default()
            }),
            persist: false,
            frozen: AtomicBool::new(false),
        }))
    }
    pub fn get(&self) -> ControlState {
        self.state.lock().unwrap().clone()
    }
    pub fn set(&self, state: ControlState) -> Result<(), ApiError> {
        if !self.persist {
            return Err(ApiError::new(
                "CONTROL_READ_ONLY",
                "Deletion-only control state cannot be persisted",
            ));
        }
        let mut current = self.state.lock().unwrap();
        atomic_json(&self.path, &state)?;
        *current = state;
        Ok(())
    }
    /// Fail closed in this process even when the disk cannot persist the gate.
    /// The unresolved operation journal will re-establish it on the next start.
    pub fn check(
        &self,
        browser: bool,
        maintenance: Option<&str>,
        diagnostic: bool,
    ) -> Result<(), ApiError> {
        let state = self.state.lock().unwrap();
        if diagnostic {
            return Ok(());
        }
        if state.recovery_required {
            return Err(ApiError::new(
                "RECOVERY_REQUIRED",
                "Recovery needs attention; inspect the operation journal",
            )
            .outcome("not_started"));
        }
        if self.frozen.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "PROJECT_BUSY",
                "Project snapshot or replacement is in progress; the request was not saved",
            )
            .outcome("not_started"));
        }
        if state.mode == "maintenance"
            && (browser
                || !state.ready_for_changes
                || maintenance != state.maintenance_id.as_deref())
        {
            return Err(ApiError::new("PROJECT_MAINTENANCE","Project is in maintenance; management changes need its ready baseline and current maintenanceId").outcome("not_started"));
        }

        if state.mode == "ready" && maintenance.is_some() {
            return Err(ApiError::new(
                "MAINTENANCE_ID_MISMATCH",
                "This maintenance session is no longer active",
            )
            .outcome("not_started"));
        }
        Ok(())
    }
}
pub(crate) struct Freeze(pub Arc<ProjectControl>);
impl Freeze {
    pub fn new(control: Arc<ProjectControl>) -> Self {
        control.frozen.store(true, Ordering::Release);
        Self(control)
    }
}
impl Drop for Freeze {
    fn drop(&mut self) {
        self.0.frozen.store(false, Ordering::Release);
    }
}
