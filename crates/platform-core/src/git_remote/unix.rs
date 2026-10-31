//! Original Unix remote environment and version policy.
use super::*;
pub(super) fn supported_version(version: &str) -> bool {
    ["2.54.0", "2.56.0"].iter().any(|base| {
        version == *base
            || version
                .strip_prefix(base)
                .and_then(|suffix| suffix.strip_prefix(" (Apple Git-")?.strip_suffix(')'))
                .is_some_and(|vendor| {
                    !vendor.is_empty() && vendor.bytes().all(|byte| byte.is_ascii_digit())
                })
    })
}
pub(super) fn isolated_command(toolchain: &GitToolchain, cwd: &Path) -> Command {
    let mut command = Command::new(&toolchain.git.path);
    command
        .env_clear()
        .current_dir(cwd)
        .env("PATH", &toolchain.path)
        .env("LC_ALL", "C")
        .env("HOME", cwd)
        .env("XDG_CONFIG_HOME", cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_CEILING_DIRECTORIES", cwd.parent().unwrap_or(cwd))
        .env("GIT_DISCOVERY_ACROSS_FILESYSTEM", "0")
        .env("GIT_ALLOW_PROTOCOL", "https")
        .env("GIT_PROTOCOL_FROM_USER", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/usr/bin/false")
        .env("GIT_SSH", "/usr/bin/false")
        .env("GIT_SSH_COMMAND", "/usr/bin/false")
        .env("SSH_ASKPASS", "/usr/bin/false")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::owned_process::prepare_command(&mut command);
    if let Some(exec_path) = &toolchain.exec_path {
        command.env("GIT_EXEC_PATH", exec_path);
    }
    if let Some(template_path) = &toolchain.template_path {
        command.env("GIT_TEMPLATE_DIR", template_path);
    }
    for setting in [
        "credential.helper=",
        "credential.interactive=false",
        "core.askPass=/usr/bin/false",
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
    ] {
        command.arg("-c").arg(setting);
    }
    // No cookieFile is configured: even an empty value enables libcurl's cookie engine.
    command
}
pub(super) fn command(tools: &GitToolchain, cwd: &Path) -> Result<Command, ApiError> {
    Ok(isolated_command(tools, cwd))
}
pub(super) fn valid_cwd(cwd: &Path) -> bool {
    cwd.to_str()
        .is_some_and(|path| !path.contains(':') && !path.chars().any(char::is_control))
}
