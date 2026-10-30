//! Closed until native filesystem accounting and cleanup are implemented.
use crate::{
    builds::BuildManager, maintenance::MaintenanceManager, ApiError, ProjectStore, RuntimePaths,
    TaskRecord,
};
use serde_json::Value;
use std::sync::Arc;

fn unavailable() -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 暂不支持此功能：存储扫描与清理尚未完成。",
    )
    .detail("platform", "windows")
    .detail("capability", "storage-accounting")
    .outcome("not_started")
}

// Construction is closed: this type cannot have accepted a scan or cleanup.
pub(crate) enum StorageManager {}
impl StorageManager {
    pub fn new(
        _: RuntimePaths,
        _: Arc<ProjectStore>,
        _: Arc<BuildManager>,
        _: Arc<MaintenanceManager>,
    ) -> Result<Arc<Self>, ApiError> {
        Err(unavailable())
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
        match *self {}
    }
    pub async fn quiesce(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        match *self {}
    }
    pub fn resume(&self) {
        match *self {}
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        match *self {}
    }
    pub(crate) async fn forget_deleted_project(&self, _: &str) -> Result<Vec<String>, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn finish_deleted_project(&self, _: &str) {
        match *self {}
    }
}
