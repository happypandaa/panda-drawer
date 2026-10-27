//! Explicit development builds from one authoritative repository. All commands
//! run in a new copy with a private environment; the original Git, dependencies,
//! application data and saved configuration are never build inputs or outputs.
//! This is working-directory isolation, not an operating-system sandbox.
use crate::{
    environment::Toolchain,
    node_app_manifest::{NodeDevelopmentBuild, TrialManifest, TrialRuntime},
    owned_process::{IoPlan, ProcessOptions, ProcessPhase, ProcessPipes},
    process_diagnostics::ProcessDiagnostics,
    projects::check_no_symlinks,
    ApiError,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    process::Command,
    sync::watch,
    time::{sleep, Instant},
};

type Result<T> = std::result::Result<T, ApiError>;
#[path = "node_process_scope.rs"]
pub(crate) mod process_scope;
pub(crate) use process_scope::NodeProcessScope;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_TREE: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 50_000;
const TIMEOUT: Duration = Duration::from_secs(600);

fn invalid(message: &str) -> ApiError {
    ApiError::new("NODE_DEVELOPMENT_BUILD_INVALID", message)
}
fn changed() -> ApiError {
    ApiError::new(
        "NODE_DEVELOPMENT_CHANGED",
        "源码或构建产物已变化，请重新构建。",
    )
}

