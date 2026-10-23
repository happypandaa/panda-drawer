//! Independent build-result discovery. No project contents or maintenance state
//! are changed by reads, and legacy timestamps never certify submission order.
use super::{atomic_json, valid_task_id};
use crate::{projects::check_no_symlinks, tasks::TaskRecord, ApiError};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::SystemTime,
};

const MAX_POINTER_BYTES: u64 = 4096;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResultRecovery {
    pub task: Option<TaskRecord>,
    pub selection: &'static str,
    pub issues: Vec<ApiError>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Pointer {
    schema_version: u32,
    project_id: String,
    task_id: String,
}

fn unavailable() -> ApiError {
    ApiError::new(
        "BUILD_RESULT_UNAVAILABLE",
        "保留的构建结果记录无法安全核对，请保留任务和日志；未猜测其他任务为最近构建。",
    )
}

fn path(workspace: &Path, project: &str) -> PathBuf {
    workspace
        .join(".platform/project-state/latest-build")
        .join(format!("{project}.json"))
}

pub(super) fn modified(path: &Path) -> Option<SystemTime> {
    check_no_symlinks(path, false).ok()?;
    let meta = fs::symlink_metadata(path).ok()?;
    (meta.is_file() && meta.nlink() == 1)
        .then(|| meta.modified().ok())
        .flatten()
}

fn read_pointer(workspace: &Path, project: &str) -> Result<Option<Pointer>, ApiError> {
    let path = path(workspace, project);
    check_no_symlinks(&path, true).map_err(|_| unavailable())?;
    let before = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(unavailable()),
    };
    if !before.is_file() || before.nlink() != 1 || before.len() > MAX_POINTER_BYTES {
        return Err(unavailable());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .map_err(|_| unavailable())?;
    let opened = file.metadata().map_err(|_| unavailable())?;
    if !opened.is_file()
        || opened.nlink() != 1
        || (opened.dev(), opened.ino()) != (before.dev(), before.ino())
    {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    file.take(MAX_POINTER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() as u64 > MAX_POINTER_BYTES {
        return Err(unavailable());
    }
    let pointer: Pointer = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if pointer.schema_version != 1
        || pointer.project_id != project
        || !valid_task_id(&pointer.task_id)
    {
        return Err(unavailable());
    }
    Ok(Some(pointer))
}

pub(super) fn remember(workspace: &Path, record: &TaskRecord) -> Result<(), ApiError> {
    let project = record
        .project_id
        .as_deref()
        .filter(|project| crate::projects::valid_id(project))
        .ok_or_else(unavailable)?;
    if record.task_type != "build" || !valid_task_id(&record.task_id) {
        return Err(unavailable());
    }
    // A malformed/special pointer is evidence, not something a new build may
    // silently replace. An absent pointer is normal for older workspaces.
    read_pointer(workspace, project)?;
    atomic_json(
        &path(workspace, project),
        &Pointer {
            schema_version: 1,
            project_id: project.into(),
            task_id: record.task_id.clone(),
        },
    )
    .map_err(|_| {
        ApiError::new(
            "BUILD_RESULT_PERSISTENCE_FAILED",
            "无法保存本次构建结果索引，构建尚未启动；请保留任务记录并检查存储状态。",
        )
    })
}

pub(super) fn recover(
    workspace: &Path,
    project: &str,
    records: &HashMap<String, TaskRecord>,
    legacy_times: &HashMap<String, Option<SystemTime>>,
) -> BuildResultRecovery {
    let outcome = (|| {
        if !crate::projects::valid_id(project) {
            return Err(unavailable());
        }
        if let Some(pointer) = read_pointer(workspace, project)? {
            let record = records
                .get(&pointer.task_id)
                .filter(|record| {
                    record.task_type == "build" && record.project_id.as_deref() == Some(project)
                })
                .ok_or_else(unavailable)?;
            return Ok(BuildResultRecovery {
                task: Some(record.clone()),
                selection: "recorded-submission",
                issues: vec![],
            });
        }
        let mut candidates = records
            .values()
            .filter(|record| {
                record.task_type == "build" && record.project_id.as_deref() == Some(project)
            })
            .map(|record| {
                legacy_times
                    .get(&record.task_id)
                    .copied()
                    .flatten()
                    .map(|modified| (modified, record))
                    .ok_or_else(unavailable)
            })
            .collect::<Result<Vec<_>, _>>()?;
        candidates.sort_by_key(|(modified, _)| *modified);
        let Some((modified, record)) = candidates.pop() else {
            return Ok(BuildResultRecovery {
                task: None,
                selection: "none",
                issues: vec![],
            });
        };
        if candidates
            .last()
            .is_some_and(|(previous, _)| *previous == modified)
        {
            return Err(ApiError::new(
                "BUILD_RESULT_ORDER_UNKNOWN",
                "多份历史构建记录的修改时间相同，无法确定应恢复哪一份；请结合保留日志核对。",
            ));
        }
        Ok(BuildResultRecovery {
            task: Some(record.clone()),
            selection: "legacy-modified-time",
            issues: vec![ApiError::new(
                "BUILD_RESULT_LEGACY_ORDER",
                "按历史构建记录的修改时间恢复显示，不代表最新提交顺序；请结合任务日志核对。",
            )],
        })
    })();
    outcome.unwrap_or_else(|error| BuildResultRecovery {
        task: None,
        selection: "unavailable",
        issues: vec![error],
    })
}
