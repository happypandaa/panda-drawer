//! Original isolated npm command, with its arguments and environment unchanged.
use super::{Result, Toolchain};
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub(super) fn command(
    tools: &Toolchain,
    cwd: &Path,
    operation: &Path,
    home: PathBuf,
    cache: PathBuf,
    user_config: PathBuf,
    global_config: PathBuf,
) -> Result<Command> {
    let mut command = Command::new(&tools.node.path);
    command
        .arg(&tools.npm.path)
        .arg("--prefix")
        .arg(cwd)
        .arg("--global=false")
        .arg("--logs-dir")
        .arg(operation.join("npm-disabled-output"))
        .args([
            "ci",
            "--omit=dev",
            "--ignore-scripts",
            "--logs-max=0",
            "--no-audit",
            "--no-fund",
            "--fetch-retries=2",
            "--fetch-timeout=60000",
        ])
        .current_dir(cwd)
        .env_clear()
        .env("PATH", &tools.path)
        .env("HOME", &home)
        .env("NPM_CONFIG_USERCONFIG", &user_config)
        .env("NPM_CONFIG_GLOBALCONFIG", &global_config)
        .env("NPM_CONFIG_CACHE", &cache)
        .env("NPM_CONFIG_LOGS_MAX", "0")
        .env("NPM_CONFIG_UPDATE_NOTIFIER", "false")
        .env("GIT_CEILING_DIRECTORIES", operation)
        .env("NO_COLOR", "1");
    Ok(command)
}
