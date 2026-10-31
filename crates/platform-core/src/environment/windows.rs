//! Native Windows discovery and the isolated managed Node probe environment.
use crate::{managed_toolchain::VerifiedToolchain, ApiError};
use platform_native::windows::system_paths::SystemPaths;
use std::{
    collections::HashSet,
    ffi::OsStr,
    path::{Path, PathBuf},
};
use tokio::process::Command;

pub(super) fn candidates() -> Vec<PathBuf> {
    candidates_from_path(std::env::var_os("PATH").as_deref())
}

fn candidates_from_path(path: Option<&OsStr>) -> Vec<PathBuf> {
    let mut paths: Vec<_> = path
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|path| path.join("node.exe"))
        .collect();
    let mut seen = HashSet::new();
    paths.retain(|path| path.is_file() && seen.insert(path.clone()));
    paths
}

fn system_paths() -> Result<SystemPaths, ApiError> {
    SystemPaths::current().map_err(|error| {
        ApiError::new(
            "TOOLCHAIN_SYSTEM_PATH_UNAVAILABLE",
            format!("无法取得 Windows 系统工具目录：{error}"),
        )
    })
}

pub(super) fn tool_path(node: &Path) -> Result<String, ApiError> {
    let system = system_paths()?;
    let selected =
        crate::node_tool_layout::command_argument(node.parent().unwrap_or(&system.system))?;
    let path = std::env::join_paths([&selected, &system.system, &system.windows])
        .map_err(|_| ApiError::new("INVALID_NODE_PATH", "Node 工具路径不能用于 PATH。"))?;
    // Toolchain.path has always been display text. Retain that representation
    // for local selections instead of adding a Windows-only UTF-8 admission.
    Ok(path.to_string_lossy().into_owned())
}

pub(super) fn managed_path(verified: &VerifiedToolchain) -> Result<String, ApiError> {
    let system = system_paths()?;
    let selected = crate::node_tool_layout::command_argument(&verified.root)?;
    std::env::join_paths([&selected, &system.system, &system.windows])
        .map_err(|_| {
            ApiError::new(
                "MANAGED_NODE_INVALID",
                "受管开发工具路径不能用于隔离的 PATH。",
            )
        })?
        .into_string()
        .map_err(|_| ApiError::new("MANAGED_NODE_INVALID", "受管开发工具路径不是有效文本。"))
}

pub(super) fn managed_command(
    verified: &VerifiedToolchain,
    path: &str,
) -> Result<Command, ApiError> {
    let system = system_paths()?;
    let mut command = Command::new(&verified.node);
    command
        .env_clear()
        .current_dir(&verified.root)
        .env("PATH", path)
        .env("LC_ALL", "C")
        .env("HOME", &verified.root)
        .env("SystemRoot", &system.windows)
        .env("WINDIR", &system.windows)
        .env("ComSpec", system.system.join("cmd.exe"))
        .env(
            "NPM_CONFIG_USERCONFIG",
            verified.root.join(".unused-user-npmrc"),
        )
        .env(
            "NPM_CONFIG_GLOBALCONFIG",
            verified.root.join(".unused-global-npmrc"),
        );
    // Windows home/temp APIs do not use HOME alone. Keep every probe-local
    // fallback in the verified tree, never in the user's real profile/temp.
    for key in ["USERPROFILE", "APPDATA", "LOCALAPPDATA", "TEMP", "TMP"] {
        command.env(key, &verified.root);
    }
    // npm enables Node's optional compile cache before loading its CLI. Version
    // probes must not create that cache or npm logs in an immutable package.
    command
        .env("NODE_DISABLE_COMPILE_CACHE", "1")
        .env("NPM_CONFIG_CACHE", verified.root.join(".npm-cache"))
        .env("NPM_CONFIG_UPDATE_NOTIFIER", "false")
        .env("NPM_CONFIG_LOGS_MAX", "0");
    Ok(command)
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
