//! Explicit source builds. Author repositories remain the only editable Git
//! checkout; installation consumes a verified, immutable program candidate.
use crate::{
    builds::BuildManager, node_apps::NodeApps, source_acquisition::SourceAcquisitionManager,
    ApiError, ProjectStore, RuntimePaths, TaskRecord,
};
use futures_util::FutureExt;
use serde_json::json;
use std::{
    collections::HashMap,
    panic::AssertUnwindSafe,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::{
    sync::{watch, Notify},
    task::JoinHandle,
};

type Result<T> = std::result::Result<T, ApiError>;
pub struct NodeDevelopmentTasks {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    git: crate::git_environment::GitEnvironmentManager,
    sources: Result<Arc<SourceAcquisitionManager>>,
    active: Mutex<HashMap<String, watch::Sender<bool>>>,
    runners: Mutex<Vec<JoinHandle<()>>>,
    stopping: AtomicBool,
    drained: Notify,
}
impl NodeDevelopmentTasks {
    pub fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        git: crate::git_environment::GitEnvironmentManager,
        sources: Result<Arc<SourceAcquisitionManager>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            paths,
            store,
            tasks,
            git,
            sources,
            active: Mutex::new(HashMap::new()),
            runners: Mutex::new(Vec::new()),
            stopping: AtomicBool::new(false),
            drained: Notify::new(),
        })
    }
    pub fn require_idle(&self) -> Result<()> {
        self.store.check_workspace_available()?;
        if self.stopping.load(Ordering::Acquire) {
            return Err(interrupted());
        }
        if !self.active.lock().unwrap().is_empty() {
            return Err(ApiError::new(
                "NODE_DEVELOPMENT_BUSY",
                "源码构建尚未结束，请等待或取消。",
            ));
        }
        Ok(())
    }
    pub async fn build(
        self: &Arc<Self>,
        apps: Arc<NodeApps>,
        app_id: String,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord> {
        self.require_idle()?;
        check(&stop)?;
        {
            let lease = apps.development_build_source_lease(&app_id).await?;
            lease.validate()?;
        }
        let mut runners = self.runners.lock().unwrap();
        let mut active = self.active.lock().unwrap();
        if self.stopping.load(Ordering::Acquire) || !active.is_empty() {
            return Err(ApiError::new(
                "NODE_DEVELOPMENT_BUSY",
                "已有源码构建正在进行。",
            ));
        }
        let task = self
            .tasks
            .create_workspace_operation("nodeDevelopmentBuild")?;
        let lease = match self
            .store
            .reserve_task(&format!("node-development:{app_id}"), &task.task_id)
        {
            Ok(lease) => lease,
            Err(error) => {
                self.tasks.finish(&task.task_id, Err(error.clone()))?;
                return Err(error);
            }
        };
        let (sender, cancel) = watch::channel(false);
        active.insert(task.task_id.clone(), sender);
        drop(active);
        let manager = self.clone();
        let task_id = task.task_id.clone();
        let runner = tokio::spawn(async move {
            let _lease = lease;
            let _active = Active {
                manager: manager.clone(),
                task_id: task_id.clone(),
            };
            let result = AssertUnwindSafe(async {
                check(&stop)?; check(&cancel)?;
                let source_lease = apps.development_build_source_lease(&app_id).await?;
                source_lease.validate()?;
                let source = source_lease.source().clone();
                let git = crate::node_development_git::NodeGit::new(manager.git.clone()).with_stop(stop.clone());
                let before = git.status(&source.source_root).await?;
                if !before.scope.complete || !before.scope.protected_tracked_paths.is_empty() || !matches!(before.repository.state.as_str(), "none" | "project-root") || (before.repository.state == "project-root" && before.repository.git.is_none()) {
                    return Err(ApiError::new("NODE_DEVELOPMENT_SCOPE_INVALID", "请先在源码版本中核对并处理源码范围。"));
                }
                manager.tasks.phase(&task_id, "building-source")?;
                let tools = manager.tasks.environment().resolve().await?;
                let build_root = manager.paths.data_root().join("node-development-builds");
                if !build_root.try_exists()? { std::fs::create_dir(&build_root)?; }
                crate::projects::check_no_symlinks(&build_root, false)?;
                let operation = build_root.join(&task_id);
                let output = crate::node_development_build::build_repository_scoped(&source.source_root, &operation, &tools, stop.clone(), cancel.clone(), &before.scope.included_paths).await?;
                source.validate()?;
                source_lease.validate()?;
                let after = git.status(&source.source_root).await?;
                if before.snapshot_digest != after.snapshot_digest || serde_json::to_value(&before.repository.head)? != serde_json::to_value(&after.repository.head)? { return Err(ApiError::new("NODE_DEVELOPMENT_CHANGED", "构建时源码或Git版本已改变，请重新构建。")); }
                let source_digest = output.source_digest.clone();
                let built = output.built;
                let commit = after.repository.head.and_then(|head| head.oid);
                manager.tasks.phase(&task_id, "capturing-program")?;
                check(&cancel)?;
                let sources = manager.sources.as_ref().map_err(Clone::clone)?;
                let name = crate::node_app_manifest::TrialManifest::load(&source.source_root).map_err(|_| ApiError::new("NODE_DEVELOPMENT_CHANGED", "源码清单已改变。"))?.app.name;
                let capture = sources.capture_built_output(output, Some(format!("{name} · 开发构建")), commit.clone(), stop.clone()).await?;
                loop {
                    let record = manager.tasks.task(&capture.task_id)?;
                    if record.status.is_final() {
                        if let Some(error) = record.error { return Err(error); }
                        let id = record.result.as_ref().and_then(|v| v["sourceId"].as_str()).ok_or_else(|| ApiError::new("NODE_DEVELOPMENT_BUILD_FAILED", "构建来源记录不完整，现场已保留。"))?;
                        let candidate = sources.info(id)?;
                        source_lease.validate()?;
                        check(&cancel)?; check(&stop)?;
                        return Ok(json!({"appId":app_id,"source":candidate,"sourceCommit":commit,"sourceDigest":source_digest,"built":built,"operationOutcome":"prepared","installed":false}));
                    }
                    if check(&stop).is_err() || check(&cancel).is_err() { let _ = sources.cancel(&capture.task_id); }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }).catch_unwind().await;
            let result = result.unwrap_or_else(|_| Err(interrupted()));
            let _ = manager.tasks.finish(&task_id, result);
        });
        // Preserve completed outcomes for the shutdown join; panic is not success.
        runners.push(runner);
        Ok(task)
    }
    pub fn cancel(&self, task_id: &str) -> Result<TaskRecord> {
        let task = self.tasks.task(task_id)?;
        if task.task_type != "nodeDevelopmentBuild" {
            return Err(ApiError::new(
                "NODE_DEVELOPMENT_TASK_INVALID",
                "这不是源码构建任务。",
            ));
        }
        if let Some(sender) = self.active.lock().unwrap().get(task_id) {
            let _ = sender.send(true);
        }
        Ok(task)
    }
    pub(crate) fn request_stop(&self) {
        let _submission = self.runners.lock().unwrap();
        self.stopping.store(true, Ordering::Release);
        for sender in self.active.lock().unwrap().values() {
            let _ = sender.send(true);
        }
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.request_stop();
        let runners = std::mem::take(&mut *self.runners.lock().unwrap());
        let issues = crate::shutdown::join_all("nodeDevelopment", runners).await;
        loop {
            let drained = self.drained.notified();
            if self.active.lock().unwrap().is_empty() {
                break;
            }
            drained.await;
        }
        issues
    }
}
struct Active {
    manager: Arc<NodeDevelopmentTasks>,
    task_id: String,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.manager.active.lock().unwrap().remove(&self.task_id);
        self.manager.drained.notify_waiters();
    }
}
fn interrupted() -> ApiError {
    ApiError::new(
        "NODE_DEVELOPMENT_INTERRUPTED",
        "构建已中断，应用未更新，操作现场已保留。",
    )
}
fn check(receiver: &watch::Receiver<bool>) -> Result<()> {
    if *receiver.borrow() || receiver.has_changed().is_err() {
        Err(interrupted())
    } else {
        Ok(())
    }
}
