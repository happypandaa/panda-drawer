//! Instance coordination and owned-directory removal are real. Cleanup plans
//! and migration content-snapshot validation remain explicitly unavailable.
use crate::{
    builds::BuildManager, file_identity::PersistedIdentity, ApiError, ProjectStore, RuntimePaths,
    TaskRecord,
};

#[path = "windows/coordinator.rs"]
mod coordinator;
#[path = "windows/removal.rs"]
mod removal;
use coordinator::Coordinator;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

fn unavailable() -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 暂不支持此功能：清理快照核对与目录移除尚未完成。",
    )
    .detail("platform", "windows")
    .detail("capability", "owned-directory-cleanup")
    .outcome("not_started")
}

// Like the macOS constructor, this retains the actual caller's context without
// scanning, replaying or claiming ownership of any persistent cleanup material.
pub(crate) struct CleanupManager {
    _paths: RuntimePaths,
    _store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    _instance_id: String,
    coordinator: Coordinator,
}
impl CleanupManager {
    pub(crate) fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        instance_id: String,
    ) -> Result<Arc<Self>, ApiError> {
        Ok(Arc::new(Self {
            _paths: paths,
            _store: store,
            tasks,
            _instance_id: instance_id,
            coordinator: Coordinator::new(),
        }))
    }
    pub(crate) fn preview(self: &Arc<Self>) -> Result<TaskRecord, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn submit(self: &Arc<Self>, _: Value) -> Result<TaskRecord, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn latest_task(&self) -> Result<Option<TaskRecord>, ApiError> {
        // This is the actual latest task of this manager instance, matching
        // the macOS in-memory query. None is not proof of absent disk journals.
        self.coordinator
            .latest
            .lock()
            .unwrap()
            .as_ref()
            .map(|id| self.tasks.task(id))
            .transpose()
    }
    pub(crate) async fn forget_deleted_project(
        &self,
        _: &str,
        _: &[String],
        _: &[PathBuf],
    ) -> Result<Vec<String>, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn finish_deleted_project(&self, id: &str) {
        // Only this instance's in-memory marker is retired. The still-closed
        // forget/removal methods cannot supply a disk-cleanup success receipt.
        self.coordinator.finish_deleted_project(id);
    }
    pub(crate) fn request_stop(&self) {
        self.coordinator.request_stop();
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.coordinator.shutdown().await
    }
}

pub(crate) fn remove_owned_directory(
    path: &Path,
    expected: &PersistedIdentity,
) -> Result<Value, ApiError> {
    removal::remove(path, expected)
}
pub(crate) fn remove_owned_directory_checked(
    _: &Path,
    _: &PersistedIdentity,
    _: impl FnOnce(&Value) -> Result<(), ApiError>,
) -> Result<Value, ApiError> {
    Err(unavailable())
}
pub(crate) fn retired_cleanup_status(_: &RuntimePaths, _: &str) -> Result<Option<Value>, ApiError> {
    Err(unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unfinished_removal_retains_disposable_objects_and_never_calls_verification() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-closed-cleanup-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let candidate = root.join("candidate");
        std::fs::create_dir(&candidate).unwrap();
        std::fs::write(candidate.join("payload"), b"disposable retained bytes").unwrap();
        let expected = PersistedIdentity::capture(&candidate).unwrap();
        let error = remove_owned_directory_checked(&candidate, &expected, |_| {
            panic!("closed cleanup cannot invoke verification")
        })
        .unwrap_err();
        assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
        assert_eq!(error.details["operationOutcome"], "not_started");
        assert!(expected.matches_path(&candidate).unwrap());
        assert_eq!(
            std::fs::read(candidate.join("payload")).unwrap(),
            b"disposable retained bytes"
        );
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
    }
}
