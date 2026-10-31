use super::*;

impl GitSourceRestoreRecoverInput {
    pub(super) fn task_kind(&self) -> &'static str {
        match self.action {
            RestoreAction::Continue => "gitSourceRestoreContinue",
            RestoreAction::Undo => "gitSourceRestoreUndo",
        }
    }
}

pub(super) fn unavailable() -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 暂不支持此功能：Git 源码/数据写入与恢复事务尚未完成；只读状态和版本列表可独立使用。",
    )
    .detail("platform", "windows")
    .detail("capability", "project-native-git-versions")
    .outcome("not_started")
}

impl ProjectGitManager {
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

    pub(super) async fn source_restore_apply_inner(
        &self,
        _: &ProjectHandle,
        _: &str,
        _: GitSourceRestoreApplyInput,
        _: &mut Context,
    ) -> Result<Value, ApiError> {
        Err(unavailable())
    }
    pub(super) async fn source_restore_recover_inner(
        &self,
        _: &ProjectHandle,
        _: &str,
        _: GitSourceRestoreRecoverInput,
        _: &mut Context,
    ) -> Result<Value, ApiError> {
        Err(unavailable())
    }
    pub(super) async fn finalize_data_save_inner(
        &self,
        _: &ProjectHandle,
        _: &str,
        _: GitDataSaveFinalizeInput,
        _: &mut Context,
    ) -> Result<Value, ApiError> {
        Err(unavailable())
    }
    #[cfg(test)]
    pub(crate) fn test_data_restore_fault(&self, _: u32) {}
}
pub(crate) fn metadata_usage(_: &Path) -> Result<(usize, u64), ApiError> {
    Err(unavailable())
}
pub(crate) fn inspect_completion(
    _: &Path,
    _: &crate::data_restore_guard::DataRestoreJournalHeader,
    _: &[u8],
    _: Option<&[u8]>,
    _: Option<&[u8]>,
) -> Result<bool, ApiError> {
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
