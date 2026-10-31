//! Admission precedes both cleanup submission and its failure-recording path.
//! Observing no selected cleanup remains a real no-work result on both hosts.
use crate::ApiError;

pub(super) fn require_cleanup() -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        Err(ApiError::new(
            "PLATFORM_FEATURE_UNAVAILABLE",
            "Windows 工作区迁移原目录的自动收尾与重试尚未完成，现有材料已保留。",
        )
        .detail("platform", "windows")
        .detail("capability", "workspace-migration-cleanup")
        .detail("accepted", false)
        .outcome("not_started"))
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

pub(super) fn with_selected<T>(
    selected: Option<T>,
    submit: impl FnOnce(T) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    if let Some(journal) = selected {
        require_cleanup()?;
        // In particular, the caller's error handler must not record a reopen
        // failure merely because this platform has not accepted cleanup work.
        submit(journal)?;
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use crate::{file_identity::PersistedIdentity, workspace::MigrationJournal};
    use serde_json::json;
    use std::{cell::Cell, fs};

    #[test]
    fn retry_is_rejected_before_accepting_or_scheduling_a_cleanup_task() {
        let error = require_cleanup().unwrap_err();
        assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
        assert_eq!(error.details["capability"], "workspace-migration-cleanup");
        assert_eq!(error.details["accepted"], false);
        assert_eq!(error.details["operationOutcome"], "not_started");
    }

    #[test]
    fn genuinely_unselected_cleanup_does_not_invoke_the_submission_or_failure_handler() {
        with_selected::<MigrationJournal>(None, |_| {
            panic!("no selected journal must not reach cleanup submission")
        })
        .unwrap();
    }

    #[test]
    fn a_real_pending_journal_never_reaches_submission_or_reopen_failure_recording() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-migration-cleanup-admission-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("retained-source");
        let destination = root.join("selected-workspace");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(source.join("source.txt"), b"disposable retained source").unwrap();
        let id = "task_disposable_migration";
        let directory = root.join("migrations").join(id);
        fs::create_dir_all(directory.join("task")).unwrap();
        let journal_path = directory.join("journal.json");
        let task_path = directory.join("task/task.json");
        let settings_path = root.join("settings.json");
        let before = serde_json::to_vec(&json!({
            "schemaVersion":1,"identifier":crate::APP_IDENTIFIER,
            "environment":crate::ENVIRONMENT,"taskId":id,
            "sourcePath":source,"destinationPath":destination,
            "sourceIdentity":PersistedIdentity::capture(&source).unwrap(),
            "targetEmptyIdentity":PersistedIdentity::capture(&destination).unwrap(),
            "destinationIdentity":PersistedIdentity::capture(&destination).unwrap(),
            "stagePath":root.join("retained-stage"),"phase":"completed",
            "published":true,"committed":true,"resolved":true,"epochs":{},
            "epochsApplied":true,"fileCount":1,"totalBytes":26,"startedAt":1,
            "sourceCleanup":{"phase":"pending","started":false,"excludedPaths":[]},
            "cleanupTaskId":"task_retained_cleanup"
        }))
        .unwrap();
        fs::write(&journal_path, &before).unwrap();
        let settings = serde_json::to_vec(&json!({
            "migrationId":id,"workspaceRoot":destination
        }))
        .unwrap();
        fs::write(&settings_path, &settings).unwrap();
        fs::write(&task_path, b"retained cleanup task bytes").unwrap();
        let selected: MigrationJournal =
            serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
        assert!(selected.source_cleanup.as_ref().unwrap().pending());
        assert!(selected.committed && selected.published);
        let entered = Cell::new(false);
        let error = with_selected(Some(selected), |_| {
            entered.set(true);
            // This is the actual dispatch boundary used by the automatic path,
            // not a fabricated runtime/store. Reaching its callback would also
            // reach the caller's old failure-recording branch.
            fs::write(&journal_path, b"must not record a reopen failure").unwrap();
            fs::write(&task_path, b"must not accept a cleanup task").unwrap();
            Err(ApiError::new("DISPOSABLE_SUBMISSION_FAILURE", "fixture"))
        })
        .unwrap_err();
        assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
        assert_eq!(error.details["operationOutcome"], "not_started");
        assert!(!entered.get());
        assert_eq!(fs::read(&journal_path).unwrap(), before);
        assert_eq!(
            fs::read(&task_path).unwrap(),
            b"retained cleanup task bytes"
        );
        assert_eq!(fs::read(&settings_path).unwrap(), settings);
        assert_eq!(
            fs::read(source.join("source.txt")).unwrap(),
            b"disposable retained source"
        );
    }
}

#[cfg(all(test, not(windows)))]
mod mac_tests {
    use super::*;

    #[test]
    fn admission_preserves_the_existing_selected_submission_and_failure() {
        require_cleanup().unwrap();
        with_selected::<()>(None, |_| panic!("unselected submission")).unwrap();
        let error = with_selected(Some("selected"), |selected| {
            assert_eq!(selected, "selected");
            Err(ApiError::new("ORIGINAL_FAILURE", "retained original error"))
        })
        .unwrap_err();
        assert_eq!(error.code, "ORIGINAL_FAILURE");
        assert_eq!(error.message, "retained original error");
    }
}
