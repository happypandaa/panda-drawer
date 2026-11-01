//! Isolated npm invocation shared by the install and build command.
use crate::{environment::Toolchain, ApiError};
use std::path::Path;
use tokio::process::Command;

pub(super) fn command(
    tools: &Toolchain,
    cwd: &Path,
    operation: &Path,
    cache: &Path,
) -> Result<Command, ApiError> {
    let argument = crate::node_tool_layout::command_argument;
    let node = argument(&tools.node.path)?;
    let npm = argument(&tools.npm.path)?;
    let cwd = argument(cwd)?;
    let operation = argument(operation)?;
    let cache = argument(cache)?;
    let home = operation.join("home");
    crate::project_file_io::ensure_directory(&home)?;
    let user_config = operation.join("user.npmrc");
    let global_config = operation.join("global.npmrc");
    for path in [&user_config, &global_config] {
        match platform_native::filesystem::create_new_private_file(path, false) {
            Ok(file) => file.sync_all()?,
            Err(issue) if issue.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(issue) => return Err(issue.into()),
        }
        crate::projects::check_no_symlinks(path, false)?;
        let file = platform_native::filesystem::open_read(
            path,
            platform_native::filesystem::ReadOptions::NoLinks,
        )?;
        if !file.metadata()?.is_file() || file.metadata()?.len() != 0 {
            return Err(ApiError::new(
                "BUILD_NPM_CONFIG_CHANGED",
                "平台隔离的 npm 配置文件已变化，构建未继续。",
            ));
        }
    }
    let mut command = Command::new(node);
    command
        .arg(npm)
        .arg("--prefix")
        .arg(&cwd)
        .arg("--global=false")
        .arg("--cache")
        .arg(&cache)
        .arg("--userconfig")
        .arg(&user_config)
        .arg("--globalconfig")
        .arg(&global_config)
        .arg("--logs-dir")
        .arg(operation.join("npm-disabled-output"))
        .args([
            "--logs-max=0",
            "--no-audit",
            "--no-fund",
            "--fetch-retries=2",
            "--fetch-timeout=60000",
        ])
        .current_dir(&cwd)
        .env_clear()
        .env("PATH", &tools.path)
        .env("HOME", &home)
        .env("NPM_CONFIG_USERCONFIG", &user_config)
        .env("NPM_CONFIG_GLOBALCONFIG", &global_config)
        .env("NPM_CONFIG_CACHE", &cache)
        .env("NPM_CONFIG_LOGS_MAX", "0")
        .env("NPM_CONFIG_UPDATE_NOTIFIER", "false")
        .env("NO_COLOR", "1");
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let system = platform_native::windows::system_paths::SystemPaths::current()?;
        if cwd.as_os_str().encode_wide().count() >= 258 {
            command.current_dir(&system.system);
        }
        command
            .env("SystemRoot", &system.windows)
            .env("WINDIR", &system.windows)
            .env("ComSpec", system.system.join("cmd.exe"));
        for key in ["USERPROFILE", "APPDATA", "LOCALAPPDATA", "TEMP", "TMP"] {
            command.env(key, &home);
        }
    }
    #[cfg(not(windows))]
    {
        command.env("TMPDIR", &home);
    }
    Ok(command)
}
