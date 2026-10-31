//! Build startup recovery owns its platform-specific admission here. The
//! existing macOS constructor continues through its original recovery body.
#[cfg(not(windows))]
use super::*;
#[cfg(windows)]
use super::{BuildManager, ProjectStore, RuntimePaths, TaskRecord};
use crate::ApiError;
use std::{collections::HashMap, path::Path, time::SystemTime};

#[cfg(windows)]
#[path = "windows_startup.rs"]
mod windows;

pub(super) fn require_supported_inventory(workspace: &Path) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        windows::require_supported_inventory(workspace)
    }
    #[cfg(not(windows))]
    {
        let _ = workspace;
        Ok(())
    }
}

type StartupRecords = (
    HashMap<String, TaskRecord>,
    HashMap<String, Option<SystemTime>>,
);

#[cfg(windows)]
pub(super) fn recover_startup(
    paths: &RuntimePaths,
    store: &ProjectStore,
    base_url: &str,
) -> Result<StartupRecords, ApiError> {
    windows::recover_startup(paths, store, base_url)
}

#[cfg(not(windows))]
pub(super) fn recover_startup(
    paths: &RuntimePaths,
    store: &ProjectStore,
    base_url: &str,
) -> Result<StartupRecords, ApiError> {
    let tasks = paths.workspace_root().join(".platform/tasks");
    let mut records = HashMap::new();
    let mut legacy_build_times = HashMap::new();
    for entry in fs::read_dir(&tasks)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        check_no_symlinks(&path, false)?;
        let record_path = path.join("task.json");
        if !record_path.is_file() {
            continue;
        }
        let mut record: TaskRecord = serde_json::from_slice(&fs::read(&record_path)?)?;
        if !valid_task_id(&record.task_id)
            || path.file_name().and_then(|x| x.to_str()) != Some(record.task_id.as_str())
        {
            return Err(ApiError::new(
                "TASK_RECORD_INVALID",
                "Task record identity does not match its directory",
            ));
        }
        if record.task_type == "build" {
            // Startup recovery rewrites task.json. Capture the old mtime
            // first; it is only a labelled fallback, never submission order.
            legacy_build_times.insert(record.task_id.clone(), build_result::modified(&record_path));
        }
        if !record.status.is_final() {
            record.status = TaskStatus::Interrupted;
            record.error = Some(ApiError::new(
                "TASK_INTERRUPTED",
                "Platform exited before the task finished; no automatic retry was performed",
            ));
            atomic_json(&record_path, &record)?;
        }
        records.insert(record.task_id.clone(), record);
    }
    recover_publications(paths, store)?;
    reconcile_recovered_tasks(paths, store, base_url, &mut records)?;
    Ok((records, legacy_build_times))
}

pub(super) fn cleanup_artifacts(manager: &BuildManager) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        manager.cleanup_startup_artifacts();
        Ok(())
    }
    #[cfg(not(windows))]
    {
        manager.cleanup_startup_artifacts();
        Ok(())
    }
}
