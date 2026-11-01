//! Restart records interrupted tasks without retrying commands or changing source.
use crate::{
    builds::{atomic_json, valid_task_id},
    projects::check_no_symlinks,
    ApiError, RuntimePaths, TaskRecord, TaskStatus,
};
use std::{collections::HashMap, fs, io::Read};
pub(super) fn recover_startup(
    paths: &RuntimePaths,
) -> Result<HashMap<String, TaskRecord>, ApiError> {
    let tasks = paths.workspace_root().join(".platform/tasks");
    #[cfg(windows)]
    let _tasks = platform_native::windows::files::pin_directory(&tasks)?;
    let mut records = HashMap::new();
    for entry in fs::read_dir(&tasks)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_dir() {
            continue;
        }
        check_no_symlinks(&path, false)?;
        #[cfg(windows)]
        let _task = platform_native::windows::files::pin_directory(&path)?;
        let record_path = path.join("task.json");
        let mut file = match platform_native::filesystem::open_read(
            &record_path,
            platform_native::filesystem::ReadOptions::NoLinks,
        ) {
            Ok(file) => file,
            Err(issue) if issue.kind() == std::io::ErrorKind::NotFound => continue,
            Err(issue) => return Err(issue.into()),
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        drop(file);
        let mut record: TaskRecord = serde_json::from_slice(&bytes)?;
        if !valid_task_id(&record.task_id)
            || path.file_name().and_then(|value| value.to_str()) != Some(record.task_id.as_str())
        {
            return Err(ApiError::new(
                "TASK_RECORD_INVALID",
                "Task record identity does not match its directory",
            ));
        }
        if !record.status.is_final() {
            record.status = TaskStatus::Interrupted;
            record.phase = "interrupted".into();
            record.error = Some(ApiError::new(
                "TASK_INTERRUPTED",
                "平台在任务结束前退出；没有自动重试命令。",
            ));
            atomic_json(&record_path, &record)?;
        }
        records.insert(record.task_id.clone(), record);
    }
    Ok(records)
}
