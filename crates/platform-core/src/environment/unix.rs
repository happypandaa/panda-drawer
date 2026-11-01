//! Original Unix discovery and probe environment.
use super::version_tuple;
use crate::{managed_toolchain::VerifiedToolchain, ApiError};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use tokio::process::Command;

pub(super) fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    // nvm installs are discoverable even when Finder supplies /usr/bin:/bin only.
    if let Some(home) = dirs::home_dir() {
        if let Ok(versions) = fs::read_dir(home.join(".nvm/versions/node")) {
            let mut found: Vec<_> = versions
                .filter_map(Result::ok)
                .map(|entry| entry.path().join("bin/node"))
                .collect();
            found.sort_by_key(|path| {
                std::cmp::Reverse(
                    version_tuple(
                        path.parent()
                            .and_then(Path::parent)
                            .and_then(Path::file_name)
                            .and_then(|x| x.to_str())
                            .unwrap_or_default(),
                    )
                    .unwrap_or((0, 0, 0)),
                )
            });
            paths.extend(found);
        }
        paths.extend([home.join(".volta/bin/node"), home.join(".local/bin/node")]);
    }
    for base in [
        "/opt/homebrew/opt/node@24/bin",
        "/opt/homebrew/opt/node@22/bin",
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
    ] {
        paths.push(Path::new(base).join("node"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path).map(|path| path.join("node")));
    }
    let mut seen = HashSet::new();
    paths.retain(|path| path.is_file() && seen.insert(path.clone()));
    paths
}

fn original_tool_path(node: &Path) -> String {
    format!(
        "{}:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin",
        node.parent().unwrap_or(Path::new("/usr/bin")).display()
    )
}

pub(super) fn tool_path(node: &Path) -> Result<String, ApiError> {
    Ok(original_tool_path(node))
}

pub(super) fn managed_path(verified: &VerifiedToolchain) -> Result<String, ApiError> {
    std::env::join_paths([
        verified.root.join("bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ])
    .map_err(|_| {
        ApiError::new(
            "MANAGED_NODE_INVALID",
            "受管开发工具路径不能用于隔离的 PATH。",
        )
    })?
    .into_string()
    .map_err(|_| ApiError::new("MANAGED_NODE_INVALID", "受管开发工具路径不是有效文本。"))
}

fn original_managed_command(verified: &VerifiedToolchain, path: &str) -> Command {
    let mut command = Command::new(&verified.node);
    command
        .env_clear()
        .current_dir(&verified.root)
        .env("PATH", path)
        .env("LC_ALL", "C")
        .env("HOME", &verified.root)
        .env(
            "NPM_CONFIG_USERCONFIG",
            verified.root.join(".unused-user-npmrc"),
        )
        .env(
            "NPM_CONFIG_GLOBALCONFIG",
            verified.root.join(".unused-global-npmrc"),
        );
    command
}

pub(super) fn managed_command(
    verified: &VerifiedToolchain,
    path: &str,
) -> Result<Command, ApiError> {
    Ok(original_managed_command(verified, path))
}
