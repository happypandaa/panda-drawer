//! Startup recovery is selected here so platform code owns its recovery proof.
use super::*;

#[cfg(windows)]
#[path = "startup/windows.rs"]
mod windows;

pub(super) fn require_before_initialization(data_root: &Path) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        windows::require_before_initialization(data_root)
    }
    #[cfg(not(windows))]
    {
        let _ = data_root;
        Ok(())
    }
}

pub(super) fn recover_startup(paths: &RuntimePaths) -> Result<(), ApiError> {
    #[cfg(windows)]
    let _inventory = windows::pin_recovery(paths.data_root())?;
    recover_selected(paths)
}

fn recover_selected(paths: &RuntimePaths) -> Result<(), ApiError> {
    let selected: Value = read_json(&paths.data_root().join("settings.json")).map_err(ioerr)?;
    for entry in fs::read_dir(paths.migrations_dir())? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if !valid_id(&id) || !entry.path().join("journal.json").is_file() {
            continue;
        }
        let mut j = read_journal(paths, &id)?;
        validate_journal(&j, &id).map_err(ioerr)?;
        let chosen = selected["migrationId"].as_str() == Some(id.as_str());
        if chosen && !j.committed {
            j.committed = true;
            save_journal(paths, &j)?;
        }
        if chosen && !j.epochs_applied {
            let store = ProjectStore::new(paths.workspace_root().into())?;
            initialize_selected(paths, &store, &mut j)?;
        }
        let Some(mut t) = task(paths, &id)? else {
            continue;
        };
        if !t.status.is_final() {
            t.status = TaskStatus::Interrupted;
            t.phase = "interrupted".into();
            let outcome = if chosen { "committed" } else { "not_committed" };
            t.result = Some(if chosen {
                completed_result(&j)
            } else {
                json!({"sourcePath":j.source_path,"destinationPath":j.destination_path,"operationOutcome":outcome,"sourceRetained":true,"targetRetained":j.destination_path.exists()||j.stage_path.exists()})
            });
            t.error=Some(ApiError::new("TASK_INTERRUPTED","Platform stopped before the migration task returned; no operation was automatically repeated").outcome(outcome));
            builds::atomic_json(&task_dir(paths.data_root(), &id).join("task.json"), &t)?;
        }
        if chosen && !j.resolved && j.source_cleanup.is_none() {
            mark_old_retired(paths, &j)?;
        }
    }
    Ok(())
}
