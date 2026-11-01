//! User-requested npm builds in the author's one project directory.
//! A caller owns admission and registration; this runner owns real processes.
use crate::{
    environment::{EnvironmentManager, ExecutableInfo, Toolchain},
    owned_process::{
        IoPlan, OwnedCommand, OwnedProcessRegistry, ProcessOptions, ProcessPhase, ProcessPipes,
    },
    panda_manifest::{BuildDecl, PandaManifest, PandaRuntime},
    process_diagnostics::ProcessDiagnostics,
    ApiError,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::watch, time::Instant};

mod npm;
mod space;
const BUILD_TIMEOUT: Duration = Duration::from_secs(600);
const METADATA_MAX: u64 = 8 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct WebBuildTarget {
    pub project_root: PathBuf,
    pub manifest: PandaManifest,
}
pub(crate) type WebBuildProgress = Arc<dyn Fn(&str) -> Result<(), ApiError> + Send + Sync>;
pub(crate) struct WebBuildContext {
    pub environment: EnvironmentManager,
    pub operation_root: PathBuf,
    pub cache_root: PathBuf,
    pub processes: OwnedProcessRegistry,
    pub owner: Arc<dyn Send + Sync>,
    pub stop: watch::Receiver<bool>,
    pub cancel: watch::Receiver<bool>,
    pub progress: WebBuildProgress,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebBuildOutcome {
    pub entry: PathBuf,
    pub node: ExecutableInfo,
    pub npm: ExecutableInfo,
    pub completed_at: u64,
}
pub(crate) struct WebBuildRunner;
impl WebBuildRunner {
    pub(crate) async fn run(
        target: WebBuildTarget,
        context: WebBuildContext,
    ) -> Result<WebBuildOutcome, ApiError> {
        let start = Instant::now();
        check(&context, start)?;
        target
            .manifest
            .validate()
            .map_err(|error| invalid(&error.to_string()))?;
        if target.manifest.runtime != PandaRuntime::Web {
            return Err(invalid("Node 项目不使用平台构建。"));
        }
        target
            .manifest
            .inspect(&target.project_root)
            .map_err(|error| invalid(&error.to_string()))?;
        let development =
            target.manifest.development.as_ref().ok_or_else(|| {
                invalid("panda.json 未声明 development.build，不能执行平台构建。")
            })?;
        let source = crate::panda_manifest::checked_project_child(
            &target.project_root,
            &development.source_dir,
            true,
        )
        .map_err(|error| invalid(&format!("development.sourceDir：{error}")))?;
        let package = read_package(
            &source.join("package.json"),
            "PACKAGE_JSON_MISSING",
            "development.sourceDir 下缺少 package.json。",
        )?;
        let lock = read_package(
            &source.join("package-lock.json"),
            "LOCKFILE_MISSING",
            "缺少 package-lock.json，请让 AI 先生成 npm v2/v3 锁文件。",
        )?;
        if source.join("npm-shrinkwrap.json").try_exists()? {
            return Err(ApiError::new(
                "LOCKFILE_INVALID",
                "平台构建只使用 package-lock.json，请让 AI 移除 npm-shrinkwrap.json 后重试。",
            ));
        }
        validate_lock(&package, &lock)?;
        let BuildDecl::Npm { script } = &development.build;
        if package["scripts"][script]
            .as_str()
            .is_none_or(|value| value.is_empty())
        {
            return Err(ApiError::new(
                "BUILD_SCRIPT_INVALID",
                format!("development.build.script：package.json 中没有脚本 {script}。"),
            ));
        }
        for directory in [&context.operation_root, &context.cache_root] {
            crate::project_file_io::ensure_directory(directory)?;
        }
        (context.progress)("environment")?;
        let tools = context
            .environment
            .with_process_registry(context.processes.clone())
            .resolve_with_keepalive(context.owner.clone())
            .await
            .map_err(crate::node_process_scope::NodeProcessScope::resolution_error)?;
        check(&context, start)?;
        (context.progress)("dependencies")?;
        command(
            &context,
            &tools,
            &source,
            &["ci", "--no-audit", "--no-fund"],
            start,
        )
        .await?;
        check(&context, start)?;
        (context.progress)("build")?;
        command(&context, &tools, &source, &["run", script], start).await?;
        check(&context, start)?;
        (context.progress)("checking-entry")?;
        let entry = target
            .manifest
            .resolve_web_entry(&target.project_root)
            .map_err(|error| {
                ApiError::new(
                    "BUILD_ENTRY_MISSING",
                    format!("构建结束后入口文件仍不可用：{error}"),
                )
            })?;
        Ok(WebBuildOutcome {
            entry,
            node: tools.node,
            npm: tools.npm,
            completed_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        })
    }
}
fn invalid(message: &str) -> ApiError {
    ApiError::new("BUILD_INPUT_INVALID", message)
}
fn check(context: &WebBuildContext, start: Instant) -> Result<(), ApiError> {
    if *context.stop.borrow() || context.stop.has_changed().is_err() || *context.cancel.borrow() {
        return Err(ApiError::new(
            "BUILD_INTERRUPTED",
            "构建已取消或平台正在退出。",
        ));
    }
    if start.elapsed() >= BUILD_TIMEOUT {
        return Err(ApiError::new("BUILD_TIMEOUT", "构建超过 10 分钟预算。"));
    }
    Ok(())
}
fn read_package(path: &Path, missing_code: &str, message: &str) -> Result<Value, ApiError> {
    crate::projects::check_no_symlinks(path, true)?;
    let file = platform_native::filesystem::open_read(
        path,
        platform_native::filesystem::ReadOptions::NoLinksNonblocking,
    )
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ApiError::new(missing_code, message)
        } else {
            error.into()
        }
    })?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > METADATA_MAX {
        return Err(invalid("npm 构建声明不是普通文件或超过读取上限。"));
    }
    let mut bytes = Vec::new();
    file.take(METADATA_MAX + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > METADATA_MAX {
        return Err(invalid("npm 构建声明超过读取上限。"));
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| invalid("npm 构建声明不是有效 JSON。"))?;
    if !value.is_object() {
        return Err(invalid("npm 构建声明必须是 JSON 对象。"));
    }
    Ok(value)
}
pub(crate) fn validate_lock(package: &Value, lock: &Value) -> Result<(), ApiError> {
    if !matches!(lock["lockfileVersion"].as_u64(), Some(2 | 3)) {
        return Err(ApiError::new(
            "LOCKFILE_INVALID",
            "请让 AI 生成 lockfileVersion 为 2 或 3 的 npm package-lock.json。",
        ));
    }
    let root = lock
        .pointer("/packages/")
        .filter(|value| value.is_object())
        .ok_or_else(|| ApiError::new("LOCKFILE_INVALID", "package-lock.json 缺少根 package。"))?;
    for key in ["dependencies", "devDependencies", "optionalDependencies"] {
        let empty = json!({});
        if package.get(key).unwrap_or(&empty) != root.get(key).unwrap_or(&empty) {
            return Err(ApiError::new(
                "LOCKFILE_MISMATCH",
                format!("package.json 的 {key} 与锁文件不同，请让 AI 更新锁文件。"),
            ));
        }
    }
    Ok(())
}
async fn command(
    context: &WebBuildContext,
    tools: &Toolchain,
    cwd: &Path,
    args: &[&str],
    start: Instant,
) -> Result<(), ApiError> {
    check(context, start)?;
    check_space(context, cwd).await?;
    let command_label = if args.first() == Some(&"run") {
        format!("npm run {}", args[1])
    } else {
        "npm ci".into()
    };
    let mut command = npm::command(tools, cwd, &context.operation_root, &context.cache_root)?;
    command.args(args);
    let diagnostics = ProcessDiagnostics::default();
    let stdout = diagnostics.clone();
    let stderr = diagnostics.clone();
    let mut run = context
        .processes
        .start(
            OwnedCommand::cleared(command),
            ProcessOptions::new(
                "web-build/npm",
                BUILD_TIMEOUT.saturating_sub(start.elapsed()),
            ),
            context.owner.clone(),
            move |pipes| {
                let ProcessPipes {
                    stdin,
                    stdout: out,
                    stderr: err,
                    ..
                } = pipes;
                drop(stdin);
                IoPlan::new(async move { stdout.read(out, "stdout", None).await })
                    .drain("npm-stderr", async move {
                        stderr.read(err, "stderr", None).await
                    })
            },
        )
        .map_err(|error| diagnostics.attach(error, &command_label, tools, None))?;
    let mut stop = context.stop.clone();
    let mut cancel = context.cancel.clone();
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut monitor_error = None;
    let result = loop {
        tokio::select! {
            result=&mut run=>break result,
            _=stop.changed()=>{run.cancel();break (&mut run).await;},
            _=cancel.changed()=>{run.cancel();break (&mut run).await;},
            _=interval.tick()=>{
                if let Err(issue)=check(context,start){monitor_error=Some(issue);run.cancel();break (&mut run).await;}
                let roots=vec![cwd.join("node_modules"),context.operation_root.clone(),context.cache_root.clone()];
                let monitored=tokio::task::spawn_blocking(move||space::check(&roots)).await;
                if let Err(issue)=monitored.unwrap_or_else(|error|Err(ApiError::new("BUILD_SPACE_CHECK_FAILED",error.to_string()))) {
                    monitor_error=Some(issue);run.cancel();break (&mut run).await;
                }
            }
        }
    };
    // A timeout reply never releases a still-owned writer or its source lease.
    while matches!(
        run.snapshot().phase,
        ProcessPhase::Running | ProcessPhase::Stopping | ProcessPhase::CleanupPending
    ) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let snapshot = run.snapshot();
    let resource_closed = snapshot.phase == ProcessPhase::Closed
        && snapshot.leader_reaped
        && snapshot.group_exited
        && snapshot.io_joined;
    if let Some(error) = monitor_error {
        return Err(diagnostics.attach(
            error.detail("processResourceClosed", resource_closed),
            &command_label,
            tools,
            None,
        ));
    }
    let output = result.map_err(|mut error| {
        let original = error.code.clone();
        error.code = match original.as_str() {
            "PROCESS_WORK_TIMEOUT" => "BUILD_TIMEOUT".into(),
            "PROCESS_CANCELLED" => "BUILD_INTERRUPTED".into(),
            _ => original.clone(),
        };
        diagnostics.attach(
            error
                .detail("processCauseCode", original)
                .detail("processResourceClosed", resource_closed),
            &command_label,
            tools,
            None,
        )
    })?;
    if !output.status.success() {
        return Err(diagnostics.attach(
            ApiError::new("BUILD_FAILED", format!("{command_label} 执行失败。")),
            &command_label,
            tools,
            Some(&output.status),
        ));
    }
    check_space(context, cwd).await?;
    Ok(())
}
async fn check_space(context: &WebBuildContext, cwd: &Path) -> Result<(), ApiError> {
    let roots = vec![
        cwd.join("node_modules"),
        context.operation_root.clone(),
        context.cache_root.clone(),
    ];
    tokio::task::spawn_blocking(move || space::check(&roots))
        .await
        .map_err(|error| ApiError::new("BUILD_SPACE_CHECK_FAILED", error.to_string()))?
}

#[cfg(test)]
#[path = "web_build/tests.rs"]
mod tests;
