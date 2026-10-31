//! Windows offline tool probes. Never discover credentials or host Git config.
use super::*;
use platform_native::windows::system_paths::SystemPaths;

fn system() -> Result<SystemPaths, ApiError> {
    SystemPaths::current().map_err(|e| {
        ApiError::new(
            "GIT_SYSTEM_PATH_UNAVAILABLE",
            format!("无法取得 Windows 系统工具目录：{e}"),
        )
    })
}
pub(super) fn tool_path(executable: &Path) -> Result<String, ApiError> {
    let system = system()?;
    let selected =
        crate::node_tool_layout::command_argument(executable.parent().unwrap_or(&system.system))?;
    std::env::join_paths([selected, system.system, system.windows])
        .ok()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| ApiError::new("INVALID_GIT_PATH", "所选工具路径无法用于隔离的 PATH。"))
}
pub(super) fn ssh_path(executable: &Path) -> Result<String, ApiError> {
    tool_path(executable)
}
pub(super) fn system_ssh() -> Result<PathBuf, ApiError> {
    Ok(system()?.system.join("OpenSSH/ssh.exe"))
}
pub(super) fn probe(executable: &Path, path: &str) -> Result<Command, ApiError> {
    let system = system()?;
    let mut command = Command::new(crate::node_tool_layout::command_argument(executable)?);
    command
        .env_clear()
        .current_dir(&system.system)
        .env("PATH", path)
        .env("LC_ALL", "C")
        .env("SystemRoot", &system.windows)
        .env("HOME", "NUL")
        .env("USERPROFILE", "NUL")
        .env("XDG_CONFIG_HOME", "NUL")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "NUL")
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    Ok(command)
}
pub(super) async fn local_git_path(
    candidate: &Path,
    _processes: &OwnedProcessRegistry,
    _keep_alive: ProbeKeepAlive,
) -> Result<PathBuf, ApiError> {
    ordinary_executable(candidate)
}
