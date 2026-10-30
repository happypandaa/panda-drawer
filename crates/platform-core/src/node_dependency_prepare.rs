//! Prepare Git-imported Node dependencies in the one published project directory.
//! This never builds or runs project scripts. Private npm state belongs to the
//! import operation and actual process ownership survives interrupted waiters.
use crate::{
    environment::Toolchain,
    node_app_manifest::TrialManifest,
    node_process_scope::NodeProcessScope,
    owned_process::{IoPlan, ProcessOptions, ProcessPhase, ProcessPipes},
    process_diagnostics::ProcessDiagnostics,
    projects::check_no_symlinks,
    ApiError,
};
use serde_json::{json, Value};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};
use tokio::{process::Command, sync::watch, time::Instant};
type Result<T> = std::result::Result<T, ApiError>;
const MAX_METADATA: u64 = 256 * 1024;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_TREE: u64 = 512 * 1024 * 1024;
const MAX_FILES: usize = 50_000;
const MAX_OUTPUT: usize = 64 * 1024;
const NPM_SPACE_INTERVAL: Duration = Duration::from_millis(250);
fn failure(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}
fn invalid(message: &str) -> ApiError {
    failure("NODE_INSTALL_INVALID", message)
}
fn changed() -> ApiError {
    failure("NODE_DEPENDENCY_CHANGED", "依赖准备时项目文件发生变化。")
}
#[derive(Clone, Copy)]
pub(crate) struct NpmSpaceBudget {
    pub(crate) max_file: u64,
    pub(crate) max_total: u64,
    pub(crate) max_entries: usize,
    pub(crate) minimum_free: u64,
}
impl Default for NpmSpaceBudget {
    fn default() -> Self {
        Self {
            max_file: MAX_FILE,
            // Extraction and npm's private download cache coexist. The final
            // program still has its separate, smaller MAX_TREE limit.
            max_total: 2 * MAX_TREE,
            max_entries: 2 * MAX_FILES,
            minimum_free: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone)]
pub(crate) struct DependencyControl {
    pub(crate) stop: watch::Receiver<bool>,
    pub(crate) cancel: watch::Receiver<bool>,
    pub(crate) deadline: Instant,
    pub(crate) npm_space: NpmSpaceBudget,
}
impl DependencyControl {
    pub(crate) fn new(stop: watch::Receiver<bool>, cancel: watch::Receiver<bool>) -> Self {
        Self {
            stop,
            cancel,
            deadline: Instant::now() + Duration::from_secs(600),
            npm_space: NpmSpaceBudget::default(),
        }
    }
    pub(crate) fn check(&self) -> Result<()> {
        if *self.stop.borrow() || self.stop.has_changed().is_err() {
            return Err(failure(
                "NODE_INSTALL_INTERRUPTED",
                "平台正在退出，依赖准备已中断。",
            ));
        }
        if *self.cancel.borrow() {
            return Err(failure("NODE_INSTALL_CANCELLED", "依赖准备已取消。"));
        }
        if Instant::now() >= self.deadline {
            return Err(failure(
                "NODE_INSTALL_TIMEOUT",
                "依赖准备超过 10 分钟预算。",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DependencyPreparation {
    Skipped,
    Installed { npm_root: PathBuf },
}

fn needs_adaptation(message: &str) -> ApiError {
    failure("NODE_DEPENDENCY_NEEDS_ADAPTATION", message)
}

fn npm_locations(root: &Path, manifest: &TrialManifest) -> Result<Vec<PathBuf>> {
    ordinary_directory(root)?;
    let backend = root.join(
        &manifest
            .backend
            .as_ref()
            .ok_or_else(|| needs_adaptation("缺少后端声明。"))?
            .root,
    );
    let mut locations = vec![root.to_path_buf()];
    if backend != root {
        locations.push(backend);
    }
    for path in &locations {
        ordinary_directory(path)?;
    }
    Ok(locations)
}

fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn dependency_root(root: &Path, manifest: &TrialManifest) -> Result<Option<PathBuf>> {
    for location in npm_locations(root, manifest)? {
        // Only test metadata; repository npm configuration may contain secrets.
        if present(&location.join(".npmrc"))? {
            return Err(needs_adaptation("Git 项目含自定义 .npmrc；请让 AI 整理依赖配置后重新添加，或准备好后作为本地项目添加。"));
        }
    }
    let mut candidates = Vec::new();
    for location in npm_locations(root, manifest)? {
        if present(&location.join("package.json"))?
            || present(&location.join("package-lock.json"))?
            || present(&location.join("npm-shrinkwrap.json"))?
        {
            candidates.push(location);
        }
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    if candidates.len() != 1 {
        return Err(needs_adaptation(
            "根目录和后端目录只能有一组 npm 依赖材料，请让 AI 整理后再添加。",
        ));
    }
    let location = candidates.remove(0);
    if !present(&location.join("package.json"))? || !present(&location.join("package-lock.json"))? {
        return Err(needs_adaptation(
            "npm 项目需要同时提交 package.json 和 npm v2/v3 锁文件，请让 AI 生成锁文件后再添加。",
        ));
    }
    validate_npm_input(&location).map_err(|error| needs_adaptation(&error.message))?;
    Ok(Some(location))
}

/// Only the import coordinator can provide the published root, private operation
/// directory and actual process owner. No application is registered or started.
pub(crate) async fn prepare_dependencies(
    project_root: &Path,
    manifest: &TrialManifest,
    operation_root: &Path,
    tools: &Toolchain,
    control: &DependencyControl,
    processes: NodeProcessScope,
) -> Result<DependencyPreparation> {
    control.check()?;
    let Some(npm_root) = dependency_root(project_root, manifest)? else {
        return Ok(DependencyPreparation::Skipped);
    };
    ordinary_directory(operation_root)?;
    if operation_root.starts_with(project_root) || project_root.starts_with(operation_root) {
        return Err(invalid("npm 临时目录必须位于项目文件夹之外。"));
    }
    npm_ci_with_budget(
        operation_root,
        &npm_root,
        project_root,
        tools,
        control,
        processes,
    )
    .await?;
    validate_dependency_tree(&npm_root.join("node_modules"), project_root)?;
    Ok(DependencyPreparation::Installed { npm_root })
}

fn validate_dependency_tree(path: &Path, project_root: &Path) -> Result<()> {
    if !present(path)? {
        return Ok(());
    }
    ordinary_directory(path)?;
    for entry in fs::read_dir(path)? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            validate_dependency_tree(&path, project_root)?;
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path)?;
            if target.is_absolute()
                || !fs::canonicalize(&path).is_ok_and(|target| target.starts_with(project_root))
            {
                return Err(needs_adaptation("依赖链接必须指向项目文件夹内部。"));
            }
        } else if metadata.is_file() {
            if metadata.nlink() != 1
                || path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("node"))
            {
                return Err(needs_adaptation(
                    "当前 Git 依赖准备不支持共享硬链接或第三方 Node 原生扩展。",
                ));
            }
        } else {
            return Err(needs_adaptation("依赖目录包含特殊文件。"));
        }
    }
    Ok(())
}

pub(crate) fn validate_npm_input(root: &Path) -> Result<()> {
    let package: Value = serde_json::from_slice(&read_small(&root.join("package.json"))?)?;
    let lock: Value = serde_json::from_slice(&read_small(&root.join("package-lock.json"))?)?;
    if !package.is_object()
        || !lock.is_object()
        || root.join("npm-shrinkwrap.json").try_exists()?
        || !matches!(lock["lockfileVersion"].as_u64(), Some(2 | 3))
        || !lock["packages"].is_object()
        || !lock["packages"][""].is_object()
        || !package["workspaces"].is_null()
    {
        return Err(invalid(
            "首批安装要求 npm v2/v3 锁文件，不支持 workspace 或覆盖锁文件。",
        ));
    }
    for field in ["dependencies", "devDependencies", "optionalDependencies"] {
        if package.get(field).unwrap_or(&json!({}))
            != lock["packages"][""].get(field).unwrap_or(&json!({}))
        {
            return Err(invalid("package.json 与锁文件依赖不一致。"));
        }
    }
    // Use an allow-list, not scheme blacklists: npm interprets owner/repo and
    // even uppercase FILE: as Git/filesystem inputs. Aliases/overrides are a
    // separate compatibility slice and must not become a hidden escape hatch.
    fn spec(value: &Value) -> Result<()> {
        let Some(value) = value.as_str() else {
            return Err(invalid("依赖版本必须为字符串。"));
        };
        if value.is_empty()
            || value.starts_with('.')
            || value.len() > 200
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".*~^<>=| +-".contains(&b))
        {
            return Err(invalid(
                "首批安装只支持 registry 版本、范围或标签，不支持路径、Git 或别名依赖。",
            ));
        }
        Ok(())
    }
    fn dependency_map(value: &Value) -> Result<()> {
        if value.is_null() {
            return Ok(());
        }
        let dependencies = value
            .as_object()
            .ok_or_else(|| invalid("依赖声明不是对象。"))?;
        for (name, value) in dependencies {
            let parts: Vec<_> = name.split('/').collect();
            let ordinary = |part: &str| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            };
            let valid = match parts.as_slice() {
                [name] => ordinary(name),
                [scope, name] => scope.strip_prefix('@').is_some_and(ordinary) && ordinary(name),
                _ => false,
            };
            if !valid || name.len() > 214 {
                return Err(invalid("依赖名称无效。"));
            }
            spec(value)?;
        }
        Ok(())
    }
    fn package_fields(value: &Value) -> Result<()> {
        if value["link"].as_bool() == Some(true)
            || !value["workspaces"].is_null()
            || value.get("overrides").is_some_and(|v| !v.is_null())
        {
            return Err(invalid("首批安装不支持链接依赖、workspace 或 overrides。"));
        }
        for field in [
            "dependencies",
            "devDependencies",
            "optionalDependencies",
            "peerDependencies",
        ] {
            dependency_map(&value[field])?;
        }
        Ok(())
    }
    fn resolved(value: &Value) -> Result<()> {
        let Some(value) = value.as_str() else {
            return Err(invalid("依赖下载地址无效。"));
        };
        let url =
            url::Url::parse(value).map_err(|_| invalid("依赖须有明确的 HTTPS 压缩包地址。"))?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !(url.path().ends_with(".tgz") || url.path().ends_with(".tar.gz"))
        {
            return Err(invalid("首批安装仅接受无账号信息的 HTTPS 依赖压缩包。"));
        }
        Ok(())
    }
    package_fields(&package)?;
    for (location, package) in lock["packages"].as_object().unwrap() {
        if !location.is_empty()
            && (!location.starts_with("node_modules/")
                || location
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
                || location.contains(['\\', '\0', ':']))
        {
            return Err(invalid("锁文件包含非受管安装位置。"));
        }
        package_fields(package)?;
        if !location.is_empty() {
            spec(&package["version"])?;
            if let Some(value) = package.get("resolved") {
                resolved(value)?;
            } else if package["inBundle"].as_bool() != Some(true) {
                return Err(invalid("锁文件依赖缺少固定下载地址。"));
            }
        }
    }
    fn legacy(value: &Value) -> Result<()> {
        if let Some(packages) = value.as_object() {
            for package in packages.values() {
                spec(&package["version"])?;
                if let Some(value) = package.get("resolved") {
                    resolved(value)?;
                }
                dependency_map(&package["requires"])?;
                legacy(&package["dependencies"])?;
            }
        }
        Ok(())
    }
    legacy(&lock["dependencies"])
}

fn scan_dependency_space(
    roots: &[&Path],
    control: &DependencyControl,
    available: impl Fn(&Path) -> std::io::Result<u64>,
) -> Result<()> {
    fn over_budget() -> ApiError {
        failure(
            "NODE_INSTALL_SPACE_LIMIT",
            "安装准备的文件大小、数量或总占用超过预算，已停止依赖准备。",
        )
    }
    fn free_space(
        operation: &Path,
        control: &DependencyControl,
        available: &impl Fn(&Path) -> std::io::Result<u64>,
    ) -> Result<()> {
        control.check()?;
        let bytes = available(operation).map_err(|_| {
            failure(
                "NODE_INSTALL_SPACE_CHECK_FAILED",
                "无法核对安装目录所在磁盘的余量，已停止准备。",
            )
        })?;
        if bytes < control.npm_space.minimum_free {
            return Err(failure(
                "NODE_INSTALL_SPACE_INSUFFICIENT",
                "磁盘可用空间不足，已停止依赖准备。",
            ));
        }
        Ok(())
    }
    fn visit(
        operation: &Path,
        path: &Path,
        depth: usize,
        control: &DependencyControl,
        available: &impl Fn(&Path) -> std::io::Result<u64>,
        usage: &mut (usize, u64),
    ) -> Result<()> {
        control.check()?;
        if depth > 128 {
            return Err(over_budget());
        }
        check_no_symlinks(path, true)?;
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            // npm may rename/delete cache and extraction entries while scanned.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            control.check()?;
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let metadata = match fs::symlink_metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            usage.0 = usage.0.checked_add(1).ok_or_else(over_budget)?;
            if usage.0 > control.npm_space.max_entries {
                return Err(over_budget());
            }
            if usage.0.is_multiple_of(256) {
                free_space(operation, control, available)?;
            }
            if metadata.is_dir() {
                visit(
                    operation,
                    &entry.path(),
                    depth + 1,
                    control,
                    available,
                    usage,
                )?;
            } else if metadata.is_file() || metadata.file_type().is_symlink() {
                // Metadata only: never open file contents or follow npm's links.
                // Final program validation separately checks relative bin links
                // and rejects hard links; transient cache links are counted here.
                if metadata.is_file() && metadata.len() > control.npm_space.max_file {
                    return Err(over_budget());
                }
                usage.1 = usage
                    .1
                    .checked_add(metadata.len())
                    .ok_or_else(over_budget)?;
                if usage.1 > control.npm_space.max_total {
                    return Err(over_budget());
                }
            } else {
                return Err(invalid("依赖准备目录出现特殊文件，已停止准备。"));
            }
        }
        Ok(())
    }
    let mut selected: Vec<&Path> = Vec::new();
    for path in roots {
        if !selected.iter().any(|parent| path.starts_with(parent)) {
            selected.retain(|child| !child.starts_with(path));
            selected.push(path);
        }
    }
    let mut usage = (0, 0);
    for path in selected {
        ordinary_directory(path)?;
        free_space(path, control, &available)?;
        visit(path, path, 0, control, &available, &mut usage)?;
        free_space(path, control, &available)?;
    }
    Ok(())
}

