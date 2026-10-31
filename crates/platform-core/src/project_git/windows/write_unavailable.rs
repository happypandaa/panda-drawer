use super::*;

pub(super) fn unavailable() -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 暂不支持此功能：已退役数据保存操作的提交收尾尚未适配；源码与数据取回入口独立核对。",
    )
    .detail("platform", "windows")
    .detail("capability", "project-native-git-versions")
    .outcome("not_started")
}

impl ProjectGitManager {
    pub async fn finalize_data_save(
        self: &Arc<Self>,
        id: &str,
        input: GitDataSaveFinalizeInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        let _ = (id, input, stop);
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
        let error = data_restore_metadata_usage(&root).unwrap_err();
        assert_eq!(error.code, "FILE_NOT_FOUND");
        assert!(!root.join("metadata-attempts").exists());
        let error = check_data_recovery_record(
            &root,
            "format-only",
            &GitDataSaveFinalizeInput {
                save_task_id: "task_disposable".into(),
                expected_journal_digest: "a".repeat(64),
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
        assert_eq!(error.details["operationOutcome"], "not_started");
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
