//! Task ownership for explicit managed Node installation. This API accepts only
//! ready source IDs and candidate IDs. Native code owns the window-operation
//! lock around apply; this manager owns task/source leases and process cleanup.
use crate::{
    builds::BuildManager,
    node_app_install::{
        InstallControl, InstallTargetSnapshot, NodeAppInstaller, VerifiedNodeSource,
    },
    node_apps::NodeApps,
    source_acquisition::SourceAcquisitionManager,
    ApiError, ProjectStore, RuntimePaths, TaskRecord,
};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareNodeInstallInput {
    pub source_id: String,
    pub expected_source_digest: String,
    pub target_app_id: Option<String>,
}

pub struct NodeInstallTasks {
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    sources: Result<Arc<SourceAcquisitionManager>>,
    installer: Result<NodeAppInstaller>,
    issues: Vec<ApiError>,
    submission: Mutex<()>,
    active: Mutex<HashMap<String, watch::Sender<bool>>>,
    runners: Mutex<Vec<JoinHandle<()>>>,
    stopping: AtomicBool,
    drained: Notify,
}
impl NodeInstallTasks {
    /// Damaged install evidence disables only installation as needed. Existing
    /// application registrations remain owned by NodeApps and are not rewritten.
    pub fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        sources: Result<Arc<SourceAcquisitionManager>>,
    ) -> Arc<Self> {
        let mut issues = Vec::new();
        let installer =
            NodeAppInstaller::new(paths.data_root().to_path_buf()).and_then(|installer| {
                for result in installer.recover(&tasks)? {
                    if result["state"] == "needs-attention" {
                        issues.push(
                            ApiError::new(
                                "NODE_INSTALL_RECOVERY_REQUIRED",
                                "一项安装记录无法确认，现场已保留；原应用登记未改动。",
                            )
                            .detail("candidateId", result["candidateId"].clone())
                            .outcome("unknown"),
                        );
                    }
                }
                Ok(installer)
            });
        if let Err(error) = &installer {
            issues.push(error.clone());
        }
        Arc::new(Self {
            store,
            tasks,
            sources,
            installer,
            issues,
            submission: Mutex::new(()),
            active: Mutex::new(HashMap::new()),
            runners: Mutex::new(Vec::new()),
            stopping: AtomicBool::new(false),
            drained: Notify::new(),
        })
    }
    pub fn issues(&self) -> &[ApiError] {
        &self.issues
    }
    /// Native application deletion holds the window-operation lock across this
    /// check and the move. Installation submissions use that same lock, while
    /// this guard also rejects an already running asynchronous prepare task.
    pub fn require_idle(&self) -> Result<()> {
        self.store.check_workspace_available()?;
        if self.stopping.load(Ordering::Acquire) {
            return Err(interrupted());
        }
        if !self.active.lock().unwrap().is_empty() {
            return Err(ApiError::new(
                "NODE_INSTALL_BUSY",
                "安装任务尚未结束，请等待完成或取消准备后再删除应用。",
            ));
        }
        Ok(())
    }
    pub fn list(&self) -> Result<Value> {
        self.store.check_workspace_available()?;
        let mut issues = self.issues.clone();
        let candidates = match self.installer().and_then(NodeAppInstaller::list) {
            Ok(candidates) => candidates,
            Err(error) => {
                if !issues
                    .iter()
                    .any(|issue| issue.code == error.code && issue.message == error.message)
                {
                    issues.push(error);
                }
                Vec::new()
            }
        };
        Ok(json!({"candidates":candidates,"issues":issues}))
    }
    fn installer(&self) -> Result<&NodeAppInstaller> {
        self.installer.as_ref().map_err(Clone::clone)
    }
    fn accepting(&self, stop: &watch::Receiver<bool>) -> Result<()> {
        self.store.check_workspace_available()?;
        if self.stopping.load(Ordering::Acquire) || *stop.borrow() || stop.has_changed().is_err() {
            return Err(interrupted());
        }
        if !self.active.lock().unwrap().is_empty() {
            return Err(ApiError::new(
                "NODE_INSTALL_BUSY",
                "已有安装任务尚未完成，请等待完成或取消准备。",
            ));
        }
        Ok(())
    }
    /// A read-only lookup for native window guards. It never opens a window or
    /// turns a source path supplied by the caller into an installation target.
    pub fn target_app_id(&self, candidate_id: &str) -> Result<String> {
        self.store.check_workspace_available()?;
        let installer = self.installer()?;
        if let Some(installed) = installer.committed(candidate_id)? {
            return Ok(installed.app_id);
        }
        Ok(installer.prepared(candidate_id)?.app_id)
    }
    pub fn candidate(&self, candidate_id: &str) -> Result<Value> {
        self.store.check_workspace_available()?;
        let installer = self.installer()?;
        if let Some(installed) = installer.committed(candidate_id)? {
            return Ok(json!({"state":"committed","result":installed}));
        }
        Ok(json!({"state":"prepared","candidate":installer.prepared(candidate_id)?}))
    }
    pub async fn prepare(
        self: &Arc<Self>,
        node_apps: Arc<NodeApps>,
        input: PrepareNodeInstallInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord> {
        self.installer()?;
        self.accepting(&stop)?;
        let sources = self.sources.as_ref().map_err(Clone::clone)?.clone();
        // Initial verification owns its short transient source lease. The task
        // then acquires the same source key and verifies again under that lease.
        let source_id = input.source_id.clone();
        let digest = input.expected_source_digest.clone();
        let check_sources = sources.clone();
        let source =
            tokio::task::spawn_blocking(move || check_sources.verify_ready(&source_id, &digest))
                .await
                .map_err(|_| interrupted())??;
        require_node_source(&source)?;
        let target = node_apps
            .install_target_snapshot(input.target_app_id.as_deref())
            .await?;
        let _submission = self.submission.lock().unwrap();
        self.accepting(&stop)?;
        let task = self
            .tasks
            .create_workspace_operation("nodeInstallPrepare")?;
        let lease = match self
            .store
            .reserve_task(&format!("source:{}", input.source_id), &task.task_id)
        {
            Ok(lease) => lease,
            Err(error) => {
                self.tasks.finish_partial(
                    &task.task_id,
                    json!({"installed":false}),
                    error.clone(),
                )?;
                return Err(error.detail("taskId", task.task_id));
            }
        };
        let (sender, cancel) = watch::channel(false);
        self.active
            .lock()
            .unwrap()
            .insert(task.task_id.clone(), sender);
        let manager = self.clone();
        let task_id = task.task_id.clone();
        let runner = tokio::spawn(async move {
            let _lease = lease;
            let _active = ActiveTask {
                manager: manager.clone(),
                task_id: task_id.clone(),
            };
            let result = AssertUnwindSafe(
                manager.run_prepare(&task_id, sources, input, target, stop, cancel),
            )
            .catch_unwind()
            .await;
            let error = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error),
                Err(_) => Some(interrupted()),
            };
            if let Some(error) = error {
                manager.finish_error(&task_id, None, error);
            }
        });
        let mut runners = self.runners.lock().unwrap();
        runners.retain(|runner| !runner.is_finished());
        runners.push(runner);
        Ok(task)
    }
    async fn run_prepare(
        &self,
        task_id: &str,
        sources: Arc<SourceAcquisitionManager>,
        input: PrepareNodeInstallInput,
        target: InstallTargetSnapshot,
        stop: watch::Receiver<bool>,
        cancel: watch::Receiver<bool>,
    ) -> Result<()> {
        let control = InstallControl::new(stop, cancel);
        control.check()?;
        let source = self
            .sources
            .as_ref()
            .map_err(Clone::clone)?
            .verify_ready_leased(&input.source_id, &input.expected_source_digest, task_id)?;
        require_node_source(&source)?;
        let check_source = input.source_id.clone();
        let check_digest = input.expected_source_digest.clone();
        let check_task = task_id.to_owned();
        let check_build_digest = source.build_source_digest.clone();
        let check_commit = source.commit.clone();
        let verifier: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(move || {
            let current = sources.verify_ready_leased(&check_source, &check_digest, &check_task)?;
            require_node_source(&current)?;
            if current.build_source_digest != check_build_digest || current.commit != check_commit {
                return Err(ApiError::new(
                    "SOURCE_CHANGED",
                    "构建来源记录已变化，请重新准备。",
                ));
            }
            Ok(())
        });
        let verified = VerifiedNodeSource::new(
            source.source_id,
            source
                .source_path
                .ok_or_else(|| ApiError::new("SOURCE_CHANGED", "源码候选没有可用的已核验目录。"))?,
            input.expected_source_digest,
            source.commit,
        )?
        .with_build_source_digest(source.build_source_digest)?
        .with_revalidation(verifier);
        self.tasks.phase(task_id, "environment")?;
        let tools = self.tasks.environment().resolve().await?;
        control.check()?;
        self.installer()?
            .prepare(task_id, verified, target, tools, control, &self.tasks)
            .await?;
        Ok(())
    }
    /// Caller must retain the native NodeWindows operation lock and positively
    /// verify no matching window/session until this future returns. Apply is not
    /// spawned: its final TaskRecord reflects the actual registration pointer.
    pub async fn apply(
        self: &Arc<Self>,
        node_apps: Arc<NodeApps>,
        candidate_id: &str,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord> {
        let installer = self.installer()?;
        let app_id = self.target_app_id(candidate_id)?;
        let (task, lease, cancel) = {
            let _submission = self.submission.lock().unwrap();
            self.accepting(&stop)?;
            let task = self.tasks.create_workspace_operation("nodeInstallApply")?;
            let lease = match self
                .store
                .reserve_task(&format!("node-install:{app_id}"), &task.task_id)
            {
                Ok(lease) => lease,
                Err(error) => {
                    self.tasks.finish_partial(
                        &task.task_id,
                        json!({"candidateId":candidate_id,"operationOutcome":"not-committed"}),
                        error.clone(),
                    )?;
                    return Err(error.detail("taskId", task.task_id));
                }
            };
            let (sender, cancel) = watch::channel(false);
            self.active
                .lock()
                .unwrap()
                .insert(task.task_id.clone(), sender);
            (task, lease, cancel)
        };
        let _lease = lease;
        let _active = ActiveTask {
            manager: self.clone(),
            task_id: task.task_id.clone(),
        };
        let result: Result<()> = async {
            if let Some(installed) = installer.committed(candidate_id)? {
                self.tasks.finish(&task.task_id, Ok(json!(installed)))?;
                return Ok(());
            }
            let candidate = installer.prepared(candidate_id)?;
            let target = installer.target(candidate_id)?;
            let control = InstallControl::new(stop, cancel);
            node_apps
                .with_install_commit(&target, &candidate.manifest, |fresh| {
                    installer.commit_stopped(
                        candidate_id,
                        fresh,
                        &task.task_id,
                        &control,
                        &self.tasks,
                    )
                })
                .await?;
            Ok(())
        }
        .await;
        if let Err(error) = result {
            self.finish_error(&task.task_id, Some(candidate_id), error);
        }
        self.tasks.task(&task.task_id)
    }
    fn finish_error(&self, task_id: &str, candidate: Option<&str>, error: ApiError) {
        if self
            .tasks
            .task(task_id)
            .is_ok_and(|task| !task.status.is_final())
        {
            let outcome = error.details["operationOutcome"]
                .as_str()
                .unwrap_or("not-committed");
            let _ = self.tasks.finish_partial(task_id, json!({"candidateId":candidate,"operationOutcome":outcome,"partialFilesRetained":true}), error);
        }
    }
    pub fn cancel(&self, task_id: &str) -> Result<TaskRecord> {
        let task = self.tasks.task(task_id)?;
        if task.task_type != "nodeInstallPrepare" {
            return Err(ApiError::new(
                "NODE_INSTALL_TASK_INVALID",
                "仅安装准备任务支持取消；提交开始后以实际登记结果为准。",
            ));
        }
        if let Some(cancel) = self.active.lock().unwrap().get(task_id) {
            let _ = cancel.send(true);
        }
        Ok(task)
    }
    pub async fn shutdown(&self) {
        {
            let _submission = self.submission.lock().unwrap();
            self.stopping.store(true, Ordering::Release);
            for cancel in self.active.lock().unwrap().values() {
                let _ = cancel.send(true);
            }
        }
        let runners = std::mem::take(&mut *self.runners.lock().unwrap());
        for runner in runners {
            let _ = runner.await;
        }
        loop {
            let drained = self.drained.notified();
            if self.active.lock().unwrap().is_empty() {
                break;
            }
            drained.await;
        }
    }
}
struct ActiveTask {
    manager: Arc<NodeInstallTasks>,
    task_id: String,
}
impl Drop for ActiveTask {
    fn drop(&mut self) {
        self.manager.active.lock().unwrap().remove(&self.task_id);
        self.manager.drained.notify_waiters();
    }
}
fn interrupted() -> ApiError {
    ApiError::new(
        "NODE_INSTALL_INTERRUPTED",
        "安装任务已中断，现场保留；提交状态以持久记录为准。",
    )
}

fn require_node_source(source: &crate::source_acquisition::SourceCandidate) -> Result<()> {
    if source
        .inspection
        .as_ref()
        .map(|inspection| inspection.kind.as_str())
        != Some("node-service-source")
    {
        return Err(ApiError::new(
            "NODE_INSTALL_SOURCE_NEEDS_ADAPTATION",
            "此来源尚未确认为可准备的单 Node 服务应用，请先核对并完成源码适配；未安装或启动程序。",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "node_install_tasks_tests.rs"]
mod tests;
