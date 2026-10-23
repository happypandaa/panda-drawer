//! A bounded, anonymous HTTPS read of an explicitly supplied GitHub/Gitee remote.
//! This does not clone, create a repository, save credentials, or change doctor state.
//! The temporary working directory and process environment are private to this check.
use crate::{
    environment::ExecutableInfo,
    git_environment::{GitEnvironmentManager, GitToolchain},
    ApiError,
};
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Stdio, sync::Arc, time::Duration};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    sync::{watch, Semaphore},
    time::Instant,
};

const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
const OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitRemoteCheckInput {
    pub url: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRemoteHead {
    pub oid: String,
    pub symbolic_ref: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRemoteCheckResult {
    pub url: String,
    pub provider: String,
    pub transport: String,
    pub state: String,
    pub head: Option<GitRemoteHead>,
    pub git: ExecutableInfo,
    pub checked_at: String,
}
#[derive(Clone)]
pub struct GitRemoteChecker {
    available: Arc<Semaphore>,
}
impl Default for GitRemoteChecker {
    fn default() -> Self {
        Self::new()
    }
}
impl GitRemoteChecker {
    pub fn new() -> Self {
        Self {
            available: Arc::new(Semaphore::new(1)),
        }
    }
    pub async fn check(
        &self,
        environment: &GitEnvironmentManager,
        input: GitRemoteCheckInput,
        stop: watch::Receiver<bool>,
    ) -> Result<GitRemoteCheckResult, ApiError> {
        self.check_with_timeout(environment, input, stop, CHECK_TIMEOUT)
            .await
    }
    async fn check_with_timeout(
        &self,
        environment: &GitEnvironmentManager,
        input: GitRemoteCheckInput,
        mut stop: watch::Receiver<bool>,
        timeout: Duration,
    ) -> Result<GitRemoteCheckResult, ApiError> {
        let deadline = Instant::now() + timeout;
        let remote = validate_url(&input.url)?;
        let _permit = self.available.try_acquire().map_err(|_| {
            ApiError::new(
                "GIT_REMOTE_CHECK_BUSY",
                "已有一个远端检查正在进行，请等待其结束。",
            )
        })?;
        checkpoint(&stop, deadline)?;
        // Resolve exactly once. A later settings edit does not switch this request's tool.
        let toolchain = tokio::select! {
            biased;
            _ = stopped(&mut stop) => return Err(cancelled()),
            _ = tokio::time::sleep_until(deadline) => return Err(timed_out()),
            result = environment.resolve() => result.map_err(|_| {
                ApiError::new("GIT_REMOTE_ENVIRONMENT_UNAVAILABLE", "Git 工具不可用，请先检查 Git 环境设置。")
            })?,
        };
        if !supported_version(&toolchain.git.version) {
            return Err(ApiError::new(
                "GIT_REMOTE_VERSION_UNSUPPORTED",
                "此远端检查仅验证支持 Git 2.54.0 和 2.56.0；当前工具选择保持不变。",
            ));
        }
        checkpoint(&stop, deadline)?;
        let temporary = tempfile::Builder::new()
            .prefix("local-projects-git-remote-check-")
            .tempdir()
            .map_err(|_| temporary_error())?;
        let cwd = temporary
            .path()
            .canonicalize()
            .map_err(|_| temporary_error())?;
        // The ceiling variable is a colon-delimited path list. Reject ambiguous paths.
        if cwd
            .to_str()
            .is_none_or(|path| path.contains(':') || path.chars().any(char::is_control))
        {
            return Err(temporary_error());
        }
        checkpoint(&stop, deadline)?;
        let command = command_for(&toolchain, &remote.url, &cwd);
        let output = execute(command, &mut stop, deadline).await?;
        if output.exit_code == 2 && !output.stdout.is_empty() {
            return Err(invalid_output());
        }
        let head = parse_head(&output.stdout)?;
        checkpoint(&stop, deadline)?;
        Ok(GitRemoteCheckResult {
            url: remote.url,
            provider: remote.provider.into(),
            transport: "public-https".into(),
            state: if head.is_some() {
                "readable"
            } else {
                "readable-no-head"
            }
            .into(),
            head,
            git: toolchain.git,
            checked_at: OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .map_err(|_| ApiError::new("GIT_REMOTE_CHECK_FAILED", "无法记录远端检查时间。"))?,
        })
    }
}
struct Remote {
    url: String,
    provider: &'static str,
}
fn invalid_url() -> ApiError {
    ApiError::new(
        "GIT_REMOTE_URL_INVALID",
        "请输入标准 GitHub 或 Gitee 的 HTTPS 仓库地址，不包含凭据、端口、查询或额外路径。",
    )
}
fn validate_url(value: &str) -> Result<Remote, ApiError> {
    // Validate the original spelling, before a URL parser could normalize away an
    // explicit :443, escaped separator, dot segment, whitespace, or credentials.
    let (host, provider, path) = if let Some(path) = value.strip_prefix("https://github.com/") {
        ("github.com", "github", path)
    } else if let Some(path) = value.strip_prefix("https://gitee.com/") {
        ("gitee.com", "gitee", path)
    } else {
        return Err(invalid_url());
    };
    if value.len() > 1024 {
        return Err(invalid_url());
    }
    let path = path.strip_suffix('/').unwrap_or(path);
    let mut parts = path.split('/');
    let owner = parts.next().ok_or_else(invalid_url)?;
    let repo = parts.next().ok_or_else(invalid_url)?;
    if parts.next().is_some() || !safe_segment(owner) || !safe_segment(repo) {
        return Err(invalid_url());
    }
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if !safe_segment(repo) {
        return Err(invalid_url());
    }
    Ok(Remote {
        url: format!("https://{host}/{owner}/{repo}.git"),
        provider,
    })
}
fn safe_segment(value: &str) -> bool {
    (1..=255).contains(&value.len())
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}
fn supported_version(version: &str) -> bool {
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
fn cancelled() -> ApiError {
    ApiError::new(
        "GIT_REMOTE_CHECK_CANCELLED",
        "服务正在停止，远端检查已取消。",
    )
}
fn timed_out() -> ApiError {
    ApiError::new(
        "GIT_REMOTE_CHECK_TIMEOUT",
        "远端检查超过 20 秒限制，请稍后重试。",
    )
}
fn temporary_error() -> ApiError {
    ApiError::new("GIT_REMOTE_CHECK_FAILED", "无法创建隔离的远端检查目录。")
}
fn failed() -> ApiError {
    ApiError::new(
        "GIT_REMOTE_CHECK_FAILED",
        "公开 HTTPS 访问未成功；请检查仓库地址、公开访问权限和网络连接。",
    )
}
fn invalid_output() -> ApiError {
    ApiError::new(
        "GIT_REMOTE_OUTPUT_INVALID",
        "远端检查没有返回可识别的 HEAD 信息。",
    )
}
fn checkpoint(stop: &watch::Receiver<bool>, deadline: Instant) -> Result<(), ApiError> {
    if *stop.borrow() || stop.has_changed().is_err() {
        Err(cancelled())
    } else if Instant::now() >= deadline {
        Err(timed_out())
    } else {
        Ok(())
    }
}
async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow() {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}
fn isolated_command(toolchain: &GitToolchain, cwd: &Path) -> Command {
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
        .kill_on_drop(true)
        .process_group(0);
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
fn command_for(toolchain: &GitToolchain, url: &str, cwd: &Path) -> Command {
    let mut command = isolated_command(toolchain, cwd);
    command.args(["ls-remote", "--symref", "--exit-code", "--", url, "HEAD"]);
    command
}
struct Process {
    child: Child,
    group: Option<u32>,
}
impl Process {
    fn stop(&mut self) {
        if let Some(pid) = self.group.take() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
    async fn finish(&mut self) {
        self.stop();
        let _ = self.child.start_kill();
        // kill_on_drop also covers interruption while reaping the direct child.
        let _ = tokio::time::timeout(Duration::from_millis(250), self.child.wait()).await;
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}
struct Output {
    stdout: Vec<u8>,
    exit_code: i32,
}
async fn read_output(
    mut stream: impl tokio::io::AsyncRead + Unpin,
    keep: bool,
) -> Result<Vec<u8>, ApiError> {
    let mut output = Vec::new();
    let mut total = 0usize;
    let mut chunk = [0u8; 4096];
    loop {
        let count = stream.read(&mut chunk).await.map_err(|_| failed())?;
        if count == 0 {
            return Ok(output);
        }
        total += count;
        if total > OUTPUT_LIMIT {
            return Err(ApiError::new(
                "GIT_REMOTE_OUTPUT_LIMIT",
                "远端检查输出超过允许大小。",
            ));
        }
        if keep {
            output.extend_from_slice(&chunk[..count]);
        }
    }
}
async fn execute(
    mut command: Command,
    stop: &mut watch::Receiver<bool>,
    deadline: Instant,
) -> Result<Output, ApiError> {
    checkpoint(stop, deadline)?;
    let child = command.spawn().map_err(|_| failed())?;
    let mut process = Process {
        group: child.id(),
        child,
    };
    let stdout = process.child.stdout.take().ok_or_else(failed)?;
    let stderr = process.child.stderr.take().ok_or_else(failed)?;
    let result = tokio::select! {
        biased;
        _ = stopped(stop) => Err(cancelled()),
        _ = tokio::time::sleep_until(deadline) => Err(timed_out()),
        output = async {
            let (stdout, _, status) = tokio::try_join!(
                read_output(stdout, true), read_output(stderr, false),
                async { process.child.wait().await.map_err(|_| failed()) }
            )?;
            let exit_code = status.code().filter(|code| *code == 0 || *code == 2).ok_or_else(failed)?;
            Ok(Output { stdout, exit_code })
        } => output,
    };
    // Stop the complete group even when Git exited and closed both output streams.
    process.finish().await;
    result
}
fn valid_symbolic_ref(value: &str) -> bool {
    value.starts_with("refs/")
        && value.len() <= 4096
        && !value.ends_with('.')
        && !value.contains("..")
        && !value.contains("@{")
        && !value
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || "~^:?*[\\".contains(ch))
        && value
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}
fn parse_head(bytes: &[u8]) -> Result<Option<GitRemoteHead>, ApiError> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid_output())?;
    let mut oid = None;
    let mut symbolic_ref = None;
    for line in text.split_terminator('\n') {
        let (value, reference) = line.split_once('\t').ok_or_else(invalid_output)?;
        if reference.chars().any(char::is_control) || reference.is_empty() {
            return Err(invalid_output());
        }
        // The HEAD pattern also matches tails such as refs/heads/HEAD.
        if reference != "HEAD" {
            continue;
        }
        if let Some(target) = value.strip_prefix("ref: ") {
            if symbolic_ref.is_some() || !valid_symbolic_ref(target) {
                return Err(invalid_output());
            }
            symbolic_ref = Some(target.to_owned());
        } else {
            if oid.is_some()
                || ![40, 64].contains(&value.len())
                || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
                || value.bytes().all(|byte| byte == b'0')
            {
                return Err(invalid_output());
            }
            oid = Some(value.to_ascii_lowercase());
        }
    }
    Ok(oid.map(|oid| GitRemoteHead { oid, symbolic_ref }))
}

#[cfg(test)]
#[path = "git_remote_tests.rs"]
mod tests;
