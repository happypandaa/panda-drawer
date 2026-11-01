//! One owned transaction for Git acquisition, in-place dependency preparation,
//! and direct-run registration. A lost reply never creates a second instance.
use crate::{
    builds::{atomic_json, BuildManager},
    file_identity::PersistedIdentity,
    node_apps::NodeApps,
    node_dependency_prepare::{dependency_root, prepare_dependencies, DependencyControl},
    node_process_scope::NodeProcessScope,
    node_project_ownership::NodeProjectOwnership,
    panda_manifest::{PandaManifest, PandaRuntime},
    projects::{check_no_symlinks, ProjectTaskLease},
    source_acquisition::{AcquireSourceInput, SourceAcquisitionManager},
    ApiError, ProjectStore, RuntimePaths, TaskRecord,
};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::{sync::watch, task::JoinHandle};
use uuid::Uuid;
#[path = "node_git_import/capability.rs"]
mod capability;

type Result<T> = std::result::Result<T, ApiError>;

/// Internal capability, never accepted from IPC or HTTP.
#[derive(Clone, Debug)]
pub(crate) struct VerifiedPublishedSource {
    pub(crate) source_root: PathBuf,
    pub(crate) source_identity: PersistedIdentity,
    pub(crate) manifest_digest: String,
    pub(crate) source_url: String,
    pub(crate) source_branch: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartNodeGitImportInput {
    pub import_id: String,
    pub url: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommitState {
    NotCommitted,
    Committed,
    Unknown,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSnapshot {
    pub task: TaskRecord,
    pub app_id: String,
    pub source_path: Option<PathBuf>,
    pub commit_state: CommitState,
    pub cleanup_pending: bool,
    pub remaining_paths: Vec<PathBuf>,
    pub app: Option<Value>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    version: u32,
    input: StartNodeGitImportInput,
    app_id: String,
    task_id: String,
    source_id: String,
    workspace: PathBuf,
    workspace_identity: PersistedIdentity,
    owner_identity: PersistedIdentity,
    phase: String,
    registration_attempted: bool,
    commit_state: CommitState,
    source_path: Option<PathBuf>,
    source_identity: Option<PersistedIdentity>,
    manifest_digest: Option<String>,
    // Exact identities are persisted before a directory can become disposable.
    temporary: BTreeMap<PathBuf, PersistedIdentity>,
    cleanup_pending: bool,
    remaining_paths: Vec<PathBuf>,
    #[serde(default)]
    app: Option<Value>,
    #[serde(default)]
    final_task: Option<TaskRecord>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Admission {
    version: u32,
    input: StartNodeGitImportInput,
    app_id: String,
    workspace: PathBuf,
    workspace_identity: PersistedIdentity,
    owner_identity: PersistedIdentity,
}
struct ImportScope {
    _lease: ProjectTaskLease,
    _operation: crate::node_apps::NodeOperationPermit,
}
pub struct NodeGitImportManager {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    sources: Result<Arc<SourceAcquisitionManager>>,
    records: Mutex<BTreeMap<String, Journal>>,
    blocked: Mutex<BTreeMap<String, ApiError>>,
    unaccepted: Mutex<BTreeMap<String, PersistedIdentity>>,
    active: Mutex<BTreeMap<String, watch::Sender<bool>>>,
    submission: Mutex<()>,
    runners: Mutex<Vec<JoinHandle<()>>>,
    stopping: AtomicBool,
}
fn capture(path: &Path) -> Result<PersistedIdentity> {
    PersistedIdentity::capture(path).map_err(|issue| invalid(&issue.to_string()))
}
fn invalid(message: &str) -> ApiError {
    ApiError::new("NODE_GIT_IMPORT_INVALID", message)
}
fn cancelled() -> ApiError {
    ApiError::new("NODE_GIT_IMPORT_CANCELLED", "添加已取消。")
}
fn check(stop: &watch::Receiver<bool>, cancel: &watch::Receiver<bool>) -> Result<()> {
    if *stop.borrow() || stop.has_changed().is_err() {
        return Err(ApiError::new("SERVICE_STOPPING", "平台正在退出。"));
    }
    if *cancel.borrow() {
        return Err(cancelled());
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<()> {
    crate::import_directory_io::private_dir(path)
}
impl NodeGitImportManager {
    pub fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        sources: Result<Arc<SourceAcquisitionManager>>,
    ) -> Result<Arc<Self>> {
        let manager = Arc::new(Self {
            paths,
            store,
            tasks,
            sources,
            records: Mutex::new(BTreeMap::new()),
            blocked: Mutex::new(BTreeMap::new()),
            unaccepted: Mutex::new(BTreeMap::new()),
            active: Mutex::new(BTreeMap::new()),
            submission: Mutex::new(()),
            runners: Mutex::new(vec![]),
            stopping: AtomicBool::new(false),
        });
        let root = manager.paths.data_root().join("node-project-ownership");
        check_no_symlinks(&root, true)?;
        if root.is_dir() {
            for entry in fs::read_dir(root)? {
                let entry = entry?;
                let id = entry.file_name().to_string_lossy().into_owned();
                if Uuid::parse_str(&id).is_err() {
                    continue;
                }
                let path = entry.path().join("import.json");
                if let Err(issue) = check_no_symlinks(&path, true) {
                    manager.block(&id, issue);
                    continue;
                }
                if !path.exists() {
                    let admission_path = entry.path().join("import-request.json");
                    if admission_path.symlink_metadata().is_ok() {
                        let admission = (|| -> Result<Admission> {
                            check_no_symlinks(&admission_path, false)?;
                            if fs::metadata(&admission_path)?.len() > 65536 {
                                return Err(invalid("添加接纳记录过大。"));
                            }
                            let admission: Admission =
                                serde_json::from_slice(&fs::read(&admission_path)?)?;
                            if admission.version != 1
                                || admission.app_id != id
                                || admission.workspace != manager.paths.workspace_root()
                                || !admission
                                    .owner_identity
                                    .matches_path(&entry.path())
                                    .unwrap_or(false)
                                || !admission
                                    .workspace_identity
                                    .matches_path(manager.paths.workspace_root())
                                    .unwrap_or(false)
                            {
                                return Err(invalid("添加接纳记录的目录身份无法核对。"));
                            }
                            Ok(admission)
                        })();
                        match admission {
                            Ok(admission) => {
                                manager
                                    .unaccepted
                                    .lock()
                                    .unwrap()
                                    .insert(id.clone(), admission.owner_identity);
                            }
                            Err(issue) => manager.block(&id, issue),
                        }
                        continue;
                    }
                    // Completed imports have no recovery journal. Existing owned
                    // task metadata supplies restart-safe request deduplication.
                    let ownership = manager.ownership();
                    let Ok(task_ids) = ownership.task_ids(&id) else {
                        continue;
                    };
                    for task_id in task_ids {
                        let Ok(task) = manager.tasks.task(&task_id) else {
                            continue;
                        };
                        if task.task_type != "nodeGitImport" {
                            continue;
                        }
                        if !task.status.is_final()
                            || task
                                .result
                                .as_ref()
                                .is_none_or(|value| value["commitState"] != "committed")
                        {
                            match capture(&entry.path()) {
                                Ok(identity) => {
                                    manager
                                        .unaccepted
                                        .lock()
                                        .unwrap()
                                        .insert(id.clone(), identity);
                                }
                                Err(issue) => manager.block(&id, issue),
                            }
                            continue;
                        }
                        let Some(result) = task.result.as_ref() else {
                            continue;
                        };
                        if result["commitState"] != "committed" || result["appId"] != id {
                            continue;
                        }
                        let Some(input) = result.get("input").and_then(|value| {
                            serde_json::from_value::<StartNodeGitImportInput>(value.clone()).ok()
                        }) else {
                            continue;
                        };
                        // A terminal request has no filesystem work left. Read
                        // only its platform registration to follow migration;
                        // never scan or adopt a replacement author directory.
                        let slot = manager.paths.data_root().join("node-apps").join(&id);
                        let registration = slot.join("registration.json");
                        let source_path = check_no_symlinks(&registration, false)
                            .ok()
                            .and_then(|()| fs::metadata(&registration).ok())
                            .filter(|metadata| metadata.is_file() && metadata.len() <= 1024 * 1024)
                            .and_then(|_| fs::read(&registration).ok())
                            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                            .and_then(|value| {
                                crate::node_registration::decode_registration(
                                    value,
                                    &id,
                                    &manager.paths.data_root().join("node-apps"),
                                )
                                .ok()
                            })
                            .map(|registration| match registration {
                                crate::node_registration::StoredNodeRegistration::Direct(
                                    record,
                                ) => record.source_path,
                            })
                            .or_else(|| result["sourcePath"].as_str().map(PathBuf::from));
                        let j = Journal {
                            version: 1,
                            input,
                            app_id: id.clone(),
                            task_id: task_id.clone(),
                            source_id: format!("src_{}", Uuid::nil().simple()),
                            workspace: manager.paths.workspace_root().into(),
                            workspace_identity: capture(manager.paths.workspace_root())?,
                            owner_identity: capture(&entry.path())?,
                            phase: "completed".into(),
                            registration_attempted: true,
                            commit_state: CommitState::Committed,
                            source_path,
                            source_identity: None,
                            manifest_digest: None,
                            temporary: BTreeMap::new(),
                            cleanup_pending: false,
                            remaining_paths: vec![],
                            app: result.get("app").filter(|v| !v.is_null()).cloned(),
                            final_task: Some(task),
                        };
                        manager
                            .records
                            .lock()
                            .unwrap()
                            .insert(j.input.import_id.clone(), j);
                    }
                    continue;
                }
                let loaded = (|| -> Result<Journal> {
                    if fs::metadata(&path)?.len() > 1024 * 1024 {
                        return Err(invalid("添加恢复记录过大。"));
                    }
                    let j: Journal = serde_json::from_slice(&fs::read(&path)?)?;
                    manager.validate(&j)?;
                    Ok(j)
                })();
                match loaded {
                    Ok(j) => {
                        manager
                            .records
                            .lock()
                            .unwrap()
                            .insert(j.input.import_id.clone(), j);
                    }
                    Err(issue) => manager.block(&id, issue),
                }
            }
        }
        Ok(manager)
    }
    fn ownership(&self) -> NodeProjectOwnership {
        NodeProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        )
    }
    fn block(&self, app_id: &str, issue: ApiError) {
        self.blocked.lock().unwrap().insert(app_id.into(), issue);
    }
    pub fn issues(&self) -> Vec<Value> {
        self.blocked
            .lock()
            .unwrap()
            .iter()
            .map(|(app_id, error)| json!({"appId":app_id,"error":error}))
            .collect()
    }
    fn journal_path(&self, j: &Journal) -> Result<PathBuf> {
        Ok(self.ownership().directory(&j.app_id)?.join("import.json"))
    }
    fn validate(&self, j: &Journal) -> Result<()> {
        if j.version != 1
            || Uuid::parse_str(&j.app_id).is_err()
            || Uuid::parse_str(&j.input.import_id).is_err()
            || !j.task_id.starts_with("task_")
            || !crate::projects::valid_id(&j.task_id)
            || !j.source_id.starts_with("src_")
            || !crate::projects::valid_id(&j.source_id)
            || j.workspace != self.paths.workspace_root()
            || !j
                .workspace_identity
                .matches_path(&j.workspace)
                .unwrap_or(false)
            || !j
                .owner_identity
                .matches_path(&self.ownership().directory(&j.app_id)?)
                .unwrap_or(false)
        {
            return Err(invalid("导入记录的归属或目录身份无法核对。"));
        }
        if let Some(source) = &j.source_path {
            let root = self.paths.workspace_root().join("project-sources");
            if source.parent() != Some(root.as_path())
                || source.file_name().is_none()
                || j.source_identity.is_none()
            {
                return Err(invalid("项目发布路径无法核对。"));
            }
        }
        for p in j.temporary.keys() {
            if p != &self
                .paths
                .workspace_root()
                .join(".platform/sources")
                .join(&j.source_id)
                && p != &self
                    .paths
                    .workspace_root()
                    .join(".platform/source-publications")
                    .join(&j.app_id)
                && p != &self
                    .paths
                    .data_root()
                    .join("onboarding/imports")
                    .join(&j.task_id)
            {
                return Err(invalid("临时材料路径不属于本次导入。"));
            }
        }
        Ok(())
    }
    fn save(&self, j: &Journal) -> Result<()> {
        self.validate(j)?;
        check_no_symlinks(&self.journal_path(j)?, true)?;
        atomic_json(&self.journal_path(j)?, j)?;
        self.records
            .lock()
            .unwrap()
            .insert(j.input.import_id.clone(), j.clone());
        Ok(())
    }
    fn phase(&self, j: &mut Journal, phase: &str) -> Result<()> {
        j.phase = phase.into();
        self.save(j)?;
        self.tasks.phase(&j.task_id, phase)
    }
    fn task(&self, j: &Journal) -> Result<TaskRecord> {
        // Project deletion purges the live task registry. Cached final output
        // cannot establish that the project still exists after that purge.
        let live = self.tasks.task(&j.task_id)?;
        Ok(j.final_task.clone().unwrap_or(live))
    }
    fn snapshot(&self, j: &Journal) -> Result<ImportSnapshot> {
        Ok(ImportSnapshot {
            task: self.task(j)?,
            app_id: j.app_id.clone(),
            source_path: j.source_path.clone(),
            commit_state: j.commit_state.clone(),
            cleanup_pending: j.cleanup_pending,
            remaining_paths: j.remaining_paths.clone(),
            app: j.app.clone(),
        })
    }
    pub fn status(&self, import_id: &str) -> Result<ImportSnapshot> {
        let j = self
            .records
            .lock()
            .unwrap()
            .get(import_id)
            .cloned()
            .ok_or_else(|| ApiError::new("NODE_GIT_IMPORT_NOT_FOUND", "尚未找到这次添加任务。"))?;
        self.snapshot(&j)
    }
    pub fn require_idle(&self) -> Result<()> {
        if self.active.lock().unwrap().is_empty() {
            Ok(())
        } else {
            Err(ApiError::new(
                "NODE_GIT_IMPORT_BUSY",
                "添加任务仍在执行或收尾。",
            ))
        }
    }
    pub async fn start(
        self: &Arc<Self>,
        apps: Arc<NodeApps>,
        input: StartNodeGitImportInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord> {
        self.start_inner(apps, input, stop)
            .map_err(|issue| issue.outcome("not_started").detail("accepted", false))
    }
    fn start_inner(
        self: &Arc<Self>,
        apps: Arc<NodeApps>,
        mut input: StartNodeGitImportInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord> {
        let _submission = self.submission.lock().unwrap();
        input.url = crate::source_acquisition::validate_url(&input.url)?.0;
        if let Some(branch) = &input.branch {
            crate::source_acquisition::validate_branch(branch)?;
        }
        if let Some(j) = self.records.lock().unwrap().get(&input.import_id).cloned() {
            if j.input.url != input.url
                || j.input.branch != input.branch
                || j.input.name != input.name
            {
                return Err(invalid("同一次添加请求的内容发生变化。"));
            }
            return self.task(&j);
        }
        if Uuid::parse_str(&input.import_id).is_err() {
            return Err(invalid("添加请求标识无效。"));
        }
        if input.name.as_ref().is_some_and(|n| {
            n.trim().is_empty() || n.chars().count() > 200 || n.chars().any(char::is_control)
        }) {
            return Err(invalid("项目名称无效。"));
        }
        capability::require_start(&self.sources)?;
        self.store.check_workspace_available()?;
        self.require_idle()?;
        if self.stopping.load(Ordering::Acquire) || *stop.borrow() {
            return Err(ApiError::new("SERVICE_STOPPING", "平台正在退出。"));
        }
        // Hold migration admission before creating even the task's metadata.
        // Once accepted, the per-instance scope takes over without a gap.
        let _admission = self.store.reserve_transient_operation(
            &format!("node-import-request:{}", input.import_id),
            "node-git-import",
        )?;
        apps.set_import_task_manager(self.tasks.clone())?;
        let app_id = Uuid::new_v4().to_string();
        let ownership = self.ownership();
        ownership
            .initialize(&app_id)
            .map_err(|issue| self.rollback_acceptance(&app_id, issue))?;
        let prepared = (|| -> Result<_> {
            let request_path = ownership.directory(&app_id)?.join("import-request.json");
            atomic_json(
                &request_path,
                &Admission {
                    version: 1,
                    input: input.clone(),
                    app_id: app_id.clone(),
                    workspace: self.paths.workspace_root().into(),
                    workspace_identity: capture(self.paths.workspace_root())?,
                    owner_identity: capture(&ownership.directory(&app_id)?)?,
                },
            )?;
            let task = self.tasks.create_node_operation(&app_id, "nodeGitImport")?;
            let scope = Arc::new(ImportScope {
                _lease: self
                    .store
                    .reserve_task(&format!("node-import:{app_id}"), &task.task_id)?,
                _operation: apps.admit_operation(&app_id, "从 Git 添加")?,
            });
            let j = Journal {
                version: 1,
                input,
                app_id: app_id.clone(),
                task_id: task.task_id.clone(),
                source_id: format!("src_{}", Uuid::new_v4().simple()),
                workspace: self.paths.workspace_root().into(),
                workspace_identity: capture(self.paths.workspace_root())?,
                owner_identity: capture(&ownership.directory(&app_id)?)?,
                phase: "queued".into(),
                registration_attempted: false,
                commit_state: CommitState::NotCommitted,
                source_path: None,
                source_identity: None,
                manifest_digest: None,
                temporary: BTreeMap::new(),
                cleanup_pending: false,
                remaining_paths: vec![],
                app: None,
                final_task: None,
            };
            self.save(&j)?;
            fs::remove_file(request_path)?;
            platform_native::filesystem::sync_directory(&ownership.directory(&app_id)?)?;
            Ok((task, scope, j))
        })();
        let (task, scope, mut j) = match prepared {
            Ok(prepared) => prepared,
            Err(issue) => return Err(self.rollback_acceptance(&app_id, issue)),
        };
        let (sender, cancel) = watch::channel(false);
        self.active
            .lock()
            .unwrap()
            .insert(task.task_id.clone(), sender);
        let manager = self.clone();
        let task_id = task.task_id.clone();
        let runner = tokio::spawn(async move {
            let work = AssertUnwindSafe(manager.run(&apps, &mut j, stop, cancel, scope.clone()))
                .catch_unwind()
                .await;
            let result = work.unwrap_or_else(|_| {
                Err(ApiError::new(
                    "NODE_GIT_IMPORT_INTERRUPTED",
                    "添加任务中断，正在核对登记与清理状态。",
                ))
            });
            let result = manager.finish_operation(&apps, &mut j, result).await;
            if let Err(issue) = result {
                let _=manager.tasks.finish_partial(&j.task_id,json!({"appId":j.app_id,"commitState":j.commit_state,"cleanupPending":true,"remainingPaths":j.remaining_paths}),issue);
            }
            manager.active.lock().unwrap().remove(&task_id);
        });
        self.runners.lock().unwrap().push(runner);
        Ok(task)
    }
    fn rollback_acceptance(&self, owner: &str, issue: ApiError) -> ApiError {
        let cleanup = (|| -> Result<()> {
            let ownership = self.ownership();
            let directory = ownership.directory(owner)?;
            check_no_symlinks(&directory, true)?;
            if !directory.try_exists()? {
                return Ok(());
            }
            if self
                .paths
                .data_root()
                .join("node-apps")
                .join(owner)
                .try_exists()?
            {
                return Err(invalid("实例目录已经存在，接纳结果需要核对，未清理。"));
            }
            if fs::read_dir(&directory)?.next().is_none() {
                // Initialization can fail after mkdir but before writing the
                // ledger. Empty removal never consumes unrecorded content.
                fs::remove_dir(&directory)?;
                return Ok(());
            }
            let tasks = ownership.task_ids(owner)?;
            let identity = capture(&directory)?;
            // No runner or tool probe has started. Only the fresh task and its
            // explicitly created owner can exist at this point.
            for path in ownership.owned_paths(owner)? {
                if path != directory
                    && !tasks.iter().any(|task| {
                        path == self
                            .paths
                            .workspace_root()
                            .join(".platform/tasks")
                            .join(task)
                    })
                {
                    return Err(invalid("未接纳的任务出现未知材料，未猜测归属。"));
                }
            }
            for task in &tasks {
                if self
                    .tasks
                    .task(task)
                    .is_ok_and(|task| !task.status.is_final())
                {
                    self.tasks.finish(task, Err(issue.clone()))?;
                }
            }
            self.tasks.forget_owned_tasks(&tasks)?;
            crate::history_cleanup::remove_owned_directory(&directory, &identity)?;
            Ok(())
        })();
        match cleanup {
            Ok(()) => {
                self.records
                    .lock()
                    .unwrap()
                    .retain(|_, j| j.app_id != owner);
                issue
            }
            Err(cleanup) => issue
                .detail("appId", owner)
                .detail("acceptanceCleanupError", json!(cleanup))
                .detail("cleanupPending", true)
                .detail(
                    "remainingPaths",
                    json!([self.ownership().directory(owner).ok()]),
                ),
        }
    }
    async fn run(
        &self,
        apps: &Arc<NodeApps>,
        j: &mut Journal,
        stop: watch::Receiver<bool>,
        cancel: watch::Receiver<bool>,
        scope: Arc<ImportScope>,
    ) -> Result<()> {
        check(&stop, &cancel)?;
        self.phase(j, "downloading")?;
        let sources = self.sources.as_ref().map_err(Clone::clone)?.clone();
        let source = sources
            .acquire_for_import(
                AcquireSourceInput {
                    url: j.input.url.clone(),
                    branch: j.input.branch.clone(),
                    name: j.input.name.clone(),
                },
                &j.app_id,
                &j.task_id,
                &j.source_id,
                stop.clone(),
                cancel.clone(),
            )
            .await?;
        self.capture_temporary(j)?;
        check(&stop, &cancel)?;
        self.phase(j, "checking")?;
        if source
            .inspection
            .as_ref()
            .is_none_or(|s| s.kind != "node-service-source")
        {
            return Err(ApiError::new(
                "SOURCE_NEEDS_ADAPTATION",
                "运行文件或应用说明不完整，请让 AI 构建或修改后添加本地项目。",
            ));
        }
        let lease = self
            .store
            .reserve_task(&format!("source:{}", j.source_id), &j.task_id)?;
        let app_id = j.app_id.clone();
        let task_id = j.task_id.clone();
        let copy_sources = sources.clone();
        self.phase(j, "publishing")?;
        let publication_scope = scope.clone();
        let root = tokio::task::spawn_blocking(move || {
            let _scope = (publication_scope, lease);
            copy_sources.publish_author_source(&source, &app_id, &task_id)
        })
        .await
        .map_err(|_| ApiError::new("NODE_GIT_IMPORT_INTERRUPTED", "项目发布中断。"))??;
        j.source_identity = Some(capture(&root)?);
        j.source_path = Some(root.clone());
        self.capture_temporary(j)?;
        self.save(j)?;
        check(&stop, &cancel)?;
        let bytes = read_manifest(&root)?;
        let manifest = PandaManifest::parse(&bytes).map_err(|e| invalid(&e.to_string()))?;
        if manifest.runtime != PandaRuntime::Node {
            return Err(invalid("仅支持 Node 应用。"));
        }
        manifest.inspect(&root).map_err(|_| {
            ApiError::new(
                "SOURCE_NEEDS_ADAPTATION",
                "仓库没有提交声明的运行文件，请让 AI 构建后添加本地项目。",
            )
        })?;
        j.manifest_digest = Some(format!("{:x}", Sha256::digest(&bytes)));
        self.save(j)?;
        self.phase(j, "dependencies")?;
        let operation = self
            .paths
            .data_root()
            .join("onboarding/imports")
            .join(&j.task_id);
        self.ownership().record_onboarding(&j.app_id, &j.task_id)?;
        private_dir(&operation)?;
        self.ownership().record_onboarding(&j.app_id, &j.task_id)?;
        j.temporary.insert(operation.clone(), capture(&operation)?);
        self.save(j)?;
        let npm = operation.join("npm");
        private_dir(&npm)?;
        let dependency_scope = Arc::new(ImportScope {
            _lease: self.store.reserve_task(
                &format!("node-import-dependencies:{}", j.app_id),
                &j.task_id,
            )?,
            _operation: apps.admit_operation(&j.app_id, "准备项目依赖")?,
        });
        let processes = NodeProcessScope::new(
            self.store
                .processes()
                .with_writer_owner(crate::durable_writer::WriterOwner::node(
                    self.paths.data_root(),
                    &j.app_id,
                )?),
            dependency_scope,
        )
        .with_operation(&npm)?;
        if dependency_root(&root, &manifest)?.is_some() {
            let tools = self
                .tasks
                .environment()
                .with_process_registry(processes.processes.clone())
                .resolve_with_keepalive(processes.owner.clone())
                .await
                .map_err(NodeProcessScope::resolution_error)?;
            prepare_dependencies(
                &root,
                &manifest,
                &npm,
                &tools,
                &DependencyControl::new(stop.clone(), cancel.clone()),
                processes,
            )
            .await?;
        }
        check(&stop, &cancel)?;
        let token = self.token(j)?;
        j.registration_attempted = true;
        j.commit_state = CommitState::Unknown;
        self.phase(j, "registering")?;
        j.app = Some(apps.register_published_source(&j.app_id, token).await?);
        j.commit_state = CommitState::Committed;
        self.save(j)?;
        Ok(())
    }
    fn token(&self, j: &Journal) -> Result<VerifiedPublishedSource> {
        Ok(VerifiedPublishedSource {
            source_root: j
                .source_path
                .clone()
                .ok_or_else(|| invalid("项目目录缺失。"))?,
            source_identity: j
                .source_identity
                .clone()
                .ok_or_else(|| invalid("项目目录身份缺失。"))?,
            manifest_digest: j
                .manifest_digest
                .clone()
                .ok_or_else(|| invalid("应用说明摘要缺失。"))?,
            source_url: j.input.url.clone(),
            source_branch: j.input.branch.clone(),
        })
    }
    fn capture_temporary(&self, j: &mut Journal) -> Result<()> {
        // The ownership ledger was written before creating these locations.
        let paths = self.ownership().owned_paths(&j.app_id)?;
        for p in [
            self.paths
                .workspace_root()
                .join(".platform/sources")
                .join(&j.source_id),
            self.paths
                .workspace_root()
                .join(".platform/source-publications")
                .join(&j.app_id),
        ] {
            if paths.contains(&p) && p.try_exists()? {
                j.temporary.entry(p.clone()).or_insert(capture(&p)?);
            }
        }
        self.save(j)
    }
    async fn reconcile(&self, apps: &NodeApps, j: &mut Journal) -> Result<()> {
        capability::reconcile_source_publication(self, j)?;
        if !j.registration_attempted {
            j.commit_state = CommitState::NotCommitted;
            return Ok(());
        }
        match apps
            .reconcile_published_registration(&j.app_id, &self.token(j)?)
            .await
        {
            Ok(Some(app)) => {
                j.commit_state = CommitState::Committed;
                j.app = Some(app);
            }
            Ok(None) => {
                j.commit_state = CommitState::NotCommitted;
                j.app = None;
            }
            Err(issue) => {
                j.commit_state = CommitState::Unknown;
                self.save(j)?;
                return Err(issue);
            }
        }
        self.save(j)
    }
    async fn finish_operation(
        &self,
        apps: &NodeApps,
        j: &mut Journal,
        result: Result<()>,
    ) -> Result<()> {
        let original = result.err();
        if let Err(issue) = self.reconcile(apps, j).await {
            j.cleanup_pending = true;
            j.remaining_paths = self.pending_paths(j);
            self.save(j)?;
            return Err(issue);
        }
        let cleanup = self.cleanup(j);
        if cleanup.is_err() {
            j.cleanup_pending = true;
            if j.remaining_paths.is_empty() {
                j.remaining_paths = self.pending_paths(j);
            }
            let _ = self.save(j);
        }
        let summary = json!({"input":j.input,"importId":j.input.import_id,"appId":j.app_id,"sourcePath":j.source_path,"commitState":j.commit_state,"cleanupPending":j.cleanup_pending,"remainingPaths":j.remaining_paths,"app":j.app});
        let issue = cleanup.err().or(original);
        let finished = if let Some(issue) = issue {
            self.tasks.finish_partial(&j.task_id, summary, issue)
        } else {
            self.tasks.finish(&j.task_id, Ok(summary))
        };
        finished?;
        j.final_task = Some(self.tasks.task(&j.task_id)?);
        if self.ownership().directory(&j.app_id)?.is_dir() {
            if j.cleanup_pending {
                self.save(j)?;
            } else {
                let path = self.journal_path(j)?;
                check_no_symlinks(&path, true)?;
                if path.exists() {
                    fs::remove_file(path)?;
                    platform_native::filesystem::sync_directory(
                        &self.ownership().directory(&j.app_id)?,
                    )?;
                }
            }
        }
        self.records
            .lock()
            .unwrap()
            .insert(j.input.import_id.clone(), j.clone());
        Ok(())
    }
    fn pending_paths(&self, j: &Journal) -> Vec<PathBuf> {
        let mut paths = vec![
            self.paths
                .workspace_root()
                .join(".platform/sources")
                .join(&j.source_id),
            self.paths
                .workspace_root()
                .join(".platform/source-publications")
                .join(&j.app_id),
            self.paths
                .data_root()
                .join("onboarding/imports")
                .join(&j.task_id),
        ];
        if j.commit_state != CommitState::Committed {
            paths.extend(j.source_path.clone());
            if j.registration_attempted {
                paths.push(self.paths.data_root().join("node-apps").join(&j.app_id));
            }
        }
        capability::retain_pending_paths(j, &mut paths);
        paths.retain(|path| !matches!(fs::symlink_metadata(path), Err(issue) if issue.kind() == std::io::ErrorKind::NotFound));
        paths.sort();
        paths.dedup();
        paths
    }
    fn cleanup(&self, j: &mut Journal) -> Result<()> {
        self.validate(j)?;
        crate::durable_writer::require_owner_clear(&crate::durable_writer::WriterOwner::node(
            self.paths.data_root(),
            &j.app_id,
        )?)?;
        self.capture_temporary(j)?;
        let ownership = self.ownership();
        let paths = ownership.owned_paths(&j.app_id)?;
        let mut targets = j.temporary.clone();
        if j.commit_state == CommitState::NotCommitted {
            for path in paths {
                if path == ownership.directory(&j.app_id)?
                    || path
                        == self
                            .paths
                            .workspace_root()
                            .join(".platform/tasks")
                            .join(&j.task_id)
                {
                    continue;
                }
                if path.try_exists()? {
                    targets.entry(path.clone()).or_insert(capture(&path)?);
                }
            }
            if let (Some(path), Some(identity)) = (&j.source_path, &j.source_identity) {
                targets.insert(path.clone(), identity.clone());
            }
        }
        if j.commit_state == CommitState::Unknown {
            return Err(invalid("登记结果待核对，暂不清理项目。"));
        }
        j.cleanup_pending = true;
        j.remaining_paths = targets.keys().cloned().collect();
        self.save(j)?;
        for (path, identity) in targets {
            if !path.try_exists()? {
                j.remaining_paths.retain(|p| p != &path);
                continue;
            }
            if let Err(issue) = crate::history_cleanup::remove_owned_directory(&path, &identity) {
                self.save(j)?;
                return Err(issue.detail("remainingPath", json!(path)));
            }
            j.remaining_paths.retain(|p| p != &path);
            self.save(j)?;
        }
        j.temporary.clear();
        j.cleanup_pending = false;
        j.remaining_paths.clear();
        self.save(j)?;
        if j.commit_state == CommitState::NotCommitted {
            // Keep the task's terminal status until the UI receives it; it holds
            // no source bytes. The project owner is the last disposable record.
            let owner = ownership.directory(&j.app_id)?;
            crate::history_cleanup::remove_owned_directory(&owner, &j.owner_identity)?;
        }
        Ok(())
    }
    pub fn cancel(&self, task_id: &str) -> Result<TaskRecord> {
        let task = self.tasks.task(task_id)?;
        if task.task_type != "nodeGitImport"
            || !self
                .records
                .lock()
                .unwrap()
                .values()
                .any(|j| j.task_id == task_id)
        {
            return Err(invalid("这不是本次 Git 添加任务。"));
        }
        if let Some(sender) = self.active.lock().unwrap().get(task_id) {
            let _ = sender.send(true);
        }
        Ok(task)
    }
    pub async fn retry_cleanup(
        &self,
        apps: Arc<NodeApps>,
        task_id: &str,
    ) -> Result<ImportSnapshot> {
        self.require_idle()?;
        let mut j = self
            .records
            .lock()
            .unwrap()
            .values()
            .find(|j| j.task_id == task_id)
            .cloned()
            .ok_or_else(|| invalid("添加任务不存在。"))?;
        // A complete rollback has no owner to mutate and is already terminal.
        if !j.cleanup_pending && j.final_task.is_some() && j.commit_state != CommitState::Unknown {
            return self.snapshot(&j);
        }
        capability::require_recovery()?;
        let _scope = ImportScope {
            _lease: self
                .store
                .reserve_transient_operation(&format!("node-import:{}", j.app_id), &j.task_id)?,
            _operation: apps.admit_operation(&j.app_id, "重试添加收尾")?,
        };
        let result = if j.commit_state == CommitState::NotCommitted {
            Err(self.task(&j)?.error.unwrap_or_else(cancelled))
        } else {
            Ok(())
        };
        self.finish_operation(&apps, &mut j, result).await?;
        self.snapshot(&j)
    }
    pub fn require_migration_clear(&self) -> Result<()> {
        self.require_idle()?;
        if !self.blocked.lock().unwrap().is_empty() || !self.unaccepted.lock().unwrap().is_empty() {
            return Err(ApiError::new(
                "NODE_GIT_IMPORT_RECOVERY_REQUIRED",
                "有添加记录需要核对，其他项目仍可使用，请先处理后再迁移。",
            )
            .detail("issues", json!(self.issues())));
        }
        if self.records.lock().unwrap().values().any(|j| {
            j.cleanup_pending || j.commit_state == CommitState::Unknown || j.final_task.is_none()
        }) {
            return Err(ApiError::new(
                "NODE_GIT_IMPORT_CLEANUP_PENDING",
                "添加任务还有待核对或待清理材料，请先重试清理。",
            ));
        }
        Ok(())
    }
    pub async fn recover(&self, apps: Arc<NodeApps>) -> Result<Vec<ImportSnapshot>> {
        capability::require_recovery()?;
        self.require_idle()?;
        let unaccepted = self.unaccepted.lock().unwrap().clone();
        for (app_id, identity) in unaccepted {
            let cleaned = (|| -> Result<()> {
                let _scope = ImportScope {
                    _lease: self.store.reserve_transient_operation(
                        &format!("node-import:{app_id}"),
                        "interrupted-admission",
                    )?,
                    _operation: apps.admit_operation(&app_id, "清理未接纳的添加")?,
                };
                if !identity
                    .matches_path(&self.ownership().directory(&app_id)?)
                    .unwrap_or(false)
                {
                    return Err(invalid("未接纳任务的目录身份已变化。"));
                }
                crate::durable_writer::require_owner_clear(
                    &crate::durable_writer::WriterOwner::node(self.paths.data_root(), &app_id)?,
                )?;
                let issue = self.rollback_acceptance(
                    &app_id,
                    ApiError::new("NODE_GIT_IMPORT_INTERRUPTED", "添加在接纳完成前中断。"),
                );
                if issue.details.get("acceptanceCleanupError").is_some() {
                    return Err(issue);
                }
                Ok(())
            })();
            match cleaned {
                Ok(()) => {
                    self.unaccepted.lock().unwrap().remove(&app_id);
                    self.blocked.lock().unwrap().remove(&app_id);
                }
                Err(issue) => self.block(&app_id, issue),
            }
        }
        let records: Vec<_> = self.records.lock().unwrap().values().cloned().collect();
        let mut result = vec![];
        for mut j in records {
            if j.final_task.is_some() && !j.cleanup_pending {
                continue;
            }
            let _scope = ImportScope {
                _lease: self.store.reserve_transient_operation(
                    &format!("node-import:{}", j.app_id),
                    &j.task_id,
                )?,
                _operation: apps.admit_operation(&j.app_id, "核对中断的添加")?,
            };
            let interrupted = ApiError::new(
                "NODE_GIT_IMPORT_INTERRUPTED",
                "上次添加中断，已核对项目登记并清理临时材料。",
            );
            if let Err(issue) = self.finish_operation(&apps, &mut j, Err(interrupted)).await {
                j.cleanup_pending = true;
                let _ = self.save(&j);
                let _=self.tasks.finish_partial(&j.task_id,json!({"appId":j.app_id,"commitState":j.commit_state,"cleanupPending":true,"remainingPaths":j.remaining_paths}),issue);
            }
            result.push(self.snapshot(&j)?);
        }
        let issues = self.issues();
        if !issues.is_empty() {
            return Err(ApiError::new(
                "NODE_GIT_IMPORT_RECOVERY_REQUIRED",
                "一项添加记录无法核对，相关材料已保留；其他项目仍可使用。",
            )
            .detail("issues", json!(issues))
            .detail("recovered", json!(result)));
        }
        Ok(result)
    }
    pub fn request_stop(&self) {
        let _guard = self.submission.lock().unwrap();
        self.stopping.store(true, Ordering::Release);
        for sender in self.active.lock().unwrap().values() {
            let _ = sender.send(true);
        }
    }
    /// Called only after project deletion drained actual operations and removed
    /// its owned tasks. A stale IPC reply must not revive an old success result.
    pub fn forget_deleted_project(&self, app_id: &str) {
        self.records
            .lock()
            .unwrap()
            .retain(|_, j| j.app_id != app_id);
        self.blocked.lock().unwrap().remove(app_id);
        self.unaccepted.lock().unwrap().remove(app_id);
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.request_stop();
        let runners = std::mem::take(&mut *self.runners.lock().unwrap());
        crate::shutdown::join_all("nodeGitImport", runners).await
    }
}
fn read_manifest(root: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = crate::files::read(&crate::files::root(root)?, "panda.json")?;
    let mut bytes = vec![];
    file.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(invalid("应用说明过大。"));
    }
    Ok(bytes)
}

#[cfg(all(test, unix))]
#[path = "node_git_import_tests.rs"]
mod tests;
#[cfg(all(test, windows))]
#[path = "node_git_import/windows_tests.rs"]
mod windows_tests;
