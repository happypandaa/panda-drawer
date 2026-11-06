//! Platform admission and publication reconciliation for the shared import.
use crate::ApiError;

pub(super) fn require_start<T>(sources: &Result<T, ApiError>) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        sources.as_ref().map(|_| ()).map_err(Clone::clone)
    }
    #[cfg(not(windows))]
    {
        // Preserve the original Mac sequence, including its deferred source
        // error after task acceptance. A source error is not a new Mac policy.
        let _ = sources;
        Ok(())
    }
}

pub(super) fn require_recovery() -> Result<(), ApiError> {
    Ok(())
}

pub(super) fn reconcile_source_publication(
    manager: &super::NodeGitImportManager,
    journal: &mut super::Journal,
) -> Result<(), ApiError> {
    if let Some(selected) = journal.input.selected_directory.clone() {
        if journal.source_path.as_ref() != Some(&selected.path) {
            if let Some(root) = manager
                .sources
                .as_ref()
                .map_err(Clone::clone)?
                .reconcile_selected_publication(
                    &journal.source_id,
                    &journal.app_id,
                    &journal.task_id,
                    &selected,
                )?
            {
                let identity = super::capture(&root)?;
                if !journal
                    .selected_root_identity
                    .as_ref()
                    .is_some_and(|expected| expected.matches_captured(&identity))
                {
                    return Err(super::invalid("所选文件夹已变化，请重新选择并检查。"));
                }
                journal.source_path = Some(root);
                journal.source_identity = Some(identity);
                manager.save(journal)?;
            }
        }
        return Ok(());
    }
    #[cfg(windows)]
    if !journal.registration_attempted
        && journal.phase == "publishing"
        && journal.source_path.is_none()
    {
        let result = manager
            .sources
            .as_ref()
            .map_err(Clone::clone)?
            .reconcile_author_publication(&journal.source_id, &journal.app_id, &journal.task_id);
        match result {
            Ok(Some(root)) => {
                journal.source_identity = Some(super::capture(&root)?);
                journal.source_path = Some(root);
                manager.save(journal)?;
            }
            Ok(None) => (),
            Err(issue) => {
                journal.commit_state = super::CommitState::Unknown;
                if let Some(paths) = issue
                    .details
                    .get("remainingPaths")
                    .and_then(|v| v.as_array())
                {
                    journal
                        .remaining_paths
                        .extend(paths.iter().filter_map(|value| {
                            serde_json::from_value::<std::path::PathBuf>(value.clone()).ok()
                        }));
                }
                return Err(issue.detail("sourcePublicationPending", true));
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (manager, journal);
    Ok(())
}

pub(super) fn retain_pending_paths(journal: &super::Journal, paths: &mut Vec<std::path::PathBuf>) {
    #[cfg(windows)]
    if journal.commit_state == super::CommitState::Unknown && journal.phase == "publishing" {
        // These are display/retry diagnostics only. Deletion always derives
        // its authority independently from the original ownership identities.
        paths.extend(journal.remaining_paths.iter().cloned());
    }
    #[cfg(not(windows))]
    let _ = (journal, paths);
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn new_import_preserves_the_actual_source_failure_without_accepting_a_task() {
        let mut issue = ApiError::new(
            "PLATFORM_FEATURE_UNAVAILABLE",
            "retained source capability failure",
        )
        .detail("platform", "windows")
        .detail("capability", "non-executing-source-acquisition")
        .detail("retainedMaterial", "owned source evidence")
        .outcome("not_started");
        issue.retryable = true;
        let expected = serde_json::to_value(&issue).unwrap();
        let sources = Err::<(), _>(issue);
        assert_eq!(
            serde_json::to_value(require_start(&sources).unwrap_err()).unwrap(),
            expected
        );
        let Err(retained) = sources else {
            panic!("The original source failure must remain retained");
        };
        assert_eq!(serde_json::to_value(retained).unwrap(), expected);
    }

    #[test]
    fn recovery_admission_defers_actual_record_and_resource_checks_to_the_manager() {
        require_recovery().unwrap();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn mac_source_errors_remain_deferred_and_recovery_keeps_its_original_entry() {
        let sources = Err::<(), _>(ApiError::new("SOURCE_RETAINED", "retained source evidence"));
        require_start(&sources).unwrap();
        require_recovery().unwrap();
    }
}
