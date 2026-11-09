//! Current explicit build-task selection, stored outside the author directory.
use super::{atomic_json, valid_task_id};
use crate::{projects::check_no_symlinks, tasks::TaskRecord, ApiError};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Read, path::PathBuf};

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

fn path(paths: &crate::RuntimePaths, project: &str) -> PathBuf {
    paths
        .data_root()
        .join("apps")
        .join(project)
        .join("latest-build.json")
}

fn read_pointer(paths: &crate::RuntimePaths, project: &str) -> Result<Option<Pointer>, ApiError> {
    let path = path(paths, project);
    check_no_symlinks(&path, true).map_err(|_| unavailable())?;
    let before = match platform_native::tool_files::Facts::capture(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(unavailable()),
    };
    if !before.is_file() || before.links() != 1 || before.len() > MAX_POINTER_BYTES {
        return Err(unavailable());
    }
    let file = platform_native::filesystem::open_read(
        &path,
        platform_native::filesystem::ReadOptions::NoLinksNonblocking,
    )
    .map_err(|_| unavailable())?;
    let opened = platform_native::tool_files::Facts::from_file(&file).map_err(|_| unavailable())?;
    if !opened.is_file() || opened.links() != 1 || !opened.same_object(&before) {
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

pub(super) fn remember(paths: &crate::RuntimePaths, record: &TaskRecord) -> Result<(), ApiError> {
    let project = record
        .project_id
        .as_deref()
        .filter(|project| crate::projects::valid_id(project))
        .ok_or_else(unavailable)?;
    if record.task_type != "build" || !valid_task_id(&record.task_id) {
        return Err(unavailable());
    }
    // A malformed/special pointer is evidence, not something a new build may
    // silently replace. An absent pointer means no build has been submitted.
    read_pointer(paths, project)?;
    atomic_json(
        &path(paths, project),
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
    paths: &crate::RuntimePaths,
    project: &str,
    records: &HashMap<String, TaskRecord>,
) -> BuildResultRecovery {
    let outcome = (|| {
        if !crate::projects::valid_id(project) {
            return Err(unavailable());
        }
        let Some(pointer) = read_pointer(paths, project)? else {
            return Ok(BuildResultRecovery {
                task: None,
                selection: "none",
                issues: vec![],
            });
        };
        let record = records
            .get(&pointer.task_id)
            .filter(|record| {
                record.task_type == "build" && record.project_id.as_deref() == Some(project)
            })
            .ok_or_else(unavailable)?;
        Ok(BuildResultRecovery {
            task: Some(record.clone()),
            selection: "recorded-submission",
            issues: vec![],
        })
    })();
    outcome.unwrap_or_else(|error| BuildResultRecovery {
        task: None,
        selection: "unavailable",
        issues: vec![error],
    })
}