async fn check_npm_space(
    operation: &Path,
    project: &Path,
    control: &DependencyControl,
) -> Result<()> {
    let operation = operation.to_path_buf();
    let project = project.to_path_buf();
    let control = control.clone();
    // Await the scanner rather than detaching it on cancellation. It checks the
    // same stop/deadline and cannot outlive the installation's source lease.
    tokio::task::spawn_blocking(move || {
        scan_dependency_space(&[&operation, &project], &control, |path| {
            fs2::available_space(path)
        })
    })
    .await
    .map_err(|_| failure("NODE_INSTALL_SPACE_CHECK_FAILED", "依赖空间检查中断。"))?
}

async fn npm_ci_with_budget(
    operation: &Path,
    cwd: &Path,
    project_root: &Path,
    tools: &Toolchain,
    control: &DependencyControl,
    processes: NodeProcessScope,
) -> Result<()> {
    control.check()?;
    let home = operation.join("npm-home");
    let cache = operation.join("npm-cache");
    private_dir(&home)?;
    private_dir(&cache)?;
    let user_config = home.join("user.npmrc");
    let global_config = home.join("global.npmrc");
    write_new(&user_config, b"", false)?;
    write_new(&global_config, b"", false)?;
    check_npm_space(operation, project_root, control).await?;
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
    let diagnostics = ProcessDiagnostics::default();
    let stdout_diagnostics = diagnostics.clone();
    let stderr_diagnostics = diagnostics.clone();
    let reader_scope = processes.clone();
    let mut run = processes
        .processes
        .start(
            command,
            ProcessOptions::new(
                "node-install/npm",
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
                        .finish_reader(
                            "node-install/npm",
                            "ci",
                            stdout_diagnostics.read(stdout, "stdout", Some(MAX_OUTPUT)),
                        )
                        .await
                })
                .drain("install-stderr", async move {
                    stderr_diagnostics
                        .read(stderr, "stderr", Some(MAX_OUTPUT))
                        .await
                })
            },
        )
        .map_err(|error| {
            diagnostics.attach(error, "npm ci --omit=dev --ignore-scripts", tools, None)
        })?;
    let mut next_space_check = Instant::now() + NPM_SPACE_INTERVAL;
    let mut interruption = None;
    let result = loop {
        if let Err(error) = control.check() {
            interruption.get_or_insert(error);
            run.cancel();
        }
        if Instant::now() >= next_space_check {
            // Own and await the actual blocking scanner; cancellation cannot
            // detach its filesystem I/O from the current source lease.
            if let Err(error) = check_npm_space(operation, project_root, control).await {
                interruption.get_or_insert(error);
                run.cancel();
            }
            next_space_check = Instant::now() + NPM_SPACE_INTERVAL;
        }
        tokio::select! {
            result = &mut run => break result,
            _ = tokio::time::sleep(Duration::from_millis(25)) => {},
        }
    };
    while matches!(
        run.snapshot().phase,
        ProcessPhase::Running | ProcessPhase::Stopping | ProcessPhase::CleanupPending
    ) {
        if let Err(error) = control.check() {
            interruption.get_or_insert(error);
            run.cancel();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let receipt = run.snapshot();
    let closed = receipt.phase == ProcessPhase::Closed
        && receipt.leader_reaped
        && receipt.group_exited
        && receipt.io_joined;
    if !closed {
        let error = result.err().unwrap_or_else(|| {
            failure(
                "NODE_INSTALL_CLEANUP_PENDING",
                "安装进程归属未确认，不能发布；现场及租约保留。",
            )
        });
        return Err(diagnostics.attach(
            error
                .detail("processResourceClosed", false)
                .outcome("unknown"),
            "npm ci --omit=dev --ignore-scripts",
            tools,
            None,
        ));
    }
    if let Some(error) = interruption {
        return Err(diagnostics.attach(
            error.detail("processResourceClosed", true),
            "npm ci --omit=dev --ignore-scripts",
            tools,
            result.as_ref().ok().map(|output| &output.status),
        ));
    }
    let output = result.map_err(|error| {
        let code = match error.code.as_str() {
            "PROCESS_CANCELLED" => "NODE_INSTALL_INTERRUPTED",
            "PROCESS_WORK_TIMEOUT" => "NODE_INSTALL_TIMEOUT",
            "PROCESS_CLEANUP_PENDING" => "NODE_INSTALL_NPM_FAILED",
            _ => "NODE_INSTALL_OUTPUT_LIMIT",
        };
        diagnostics.attach(
            failure(code, "依赖准备未完成；进程和输出已收尾，当前程序未改变。")
                .detail("processCauseCode", error.code)
                .detail("processResourceClosed", true),
            "npm ci --omit=dev --ignore-scripts",
            tools,
            None,
        )
    })?;
    if !output.status.success() {
        return Err(diagnostics.attach(
            failure(
                "NODE_INSTALL_NPM_FAILED",
                "依赖下载或准备失败；当前程序、配置及数据未改变。",
            )
            .detail("processResourceClosed", true),
            "npm ci --omit=dev --ignore-scripts",
            tools,
            Some(&output.status),
        ));
    }
    check_npm_space(operation, project_root, control).await
}
fn ordinary_directory(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return Err(invalid("安装路径必须为规范的绝对路径。"));
    }
    check_no_symlinks(path, false)?;
    if !fs::metadata(path)?.is_dir() {
        return Err(invalid("安装路径不是普通目录。"));
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<()> {
    check_no_symlinks(path, true)?;
    fs::create_dir_all(path)?;
    ordinary_directory(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    check_no_symlinks(path, false)?;
    let before = fs::symlink_metadata(path)?;
    if !before.is_file() || before.nlink() != 1 || before.len() > limit {
        return Err(invalid("安装文件必须是预算内的独立普通文件。"));
    }
    let file = platform_native::filesystem::open_read(
        path,
        platform_native::filesystem::ReadOptions::NoLinks,
    )?;
    let opened = file.metadata()?;
    if before.dev() != opened.dev() || before.ino() != opened.ino() {
        return Err(changed());
    }
    let mut bytes = vec![];
    file.take(limit + 1).read_to_end(&mut bytes)?;
    let after = fs::symlink_metadata(path)?;
    if bytes.len() as u64 > limit
        || before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.mode() != after.mode()
    {
        return Err(changed());
    }
    Ok(bytes)
}
pub(crate) fn read_small(path: &Path) -> Result<Vec<u8>> {
    read_file(path, MAX_METADATA)
}
fn write_new(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    check_no_symlinks(path, true)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(if executable { 0o700 } else { 0o600 })
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[path = "node_dependency_prepare_tests.rs"]
mod tests;