/// Only this module can construct this token. Source acquisition rechecks its
/// ordinary root identity and complete materialized program before capturing it.
/// It is deliberately not Deserialize and cannot be supplied by an API caller.
#[derive(Debug)]
pub struct NodeDevelopmentBuildOutput {
    pub program_root: PathBuf,
    pub source_digest: String,
    pub excluded_paths: Vec<String>,
    pub built: bool,
    identity: (u64, u64),
    program: Tree,
}
impl NodeDevelopmentBuildOutput {
    pub(crate) fn verify(&self) -> Result<()> {
        if directory_identity(&self.program_root)? != self.identity
            || scan(&self.program_root, None, false)?.0 != self.program
        {
            return Err(changed());
        }
        validate_program(&self.program_root)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Blob {
    bytes: u64,
    executable: bool,
    digest: [u8; 32],
}
type Tree = BTreeMap<String, Blob>;

#[derive(Clone)]
struct Control {
    stop: watch::Receiver<bool>,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
}
impl Control {
    fn check(&self) -> Result<()> {
        if *self.stop.borrow() || self.stop.has_changed().is_err() {
            return Err(ApiError::new(
                "NODE_DEVELOPMENT_INTERRUPTED",
                "平台正在退出，构建现场已保留。",
            ));
        }
        if *self.cancel.borrow() || self.cancel.has_changed().is_err() {
            return Err(ApiError::new(
                "NODE_DEVELOPMENT_CANCELLED",
                "已停止构建，现场已保留。",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(ApiError::new(
                "NODE_DEVELOPMENT_TIMEOUT",
                "构建超过时间限制，现场已保留。",
            ));
        }
        Ok(())
    }
}

/// `operation` must not exist and must not overlap the selected repository.
/// The caller owns the application's development lease until this future and
/// the later source capture complete. No build result is installed or started.
pub async fn build_repository(
    repository: &Path,
    operation: &Path,
    tools: &Toolchain,
    stop: watch::Receiver<bool>,
    cancel: watch::Receiver<bool>,
) -> Result<NodeDevelopmentBuildOutput> {
    build_repository_inner(
        repository,
        operation,
        tools,
        stop,
        cancel,
        None,
        NodeProcessScope::standalone(),
    )
    .await
}

/// The task manager supplies the freshly reviewed Git scope. It includes
/// tracked files even when later ignored, and omits ignored untracked files.
/// Unknown entries outside that scope are skipped before reading their contents.
pub(crate) async fn build_repository_scoped(
    repository: &Path,
    operation: &Path,
    tools: &Toolchain,
    stop: watch::Receiver<bool>,
    cancel: watch::Receiver<bool>,
    included_paths: &[String],
    processes: NodeProcessScope,
) -> Result<NodeDevelopmentBuildOutput> {
    let scope = BuildScope::new(included_paths)?;
    build_repository_inner(
        repository,
        operation,
        tools,
        stop,
        cancel,
        Some(&scope),
        processes,
    )
    .await
}

struct BuildScope {
    paths: BTreeSet<String>,
    parents: BTreeSet<String>,
}
impl BuildScope {
    fn new(paths: &[String]) -> Result<Self> {
        if paths.len() > MAX_ENTRIES || !paths.iter().any(|path| path == "local-app.json") {
            return Err(invalid("源码范围缺少应用声明或超过数量限制。"));
        }
        let mut result = Self {
            paths: BTreeSet::new(),
            parents: BTreeSet::new(),
        };
        for path in paths {
            if path.len() > 4096
                || path.contains(['\\', '\0', ':'])
                || path.split('/').any(|part| {
                    part.is_empty()
                        || part == "."
                        || part == ".."
                        || part.chars().any(char::is_control)
                })
            {
                return Err(invalid("源码范围包含不支持的路径。"));
            }
            result.paths.insert(path.clone());
            let mut parent = Path::new(path).parent();
            while let Some(value) = parent {
                if value.as_os_str().is_empty() {
                    break;
                }
                result.parents.insert(
                    value
                        .to_str()
                        .ok_or_else(|| invalid("源码范围路径无效。"))?
                        .into(),
                );
                parent = value.parent();
            }
        }
        Ok(result)
    }
    fn includes(&self, path: &str) -> bool {
        self.paths.contains(path) || self.parents.contains(path)
    }
}

async fn build_repository_inner(
    repository: &Path,
    operation: &Path,
    tools: &Toolchain,
    stop: watch::Receiver<bool>,
    cancel: watch::Receiver<bool>,
    scope: Option<&BuildScope>,
    processes: NodeProcessScope,
) -> Result<NodeDevelopmentBuildOutput> {
    let control = Control {
        stop,
        cancel,
        deadline: Instant::now() + TIMEOUT,
    };
    control.check()?;
    let original_identity = directory_identity(repository)?;
    check_no_symlinks(operation, true)?;
    if !operation.is_absolute()
        || operation.starts_with(repository)
        || repository.starts_with(operation)
        || operation.try_exists()?
    {
        return Err(invalid("构建须使用源码目录之外的全新临时目录。"));
    }
    let original_manifest =
        TrialManifest::load(repository).map_err(|error| invalid(&error.to_string()))?;
    if original_manifest.runtime != TrialRuntime::NodeService {
        return Err(invalid("此开发入口只支持 Node 服务应用。"));
    }
    let declaration = original_manifest.development.as_ref();
    let output_dir = declaration.map_or(".", |value| value.output_dir.as_str());
    let build = declaration.map_or(NodeDevelopmentBuild::None {}, |value| value.build.clone());
    let is_npm = matches!(build, NodeDevelopmentBuild::Npm { .. });
    let excluded_output = (output_dir != ".").then_some(output_dir);
    let (before, excluded_paths) = scan_scoped(repository, excluded_output, true, scope)?;
    control.check()?;
    create_new_dir(operation)?;
    let processes = processes.with_operation(operation)?;
    let copied = operation.join("repository");
    create_new_dir(&copied)?;
    copy_tree(repository, &copied, &before, Some(&control))?;
    if directory_identity(repository)? != original_identity
        || scan_scoped(repository, excluded_output, true, scope)?.0 != before
        || scan_scoped(&copied, excluded_output, true, scope)?.0 != before
    {
        return Err(changed());
    }
    if let NodeDevelopmentBuild::Npm { script } = &build {
        crate::node_app_install::validate_npm_input(&copied)?;
        let package: Value = serde_json::from_slice(&read(&copied.join("package.json"))?)?;
        if package["scripts"][script]
            .as_str()
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(invalid("清单声明的构建脚本不存在。"));
        }
        run_npm(
            operation,
            &copied,
            tools,
            &["ci", "--ignore-scripts", "--no-audit", "--no-fund"],
            &control,
            processes.clone(),
        )
        .await?;
        // ignore-scripts disables pre/post lifecycle hooks while the explicit
        // named build script still runs. It does not grant startup authority.
        run_npm(
            operation,
            &copied,
            tools,
            &["run", "--ignore-scripts", script],
            &control,
            processes.clone(),
        )
        .await?;
    }
    control.check()?;
    let from = if output_dir == "." {
        copied.clone()
    } else {
        copied.join(output_dir)
    };
    validate_program(&from)?;
    let built_manifest: Value = serde_json::from_slice(&read(&from.join("local-app.json"))?)?;
    let input_manifest: Value = serde_json::from_slice(&read(&repository.join("local-app.json"))?)?;
    if built_manifest["app"]["id"] != input_manifest["app"]["id"]
        || built_manifest.get("configuration") != input_manifest.get("configuration")
    {
        return Err(invalid("构建产物的应用标识或配置声明与源码不一致。"));
    }
    let (program, _) = scan(&from, None, true)?;
    let program_root = operation.join("program");
    create_new_dir(&program_root)?;
    copy_tree(&from, &program_root, &program, Some(&control))?;
    validate_program(&program_root)?;
    if scan(&from, None, true)?.0 != program
        || scan(&program_root, None, false)?.0 != program
        || directory_identity(repository)? != original_identity
        || scan_scoped(repository, excluded_output, true, scope)?.0 != before
    {
        return Err(changed());
    }
    control.check()?;
    let output = NodeDevelopmentBuildOutput {
        identity: directory_identity(&program_root)?,
        program_root,
        source_digest: tree_digest(&before),
        excluded_paths,
        built: is_npm,
        program,
    };
    output.verify()?;
    Ok(output)
}

fn validate_program(root: &Path) -> Result<()> {
    directory_identity(root)?;
    let manifest = TrialManifest::load(root).map_err(|error| invalid(&error.to_string()))?;
    if manifest.runtime != TrialRuntime::NodeService {
        return Err(invalid("构建输出必须是完整的 Node 应用。"));
    }
    manifest
        .inspect(root)
        .map_err(|error| invalid(&error.to_string()))?;
    let backend = root.join(
        &manifest
            .backend
            .as_ref()
            .ok_or_else(|| invalid("构建输出缺少后端声明。"))?
            .root,
    );
    let package_roots: Vec<_> = [root.to_path_buf(), backend]
        .into_iter()
        .filter(|path| {
            path.join("package.json").try_exists().unwrap_or(true)
                || path.join("package-lock.json").try_exists().unwrap_or(true)
        })
        .collect();
    if package_roots.len() != 1 {
        return Err(invalid(
            "构建输出须在根目录或后端目录包含一套完整的 package.json 和锁文件。",
        ));
    }
    crate::node_app_install::validate_npm_input(&package_roots[0])?;
    Ok(())
}

fn directory_identity(path: &Path) -> Result<(u64, u64)> {
    if !path.is_absolute() {
        return Err(invalid("开发目录必须是绝对路径。"));
    }
    check_no_symlinks(path, false)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(invalid("开发目录不是普通文件夹。"));
    }
    Ok((metadata.dev(), metadata.ino()))
}
fn create_new_dir(path: &Path) -> Result<()> {
    check_no_symlinks(path, true)?;
    fs::create_dir(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn read(path: &Path) -> Result<Vec<u8>> {
    check_no_symlinks(path, false)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > MAX_FILE {
        return Err(invalid("开发材料包含链接、特殊文件或过大的文件。"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != metadata.len() {
        return Err(changed());
    }
    Ok(bytes)
}

fn excluded(path: &str, output: Option<&str>) -> bool {
    let folded = path.to_ascii_lowercase();
    if output.is_some_and(|output| {
        folded == output.to_ascii_lowercase()
            || folded.starts_with(&format!("{}/", output.to_ascii_lowercase()))
    }) {
        return true;
    }
    if ["user", "work"]
        .iter()
        .any(|name| folded == *name || folded.starts_with(&format!("{name}/")))
    {
        return true;
    }
    folded.split('/').any(|name| {
        matches!(
            name,
            ".git"
                | ".hg"
                | ".svn"
                | ".platform"
                | "node_modules"
                | "data"
                | ".data"
                | "attachments"
                | "config"
                | "configuration"
                | "credentials"
                | "secrets"
                | "tokens"
                | ".ssh"
                | ".aws"
                | ".azure"
                | ".npmrc"
                | ".netrc"
                | ".git-credentials"
                | ".pypirc"
                | "id_rsa"
                | "id_ed25519"
                | "id_ecdsa"
                | "id_dsa"
                | "cache"
                | ".cache"
                | ".vite"
                | ".next"
                | ".nuxt"
                | "coverage"
                | "tmp"
                | ".tmp"
                | "temp"
                | "runs"
                | "backups"
                | "trash"
                | ".ds_store"
        ) || (name.starts_with(".env") && name != ".env.example")
            || [
                ".pem", ".key", ".p12", ".pfx", ".sqlite", ".sqlite3", ".db", "-wal", "-shm",
                "-journal",
            ]
            .iter()
            .any(|suffix| name.ends_with(suffix))
    })
}

fn scan(root: &Path, output: Option<&str>, apply_exclusions: bool) -> Result<(Tree, Vec<String>)> {
    scan_scoped(root, output, apply_exclusions, None)
}
fn scan_scoped(
    root: &Path,
    output: Option<&str>,
    apply_exclusions: bool,
    scope: Option<&BuildScope>,
) -> Result<(Tree, Vec<String>)> {
    #[allow(clippy::too_many_arguments)] // The recursive walk keeps every bound explicit.
    fn visit(
        root: &Path,
        relative: &str,
        output: Option<&str>,
        apply_exclusions: bool,
        scope: Option<&BuildScope>,
        tree: &mut Tree,
        excluded_paths: &mut Vec<String>,
        budget: &mut (usize, u64),
        depth: usize,
    ) -> Result<()> {
        if depth > 100 {
            return Err(invalid("开发材料目录层级超过限制。"));
        }
        let mut entries =
            fs::read_dir(root.join(relative))?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("开发材料文件名必须是有效文字。"))?;
            if name.contains(['\\', '\0', ':']) || name.chars().any(char::is_control) {
                return Err(invalid("开发材料包含不支持的文件名。"));
            }
            let path = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            budget.0 += 1;
            if budget.0 > MAX_ENTRIES {
                return Err(invalid("开发材料数量超过限制。"));
            }
            if apply_exclusions && excluded(&path, output) {
                excluded_paths.push(path);
                continue;
            }
            if scope.is_some_and(|scope| !scope.includes(&path)) {
                excluded_paths.push(path);
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if apply_exclusions {
                let allowed = crate::node_development_git::source_path_allowed(&path)
                    || (metadata.is_dir()
                        && crate::node_development_git::source_path_allowed(&format!(
                            "{path}/file"
                        )));
                if !allowed {
                    if !metadata.is_dir() && !metadata.is_file() {
                        return Err(invalid("开发材料包含链接或特殊文件。"));
                    }
                    excluded_paths.push(path);
                    continue;
                }
            }
            if metadata.is_dir() {
                check_no_symlinks(&entry.path(), false)?;
                visit(
                    root,
                    &path,
                    output,
                    apply_exclusions,
                    scope,
                    tree,
                    excluded_paths,
                    budget,
                    depth + 1,
                )?;
            } else if metadata.is_file() {
                let bytes = read(&entry.path())?;
                budget.1 = budget
                    .1
                    .checked_add(bytes.len() as u64)
                    .ok_or_else(|| invalid("开发材料过大。"))?;
                if budget.1 > MAX_TREE {
                    return Err(invalid("开发材料总大小超过限制。"));
                }
                tree.insert(
                    path,
                    Blob {
                        bytes: bytes.len() as u64,
                        executable: metadata.mode() & 0o111 != 0,
                        digest: Sha256::digest(bytes).into(),
                    },
                );
            } else {
                return Err(invalid("开发材料包含链接或特殊文件。"));
            }
        }
        Ok(())
    }
    directory_identity(root)?;
    let mut tree = BTreeMap::new();
    let mut paths = Vec::new();
    visit(
        root,
        "",
        output,
        apply_exclusions,
        scope,
        &mut tree,
        &mut paths,
        &mut (0, 0),
        0,
    )?;
    Ok((tree, paths))
}

fn tree_digest(tree: &Tree) -> String {
    let mut hash = Sha256::new();
    for (path, blob) in tree {
        hash.update(path.as_bytes());
        hash.update([0]);
        hash.update(blob.bytes.to_le_bytes());
        hash.update([u8::from(blob.executable)]);
        hash.update(blob.digest);
    }
    format!("{:x}", hash.finalize())
}
fn copy_tree(from: &Path, to: &Path, tree: &Tree, control: Option<&Control>) -> Result<()> {
    for (path, expected) in tree {
        if let Some(control) = control {
            control.check()?;
        }
        let bytes = read(&from.join(path))?;
        if bytes.len() as u64 != expected.bytes
            || <[u8; 32]>::from(Sha256::digest(&bytes)) != expected.digest
        {
            return Err(changed());
        }
        let target = to.join(path);
        let parent = target
            .parent()
            .ok_or_else(|| invalid("开发材料路径无效。"))?;
        check_no_symlinks(parent, true)?;
        fs::create_dir_all(parent)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(if expected.executable { 0o700 } else { 0o600 })
            .custom_flags(libc::O_NOFOLLOW)
            .open(&target)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    File::open(to)?.sync_all()?;
    Ok(())
}

fn npm_command(operation: &Path, cwd: &Path, tools: &Toolchain) -> Command {
    let mut command = Command::new(&tools.node.path);
    command
        .arg(&tools.npm.path)
        .arg("--logs-dir")
        .arg(operation.join("npm-disabled-output"))
        .arg("--logs-max=0")
        .env_clear()
        .current_dir(cwd)
        .env("PATH", &tools.path)
        .env("HOME", operation.join("home"))
        .env("XDG_CONFIG_HOME", operation.join("home"))
        .env("TMPDIR", operation.join("tmp"))
        .env("npm_config_cache", operation.join("npm-cache"))
        .env("npm_config_userconfig", operation.join("home/user.npmrc"))
        .env(
            "npm_config_globalconfig",
            operation.join("home/global.npmrc"),
        )
        .env("npm_config_ignore_scripts", "true")
        .env("npm_config_audit", "false")
        .env("npm_config_fund", "false")
        .env("npm_config_update_notifier", "false")
        .env("npm_config_logs_max", "0")
        .env("CI", "true")
        .env("LC_ALL", "C");
    command
}

async fn run_npm(
    operation: &Path,
    cwd: &Path,
    tools: &Toolchain,
    args: &[&str],
    control: &Control,
    processes: NodeProcessScope,
) -> Result<()> {
    for name in ["home", "tmp", "npm-cache"] {
        let path = operation.join(name);
        if !path.try_exists()? {
            create_new_dir(&path)?;
        }
    }
    for name in ["user.npmrc", "global.npmrc"] {
        let config = operation.join("home").join(name);
        if !config.try_exists()? {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(config)?
                .sync_all()?;
        }
    }
    control.check()?;
    let mut command = npm_command(operation, cwd, tools);
    command.args(args);
    let reader_scope = processes.clone();
    let step = args.first().copied().unwrap_or_default().to_owned();
    let diagnostics = ProcessDiagnostics::default();
    let stdout_diagnostics = diagnostics.clone();
    let stderr_diagnostics = diagnostics.clone();
    let command_label = if args.first().copied() == Some("run") {
        format!("npm run {}", args.get(1).copied().unwrap_or_default())
    } else {
        format!("npm {}", args.first().copied().unwrap_or_default())
    };
    let mut run = processes
        .processes
        .start(
            command,
            ProcessOptions::new(
                "node-development/npm",
                control.deadline.saturating_duration_since(Instant::now()),
            ),
            processes.owner,
            move |pipes| {
                let ProcessPipes {
                    stdin,
                    stdout,
                    stderr,
                    stop: _,
                } = pipes;
                drop(stdin);
                IoPlan::new(async move {
                    reader_scope
                        .finish_reader("node-development/npm", &step, async move {
                            stdout_diagnostics.read(stdout, "stdout", None).await
                        })
                        .await
                })
                .drain("build-stderr", async move {
                    stderr_diagnostics.read(stderr, "stderr", None).await
                })
            },
        )
        .map_err(|error| diagnostics.attach(error, &command_label, tools, None))?;
    let mut interruption = None;
    let result = loop {
        if let Err(error) = control
            .check()
            .and_then(|()| scan_operation_budget(operation))
        {
            interruption.get_or_insert(error);
            run.cancel();
        }
        tokio::select! {
            result = &mut run => break result,
            _ = sleep(Duration::from_millis(250)) => {},
        }
    };
    // A cleanup reply is consumed once. Keep the actual build context and its
    // source lease until the independently owned writer really closes.
    while matches!(
        run.snapshot().phase,
        ProcessPhase::Running | ProcessPhase::Stopping | ProcessPhase::CleanupPending
    ) {
        if let Err(error) = control.check() {
            interruption.get_or_insert(error);
            run.cancel();
        }
        sleep(Duration::from_millis(10)).await;
    }
    let receipt = run.snapshot();
    let closed = receipt.phase == ProcessPhase::Closed
        && receipt.leader_reaped
        && receipt.group_exited
        && receipt.io_joined;
    if !closed {
        let error = result.err().unwrap_or_else(|| {
            ApiError::new(
                "NODE_DEVELOPMENT_CLEANUP_REQUIRED",
                "构建进程归属未确认，不能发布；现场及租约保留。",
            )
        });
        return Err(diagnostics.attach(
            error
                .detail("processResourceClosed", false)
                .outcome("unknown"),
            &command_label,
            tools,
            None,
        ));
    }
    if let Some(error) = interruption {
        return Err(diagnostics.attach(
            error.detail("processResourceClosed", true),
            &command_label,
            tools,
            None,
        ));
    }
    let result = result.map_err(|error| {
        let code = match error.code.as_str() {
            "PROCESS_WORK_TIMEOUT" => "NODE_DEVELOPMENT_TIMEOUT",
            "PROCESS_CANCELLED" => "NODE_DEVELOPMENT_INTERRUPTED",
            _ => "NODE_DEVELOPMENT_BUILD_FAILED",
        };
        diagnostics.attach(
            ApiError::new(code, "构建未完成；进程和输出已收尾，当前程序未改变。")
                .detail("processCauseCode", error.code)
                .detail("processResourceClosed", true),
            &command_label,
            tools,
            None,
        )
    })?;
    if !result.status.success() {
        return Err(diagnostics.attach(
            ApiError::new(
                "NODE_DEVELOPMENT_BUILD_FAILED",
                "构建未完成，当前程序未改变；请检查构建脚本和依赖。",
            )
            .detail("processResourceClosed", true),
            &command_label,
            tools,
            Some(&result.status),
        ));
    }
    Ok(())
}
fn scan_operation_budget(root: &Path) -> Result<()> {
    fn visit(path: &Path, budget: &mut (usize, u64), depth: usize) -> Result<()> {
        if depth > 128 {
            return Err(invalid("构建现场超过目录层级限制。"));
        }
        check_no_symlinks(path, false)?;
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = match entry {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let metadata = match fs::symlink_metadata(entry.path()) {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            budget.0 += 1;
            budget.1 = budget
                .1
                .checked_add(metadata.len())
                .ok_or_else(|| invalid("构建现场过大。"))?;
            if budget.0 > 2 * MAX_ENTRIES
                || budget.1 > 2 * MAX_TREE
                || (metadata.is_file() && metadata.len() > MAX_FILE)
            {
                return Err(invalid("构建现场超过大小或文件数量限制。"));
            }
            if metadata.is_dir() {
                visit(&entry.path(), budget, depth + 1)?;
            } else if !metadata.is_file() && !metadata.file_type().is_symlink() {
                return Err(invalid("构建现场出现特殊文件。"));
            }
        }
        Ok(())
    }
    directory_identity(root)?;
    if fs2::available_space(root)? < 64 * 1024 * 1024 {
        return Err(invalid("磁盘空间不足，构建已停止。"));
    }
    visit(root, &mut (0, 0), 0)
}

#[cfg(test)]
#[path = "node_development_build_tests.rs"]
mod tests;
