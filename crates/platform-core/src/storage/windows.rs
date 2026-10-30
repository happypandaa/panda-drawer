//! Real instance coordination without admitting unfinished accounting/cleanup.
use crate::{
    builds::BuildManager, maintenance::MaintenanceManager, ApiError, ProjectStore, RuntimePaths,
    TaskRecord,
};
use serde_json::Value;
use std::sync::Arc;

#[path = "windows/coordinator.rs"]
mod coordinator;
use coordinator::Coordinator;

fn unavailable() -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 暂不支持此功能：存储扫描与清理尚未完成。",
    )
    .detail("platform", "windows")
    .detail("capability", "storage-accounting")
    .outcome("not_started")
}

// As on macOS, construction retains the actual caller context and starts no
// scan or recovery. It does not inventory or declare old on-disk material clear.
pub(crate) struct StorageManager {
    _paths: RuntimePaths,
    _store: Arc<ProjectStore>,
    _tasks: Arc<BuildManager>,
    _maintenance: Arc<MaintenanceManager>,
    coordinator: Coordinator,
}
impl StorageManager {
    pub fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        maintenance: Arc<MaintenanceManager>,
    ) -> Result<Arc<Self>, ApiError> {
        Ok(Arc::new(Self {
            _paths: paths,
            _store: store,
            _tasks: tasks,
            _maintenance: maintenance,
            coordinator: Coordinator::new(),
        }))
    }
    pub async fn info(self: &Arc<Self>, _: &str) -> Result<Value, ApiError> {
        Err(unavailable())
    }
    pub async fn submit_scan(self: &Arc<Self>, _: &str) -> Result<TaskRecord, ApiError> {
        Err(unavailable())
    }
    pub async fn files(&self, _: &str, _: Value) -> Result<Value, ApiError> {
        Err(unavailable())
    }
    pub async fn submit_cleanup(
        self: &Arc<Self>,
        _: &str,
        _: Value,
    ) -> Result<TaskRecord, ApiError> {
        Err(unavailable())
    }
    pub async fn workspace_info(&self) -> Result<Value, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn request_stop(&self) {
        self.coordinator.request_stop();
    }
    pub async fn quiesce(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.coordinator.quiesce().await
    }
    pub fn resume(&self) {
        self.coordinator.resume();
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.quiesce().await
    }
    pub(crate) async fn forget_deleted_project(&self, _: &str) -> Result<Vec<String>, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn finish_deleted_project(&self, id: &str) {
        // This retires this instance's in-memory admission marker only; it is
        // not a substitute for the still-unavailable forget/cache deletion.
        self.coordinator.finish_deleted_project(id);
    }
}
