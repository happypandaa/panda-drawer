//! Closed until native snapshot/removal, occupancy retry and read-only handling
//! are implemented. No cleanup plan, ownership receipt or deletion is fabricated.
use crate::{
    builds::BuildManager, file_identity::PersistedIdentity, ApiError, ProjectStore, RuntimePaths,
    TaskRecord,
};
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

// No instance can exist while construction is closed. In particular shutdown
// cannot invent a successful receipt for a manager that accepted removal work.
pub(crate) enum CleanupManager {}
impl CleanupManager {
    pub(crate) fn new(
        _: RuntimePaths,
        _: Arc<ProjectStore>,
        _: Arc<BuildManager>,
        _: String,
    ) -> Result<Arc<Self>, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn preview(self: &Arc<Self>) -> Result<TaskRecord, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn submit(self: &Arc<Self>, _: Value) -> Result<TaskRecord, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn latest_task(&self) -> Result<Option<TaskRecord>, ApiError> {
        Err(unavailable())
    }
    pub(crate) async fn forget_deleted_project(
        &self,
        _: &str,
        _: &[String],
        _: &[PathBuf],
    ) -> Result<Vec<String>, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn finish_deleted_project(&self, _: &str) {
        match *self {}
    }
    pub(crate) fn request_stop(&self) {
        match *self {}
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        match *self {}
    }
}

pub(crate) fn remove_owned_directory(_: &Path, _: &PersistedIdentity) -> Result<Value, ApiError> {
    Err(unavailable())
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
        for error in [
            remove_owned_directory(&candidate, &expected).unwrap_err(),
            remove_owned_directory_checked(&candidate, &expected, |_| {
                panic!("closed cleanup cannot invoke verification")
            })
            .unwrap_err(),
        ] {
            assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
            assert_eq!(error.details["operationOutcome"], "not_started");
        }
        assert!(expected.matches_path(&candidate).unwrap());
        assert_eq!(
            std::fs::read(candidate.join("payload")).unwrap(),
            b"disposable retained bytes"
        );
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
    }
}
