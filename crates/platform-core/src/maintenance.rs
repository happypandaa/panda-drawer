//! Project control status, change notifications and storage invalidation.
//! The independent backup, maintenance and ZIP-restore workflow is retired;
//! this module no longer creates, lists or restores backup archives.
use crate::{
    builds::{self, BuildManager},
    projects::{check_no_symlinks, valid_id},
    ApiError, ProjectStore,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    sync::{atomic::Ordering, Arc},
};
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OperationInput {
    pub maintenance_id: Option<String>,
}
fn sync_parent(path: &Path) -> Result<(), ApiError> {
    platform_native::filesystem::sync_directory(path.parent().unwrap())?;
    Ok(())
}
pub(crate) struct MaintenanceManager {
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    events: broadcast::Sender<String>,
}
impl MaintenanceManager {
    pub fn new(store: Arc<ProjectStore>, tasks: Arc<BuildManager>) -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        Arc::new(Self {
            store,
            tasks,
            events,
        })
    }
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.events.subscribe()
    }
    pub(crate) fn notify(&self, id: &str) {
        let _ = self.events.send(id.into());
    }
    pub fn status(&self, id: &str) -> Result<Value, ApiError> {
        let control = self.store.control(id)?;
        let mut value = json!(control.get());
        value["busy"] = json!(control.frozen.load(Ordering::Acquire));
        value["activeTask"] = json!(self.store.active_task(id));
        value["latestBuild"] = json!(self.tasks.latest_build(id));
        value["documentEpoch"] = json!(self.store.project(id).ok().map(|p| p.document_epoch));
        Ok(value)
    }
    pub fn browser_status(&self, id: &str, epoch: &str) -> Value {
        let status = self
            .status(id)
            .unwrap_or(json!({"mode":"maintenance","recoveryRequired":true}));
        json!({"mode":if status["mode"]=="ready"{json!("normal")}else{status["mode"].clone()},"busy":status["busy"],"recoveryRequired":status["recoveryRequired"],"reloadRequired":status["documentEpoch"].as_str()!=Some(epoch),"removed":self.store.deleted_project_identity_absent(id).unwrap_or(false),"workspaceMigrating":self.store.workspace_paused()})
    }
    pub(crate) fn invalidate_storage(workspace: &Path, id: &str) -> Result<(), ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let invalidations = workspace.join(".platform/cache/storage/invalidations");
        check_no_symlinks(&invalidations, true)?;
        let token = json!({"generation":Uuid::new_v4().to_string()});
        builds::atomic_json(
            &invalidations.join("projects").join(format!("{id}.json")),
            &token,
        )?;
        builds::atomic_json(&invalidations.join("workspace.json"), &token)?;
        for path in [
            workspace
                .join(".platform/cache/storage/projects")
                .join(format!("{id}.json")),
            workspace.join(".platform/cache/storage/workspace.json"),
        ] {
            check_no_symlinks(&path, true)?;
            if path.is_file() {
                fs::remove_file(&path)?;
                sync_parent(&path)?;
            }
        }
        Ok(())
    }
}
