//! Git tool selection is independent of Node and of repository/SSH authentication.
//! Doctor runs only offline version probes in an isolated environment. A successful
//! probe grants no repository operation and proves neither identity nor remote access.
use crate::{
    environment::ExecutableInfo,
    owned_process::{IoPlan, OwnedProcessRegistry, ProcessOptions},
    ApiError,
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

#[path = "git_environment/files.rs"]
mod files;
use files::{absolute_clean, ordinary_executable, read_settings_bytes, settings_error};

const MAX_PROBE_BYTES: usize = 16 * 1024;
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshEnvironmentInfo {
    pub ready: bool,
    pub selection: String,
    pub executable: Option<ExecutableInfo>,
    pub issues: Vec<ApiError>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitAuthenticationInfo {
    pub state: String,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitEnvironmentInfo {
    /// Only Git executable availability; does not imply repository or remote access.
    pub ready: bool,
    pub git: Option<ExecutableInfo>,
    pub selection: String,
    pub configured_git_path: Option<PathBuf>,
    pub managed_git_path: Option<PathBuf>,
    pub ssh: SshEnvironmentInfo,
    pub authentication: GitAuthenticationInfo,
    pub issues: Vec<ApiError>,
}
#[derive(Clone, Debug)]
pub struct GitToolchain {
    pub git: ExecutableInfo,
    pub path: String,
    pub exec_path: Option<PathBuf>,
    pub template_path: Option<PathBuf>,
}
#[derive(Clone)]
pub struct GitEnvironmentManager {
    settings_path: PathBuf,
    managed_root: Option<PathBuf>,
    processes: OwnedProcessRegistry,
    project_boundary: Option<crate::project_tool_boundary::ProjectToolBoundary>,
}
pub(crate) enum PreparedGitSelection {
    Local(PathBuf),
    Managed(crate::managed_git::VerifiedGit),
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    schema_version: u32,
    #[serde(deserialize_with = "required_git_path")]
    git_path: Option<PathBuf>,
}
fn required_git_path<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<PathBuf>, D::Error> {
    Option::<PathBuf>::deserialize(deserializer)
}

impl GitEnvironmentManager {
    pub fn new(data_root: PathBuf, managed_root: Option<PathBuf>) -> Self {
        Self::with_processes(data_root, managed_root, OwnedProcessRegistry::default())
    }
    pub(crate) fn with_processes(
        data_root: PathBuf,
        managed_root: Option<PathBuf>,
        processes: OwnedProcessRegistry,
    ) -> Self {
        Self {
            settings_path: data_root.join("git-toolchain.json"),
            managed_root,
            processes,
            project_boundary: None,
        }
    }
    pub(crate) fn with_project_boundary(mut self, workspace: PathBuf) -> Self {
        self.project_boundary = Some(crate::project_tool_boundary::ProjectToolBoundary::new(
            self.settings_path.parent().unwrap().into(),
            workspace,
        ));
        self
    }
    pub(crate) fn with_process_registry(&self, processes: OwnedProcessRegistry) -> Self {
        let mut scoped = self.clone();
        scoped.processes = processes;
        scoped
    }
    fn configured(&self) -> Result<Option<PathBuf>, ApiError> {
        let Some(bytes) = read_settings_bytes(&self.settings_path)? else {
            return Ok(None);
        };
        let settings: Settings = serde_json::from_slice(&bytes)
            .map_err(|_| settings_error("Git 工具选择文件格式无效，请重新选择 Git。"))?;
        if settings.schema_version != 1 {
            return Err(settings_error("Git 工具选择文件版本不受支持。"));
        }
        Ok(settings.git_path)
    }
    pub async fn configure_git(
        &self,
        path: Option<PathBuf>,
    ) -> Result<GitEnvironmentInfo, ApiError> {
        crate::platform_support::require_runtime().map_err(ApiError::from)?;
        // Check an existing settings file before replacement; never follow links.
        read_settings_bytes(&self.settings_path)?;
        let tools = if let Some(path) = &path {
            if let Some(boundary) = &self.project_boundary {
                boundary.require_path(path)?;
            }
            let git = local_git_path(path, &self.processes, Arc::new(())).await?;
            if let Some(boundary) = &self.project_boundary {
                boundary.require_path(&git)?;
            }
            Some(probe_git(git, None, None, &self.processes, Arc::new(())).await?)
        } else {
            None
        };
        crate::node_apps::development::with_source_ownership_admission(|| {
            if let (Some(boundary), Some(path), Some(tools)) =
                (&self.project_boundary, &path, &tools)
            {
                boundary.require_path(path)?;
                boundary.require_path(&tools.git.path)?;
            }
            crate::builds::atomic_json(
                &self.settings_path,
                &Settings {
                    schema_version: 1,
                    git_path: path,
                },
            )
        })?;
        Ok(self.doctor().await)
    }
    pub async fn doctor(&self) -> GitEnvironmentInfo {
        let mut info = GitEnvironmentInfo {
            ready: false,
            git: None,
            selection: "unresolved".into(),
            configured_git_path: None,
            managed_git_path: self
                .managed_root
                .as_ref()
                .map(|root| crate::managed_git::executable_path(root)),
            ssh: SshEnvironmentInfo {
                ready: false,
                selection: "system".into(),
                executable: None,
                issues: vec![],
            },
            authentication: GitAuthenticationInfo {
                state: "not-checked".into(),
                reason: "工具诊断不验证 SSH 身份或远端访问；版本检查不会读取私钥或连接仓库。"
                    .into(),
            },
            issues: vec![],
        };
        if let Err(error) = crate::platform_support::require_runtime() {
            info.issues.push(error.into());
            return info;
        }
        // SSH availability is independent; failure never invalidates a usable Git.
        info.ssh = inspect_ssh(Path::new("/usr/bin/ssh"), &self.processes).await;
        let configured = match self.configured() {
            Ok(value) => value,
            Err(error) => {
                info.issues.push(error);
                return info;
            }
        };
        info.selection = if configured.is_some() {
            "explicit"
        } else {
            "managed"
        }
        .into();
        info.configured_git_path = configured.clone();
        match self.selected(configured).await {
            Ok(toolchain) => {
                info.ready = true;
                info.git = Some(toolchain.git);
            }
            Err(error) => info.issues.push(error),
        }
        info
    }
    pub async fn resolve(&self) -> Result<GitToolchain, ApiError> {
        self.resolve_with_keep_alive(Arc::new(())).await
    }
    // A remote check keeps its actual admission and temporary directory alive
    // through version resolution too, even if its normal waiter disappears.
    pub(crate) async fn resolve_with_keep_alive(
        &self,
        keep_alive: ProbeKeepAlive,
    ) -> Result<GitToolchain, ApiError> {
        let prepared = self.prepare_resolution().await?;
        self.resolve_prepared_with_keep_alive(prepared, keep_alive)
            .await
    }
    // Preparation can own blocking verification before a process record exists.
    // Product callers await this phase before selecting on their stop/deadline.
    pub(crate) async fn prepare_resolution(&self) -> Result<PreparedGitSelection, ApiError> {
        crate::platform_support::require_runtime().map_err(ApiError::from)?;
        self.prepare_selection(self.configured()?).await
    }
    async fn selected(&self, configured: Option<PathBuf>) -> Result<GitToolchain, ApiError> {
        let prepared = self.prepare_selection(configured).await?;
        self.resolve_prepared_with_keep_alive(prepared, Arc::new(()))
            .await
    }
    async fn prepare_selection(
        &self,
        configured: Option<PathBuf>,
    ) -> Result<PreparedGitSelection, ApiError> {
        if let Some(path) = configured {
            return Ok(PreparedGitSelection::Local(path));
        }
        let root = self.managed_root.clone().ok_or_else(|| {
            ApiError::new(
                "MANAGED_GIT_UNAVAILABLE",
                "未找到私有 Git 资源，请修复应用安装或明确选择本机 Git。",
            )
        })?;
        // One complete verification per resolution, off the async executor.
        Ok(PreparedGitSelection::Managed(
            crate::managed_git::verify(root).await?,
        ))
    }
    // All asynchronous work in this phase belongs to an owned process record;
    // dropping its reply requests stop without dropping the real I/O or scope.
    pub(crate) async fn resolve_prepared_with_keep_alive(
        &self,
        prepared: PreparedGitSelection,
        keep_alive: ProbeKeepAlive,
    ) -> Result<GitToolchain, ApiError> {
        match prepared {
            PreparedGitSelection::Local(path) => {
                probe_local(&path, &self.processes, keep_alive).await
            }
            PreparedGitSelection::Managed(verified) => {
                probe_managed(verified, &self.processes, keep_alive).await
            }
        }
    }
}

fn tool_path(executable: &Path) -> Result<String, ApiError> {
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
fn isolated_command(executable: &Path, path: &str) -> Command {
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
struct ProbeOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: ExitStatus,
}
type ProbeKeepAlive = Arc<dyn Send + Sync>;
async fn read_probe(mut stream: impl tokio::io::AsyncRead + Unpin) -> Result<Vec<u8>, ApiError> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let count = stream
            .read(&mut chunk)
            .await
            .map_err(|_| ApiError::new("GIT_TOOL_PROBE_FAILED", "工具版本输出无法读取。"))?;
        if count == 0 {
            return Ok(bytes);
        }
        if bytes.len() + count > MAX_PROBE_BYTES {
            return Err(ApiError::new(
                "GIT_TOOL_PROBE_FAILED",
                "工具版本输出超过允许大小。",
            ));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}
async fn run_probe(
    command: Command,
    timeout: Duration,
    processes: &OwnedProcessRegistry,
    keep_alive: ProbeKeepAlive,
) -> Result<ProbeOutput, ApiError> {
    let (stderr_sender, mut stderr_receiver) = tokio::sync::oneshot::channel();
    let mut run = processes.start(
        command,
        ProcessOptions::new("git-environment", timeout),
        keep_alive,
        move |pipes| {
            drop(pipes.stdin);
            IoPlan::new(read_probe(pipes.stdout)).drain("stderr", async move {
                let stderr = read_probe(pipes.stderr).await?;
                let _ = stderr_sender.send(stderr);
                Ok(())
            })
        },
    )?;
    let output = tokio::select! {
        biased;
        _ = tokio::time::sleep(timeout) => {
            run.cancel();
            match run.await {
                Err(error) if error.code != "PROCESS_CANCELLED" && error.code != "PROCESS_WORK_TIMEOUT" => return Err(error),
                _ => return Err(probe_timeout()),
            }
        },
        output = &mut run => output.map_err(|error| match error.code.as_str() {
            "PROCESS_WORK_TIMEOUT" => probe_timeout(),
            "PROCESS_SPAWN_FAILED" => ApiError::new("GIT_TOOL_PROBE_FAILED", "工具版本检查无法启动。"),
            _ => error,
        })?,
    };
    let stderr = stderr_receiver
        .try_recv()
        .map_err(|_| ApiError::new("GIT_TOOL_PROBE_FAILED", "工具版本输出未完整收尾。"))?;
    Ok(ProbeOutput {
        stdout: output.value,
        stderr,
        status: output.status,
    })
}
fn probe_timeout() -> ApiError {
    ApiError::new("GIT_TOOL_PROBE_TIMEOUT", "工具版本检查超时。")
}
fn single_line(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    (!text.is_empty() && !text.chars().any(char::is_control)).then_some(text)
}
fn git_version(bytes: &[u8]) -> Result<String, ApiError> {
    let value = single_line(bytes)
        .and_then(|line| line.strip_prefix("git version "))
        .filter(|value| {
            value.split_whitespace().next().is_some_and(|version| {
                let parts: Vec<_> = version.split('.').collect();
                parts.len() >= 2
                    && parts.iter().take(2).all(|part| {
                        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                    })
            })
        })
        .ok_or_else(|| {
            ApiError::new(
                "GIT_TOOL_PROBE_FAILED",
                "所选工具没有返回可识别的 Git 版本。",
            )
        })?;
    // Preserve vendor suffixes. This is identification, not a feature support claim.
    Ok(value.to_owned())
}
fn developer_git(output: &[u8]) -> Result<PathBuf, ApiError> {
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
fn developer_tools_error(message: &str) -> ApiError {
    ApiError::new("GIT_DEVELOPER_TOOLS_UNAVAILABLE", message)
}
async fn local_git_path(
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
async fn probe_git(
    git: PathBuf,
    exec_path: Option<PathBuf>,
    template_path: Option<PathBuf>,
    processes: &OwnedProcessRegistry,
    keep_alive: ProbeKeepAlive,
) -> Result<GitToolchain, ApiError> {
    let path = tool_path(&git)?;
    let mut command = isolated_command(&git, &path);
    command.arg("--version");
    if let Some(value) = &exec_path {
        command.env("GIT_EXEC_PATH", value);
    }
    if let Some(value) = &template_path {
        command.env("GIT_TEMPLATE_DIR", value);
    }
    let output = run_probe(command, Duration::from_secs(5), processes, keep_alive).await?;
    if !output.status.success() {
        return Err(ApiError::new("GIT_TOOL_PROBE_FAILED", "Git 版本检查失败。"));
    }
    Ok(GitToolchain {
        git: ExecutableInfo {
            path: git,
            version: git_version(&output.stdout)?,
        },
        path,
        exec_path,
        template_path,
    })
}
async fn probe_local(
    candidate: &Path,
    processes: &OwnedProcessRegistry,
    keep_alive: ProbeKeepAlive,
) -> Result<GitToolchain, ApiError> {
    probe_git(
        local_git_path(candidate, processes, keep_alive.clone()).await?,
        None,
        None,
        processes,
        keep_alive,
    )
    .await
}
async fn probe_managed(
    verified: crate::managed_git::VerifiedGit,
    processes: &OwnedProcessRegistry,
    keep_alive: ProbeKeepAlive,
) -> Result<GitToolchain, ApiError> {
    debug_assert_eq!(
        verified.git,
        crate::managed_git::executable_path(&verified.root)
    );
    let toolchain = probe_git(
        verified.git,
        Some(verified.exec_path),
        Some(verified.template_path),
        processes,
        keep_alive,
    )
    .await?;
    if toolchain.git.version != verified.git_version {
        return Err(ApiError::new(
            "MANAGED_GIT_INVALID",
            "私有 Git 的版本输出与固定发行声明不一致。",
        ));
    }
    Ok(toolchain)
}
async fn inspect_ssh(path: &Path, processes: &OwnedProcessRegistry) -> SshEnvironmentInfo {
    let mut info = SshEnvironmentInfo {
        ready: false,
        selection: "system".into(),
        executable: None,
        issues: vec![],
    };
    let result = async {
        let path = ordinary_executable(path)
            .map_err(|_| ApiError::new("SSH_NOT_FOUND", "未找到可用的系统 SSH 工具。"))?;
        let mut command = isolated_command(&path, SYSTEM_PATH);
        command.arg("-V");
        let output = run_probe(command, Duration::from_secs(5), processes, Arc::new(())).await?;
        if !output.status.success() {
            return Err(ApiError::new("SSH_PROBE_FAILED", "系统 SSH 版本检查失败。"));
        }
        let version = single_line(&output.stderr)
            .or_else(|| single_line(&output.stdout))
            .filter(|line| line.starts_with("OpenSSH_"))
            .ok_or_else(|| ApiError::new("SSH_PROBE_FAILED", "系统 SSH 没有返回可识别的版本。"))?;
        Ok(ExecutableInfo {
            path,
            version: version.to_owned(),
        })
    }
    .await;
    match result {
        Ok(executable) => {
            info.ready = true;
            info.executable = Some(executable);
        }
        Err(error) => info.issues.push(error),
    }
    info
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use tempfile::{Builder, TempDir};

    fn disposable() -> (TempDir, PathBuf) {
        let temp = Builder::new()
            .prefix("disposable-git-environment-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        (temp, root)
    }
    fn script(root: &Path, name: &str, body: &str) -> PathBuf {
        let path = root.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
    fn fake_git(root: &Path) -> PathBuf {
        script(
            root,
            "synthetic-git",
            "printf 'git version 2.50.1 (Apple Git-155)\\n'",
        )
    }
    fn write_choice(root: &Path, path: &Path) {
        fs::write(
            root.join("git-toolchain.json"),
            serde_json::to_vec(&Settings {
                schema_version: 1,
                git_path: Some(path.to_path_buf()),
            })
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn global_git_selection_rejects_project_paths_and_both_symlink_directions_without_execution(
    ) {
        use std::os::unix::fs::symlink;
        let (_temporary, root) = disposable();
        let data = root.join("data");
        let workspace = root.join("workspace");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&workspace).unwrap();
        let manager =
            GitEnvironmentManager::new(data.clone(), None).with_project_boundary(workspace.clone());
        let external = fake_git(&root);
        manager.configure_git(Some(external.clone())).await.unwrap();
        let settings = fs::read(data.join("git-toolchain.json")).unwrap();
        let source = workspace.join("projects/disposable");
        fs::create_dir_all(&source).unwrap();
        let marker = root.join("project-git-was-executed");
        let project_git = script(
            &source,
            "git",
            &format!(
                "touch '{}'\nprintf 'git version 2.50.1\\n'",
                marker.display()
            ),
        );
        let outside_alias = root.join("outside-git-alias");
        symlink(&project_git, &outside_alias).unwrap();
        let inside_alias = source.join("external-git-alias");
        symlink(&external, &inside_alias).unwrap();
        for selected in [&project_git, &outside_alias, &inside_alias] {
            assert_eq!(
                manager
                    .configure_git(Some(selected.clone()))
                    .await
                    .unwrap_err()
                    .code,
                "GLOBAL_TOOL_PROJECT_OWNED"
            );
            assert_eq!(fs::read(data.join("git-toolchain.json")).unwrap(), settings);
        }
        assert!(!marker.exists(), "a rejected project Git must not execute");
    }
    #[tokio::test]
    async fn explicit_choice_wins_and_disappearance_never_falls_back() {
        let (_temp, root) = disposable();
        let git = fake_git(&root);
        write_choice(&root, &git);
        let previous = fs::read(root.join("git-toolchain.json")).unwrap();
        let manager = GitEnvironmentManager::new(root.clone(), Some(root.join("missing-managed")));
        let toolchain = manager
            .selected(manager.configured().unwrap())
            .await
            .unwrap();
        assert_eq!(toolchain.git.path, git);
        assert_eq!(toolchain.git.version, "2.50.1 (Apple Git-155)");
        assert!(toolchain.exec_path.is_none());
        fs::remove_file(&git).unwrap();
        assert_eq!(
            manager
                .selected(manager.configured().unwrap())
                .await
                .unwrap_err()
                .code,
            "GIT_NOT_FOUND"
        );
        assert_eq!(fs::read(root.join("git-toolchain.json")).unwrap(), previous);
    }

    #[tokio::test]
    async fn no_private_resource_is_unavailable_and_does_not_discover_host_git() {
        let (_temp, root) = disposable();
        let manager = GitEnvironmentManager::new(root.clone(), None);
        assert_eq!(
            manager.selected(None).await.unwrap_err().code,
            "MANAGED_GIT_UNAVAILABLE"
        );
        assert!(!root.join("git-toolchain.json").exists());
        let manager = GitEnvironmentManager::new(root.clone(), Some(root.join("missing-managed")));
        let error = manager.selected(None).await.unwrap_err();
        assert!(matches!(
            error.code.as_str(),
            "MANAGED_GIT_INVALID" | "MANAGED_GIT_PLATFORM_UNSUPPORTED"
        ));
    }

    #[tokio::test]
    async fn explicit_probe_is_offline_and_isolated_from_repository_and_user_environment() {
        let (_temp, root) = disposable();
        let log = root.join("probe-observation");
        let quoted = format!("'{}'", log.to_str().unwrap().replace('\'', "'\\''"));
        let git = script(&root, "synthetic-isolation-probe", &format!(
            "{{ printf 'arg=%s\\n' \"$@\"; printf 'cwd=%s\\n' \"$PWD\"; /usr/bin/env; }} > {quoted}\nprintf 'git version 2.56.0\\n'"
        ));
        let toolchain = probe_local(&git, &OwnedProcessRegistry::default(), Arc::new(()))
            .await
            .unwrap();
        let output = fs::read_to_string(log).unwrap();
        assert!(output.contains("arg=--version\n"));
        assert!(output.contains("cwd=/\n"));
        assert!(output.contains("GIT_CONFIG_GLOBAL=/dev/null\n"));
        assert!(output.contains("GIT_CONFIG_SYSTEM=/dev/null\n"));
        assert!(output.contains("GIT_CONFIG_NOSYSTEM=1\n"));
        assert!(output.contains("GIT_TERMINAL_PROMPT=0\n"));
        assert!(output.contains("HOME=/dev/null\n"));
        for forbidden in [
            "SSH_AUTH_SOCK=",
            "GIT_DIR=",
            "GIT_WORK_TREE=",
            "GIT_CONFIG_COUNT=",
            "GIT_TRACE=",
        ] {
            assert!(!output.contains(forbidden));
        }
        assert_eq!(toolchain.path, format!("{}:{SYSTEM_PATH}", root.display()));
        assert!(!toolchain.path.contains("/opt/homebrew"));
    }

    #[test]
    fn local_version_identification_preserves_vendor_suffix_without_support_claims() {
        for (output, version) in [
            ("git version 1.8.3\n", "1.8.3"),
            ("git version 2.56.0.rc1\n", "2.56.0.rc1"),
            (
                "git version 2.50.1 (Apple Git-155)\n",
                "2.50.1 (Apple Git-155)",
            ),
        ] {
            assert_eq!(git_version(output.as_bytes()).unwrap(), version);
        }
        assert!(git_version(b"not git\n").is_err());
        assert!(git_version(b"git version 2.56.0\nextra output").is_err());
    }

    #[tokio::test]
    async fn local_package_manager_link_is_explicitly_supported() {
        let (_temp, root) = disposable();
        let git = fake_git(&root);
        let link = root.join("selected-git");
        symlink(&git, &link).unwrap();
        let toolchain = probe_local(&link, &OwnedProcessRegistry::default(), Arc::new(()))
            .await
            .unwrap();
        assert_eq!(toolchain.git.path, git);
    }

    #[test]
    fn invalid_settings_never_resolve_to_default() {
        let (_temp, root) = disposable();
        let path = root.join("git-toolchain.json");
        let manager = GitEnvironmentManager::new(root.clone(), Some(root.join("unused-managed")));
        for bytes in [
            "invalid json",
            "{\"schemaVersion\":1}",
            "{\"schemaVersion\":2,\"gitPath\":null}",
            "{\"schemaVersion\":1,\"gitPath\":null,\"extra\":1}",
        ] {
            fs::write(&path, bytes).unwrap();
            assert_eq!(
                manager.configured().unwrap_err().code,
                "GIT_TOOLCHAIN_SETTINGS_INVALID"
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn linked_special_and_oversized_settings_are_rejected() {
        for kind in ["symlink", "hardlink", "fifo", "oversized"] {
            let (_temp, root) = disposable();
            let path = root.join("git-toolchain.json");
            let original = root.join("preserved-file");
            fs::write(&original, "existing evidence").unwrap();
            match kind {
                "symlink" => symlink(&original, &path).unwrap(),
                "hardlink" => fs::hard_link(&original, &path).unwrap(),
                "fifo" => {
                    let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
                }
                _ => fs::write(&path, vec![b'x'; MAX_SETTINGS_BYTES as usize + 1]).unwrap(),
            }
            assert_eq!(
                read_settings_bytes(&path).unwrap_err().code,
                "GIT_TOOLCHAIN_SETTINGS_INVALID"
            );
            assert_eq!(fs::read_to_string(original).unwrap(), "existing evidence");
        }
    }

    #[tokio::test]
    async fn ssh_version_on_stderr_is_only_tool_availability() {
        let (_temp, root) = disposable();
        let ssh = script(&root, "synthetic-ssh", "test \"$#\" = 1 && test \"$1\" = '-V' || exit 2\nprintf 'OpenSSH_9.9p2, LibreSSL 3.3.6\\n' >&2");
        let info = inspect_ssh(&ssh, &OwnedProcessRegistry::default()).await;
        assert!(info.ready);
        assert_eq!(
            info.executable.unwrap().version,
            "OpenSSH_9.9p2, LibreSSL 3.3.6"
        );
        assert!(
            !inspect_ssh(&root.join("missing-ssh"), &OwnedProcessRegistry::default())
                .await
                .ready
        );
    }

    #[test]
    fn apple_developer_lookup_never_returns_the_system_shim() {
        let (_temp, root) = disposable();
        assert!(developer_git(b"relative/developer\n").is_err());
        assert!(developer_git(b"/\n").is_err());
        fs::create_dir_all(root.join("usr/bin")).unwrap();
        let git = script(&root.join("usr/bin"), "git", "exit 9");
        assert_eq!(
            developer_git(root.to_str().unwrap().as_bytes()).unwrap(),
            git
        );
        fs::remove_file(&git).unwrap();
        symlink("/usr/bin/git", &git).unwrap();
        assert!(developer_git(root.to_str().unwrap().as_bytes()).is_err());
    }

    #[tokio::test]
    async fn oversized_or_stalled_probes_are_bounded() {
        let (_temp, root) = disposable();
        let large = script(&root, "synthetic-large-output", "printf '%20000s' x");
        let error = run_probe(
            isolated_command(&large, SYSTEM_PATH),
            Duration::from_secs(2),
            &OwnedProcessRegistry::default(),
            Arc::new(()),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.code, "GIT_TOOL_PROBE_FAILED");
        let stalled = script(&root, "synthetic-stalled-output", "/bin/sleep 60 & wait");
        let started = std::time::Instant::now();
        let error = run_probe(
            isolated_command(&stalled, SYSTEM_PATH),
            Duration::from_millis(50),
            &OwnedProcessRegistry::default(),
            Arc::new(()),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.code, "GIT_TOOL_PROBE_TIMEOUT");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn successful_version_probe_also_stops_detached_output_children() {
        let (_temp, root) = disposable();
        let marker = root.join("unexpected-child-survived");
        let quoted = format!("'{}'", marker.to_str().unwrap().replace('\'', "'\\''"));
        let git = script(&root, "synthetic-background-child", &format!(
            "(/bin/sleep 0.3; printf survived > {quoted}) </dev/null >/dev/null 2>&1 &\nprintf 'git version 2.56.0\\n'"
        ));
        probe_local(&git, &OwnedProcessRegistry::default(), Arc::new(()))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(450)).await;
        assert!(
            !marker.exists(),
            "version probe left its background child running"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn configuring_repairs_invalid_json_and_preserves_node_settings_and_failed_choice() {
        let (_temp, root) = disposable();
        let git = fake_git(&root);
        let node_settings = root.join("toolchain.json");
        fs::write(
            &node_settings,
            "existing Node preference must remain byte-for-byte",
        )
        .unwrap();
        fs::write(root.join("git-toolchain.json"), "{\"schemaVersion\":1}").unwrap();
        let manager = GitEnvironmentManager::new(root.clone(), None);
        assert_eq!(manager.doctor().await.selection, "unresolved");
        let repaired = manager.configure_git(None).await.unwrap();
        assert_eq!(repaired.selection, "managed");
        fs::write(root.join("git-toolchain.json"), "damaged JSON").unwrap();
        let info = manager.doctor().await;
        assert_eq!(info.selection, "unresolved");
        assert_eq!(info.authentication.state, "not-checked");
        assert!(!info.ready);
        let configured = manager.configure_git(Some(git.clone())).await.unwrap();
        assert!(configured.ready);
        assert_eq!(configured.selection, "explicit");
        assert_eq!(configured.configured_git_path, Some(git));
        assert_eq!(configured.authentication.state, "not-checked");
        let bytes = fs::read(root.join("git-toolchain.json")).unwrap();
        assert!(manager
            .configure_git(Some(root.join("missing")))
            .await
            .is_err());
        assert_eq!(fs::read(root.join("git-toolchain.json")).unwrap(), bytes);
        let cleared = manager.configure_git(None).await.unwrap();
        assert_eq!(cleared.selection, "managed");
        assert!(!cleared.ready);
        assert_eq!(cleared.issues[0].code, "MANAGED_GIT_UNAVAILABLE");
        assert_eq!(
            fs::read_to_string(node_settings).unwrap(),
            "existing Node preference must remain byte-for-byte"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "Opt-in: PLATFORM_TEST_MANAGED_GIT must name a disposable official sample under .test-artifacts"]
    async fn official_disposable_managed_sample_is_verified_and_probed_without_setting_changes() {
        let sample = PathBuf::from(
            std::env::var_os("PLATFORM_TEST_MANAGED_GIT").expect("sample path required"),
        );
        let sample = fs::canonicalize(sample).unwrap();
        let workspace =
            fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
        assert!(sample.starts_with(workspace.join(".test-artifacts")));
        assert!(sample.components().any(|part| part
            .as_os_str()
            .to_string_lossy()
            .starts_with("disposable-")));
        let (_temp, root) = disposable();
        let manager = GitEnvironmentManager::new(root.clone(), Some(sample.clone()));
        let before = fs::read(sample.join("development-git.json")).unwrap();
        let info = manager.doctor().await;
        assert!(info.ready, "{:?}", info.issues);
        assert_eq!(info.selection, "managed");
        assert_eq!(info.git.unwrap().path, sample.join("bin/git"));
        assert_eq!(info.authentication.state, "not-checked");
        let resolved = manager.resolve().await.unwrap();
        assert_eq!(resolved.exec_path, Some(sample.join("libexec/git-core")));
        assert_eq!(
            resolved.template_path,
            Some(sample.join("share/git-core/templates"))
        );
        assert!(!resolved.path.contains("/opt/homebrew"));
        assert_eq!(
            fs::read(sample.join("development-git.json")).unwrap(),
            before
        );
        assert!(!root.join("git-toolchain.json").exists());
        assert!(!root.join("toolchain.json").exists());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn all_environment_paths_use_the_injected_registry_and_sealed_admission() {
        let (_temp, root) = disposable();
        let git = fake_git(&root);
        let processes = OwnedProcessRegistry::default();
        let manager = GitEnvironmentManager::with_processes(root, None, processes.clone());
        let info = manager.configure_git(Some(git)).await.unwrap();
        assert!(info.ready);
        // configure local probe, doctor SSH and doctor's selected Git all belong
        // to this one instance, rather than ephemeral per-command registries.
        let snapshots = processes.snapshot();
        assert_eq!(snapshots.len(), 3);
        assert!(snapshots
            .iter()
            .all(|item| item.component == "git-environment"));
        assert!(processes.shutdown().await.is_empty());
        assert!(processes
            .snapshot()
            .iter()
            .all(|item| item.resource_closed()));
        assert_eq!(
            manager.resolve().await.unwrap_err().code,
            "SERVICE_STOPPING"
        );
        assert_eq!(
            processes.snapshot().len(),
            3,
            "sealed registry started another probe"
        );
    }
}
