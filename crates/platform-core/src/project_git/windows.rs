// Native Git transactions remain closed until Windows repository snapshots,
// owned commands, publications and recovery are implemented.
use crate::{
    builds::BuildManager, git_environment::GitEnvironmentManager, ApiError, ProjectStore,
    RuntimePaths, TaskRecord,
};
use std::{path::Path, sync::Arc};
use tokio::sync::watch;

#[path = "windows/build_guard.rs"]
mod build_guard;
#[path = "windows/data_inventory.rs"]
mod data_inventory;
pub(crate) use build_guard::BuildGitGuard;

fn unavailable() -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 暂不支持此功能：本机 Git 版本、仓库快照与源码/数据恢复事务尚未完成。",
    )
    .detail("platform", "windows")
    .detail("capability", "project-native-git-versions")
    .outcome("not_started")
}

pub enum ProjectGitManager {}
impl ProjectGitManager {
    pub fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        git: GitEnvironmentManager,
    ) -> Result<Arc<Self>, ApiError> {
        let _ = (paths, store, tasks, git);
        Err(unavailable())
    }
    pub async fn status(
        &self,
        project_id: &str,
        stop: watch::Receiver<bool>,
    ) -> Result<ProjectGitStatus, ApiError> {
        let _ = (project_id, stop);
        Err(unavailable())
    }
    pub async fn versions(
        &self,
        project_id: &str,
        limit: u32,
        stop: watch::Receiver<bool>,
    ) -> Result<ProjectGitVersions, ApiError> {
        let _ = (project_id, limit, stop);
        Err(unavailable())
    }
    pub async fn source_restore_preview(
        &self,
        id: &str,
        input: GitSourceRestorePreviewInput,
        stop: watch::Receiver<bool>,
    ) -> Result<GitSourceRestorePreview, ApiError> {
        let _ = (id, input, stop);
        Err(unavailable())
    }
    pub async fn source_restore_apply(
        self: &Arc<Self>,
        id: &str,
        input: GitSourceRestoreApplyInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        let _ = (id, input, stop);
        Err(unavailable())
    }
    pub async fn source_restore_recover(
        self: &Arc<Self>,
        id: &str,
        input: GitSourceRestoreRecoverInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        let _ = (id, input, stop);
        Err(unavailable())
    }
    pub async fn source_restore_status(
        &self,
        id: &str,
        stop: watch::Receiver<bool>,
    ) -> Result<GitSourceRestoreRecoveryStatus, ApiError> {
        let _ = (id, stop);
        Err(unavailable())
    }
    pub async fn data_restore_preview(
        &self,
        id: &str,
        input: GitDataRestorePreviewInput,
        stop: watch::Receiver<bool>,
    ) -> Result<GitDataRestorePreview, ApiError> {
        let _ = (id, input, stop);
        Err(unavailable())
    }
    pub async fn data_restore_apply(
        self: &Arc<Self>,
        id: &str,
        input: GitDataRestoreApplyInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        let _ = (id, input, stop);
        Err(unavailable())
    }
    pub async fn data_restore_recover(
        self: &Arc<Self>,
        id: &str,
        input: GitDataRestoreRecoverInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        let _ = (id, input, stop);
        Err(unavailable())
    }
    pub async fn data_restore_status(
        &self,
        id: &str,
        stop: watch::Receiver<bool>,
    ) -> Result<GitDataRestoreRecoveryStatus, ApiError> {
        let _ = (id, stop);
        Err(unavailable())
    }
    pub async fn data_save_status(
        &self,
        id: &str,
        stop: watch::Receiver<bool>,
    ) -> Result<serde_json::Value, ApiError> {
        let _ = (id, stop);
        Err(unavailable())
    }
    pub async fn finalize_data_save(
        self: &Arc<Self>,
        id: &str,
        input: GitDataSaveFinalizeInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        let _ = (id, input, stop);
        Err(unavailable())
    }
    pub fn cancel(&self, task_id: &str) -> Result<TaskRecord, ApiError> {
        let _ = task_id;
        Err(unavailable())
    }
    pub(crate) fn request_stop(&self) {
        match *self {}
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        match *self {}
    }
    #[cfg(test)]
    pub(crate) fn fixture_shutdown_runner(&self, _: tokio::task::JoinHandle<()>) {
        match *self {}
    }
    #[cfg(test)]
    pub(crate) fn fixture_shutdown_cancel(self: &Arc<Self>, _: std::sync::mpsc::Sender<()>) {
        match **self {}
    }
    #[cfg(test)]
    pub(crate) fn test_data_restore_fault(&self, _: u32) {
        match *self {}
    }
}

pub(crate) fn check_data_operations_available(workspace: &Path, id: &str) -> Result<(), ApiError> {
    data_inventory::check_project(workspace, id)
}
pub(crate) fn check_data_workspace_available(workspace: &Path) -> Result<(), ApiError> {
    data_inventory::check_workspace(workspace)
}
pub(crate) fn check_data_recovery_record(
    workspace: &Path,
    id: &str,
    input: &GitDataSaveFinalizeInput,
) -> Result<(), ApiError> {
    let _ = (workspace, id, input);
    Err(unavailable())
}
pub(crate) fn data_restore_metadata_usage(operation: &Path) -> Result<(usize, u64), ApiError> {
    let _ = operation;
    Err(unavailable())
}
pub(crate) fn inspect_data_restore_completion(
    root: &Path,
    header: &crate::data_restore_guard::DataRestoreJournalHeader,
    bytes: &[u8],
    completion: Option<&[u8]>,
    terminal: Option<&[u8]>,
) -> Result<bool, ApiError> {
    let _ = (root, header, bytes, completion, terminal);
    Err(unavailable())
}

#[cfg(test)]
mod closed_boundary_tests {
    use super::*;

    #[test]
    fn unfinished_version_guards_do_not_inspect_create_or_confirm_recovery_material() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-closed-project-git-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let retained = root.join("retained.json");
        std::fs::write(&retained, b"disposable evidence; not a recovery record").unwrap();
        let before = crate::file_identity::PersistedIdentity::capture(&retained).unwrap();
        for error in [
            data_restore_metadata_usage(&root).unwrap_err(),
            check_data_recovery_record(
                &root,
                "format-only",
                &GitDataSaveFinalizeInput {
                    save_task_id: "task_disposable".into(),
                    expected_journal_digest: "a".repeat(64),
                },
            )
            .unwrap_err(),
        ] {
            assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
            assert_eq!(error.details["operationOutcome"], "not_started");
        }
        check_data_operations_available(&root, "format-only").unwrap();
        check_data_workspace_available(&root).unwrap();
        let absent = root.join("missing-root");
        assert!(check_data_workspace_available(&absent).is_err());
        assert!(!absent.exists());
        assert_eq!(
            std::fs::read(&retained).unwrap(),
            b"disposable evidence; not a recovery record"
        );
        assert!(before.matches_path(&retained).unwrap());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    }
}
