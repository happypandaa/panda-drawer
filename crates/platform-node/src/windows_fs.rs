//! Windows handle-backed boundaries shared by slot/configuration and launch.
use crate::ApiError;
use platform_host::{WindowsSourceDirectory, WorkspaceDirectory, WorkspaceError};
use std::path::{Path, PathBuf};

pub(crate) fn error(value: WorkspaceError) -> ApiError {
    ApiError::new(value.code, value.message)
}

pub fn runtime_path_guard(
    path: &Path,
    directory: bool,
    private: bool,
) -> Result<WindowsSourceDirectory, ApiError> {
    if directory {
        if private {
            WindowsSourceDirectory::open_private(path)
        } else {
            WindowsSourceDirectory::open(path)
        }
        .map_err(error)
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| ApiError::new("NODE_PATH_INVALID", "文件没有普通父目录。"))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| ApiError::new("NODE_PATH_INVALID", "文件名称无效。"))?;
        let guard = runtime_path_guard(parent, true, private)?;
        guard.pin_file(name).map_err(error)
    }
}

pub fn checked_runtime_path(path: &Path, directory: bool) -> Result<PathBuf, ApiError> {
    let guard = runtime_path_guard(path, directory, false)?;
    if directory {
        Ok(guard.path().to_owned())
    } else {
        Ok(guard.path().join(
            path.file_name()
                .ok_or_else(|| ApiError::new("NODE_PATH_INVALID", "文件名无效。"))?,
        ))
    }
}
pub fn private_runtime_directory(path: &Path) -> Result<WindowsSourceDirectory, ApiError> {
    runtime_path_guard(path, true, true)
}
pub fn create_private_runtime_dir(path: &Path) -> Result<WindowsSourceDirectory, ApiError> {
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::new("NODE_PATH_INVALID", "目录没有普通父目录。"))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ApiError::new("NODE_PATH_INVALID", "目录名称无效。"))?;
    let parent = private_runtime_directory(parent)?;
    match parent.create_private_directory(name) {
        Ok(directory) => Ok(directory),
        Err(value) if value.code == "WORKSPACE_ALREADY_EXISTS" => {
            parent.open_directory(name).map_err(error)
        }
        Err(value) => Err(error(value)),
    }
}
pub(crate) fn managed_directory(
    root: &WorkspaceDirectory,
    path: &Path,
) -> Result<WindowsSourceDirectory, ApiError> {
    root.verify_binding().map_err(error)?;
    let relative = path
        .strip_prefix(root.path())
        .map_err(|_| ApiError::new("NODE_ROOT_BOUNDARY", "受管路径不属于当前已准入数据根。"))?;
    let mut directory = WindowsSourceDirectory::from_workspace(root).map_err(error)?;
    for part in relative.components() {
        let std::path::Component::Normal(name) = part else {
            return Err(ApiError::new(
                "NODE_PATH_INVALID",
                "受管路径包含非普通片段。",
            ));
        };
        directory = directory
            .open_directory(
                name.to_str()
                    .ok_or_else(|| ApiError::new("NODE_PATH_INVALID", "目录名无效。"))?,
            )
            .map_err(error)?;
    }
    Ok(directory)
}
