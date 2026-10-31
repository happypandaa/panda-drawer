//! Native Windows npm invocation. Path conversion changes only equivalent
//! Win32 spelling; it grants no directory ownership or filesystem permission.
use super::{Result, Toolchain};
use platform_native::windows::system_paths::SystemPaths;
use std::{
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
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
    let argument = crate::node_tool_layout::command_argument;
    let node = argument(&tools.node.path)?;
    let npm = argument(&tools.npm.path)?;
    let cwd = argument(cwd)?;
    let operation = argument(operation)?;
    let logs = argument(&operation.join("npm-disabled-output"))?;
    let home = argument(&home)?;
    let cache = argument(&cache)?;
    let user_config = argument(&user_config)?;
    let global_config = argument(&global_config)?;
    let system = SystemPaths::current()?;
    let mut command = Command::new(node);
    command
        .arg(npm)
        .arg("--prefix")
        .arg(&cwd)
        .arg("--global=false")
        .arg("--logs-dir")
        .arg(logs)
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
        // CreateProcessW and Node chdir both reject a cwd beyond MAX_PATH.
        // npm's explicit --prefix selects the original project independently
        // of its launch cwd. Keep that target and all file operations long;
        // only the Win32 launch directory falls back to the system directory.
        .current_dir(if cwd.as_os_str().encode_wide().count() >= 258 {
            &system.system
        } else {
            &cwd
        })
        .env_clear()
        .env("PATH", &tools.path)
        .env("HOME", &home)
        .env("NPM_CONFIG_USERCONFIG", user_config)
        .env("NPM_CONFIG_GLOBALCONFIG", global_config)
        .env("NPM_CONFIG_CACHE", cache)
        .env("NPM_CONFIG_LOGS_MAX", "0")
        .env("NPM_CONFIG_UPDATE_NOTIFIER", "false")
        .env("GIT_CEILING_DIRECTORIES", operation)
        .env("NO_COLOR", "1")
        .env("SystemRoot", &system.windows)
        .env("WINDIR", &system.windows)
        .env("ComSpec", system.system.join("cmd.exe"));
    // Windows home/temp APIs do not use HOME alone. All npm and optional Node
    // cache/temp descendants remain in the already-created private operation
    // home, covered by the existing operation space scanner and phase owner.
    for key in ["USERPROFILE", "APPDATA", "LOCALAPPDATA", "TEMP", "TMP"] {
        command.env(key, &home);
    }
    Ok(command)
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
