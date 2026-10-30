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
fn observed_empty_records(workspace: &Path) -> Result<StartupRecords, ApiError> {
    // These empty registries describe the actual missing/empty physical trees.
    // This second observation occurs after directory preparation, rather than
    // using the early snapshot as permission to replay the macOS recovery loop.
    windows::require_supported_inventory(workspace)?;
    Ok((HashMap::new(), HashMap::new()))
}

#[cfg(windows)]
pub(super) fn recover_startup(
    paths: &RuntimePaths,
    _store: &ProjectStore,
    _base_url: &str,
) -> Result<StartupRecords, ApiError> {
    observed_empty_records(paths.workspace_root())
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
        let _ = manager;
        Err(ApiError::new(
            "PLATFORM_FEATURE_UNAVAILABLE",
            "Windows 构建产物的启动自动清理尚未完成，现有产物与证据已保留。",
        )
        .detail("platform", "windows")
        .detail("capability", "build-artifact-startup-cleanup")
        .outcome("not_started"))
    }
    #[cfg(not(windows))]
    {
        manager.cleanup_startup_artifacts();
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn records_are_empty_only_after_actual_scope_observation() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-observed-build-records-")
            .tempdir()
            .unwrap();
        let workspace = temporary.path().canonicalize().unwrap();
        fs::create_dir(workspace.join("projects")).unwrap();
        fs::write(workspace.join("projects/retained"), b"project source").unwrap();
        for present in [false, true] {
            if present {
                fs::create_dir_all(workspace.join(".platform/tasks")).unwrap();
                fs::create_dir_all(workspace.join(".platform/project-state/builds")).unwrap();
            }
            let (records, legacy_times) = observed_empty_records(&workspace).unwrap();
            assert!(records.is_empty());
            assert!(legacy_times.is_empty());
            assert_eq!(
                fs::read(workspace.join("projects/retained")).unwrap(),
                b"project source"
            );
        }
    }

    #[test]
    fn material_appearing_after_early_admission_is_not_recovered_or_rewritten() {
        for publication in [false, true] {
            let temporary = tempfile::Builder::new()
                .prefix("panda-disposable-late-build-recovery-")
                .tempdir()
                .unwrap();
            let workspace = temporary.path().canonicalize().unwrap();
            require_supported_inventory(&workspace).unwrap();
            let path = if publication {
                workspace.join(".platform/project-state/builds/prj_disposable.publication.json")
            } else {
                workspace.join(".platform/tasks/task_disposable/task.json")
            };
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let record = TaskRecord {
                task_id: "task_disposable".into(),
                project_id: Some("prj_disposable".into()),
                task_type: "build".into(),
                status: super::super::TaskStatus::Running,
                phase: "dependencies".into(),
                result: None,
                error: None,
            };
            let before = serde_json::to_vec(&record).unwrap();
            fs::write(&path, &before).unwrap();
            let error = observed_empty_records(&workspace).unwrap_err();
            assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
            assert_eq!(error.details["operationOutcome"], "not_started");
            assert_eq!(fs::read(&path).unwrap(), before);
            if !publication {
                let retained: TaskRecord =
                    serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                assert_eq!(retained.status, super::super::TaskStatus::Running);
                assert!(retained.error.is_none());
            }
        }
    }
}
