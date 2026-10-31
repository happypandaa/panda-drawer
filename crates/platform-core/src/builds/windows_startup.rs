//! Windows startup uses the existing task/publication recovery policy while
//! holding the actual native directory scopes used to read and rewrite tasks.
use super::{ProjectStore, RuntimePaths, StartupRecords, TaskRecord};
use crate::{
    builds::{
        atomic_json, build_result, reconcile_recovered_tasks, recover_publications, valid_task_id,
    },
    tasks::TaskStatus,
    ApiError,
};
use platform_native::windows::files::{self as native, PinnedDirectory};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, ApiError>;

#[derive(Clone, Copy)]
enum Scope {
    Tasks,
    Publications,
}
impl Scope {
    fn components(self) -> &'static [&'static str] {
        match self {
            Self::Tasks => &[".platform", "tasks"],
            Self::Publications => &[".platform", "project-state", "builds"],
        }
    }
    fn capability(self) -> &'static str {
        match self {
            Self::Tasks => "task-startup-recovery",
            Self::Publications => "build-publication-recovery",
        }
    }
}

struct BoundScope {
    path: PathBuf,
    _directories: Vec<PinnedDirectory>,
}

fn unknown(scope: Scope, path: &Path, cause: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        "BUILD_STARTUP_INVENTORY_UNCONFIRMED",
        "Windows 启动恢复目录无法核实，现有材料已保留。",
    )
    .detail("platform", "windows")
    .detail("capability", scope.capability())
    .detail("inventory", "unconfirmed")
    .detail("path", path.to_string_lossy().into_owned())
    .detail("cause", cause.to_string())
    .outcome("unknown")
}

/// Only fixed startup ancestors are inspected here. Existing records are read
/// by recovery, with the same skip and parse/error policy as the Mac loop.
fn bind_scope(workspace: &Path, scope: Scope) -> Result<Option<BoundScope>> {
    let selected =
        native::pin_directory(workspace).map_err(|error| unknown(scope, workspace, error))?;
    let (mut path, directory) = native::directory_scope(&selected.file)
        .map_err(|error| unknown(scope, workspace, error))?;
    let mut directories = vec![directory];
    for name in scope.components() {
        path.push(name);
        let directory = match native::pin_directory(&path) {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Distinguish actual absence from reparse points or other I/O
                // errors. Existing ancestors remain pinned until this returns.
                return match fs::symlink_metadata(&path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(unknown(scope, &path, error)),
                    Ok(_) => Err(unknown(scope, &path, "目录在检查期间出现。")),
                };
            }
            Err(error) => return Err(unknown(scope, &path, error)),
        };
        directories.push(directory);
    }
    Ok(Some(BoundScope {
        path,
        _directories: directories,
    }))
}

pub(super) fn require_supported_inventory(workspace: &Path) -> Result<()> {
    for scope in [Scope::Tasks, Scope::Publications] {
        bind_scope(workspace, scope)?;
    }
    Ok(())
}

pub(super) fn recover_startup(
    paths: &RuntimePaths,
    store: &ProjectStore,
    base_url: &str,
) -> Result<StartupRecords> {
    let required_scope = |scope| {
        bind_scope(paths.workspace_root(), scope)?
            .ok_or_else(|| unknown(scope, paths.workspace_root(), "已准备的启动恢复目录缺失。"))
    };
    let tasks = required_scope(Scope::Tasks)?;
    let _publications = required_scope(Scope::Publications)?;
    let mut records = HashMap::new();
    let mut legacy_build_times = HashMap::new();
    for entry in fs::read_dir(&tasks.path)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let _task =
            native::pin_directory(&path).map_err(|error| unknown(Scope::Tasks, &path, error))?;
        let record_path = path.join("task.json");
        if !record_path.is_file() {
            continue;
        }
        // Mac does not impose an ACL, single-link, byte-budget or modification
        // stamp admission here. Preserve that policy while reading the actual
        // no-reparse file HANDLE and retaining its native parent binding.
        let mut file = native::open_read_file_allowing_hard_links(&record_path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let mut record: TaskRecord = serde_json::from_slice(&bytes)?;
        drop(file);
        if !valid_task_id(&record.task_id)
            || path.file_name().and_then(|x| x.to_str()) != Some(record.task_id.as_str())
        {
            return Err(ApiError::new(
                "TASK_RECORD_INVALID",
                "Task record identity does not match its directory",
            ));
        }
        if record.task_type == "build" {
            // Capture only the original labelled fallback, before rewriting;
            // it neither establishes submission order nor admits the record.
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

#[cfg(test)]
#[path = "windows_startup_tests.rs"]
mod tests;
