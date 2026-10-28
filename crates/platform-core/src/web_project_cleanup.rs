//! Final material purge after the web project's real writers have drained.
use crate::{
    builds::BuildManager,
    projects::check_no_symlinks,
    web_project_ownership::{fixed_paths, WebProjectOwnership},
    ApiError, ProjectStore, RuntimePaths,
};
use std::{
    fs,
    path::{Path, PathBuf},
};
type Result<T> = std::result::Result<T, ApiError>;
pub(crate) fn project_material_paths(paths: &RuntimePaths, id: &str) -> Result<Vec<PathBuf>> {
    WebProjectOwnership::new(paths.data_root().into(), paths.workspace_root().into())
        .material_paths(id)
}
pub(crate) fn remove_project_materials(
    paths: &RuntimePaths,
    store: &ProjectStore,
    tasks: &BuildManager,
    id: &str,
    delete_task: &str,
) -> Result<()> {
    if store.active_task(id).is_some() {
        return Err(ApiError::new("PROJECT_BUSY", "项目写入者尚未结束。"));
    }
    crate::durable_writer::require_owner_clear(&crate::durable_writer::WriterOwner::web(
        paths.data_root(),
        id,
    )?)?;
    let ownership =
        WebProjectOwnership::new(paths.data_root().into(), paths.workspace_root().into());
    let owner_present = ownership.path(id)?.exists();
    let task_ids = if owner_present {
        ownership.task_ids(id)?
    } else {
        tasks.project_task_ids(id)
    };
    for task in &task_ids {
        if tasks.task(task).is_ok_and(|task| !task.status.is_final()) {
            return Err(ApiError::new("PROJECT_BUSY", "项目任务尚未结束。"));
        }
    }
    // A previous worker can have moved an external file while its public name
    // is now absent. Resume that exact saved plan before ordinary material
    // verification; never recapture the staged file as a new authorization.
    if owner_present {
        for plan in ownership.file_deletion_plans(id)? {
            crate::web_file_deletion::remove_owned_file(
                &ownership,
                id,
                &plan.original,
                &plan.expected,
            )?;
            ownership.mark_material_removed(id, &plan.original, &plan.expected)?;
        }
    }
    let mut material_paths = if owner_present {
        ownership.material_paths(id)?
    } else {
        fixed_paths(paths.workspace_root(), id)
    };
    if !owner_present {
        for task in &task_ids {
            material_paths.push(paths.workspace_root().join(".platform/tasks").join(task));
            material_paths.push(
                paths
                    .workspace_root()
                    .join(".platform/operations")
                    .join(task),
            );
        }
    }
    let keep_task = paths
        .workspace_root()
        .join(".platform/tasks")
        .join(delete_task);
    for path in material_paths.into_iter().filter(|path| path != &keep_task) {
        if ownership.path(id)?.exists() {
            if let Some(identity) = ownership.material_identity(id, &path)? {
                if remove_path_with_identity(&ownership, id, &path, &identity)? {
                    ownership.mark_material_removed(id, &path, &identity)?;
                }
            }
        } else {
            check_no_symlinks(&path, true)?;
            match fs::symlink_metadata(&path) {
                Ok(_) => {
                    return Err(ApiError::new(
                        "WEB_PROJECT_OWNERSHIP_INVALID",
                        "项目材料缺少本次删除归属，未接管或删除。",
                    ))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
    }
    tasks.forget_owned_tasks(
        &task_ids
            .into_iter()
            .filter(|task| task != delete_task)
            .collect::<Vec<_>>(),
    )
}
pub(crate) fn finish_project_ownership(paths: &RuntimePaths, id: &str) -> Result<()> {
    WebProjectOwnership::new(paths.data_root().into(), paths.workspace_root().into()).finalize(id)
}
pub(crate) fn project_materials_absent(
    paths: &RuntimePaths,
    tasks: &BuildManager,
    id: &str,
) -> Result<bool> {
    let ownership =
        WebProjectOwnership::new(paths.data_root().into(), paths.workspace_root().into());
    check_no_symlinks(&ownership.directory(id)?, true)?;
    match fs::symlink_metadata(ownership.directory(id)?) {
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    for path in fixed_paths(paths.workspace_root(), id) {
        check_no_symlinks(&path, true)?;
        if path.exists() {
            return Ok(false);
        }
    }
    Ok(tasks.project_task_ids(id).is_empty())
}
fn remove_path_with_identity(
    ownership: &WebProjectOwnership,
    id: &str,
    path: &Path,
    identity: &crate::file_identity::PersistedIdentity,
) -> Result<bool> {
    check_no_symlinks(path, true)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            identity.absent_on_expected_volume(path).map_err(|error| {
                ApiError::new("WEB_PROJECT_MATERIAL_UNAVAILABLE", error.to_string())
            })?;
            return Ok(false);
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.is_dir() {
        crate::history_cleanup::remove_owned_directory(path, identity)?;
    } else if metadata.is_file() {
        crate::web_file_deletion::remove_owned_file(ownership, id, path, identity)?;
        return Ok(true);
    } else {
        return Err(ApiError::new("UNSAFE_PATH", "项目材料不是普通文件或目录。"));
    }
    Ok(false)
}
