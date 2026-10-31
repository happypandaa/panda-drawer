//! Original Unix Git selection and offline command policy.
use super::*;
pub(super) fn tool_path(executable: &Path) -> Result<String, ApiError> {
    std::env::join_paths([
        executable
            .parent()
            .unwrap_or(Path::new("/usr/bin"))
            .to_path_buf(),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ])
    .ok()
    .and_then(|value| value.into_string().ok())
    .ok_or_else(|| ApiError::new("INVALID_GIT_PATH", "所选工具路径无法用于隔离的 PATH。"))
}

pub(super) fn isolated_command(executable: &Path, path: &str) -> Command {
    let mut command = Command::new(executable);
    command
        .env_clear()
        .current_dir("/")
        .env("PATH", path)
        .env("LC_ALL", "C")
        .env("HOME", "/dev/null")
        .env("XDG_CONFIG_HOME", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::owned_process::prepare_command(&mut command);
    command
}

pub(super) fn developer_git(output: &[u8]) -> Result<PathBuf, ApiError> {
    let developer = single_line(output)
        .map(PathBuf::from)
        .filter(|path| absolute_clean(path))
        .ok_or_else(|| developer_tools_error("无法识别已选择的 Apple 开发工具目录。"))?;
    let git = ordinary_executable(&developer.join("usr/bin/git"))
        .map_err(|_| developer_tools_error("已选择的 Apple 开发工具中没有可执行的 Git。"))?;
    if git == Path::new("/usr/bin/git") {
        return Err(developer_tools_error("Apple Git 路径仍指向系统安装入口。"));
    }
    Ok(git)
}

pub(super) fn developer_tools_error(message: &str) -> ApiError {
    ApiError::new("GIT_DEVELOPER_TOOLS_UNAVAILABLE", message)
}

pub(super) async fn local_git_path(
    candidate: &Path,
    processes: &OwnedProcessRegistry,
    keep_alive: ProbeKeepAlive,
) -> Result<PathBuf, ApiError> {
    let path = ordinary_executable(candidate)?;
    if cfg!(target_os = "macos") && path == Path::new("/usr/bin/git") {
        // Never execute Apple's shim or xcrun: either may offer an installation UI.
        let mut command = isolated_command(Path::new("/usr/bin/xcode-select"), SYSTEM_PATH);
        command.arg("-p");
        let output = run_probe(command, Duration::from_secs(5), processes, keep_alive)
            .await
            .map_err(|error| {
                if error.code.starts_with("PROCESS_") {
                    error
                } else {
                    developer_tools_error(
                        "无法查询已安装的 Apple 开发工具；未运行系统 Git 安装入口。",
                    )
                }
            })?;
        if !output.status.success() {
            return Err(developer_tools_error(
                "尚未选择可用的 Apple 开发工具；未运行系统 Git 安装入口。",
            ));
        }
        return developer_git(&output.stdout);
    }
    Ok(path)
}

pub(super) fn system_ssh() -> Result<PathBuf, ApiError> {
    Ok(PathBuf::from("/usr/bin/ssh"))
}

pub(super) fn ssh_path(_path: &Path) -> Result<String, ApiError> {
    Ok(SYSTEM_PATH.into())
}

pub(super) fn probe(executable: &Path, path: &str) -> Result<Command, ApiError> {
    Ok(isolated_command(executable, path))
}
