//! Windows anonymous HTTPS Git commands. No shell or host configuration.
use super::*;
use platform_native::windows::system_paths::SystemPaths;

pub(super) fn valid_cwd(cwd: &Path) -> bool {
    cwd.to_str()
        .is_some_and(|path| !path.chars().any(char::is_control))
}
pub(super) fn supported_version(version: &str) -> bool {
    ["2.54.0", "2.56.0"].iter().any(|base| {
        version == *base
            || version.strip_prefix(base).is_some_and(|suffix| {
                suffix.strip_prefix(".windows.").is_some_and(|revision| {
                    !revision.is_empty() && revision.bytes().all(|c| c.is_ascii_digit())
                }) || suffix
                    .strip_prefix(" (Apple Git-")
                    .and_then(|v| v.strip_suffix(')'))
                    .is_some_and(|v| !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit()))
            })
    })
}
pub(super) fn command(tools: &GitToolchain, cwd: &Path) -> Result<Command, ApiError> {
    let system = SystemPaths::current().map_err(|_| temporary_error())?;
    let cwd = crate::node_tool_layout::command_argument(cwd)?;
    let mut command = Command::new(crate::node_tool_layout::command_argument(&tools.git.path)?);
    command
        .env_clear()
        .current_dir(&cwd)
        .env("PATH", &tools.path)
        .env("LC_ALL", "C")
        .env("SystemRoot", &system.windows)
        .env("HOME", &cwd)
        .env("USERPROFILE", &cwd)
        .env("XDG_CONFIG_HOME", &cwd)
        .env("TEMP", &cwd)
        .env("TMP", &cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "NUL")
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_CEILING_DIRECTORIES", cwd.parent().unwrap_or(&cwd))
        .env("GIT_DISCOVERY_ACROSS_FILESYSTEM", "0")
        .env("GIT_ALLOW_PROTOCOL", "https")
        .env("GIT_PROTOCOL_FROM_USER", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "NUL")
        .env("GIT_SSH", "NUL")
        .env("GIT_SSH_COMMAND", "NUL")
        .env("SSH_ASKPASS", "NUL")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(path) = &tools.exec_path {
        command.env(
            "GIT_EXEC_PATH",
            crate::node_tool_layout::command_argument(path)?,
        );
    }
    if let Some(path) = &tools.template_path {
        command.env(
            "GIT_TEMPLATE_DIR",
            crate::node_tool_layout::command_argument(path)?,
        );
    }
    for setting in [
        "credential.helper=",
        "credential.interactive=false",
        "core.askPass=NUL",
        "protocol.allow=never",
        "protocol.https.allow=always",
        "http.followRedirects=false",
        "http.emptyAuth=false",
        "http.proactiveAuth=none",
        "http.delegation=none",
        "http.extraHeader=",
        "http.proxy=",
        "http.sslVerify=true",
        "http.saveCookies=false",
        "core.longpaths=true",
    ] {
        command.arg("-c").arg(setting);
    }
    Ok(command)
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
