// Explicit local source and project-v1 data versions in the project's own Git.
// No network access, checkout, global identity, user hook or content filter is
// part of these operations. Scopes describe bounded saves and data previews,
// never authorization to move a repository or implicitly replace live data.
use crate::{
    builds::{atomic_json, BuildManager},
    environment::ExecutableInfo,
    git_environment::{GitEnvironmentManager, GitToolchain},
    projects::{check_no_symlinks, checked_relative, ProjectTaskLease},
    ApiError, ProjectHandle, ProjectManifest, ProjectStore, RuntimePaths, TaskRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tempfile::TempDir;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::watch,
    task::JoinHandle,
    time::Instant,
};

const POLICY: &str = "project-source-v1";
const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_FILES: usize = 10_000;
const MAX_BLOB: u64 = 64 * 1024 * 1024;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_METADATA: u64 = 8 * 1024 * 1024;

#[path = "../project_git_process.rs"]
mod process_io;
use process_io::{BlockingPipe, Context};

#[path = "../project_git_build_guard.rs"]
mod build_guard;
pub(crate) use build_guard::BuildGitGuard;

#[path = "../project_git_data_preview.rs"]
mod data_preview;
#[path = "../project_git_data_restore.rs"]
pub(crate) mod data_restore;
#[path = "../project_git_data.rs"]
mod data_versions;
pub(crate) use data_restore::inspect_completion as inspect_data_restore_completion;
pub(crate) use data_restore::metadata_usage as data_restore_metadata_usage;
#[path = "../project_git_restore.rs"]
mod source_restore;
#[path = "../project_version_data.rs"]
mod version_data;
pub(crate) use data_versions::{
    check_data_operations_available, check_data_recovery_record, check_data_workspace_available,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    project_id: String,
    task_id: String,
    operation: String,
    phase: String,
    old_head: Option<String>,
    new_head: Option<String>,
    branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_branch: Option<String>,
    snapshot_digest: String,
    paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data_publication: Option<data_versions::DataPublication>,
}

pub struct ProjectGitManager {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    git: GitEnvironmentManager,
    active: Mutex<HashMap<String, watch::Sender<bool>>>,
    runners: Mutex<Vec<JoinHandle<()>>>,
    submission: Mutex<()>,
    stopping: AtomicBool,
    #[cfg(test)]
    data_save_fault: std::sync::atomic::AtomicU8,
    #[cfg(test)]
    data_restore_fault: std::sync::atomic::AtomicU32,
    #[cfg(test)]
    data_restore_pause: Mutex<Option<DataRestoreTestPause>>,
}
#[cfg(test)]
type DataRestoreTestPause = (std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>);
impl ProjectGitManager {
    pub fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        git: GitEnvironmentManager,
    ) -> Result<Arc<Self>, ApiError> {
        // Construction neither initializes repositories nor replays interrupted commits.
        Ok(Arc::new(Self {
            paths,
            store,
            tasks,
            git,
            active: Mutex::new(HashMap::new()),
            runners: Mutex::new(Vec::new()),
            submission: Mutex::new(()),
            stopping: AtomicBool::new(false),
            #[cfg(test)]
            data_save_fault: std::sync::atomic::AtomicU8::new(0),
            #[cfg(test)]
            data_restore_fault: std::sync::atomic::AtomicU32::new(0),
            #[cfg(test)]
            data_restore_pause: Mutex::new(None),
        }))
    }
    pub async fn status(
        &self,
        project_id: &str,
        stop: watch::Receiver<bool>,
    ) -> Result<ProjectGitStatus, ApiError> {
        self.available()?;
        // Migration waits for transient operations before pausing the workspace.
        // The project read gate alone is acquired too late in that workflow, and
        // also would permit a platform build/save to run alongside this preview.
        let operation = Arc::new(
            self.store
                .reserve_transient_operation(project_id, "git-status")?,
        );
        let project = self.store.project(project_id)?;
        let (_sender, cancel) = watch::channel(false);
        let mut context = Context::new(stop, cancel)?;
        context.bind(self.store.processes(), operation);
        context.checkpoint()?;
        let _gate = tokio::select! {
            biased;
            _ = watch_stop(&mut context.stop) => return Err(interrupted()),
            _ = tokio::time::sleep_until(context.deadline) => return Err(error("PROJECT_GIT_TIMEOUT", "等待项目读取窗口超过内部时间限制。")),
            gate = project.operation_gate.clone().read_owned() => Arc::new(gate),
        };
        context.phase(_gate.clone());
        project.check()?;
        let tools = context.resolve(&self.git).await;
        let mut snapshot = inspect(&project, tools.as_ref().ok(), &mut context).await?;
        if let Err(error) = tools {
            snapshot.view.issues.push(error);
        }
        self.add_recovery_issue(project_id, &mut snapshot.view)?;
        snapshot.view.source_restore =
            crate::source_restore_guard::inspect(self.paths.workspace_root(), project_id);
        finalize_capabilities(&mut snapshot.view);
        snapshot.view.can_initialize = false;
        snapshot.view.can_save = false;
        snapshot.view.can_create_branch = false;
        snapshot.view.database["canSave"] = json!(false);
        Ok(snapshot.view)
    }
    /// A bounded prefix of the current HEAD's local history. No revision supplied
    /// by the caller is interpreted, and the transient lease also blocks migration.
    pub async fn versions(
        &self,
        project_id: &str,
        limit: u32,
        stop: watch::Receiver<bool>,
    ) -> Result<ProjectGitVersions, ApiError> {
        if !(1..=100).contains(&limit) {
            return Err(input_error());
        }
        self.available()?;
        let operation = Arc::new(
            self.store
                .reserve_transient_operation(project_id, "git-versions")?,
        );
        let project = self.store.project(project_id)?;
        let (_sender, cancel) = watch::channel(false);
        let mut context = Context::new(stop, cancel)?;
        context.bind(self.store.processes(), operation);
        context.checkpoint()?;
        let _gate = tokio::select! {
            biased;
            _ = watch_stop(&mut context.stop) => return Err(interrupted()),
            _ = tokio::time::sleep_until(context.deadline) => return Err(error("PROJECT_GIT_TIMEOUT", "等待项目读取窗口超过内部时间限制。")),
            gate = project.operation_gate.clone().read_owned() => Arc::new(gate),
        };
        context.phase(_gate.clone());
        let tools = context.resolve(&self.git).await?;
        let mut snapshot = inspect(&project, Some(&tools), &mut context).await?;
        self.add_recovery_issue(project_id, &mut snapshot.view)?;
        finalize_capabilities(&mut snapshot.view);
        if !snapshot.view.can_list_versions {
            return Err(blocked(&snapshot.view));
        }
        let head = snapshot
            .view
            .repository
            .head
            .clone()
            .ok_or_else(output_invalid)?;
        let mut command = git_command(&tools, context.root(), Some(&project));
        command
            .args([
                "log",
                "--no-show-signature",
                "--no-decorate",
                "--no-notes",
                "--no-use-mailmap",
                "--encoding=UTF-8",
                "-z",
                "--format=%H%x00%P%x00%an%x00%ae%x00%aI%x00%s",
            ])
            .arg(format!("--max-count={}", limit + 1))
            .arg(head.oid.as_ref().ok_or_else(output_invalid)?)
            .arg("--");
        let bytes = context.run(command, Vec::new()).await?.success()?;
        let mut versions = parse_versions(&bytes, limit as usize + 1)?;
        if versions.first().map(|version| &version.oid) != head.oid.as_ref() {
            return Err(output_invalid());
        }
        let verify = inspect(&project, Some(&tools), &mut context).await?;
        if verify.view.snapshot_digest != snapshot.view.snapshot_digest {
            return Err(stale());
        }
        let has_more = versions.len() > limit as usize;
        versions.truncate(limit as usize);
        Ok(ProjectGitVersions {
            repository_root: project.root.clone(),
            head,
            versions,
            has_more,
            limit,
            git: tools.git,
            database: database_boundary(),
        })
    }
    fn submit(
        self: &Arc<Self>,
        project_id: &str,
        operation: Operation,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        let _submission = self.submission.lock().unwrap();
        self.available()?;
        if *stop.borrow() || stop.has_changed().is_err() {
            return Err(interrupted());
        }
        self.store.control(project_id)?.check(false, None, false)?;
        self.store.project(project_id)?.check()?;
        let kind = match operation {
            Operation::DataFinalize(_) => "gitDataSaveFinalize",
            Operation::RestoreApply(_) => "gitSourceRestoreApply",
            Operation::RestoreRecover(ref input) => input.task_kind(),
        };
        let (task, lease) = if let Operation::DataFinalize(input) = &operation {
            self.tasks
                .create_data_save_finalization_operation(project_id, input)?
        } else if let Operation::RestoreRecover(input) = &operation {
            let permit = crate::source_restore_guard::validate_recovery(
                self.paths.workspace_root(),
                project_id,
                &input.restore_task_id,
                &input.expected_plan_digest,
                &input.expected_journal_digest,
            )?;
            self.tasks
                .create_source_restore_recovery_operation(project_id, kind, &permit)?
        } else {
            self.tasks.create_operation(project_id, kind)?
        };
        let (sender, cancel) = watch::channel(false);
        self.active
            .lock()
            .unwrap()
            .insert(task.task_id.clone(), sender);
        let manager = self.clone();
        let task_id = task.task_id.clone();
        let project_id = project_id.to_owned();
        let runner = tokio::spawn(async move {
            let lease = Arc::new(lease);
            let restore_operation = matches!(
                operation,
                Operation::RestoreApply(_) | Operation::RestoreRecover(_)
            );
            let recovery_task = match &operation {
                Operation::RestoreRecover(input) => Some(input.restore_task_id.clone()),
                _ => None,
            };
            let result = manager
                .run(&project_id, &task_id, operation, lease, stop, cancel)
                .await
                .map_err(|mut error| {
                    // A recovery task can stop before entering its filesystem
                    // runner (for example while waiting for the read gate). The
                    // original published transaction still needs explicit action.
                    if let Some(task) = recovery_task {
                        if error.details.get("pending").is_none() {
                            error = error.detail("restoreTaskId", task).detail("pending", true);
                        }
                    }
                    if restore_operation && error.details.get("operationOutcome").is_none() {
                        error.outcome("not_committed")
                    } else {
                        error
                    }
                });
            let _ = manager.tasks.finish(&task_id, result);
            manager.active.lock().unwrap().remove(&task_id);
        });
        let mut runners = self.runners.lock().unwrap();
        // Preserve completed outcomes for the shutdown join; panic is not success.
        runners.push(runner);
        Ok(task)
    }
    pub fn cancel(&self, task_id: &str) -> Result<TaskRecord, ApiError> {
        let task = self.tasks.task(task_id)?;
        if !matches!(
            task.task_type.as_str(),
            "gitInitialize"
                | "gitSave"
                | "gitDataSave"
                | "gitDataSaveFinalize"
                | "gitDataRestoreApply"
                | "gitDataRestoreContinue"
                | "gitDataRestoreUndo"
                | "gitBranchCreate"
                | "gitSourceRestoreApply"
                | "gitSourceRestoreContinue"
                | "gitSourceRestoreUndo"
        ) {
            return Err(input_error());
        }
        if let Some(sender) = self.active.lock().unwrap().get(task_id) {
            let _ = sender.send(true);
        }
        Ok(task)
    }
    /// Inject an owned artificial runner into an isolated test manager.
    #[cfg(test)]
    pub(crate) fn fixture_shutdown_runner(&self, runner: JoinHandle<()>) {
        self.runners.lock().unwrap().push(runner);
    }
    /// An isolated cancellation-dependent runner for shutdown ordering tests.
    #[cfg(test)]
    pub(crate) fn fixture_shutdown_cancel(self: &Arc<Self>, release: std::sync::mpsc::Sender<()>) {
        let id = "artificial-shutdown-cancel".to_owned();
        let (sender, mut cancel) = watch::channel(false);
        self.active.lock().unwrap().insert(id.clone(), sender);
        let manager = self.clone();
        let runner = tokio::spawn(async move {
            while !*cancel.borrow() {
                if cancel.changed().await.is_err() {
                    break;
                }
            }
            let _ = release.send(());
            manager.active.lock().unwrap().remove(&id);
        });
        self.runners.lock().unwrap().push(runner);
    }
    pub(crate) fn request_stop(&self) {
        {
            let _submission = self.submission.lock().unwrap();
            self.stopping.store(true, Ordering::Release);
            for sender in self.active.lock().unwrap().values() {
                let _ = sender.send(true);
            }
        }
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.request_stop();
        let runners = std::mem::take(&mut *self.runners.lock().unwrap());
        let issues = crate::shutdown::join_all("projectGit", runners).await;
        issues
    }
    fn available(&self) -> Result<(), ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(interrupted());
        }
        self.store.check_workspace_available()
    }
    fn journal_path(&self, project_id: &str, task_id: &str) -> PathBuf {
        self.paths
            .workspace_root()
            .join(".platform/project-state/git")
            .join(project_id)
            .join(format!("{task_id}.json"))
    }
    fn add_recovery_issue(
        &self,
        project_id: &str,
        status: &mut ProjectGitStatus,
    ) -> Result<(), ApiError> {
        let root = self
            .paths
            .workspace_root()
            .join(".platform/project-state/git")
            .join(project_id);
        check_no_symlinks(&root, true)?;
        if !root.try_exists()? {
            return Ok(());
        }
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            if path.extension().is_none_or(|v| v != "json") {
                continue;
            }
            let journal: Journal =
                serde_json::from_slice(&read_file(&path, MAX_METADATA)?).map_err(|_| recovery())?;
            if journal.schema_version != 1 || journal.project_id != project_id {
                return Err(recovery());
            }
            if !matches!(journal.phase.as_str(), "completed" | "not-committed") {
                status
                    .issues
                    .push(recovery().detail("taskId", journal.task_id));
            }
        }
        Ok(())
    }
    async fn run(
        &self,
        project_id: &str,
        task_id: &str,
        operation: Operation,
        lease: Arc<ProjectTaskLease>,
        stop: watch::Receiver<bool>,
        cancel: watch::Receiver<bool>,
    ) -> Result<Value, ApiError> {
        let project = self.store.project(project_id)?;
        let mut context = Context::new(stop, cancel)?;
        context.bind(self.store.processes(), lease);
        context.checkpoint()?;
        let operation = match operation {
            Operation::DataFinalize(input) => {
                return self
                    .finalize_data_save_inner(&project, task_id, input, &mut context)
                    .await
            }
            other => other,
        };
        let gate = tokio::select! {
            biased;
            _ = watch_stop(&mut context.stop) => return Err(interrupted()),
            _ = watch_stop(&mut context.cancel) => return Err(interrupted()),
            _ = tokio::time::sleep_until(context.deadline) => return Err(error("PROJECT_GIT_TIMEOUT", "等待项目读取窗口超过内部时间限制。")),
            gate = project.operation_gate.clone().read_owned() => Arc::new(gate),
        };
        context.phase(gate);
        project.check()?;
        self.store.control(project_id)?.check(false, None, false)?;
        match operation {
            Operation::RestoreApply(input) => {
                self.source_restore_apply_inner(&project, task_id, input, &mut context)
                    .await
            }
            Operation::RestoreRecover(input) => {
                self.source_restore_recover_inner(&project, task_id, input, &mut context)
                    .await
            }
            Operation::DataFinalize(_) => {
                unreachable!("legacy finalization is dispatched before acquiring its gate")
            }
        }
    }
}

enum Operation {
    DataFinalize(GitDataSaveFinalizeInput),
    RestoreApply(GitSourceRestoreApplyInput),
    RestoreRecover(GitSourceRestoreRecoverInput),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SourceFile {
    mode: String,
    digest: String,
    size: u64,
}
struct Snapshot {
    view: ProjectGitStatus,
    files: BTreeMap<String, SourceFile>,
    source_digest: String,
}
fn database_boundary() -> Value {
    json!({"state":"not-supported","reason":"源码版本由外部 AI 使用标准 Git 管理。当前业务数据库和附件不会随源码版本自动回退。"})
}
fn head_oid(view: &ProjectGitStatus) -> Option<&str> {
    view.repository
        .head
        .as_ref()
        .and_then(|head| head.oid.as_deref())
}
fn finalize_capabilities(view: &mut ProjectGitStatus) {
    let common = view.repository.git.is_some() && view.scope.complete && view.issues.is_empty();
    view.can_initialize = false;
    view.can_save = common
        && view.repository.state == "project-root"
        && view.repository.index_state == "matched"
        && view
            .repository
            .head
            .as_ref()
            .is_some_and(|head| head.branch.is_some());
    view.can_list_versions = view.repository.git.is_some()
        && view.repository.state == "project-root"
        && head_oid(view).is_some()
        && view.issues.iter().all(|issue| {
            matches!(
                issue.code.as_str(),
                "PROJECT_GIT_INDEX_NOT_CLEAN"
                    | "PROJECT_GIT_DETACHED"
                    | "PROJECT_GIT_SCOPE_INCOMPLETE"
            )
        });
    view.branch_creation_issue = None;
    view.can_create_branch = false;
    if view.source_restore.check().is_err() {
        view.can_initialize = false;
        view.can_save = false;
        view.can_create_branch = false;
    }
    view.database["canSave"] = json!(view.can_save);
}
async fn inspect(
    project: &ProjectHandle,
    tools: Option<&GitToolchain>,
    context: &mut Context,
) -> Result<Snapshot, ApiError> {
    context.checkpoint()?;
    project.check()?;
    let manifest_bytes = read_file(&project.root.join("project.json"), MAX_METADATA)?;
    let manifest: ProjectManifest = serde_json::from_slice(&manifest_bytes).map_err(|_| stale())?;
    if serde_json::to_value(&manifest)? != serde_json::to_value(&project.manifest)? {
        return Err(stale());
    }
    for path in [
        &manifest.source_dir,
        &manifest.output_dir,
        &manifest.database,
        &manifest.attachments_dir,
    ] {
        valid_relative(path)?;
    }
    let roots = included_roots(&manifest);
    let mut scan = scan_source(&project.root, &manifest, &roots)?;
    let data = data_versions::inspect_current_data(project, context)?;
    let initial_source = source_fingerprint(&scan)?;
    let mut markers = Vec::new();
    marker(&project.root.join(".git"), "project", &mut markers)?;
    for ancestor in project.root.ancestors().skip(1) {
        marker(&ancestor.join(".git"), "ancestor", &mut markers)?;
    }
    markers.extend(scan.markers.clone());
    markers.sort_by(|a, b| a.path.cmp(&b.path));
    markers.dedup_by(|a, b| a.path == b.path);
    let root_marker = markers.iter().find(|marker| marker.scope == "project");
    let state = if markers.iter().any(|marker| marker.kind == "unsafe") {
        "unsafe"
    } else if markers.len() > 1 {
        "ambiguous"
    } else if let Some(root) = root_marker {
        if root.kind == "directory" {
            "project-root"
        } else {
            "unsupported"
        }
    } else if markers.iter().any(|marker| marker.scope == "source") {
        "source-only"
    } else if !markers.is_empty() {
        "ancestor"
    } else {
        "none"
    };
    let mut view = ProjectGitStatus {
        repository: GitRepositoryState {
            state: state.into(),
            root: if state == "project-root" {
                Some(project.root.clone())
            } else {
                None
            },
            markers,
            head: None,
            index_state: "not-inspected".into(),
            git: tools.map(|tool| tool.git.clone()),
        },
        scope: GitSourceScope {
            policy: POLICY.into(),
            included_roots: roots,
            included_paths: scan.files.keys().cloned().collect(),
            excluded: scan.excluded.clone(),
            unclassified_paths: scan.unclassified.clone(),
            protected_tracked_paths: vec![],
            complete: scan.unclassified.is_empty() && scan.issues.is_empty(),
        },
        changes: vec![],
        snapshot_digest: String::new(),
        can_initialize: false,
        can_save: false,
        can_list_versions: false,
        can_create_branch: false,
        branch_creation_issue: None,
        source_restore: Default::default(),
        database: data_versions::data_status(data.as_ref(), state),
        issues: std::mem::take(&mut scan.issues),
    };
    if !matches!(state, "none" | "project-root") {
        view.issues.push(error(
            "PROJECT_GIT_SCOPE_UNSUPPORTED",
            "检测到已有源码、父级、多重或非普通根仓库；不会移动、包裹或替换其 Git 根。",
        ));
    }
    let mut head_entries = BTreeMap::new();
    let mut index_entries = BTreeMap::new();
    let mut repo_digest = String::new();
    let mut root_ready = false;
    if state == "project-root" {
        let git_dir = project.root.join(".git");
        inspect_repository_files(&git_dir, &mut view.issues)?;
        repo_digest = repository_digest(&git_dir)?;
        if view.issues.is_empty() {
            if let Some(tools) = tools {
                check_config_file(project, tools, context, &mut view.issues).await?;
                root_ready = view.issues.is_empty();
            }
        }
        // Even read-only Git commands may follow alternates or config includes.
        // Inspect those boundaries before allowing Git to open this repository.
        if let Some(tools) = tools.filter(|_| root_ready) {
            let mut command = git_command(tools, context.root(), Some(project));
            command.args(["symbolic-ref", "--quiet", "HEAD"]);
            let branch_output = context.run(command, Vec::new()).await?;
            let branch = match branch_output.code {
                0 => Some(one_ref(&branch_output.stdout)?),
                1 => None,
                _ => return Err(git_failed()),
            };
            let mut command = git_command(tools, context.root(), Some(project));
            command.args(["rev-parse", "--verify", "HEAD^{commit}"]);
            let head_output = context.run(command, Vec::new()).await?;
            let oid = if head_output.code == 0 {
                Some(one_oid(&head_output.stdout)?)
            } else if let Some(branch) = &branch {
                let mut command = git_command(tools, context.root(), Some(project));
                command.args(["show-ref", "--verify", "--quiet", branch]);
                if context.run(command, Vec::new()).await?.code != 1 {
                    return Err(git_failed());
                }
                None
            } else {
                return Err(git_failed());
            };
            view.repository.head = Some(GitHead {
                oid: oid.clone(),
                branch,
            });
            if oid.is_some() {
                let mut command = git_command(tools, context.root(), Some(project));
                command.args(["ls-tree", "-r", "-z", "--full-tree", "HEAD"]);
                head_entries = parse_tree(
                    &context.run(command, Vec::new()).await?.success()?,
                    false,
                    &mut view.issues,
                )?;
            }
            let mut command = git_command(tools, context.root(), Some(project));
            command.args(["ls-files", "--stage", "-z"]);
            index_entries = parse_tree(
                &context.run(command, Vec::new()).await?.success()?,
                true,
                &mut view.issues,
            )?;
            view.repository.index_state = if index_entries == head_entries {
                "matched"
            } else {
                "staged-changes"
            }
            .into();
            if view.repository.index_state != "matched" {
                view.issues.push(error(
                    "PROJECT_GIT_INDEX_NOT_CLEAN",
                    "已有暂存或冲突内容；请先在标准 Git 工具中处理，本步不覆盖索引。",
                ));
            }
            if view
                .repository
                .head
                .as_ref()
                .is_some_and(|head| head.branch.is_none())
            {
                view.issues.push(error(
                    "PROJECT_GIT_DETACHED",
                    "当前是分离 HEAD，本步不代选分支。",
                ));
            }
            for path in head_entries.keys().chain(index_entries.keys()) {
                if protected(path, &manifest).is_some() {
                    view.scope.protected_tracked_paths.push(path.clone());
                } else if !included(path, &view.scope.included_roots)
                    && !data_versions::is_data_path(path)
                {
                    view.scope.unclassified_paths.push(path.clone());
                }
            }
            view.scope.protected_tracked_paths.sort();
            view.scope.protected_tracked_paths.dedup();
            view.scope.unclassified_paths.sort();
            view.scope.unclassified_paths.dedup();
            if !view.scope.protected_tracked_paths.is_empty() {
                view.issues.push(error("PROJECT_GIT_PROTECTED_TRACKED", "HEAD 或索引已跟踪受保护的数据、凭据、依赖或产物；本步不会删除跟踪项或改写历史。"));
            }
            check_repository_options(project, tools, context, &mut view.issues).await?;
            data_versions::check_data_tracking(
                data.as_ref(),
                &head_entries,
                &index_entries,
                &mut view.issues,
                project,
                tools,
                context,
            )
            .await?;
            // Ignore rules never exempt tracked protected files from the checks above.
            let untracked: Vec<String> = scan
                .files
                .keys()
                .filter(|path| {
                    !head_entries.contains_key(*path) && !index_entries.contains_key(*path)
                })
                .cloned()
                .collect();
            if !untracked.is_empty() {
                let mut input = Vec::new();
                for path in &untracked {
                    input.extend_from_slice(path.as_bytes());
                    input.push(0);
                }
                let mut command = git_command(tools, context.root(), Some(project));
                command.args(["check-ignore", "-z", "--stdin"]);
                let output = context.run(command, input).await?;
                if !matches!(output.code, 0 | 1) {
                    return Err(git_failed());
                }
                for path in parse_paths(&output.stdout)? {
                    if !untracked.contains(&path) {
                        return Err(output_invalid());
                    }
                    view.scope.excluded.push(GitExcludedPath {
                        path: path.clone(),
                        reason:
                            "gitignore：此开发文件未纳入；请调整忽略规则或明确适配范围后重新预览。"
                                .into(),
                    });
                    view.scope.unclassified_paths.push(path.clone());
                    scan.files.remove(&path);
                }
            }
            check_attributes(
                project,
                tools,
                context,
                scan.files.keys().chain(head_entries.keys()),
                &mut view.issues,
            )
            .await?;
        }
    }
    if let Some(tools) = tools.filter(|_| state != "project-root" || root_ready) {
        let hashes = hash_paths(
            tools,
            context,
            if state == "project-root" {
                Some(project)
            } else {
                None
            },
            &project.root,
            scan.files.keys(),
            false,
        )
        .await?;
        for (path, entry) in &scan.files {
            let object = hashes.get(path).ok_or_else(output_invalid)?;
            let expected = (entry.mode.clone(), object.clone());
            if head_entries.get(path) != Some(&expected) {
                view.changes.push(GitChange {
                    path: path.clone(),
                    kind: if head_entries.contains_key(path) {
                        "modify"
                    } else {
                        "add"
                    }
                    .into(),
                    staged: index_entries.get(path) != head_entries.get(path),
                });
            }
        }
        for path in head_entries
            .keys()
            .filter(|path| !scan.files.contains_key(*path))
            .filter(|path| !data_versions::is_data_path(path))
        {
            view.changes.push(GitChange {
                path: path.clone(),
                kind: "delete".into(),
                staged: index_entries.get(path) != head_entries.get(path),
            });
        }
    }
    let after = scan_source(&project.root, &manifest, &view.scope.included_roots)?;
    // The second scan binds file bytes to Git's raw-object view, despite external
    // editors not participating in platform project leases.
    if source_fingerprint(&after)? != initial_source {
        return Err(stale());
    }
    for (path, entry) in &scan.files {
        if after
            .files
            .get(path)
            .is_none_or(|file| file.digest != entry.digest || file.mode != entry.mode)
        {
            return Err(stale());
        }
    }
    if read_file(&project.root.join("project.json"), MAX_METADATA)? != manifest_bytes {
        return Err(stale());
    }
    if state == "project-root" && repository_digest(&project.root.join(".git"))? != repo_digest {
        return Err(stale());
    }
    view.scope.included_paths = scan.files.keys().cloned().collect();
    view.scope.unclassified_paths.sort();
    view.scope.unclassified_paths.dedup();
    view.scope.excluded.sort_by(|a, b| a.path.cmp(&b.path));
    view.scope.complete = view.scope.complete
        && view.scope.unclassified_paths.is_empty()
        && view.scope.protected_tracked_paths.is_empty();
    if !view.scope.unclassified_paths.is_empty() {
        view.issues.push(error("PROJECT_GIT_SCOPE_INCOMPLETE", "存在未纳入的开发文件或目录。请将开发材料放入声明的源码/文档目录，或调整忽略规则后重新预览；本步不会声称已全部保存。"));
    }
    view.changes.sort_by(|a, b| a.path.cmp(&b.path));
    view.snapshot_digest = sha256(&serde_json::to_vec(
        &json!({"policy":POLICY,"root":project.root,"rootIdentity":identity(&project.root)?,"manifest":manifest,"repository":view.repository,"repositoryDigest":repo_digest,"index":index_entries,"head":head_entries,"files":scan.files,"scope":view.scope,"sourceFingerprint":source_fingerprint(&after)?,"dataManifest":data.as_ref().map(|data| &data.manifest_digest)}),
    )?);
    finalize_capabilities(&mut view);
    Ok(Snapshot {
        view,
        files: scan.files,
        source_digest: initial_source,
    })
}

struct Scan {
    files: BTreeMap<String, SourceFile>,
    excluded: Vec<GitExcludedPath>,
    unclassified: Vec<String>,
    markers: Vec<GitMarker>,
    issues: Vec<ApiError>,
}
fn included_roots(manifest: &ProjectManifest) -> Vec<String> {
    let mut roots = vec![
        "project.json".into(),
        manifest.source_dir.clone(),
        "README.md".into(),
        "AGENTS.md".into(),
        "CLAUDE.md".into(),
        "docs".into(),
        ".agents".into(),
        "meta".into(),
        ".gitignore".into(),
        ".gitattributes".into(),
    ];
    if let Some(docs) = manifest.docs.as_object() {
        roots.extend(docs.values().filter_map(Value::as_str).map(str::to_owned));
    }
    roots.sort();
    roots.dedup();
    roots
}
fn included(path: &str, roots: &[String]) -> bool {
    roots
        .iter()
        .any(|root| path == root || path.starts_with(&format!("{root}/")))
}
fn is_ancestor(path: &str, roots: &[String]) -> bool {
    roots
        .iter()
        .any(|root| root.starts_with(&format!("{path}/")))
}
fn protected(path: &str, manifest: &ProjectManifest) -> Option<&'static str> {
    // The supported macOS volume may be case-insensitive. Case variants must not
    // turn known control/data names into ordinary source in a Git tree.
    let folded = path.to_ascii_lowercase();
    let path_value = Path::new(&folded);
    if path_value.starts_with("data") {
        return Some("活跃数据库和附件未纳入版本");
    }
    if path_value.starts_with(".platform") {
        return Some("本机控制状态、任务与操作现场");
    }
    if path_value.starts_with(manifest.output_dir.to_ascii_lowercase()) {
        return Some("当前运行产物，本步只保存开发材料");
    }
    let names: Vec<&str> = folded.split('/').collect();
    if names.contains(&".git") {
        return Some("嵌套 Git 元数据不能作为普通开发文件保存");
    }
    if names.iter().any(|name| {
        matches!(
            *name,
            "node_modules" | ".cache" | ".vite" | ".hg" | ".svn" | ".ds_store"
        )
    }) {
        return Some("依赖或缓存目录");
    }
    if names.iter().any(|name| {
        let name = name.to_ascii_lowercase();
        name == "credentials"
            || name == "configuration"
            || name == ".ssh"
            || name.starts_with(".env")
            || matches!(
                name.as_str(),
                ".npmrc" | ".netrc" | "id_rsa" | "id_ed25519" | "id_ecdsa" | "id_dsa"
            )
            || [".pem", ".key", ".p12", ".pfx"]
                .iter()
                .any(|suffix| name.ends_with(suffix))
    }) {
        return Some("已知凭据、私钥或本机配置名称；仅按路径排除，不承诺通用秘密识别");
    }
    None
}
fn scan_source(
    root: &Path,
    manifest: &ProjectManifest,
    roots: &[String],
) -> Result<Scan, ApiError> {
    let mut scan = Scan {
        files: BTreeMap::new(),
        excluded: vec![],
        unclassified: vec![],
        markers: vec![],
        issues: vec![],
    };
    let mut bytes = 0;
    let mut count = 0;
    fn visit(
        root: &Path,
        relative: &Path,
        manifest: &ProjectManifest,
        roots: &[String],
        scan: &mut Scan,
        bytes: &mut u64,
        count: &mut usize,
    ) -> Result<(), ApiError> {
        check_no_symlinks(&root.join(relative), false)?;
        let mut entries = fs::read_dir(root.join(relative))?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            *count += 1;
            if *count > MAX_FILES * 2 {
                return Err(limit());
            }
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(unsafe_path)?;
            if name.chars().any(char::is_control) || name.contains('\\') {
                return Err(unsafe_path());
            }
            let path = relative.join(name);
            let label = path.to_str().ok_or_else(unsafe_path)?.to_owned();
            if name.eq_ignore_ascii_case(".git") {
                if !relative.as_os_str().is_empty() {
                    marker(&entry.path(), "source", &mut scan.markers)?;
                }
                continue;
            }
            if relative.as_os_str().is_empty() && name.eq_ignore_ascii_case(version_data::DIRECTORY)
            {
                if name != version_data::DIRECTORY {
                    return Err(unsafe_path());
                }
                scan.excluded.push(GitExcludedPath {
                    path: label,
                    reason: "数据版本快照单独核验，源码取回不修改此目录".into(),
                });
                continue;
            }
            if let Some(reason) = protected(&label, manifest) {
                scan.excluded.push(GitExcludedPath {
                    path: label,
                    reason: reason.into(),
                });
                continue;
            }
            if !included(&label, roots) && !is_ancestor(&label, roots) {
                scan.unclassified.push(label);
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_dir() {
                visit(root, &path, manifest, roots, scan, bytes, count)?;
            } else if metadata.is_file() && metadata.nlink() == 1 {
                if metadata.len() > MAX_BLOB || scan.files.len() >= MAX_FILES {
                    return Err(limit());
                }
                *bytes = bytes.checked_add(metadata.len()).ok_or_else(limit)?;
                if *bytes > MAX_BYTES {
                    return Err(limit());
                }
                let contents = read_file(&entry.path(), MAX_BLOB)?;
                scan.files.insert(
                    label,
                    SourceFile {
                        mode: if metadata.mode() & 0o111 != 0 {
                            "100755"
                        } else {
                            "100644"
                        }
                        .into(),
                        digest: sha256(&contents),
                        size: contents.len() as u64,
                    },
                );
            } else {
                scan.issues.push(unsafe_path().detail("path", label));
            }
        }
        Ok(())
    }
    visit(
        root,
        Path::new(""),
        manifest,
        roots,
        &mut scan,
        &mut bytes,
        &mut count,
    )?;
    Ok(scan)
}
fn source_fingerprint(scan: &Scan) -> Result<String, ApiError> {
    Ok(sha256(&serde_json::to_vec(
        &json!({"files":scan.files,"excluded":scan.excluded,"unclassified":scan.unclassified,"markers":scan.markers,"issues":scan.issues}),
    )?))
}
fn marker(path: &Path, scope: &str, markers: &mut Vec<GitMarker>) -> Result<(), ApiError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => markers.push(GitMarker {
            path: path.to_path_buf(),
            scope: scope.into(),
            kind: if metadata.is_dir() {
                "directory"
            } else if metadata.is_file() && metadata.nlink() == 1 {
                "file"
            } else {
                "unsafe"
            }
            .into(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err(unsafe_path()),
    }
    Ok(())
}
fn inspect_repository_files(root: &Path, issues: &mut Vec<ApiError>) -> Result<(), ApiError> {
    check_no_symlinks(root, false)?;
    let mut count = 0usize;
    fn visit(
        root: &Path,
        path: &Path,
        issues: &mut Vec<ApiError>,
        count: &mut usize,
        depth: usize,
    ) -> Result<(), ApiError> {
        if depth > 64 {
            return Err(limit());
        }
        check_no_symlinks(path, false)?;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            *count += 1;
            if *count > 100_000 {
                return Err(limit());
            }
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)?;
            if meta.is_dir() {
                visit(root, &path, issues, count, depth + 1)?;
            } else if !meta.is_file() || meta.nlink() != 1 {
                return Err(unsafe_path());
            }
            let relative = path.strip_prefix(root).map_err(|_| unsafe_path())?;
            let label = relative.to_str().ok_or_else(unsafe_path)?;
            if label.ends_with(".lock") {
                issues.push(error(
                    "PROJECT_GIT_LOCKED",
                    "仓库存在其他 Git 操作锁；不会删除锁文件或继续写入。",
                ));
            }
            if matches!(
                label,
                "commondir"
                    | "config.worktree"
                    | "objects/info/alternates"
                    | "objects/info/http-alternates"
                    | "info/grafts"
                    | "info/sparse-checkout"
                    | "MERGE_HEAD"
                    | "CHERRY_PICK_HEAD"
                    | "REVERT_HEAD"
                    | "BISECT_LOG"
            ) || label.starts_with("sharedindex.")
                || relative.starts_with("worktrees")
                || relative.starts_with("rebase-merge")
                || relative.starts_with("rebase-apply")
                || relative.starts_with("sequencer")
            {
                issues.push(error("PROJECT_GIT_REPOSITORY_UNSUPPORTED","仓库包含本步未支持的共享对象、工作树、稀疏索引或未完成 Git 操作；保持现场不变。"));
            }
        }
        Ok(())
    }
    visit(root, root, issues, &mut count, 0)
}
fn repository_digest(root: &Path) -> Result<String, ApiError> {
    let mut digest = Sha256::new();
    for name in [
        "HEAD",
        "config",
        "index",
        "packed-refs",
        "shallow",
        "info/attributes",
        "info/exclude",
    ] {
        let path = root.join(name);
        check_no_symlinks(&path, true)?;
        digest.update(name.as_bytes());
        digest.update([0]);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                digest.update(identity(&path)?.as_bytes());
                digest.update(read_file(&path, MAX_METADATA)?);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => digest.update(b"missing"),
            Err(_) => return Err(unsafe_path()),
        }
        digest.update([0]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
// Run outside the repository with an explicit local config file and includes
// disabled, before any command can discover a repository's external settings.
async fn check_config_file(
    project: &ProjectHandle,
    tools: &GitToolchain,
    context: &mut Context,
    issues: &mut Vec<ApiError>,
) -> Result<(), ApiError> {
    check_config_path(&project.root.join(".git"), tools, context, issues).await
}
async fn check_config_path(
    git_dir: &Path,
    tools: &GitToolchain,
    context: &mut Context,
    issues: &mut Vec<ApiError>,
) -> Result<(), ApiError> {
    for key in [
        "core.autocrlf",
        "core.attributesFile",
        "core.sparseCheckout",
        "core.splitIndex",
        "core.bare",
        "extensions.worktreeConfig",
        "extensions.partialClone",
    ] {
        let mut command = git_command(tools, context.root(), None);
        command
            .args(["config", "--file"])
            .arg(git_dir.join("config"))
            .args(["--no-includes", "--get", key]);
        let output = context.run(command, Vec::new()).await?;
        if output.code == 1 {
            continue;
        }
        if output.code != 0 {
            return Err(git_failed());
        }
        let value = std::str::from_utf8(&output.stdout)
            .map_err(|_| output_invalid())?
            .trim()
            .to_ascii_lowercase();
        let harmless_false = matches!(value.as_str(), "false" | "no" | "off" | "0");
        if !(matches!(
            key,
            "core.autocrlf"
                | "core.sparseCheckout"
                | "core.splitIndex"
                | "core.bare"
                | "extensions.worktreeConfig"
        ) && harmless_false)
        {
            issues.push(error(
                "PROJECT_GIT_CONFIGURATION_UNSUPPORTED",
                "仓库使用本步未支持的内容转换、属性来源、稀疏/共享索引或特殊配置；未修改配置。",
            ));
        }
    }
    let mut command = git_command(tools, context.root(), None);
    command
        .args(["config", "--file"])
        .arg(git_dir.join("config"))
        .args(["--no-includes", "--get-regexp", "^include"]);
    let output = context.run(command, Vec::new()).await?;
    if output.code == 0 {
        issues.push(error(
            "PROJECT_GIT_CONFIGURATION_UNSUPPORTED",
            "仓库配置包含外部 include；本步不会采用外部配置保存版本。",
        ));
    } else if output.code != 1 {
        return Err(git_failed());
    }
    Ok(())
}
async fn check_repository_options(
    project: &ProjectHandle,
    tools: &GitToolchain,
    context: &mut Context,
    issues: &mut Vec<ApiError>,
) -> Result<(), ApiError> {
    for (flag, expected) in [
        ("--show-object-format", "sha1"),
        ("--show-ref-format", "files"),
    ] {
        let mut command = git_command(tools, context.root(), Some(project));
        command.args(["rev-parse", flag]);
        let value = context.run(command, Vec::new()).await?.success()?;
        if value != format!("{expected}\n").as_bytes() {
            issues.push(error(
                "PROJECT_GIT_REPOSITORY_UNSUPPORTED",
                "本步仅支持普通 SHA-1 对象和 files 引用存储的本地仓库。",
            ));
        }
    }
    // ls-files -v reports assume-unchanged and skip-worktree flags without
    // refreshing the index or consulting a user fsmonitor program.
    let mut command = git_command(tools, context.root(), Some(project));
    command.args(["ls-files", "-v", "-z"]);
    for entry in context
        .run(command, Vec::new())
        .await?
        .success()?
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        if entry.len() < 3 || entry[1] != b' ' {
            return Err(output_invalid());
        }
        if entry[0].is_ascii_lowercase() || entry[0] == b'S' {
            issues.push(error(
                "PROJECT_GIT_INDEX_UNSUPPORTED",
                "索引包含 assume-unchanged 或 skip-worktree 标记；本步不会清除这些用户选择。",
            ));
            break;
        }
    }
    Ok(())
}
async fn check_attributes<'a>(
    project: &ProjectHandle,
    tools: &GitToolchain,
    context: &mut Context,
    paths: impl Iterator<Item = &'a String>,
    issues: &mut Vec<ApiError>,
) -> Result<(), ApiError> {
    let paths: BTreeSet<&String> = paths.collect();
    if paths.is_empty() {
        return Ok(());
    }
    let mut input = Vec::new();
    for path in paths {
        input.extend_from_slice(path.as_bytes());
        input.push(0);
    }
    let mut command = git_command(tools, context.root(), Some(project));
    command.args([
        "check-attr",
        "-z",
        "--stdin",
        "filter",
        "text",
        "eol",
        "ident",
        "working-tree-encoding",
    ]);
    let output = context.run(command, input).await?.success()?;
    let fields: Vec<&[u8]> = output.split(|byte| *byte == 0).collect();
    if fields.last() != Some(&b"".as_slice()) || !(fields.len() - 1).is_multiple_of(3) {
        return Err(output_invalid());
    }
    for fields in fields[..fields.len() - 1].chunks(3) {
        if !matches!(fields[2], b"unspecified" | b"unset") {
            issues.push(error("PROJECT_GIT_ATTRIBUTES_UNSUPPORTED","存在生效的内容转换属性；本步不会运行用户 filter，也不会绕过转换后冒称标准内容已保存。"));
            break;
        }
    }
    Ok(())
}
type Entries = BTreeMap<String, (String, String)>;
fn parse_tree(bytes: &[u8], index: bool, issues: &mut Vec<ApiError>) -> Result<Entries, ApiError> {
    let mut entries = BTreeMap::new();
    for record in bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let text = std::str::from_utf8(record).map_err(|_| output_invalid())?;
        let (fields, path) = text.split_once('\t').ok_or_else(output_invalid)?;
        valid_relative(path)?;
        let fields: Vec<&str> = fields.split(' ').collect();
        if fields.len() != 3 {
            return Err(output_invalid());
        }
        let (mode, oid, stage) = if index {
            (fields[0], fields[1], fields[2])
        } else {
            (fields[0], fields[2], "0")
        };
        if !valid_oid(oid) {
            return Err(output_invalid());
        }
        if !matches!(mode, "100644" | "100755") || (!index && fields[1] != "blob") || stage != "0" {
            issues.push(error(
                "PROJECT_GIT_ENTRY_UNSUPPORTED",
                "已有树或索引包含符号链接、子模块或冲突条目；本步不展开或替换这些内容。",
            ));
        }
        if entries
            .insert(path.into(), (mode.into(), oid.into()))
            .is_some()
            && stage == "0"
        {
            return Err(output_invalid());
        }
    }
    if entries.len() > MAX_FILES {
        return Err(limit());
    }
    Ok(entries)
}
fn parse_paths(bytes: &[u8]) -> Result<Vec<String>, ApiError> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let path = std::str::from_utf8(entry).map_err(|_| output_invalid())?;
            valid_relative(path)?;
            Ok(path.into())
        })
        .collect()
}
async fn hash_paths<'a>(
    tools: &GitToolchain,
    context: &mut Context,
    project: Option<&ProjectHandle>,
    root: &Path,
    paths: impl Iterator<Item = &'a String>,
    write: bool,
) -> Result<BTreeMap<String, String>, ApiError> {
    let paths: Vec<&String> = paths.collect();
    if paths.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut input = Vec::new();
    for path in &paths {
        let full = checked_relative(root, path, false)?;
        let value = full.to_str().ok_or_else(unsafe_path)?;
        // --stdin-paths understands Git's C-style quoting; paths have no control
        // characters and Unicode bytes are preserved without JSON \u escapes.
        let quoted = format!("\"{}\"\n", value.replace('\\', "\\\\").replace('"', "\\\""));
        input.extend_from_slice(quoted.as_bytes());
    }
    let mut command = git_command(tools, context.root(), project);
    command.args(["hash-object", "--no-filters", "--stdin-paths"]);
    if write {
        command.arg("-w");
    }
    let output = context.run(command, input).await?.success()?;
    let text = std::str::from_utf8(&output).map_err(|_| output_invalid())?;
    let hashes: Vec<&str> = text.lines().collect();
    if hashes.len() != paths.len() {
        return Err(output_invalid());
    }
    paths
        .into_iter()
        .zip(hashes)
        .map(|(path, oid)| {
            if !valid_oid(oid) {
                return Err(output_invalid());
            }
            Ok((path.clone(), oid.into()))
        })
        .collect()
}

fn index_fingerprint(path: &Path) -> Result<Option<String>, ApiError> {
    check_no_symlinks(path, true)?;
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(Some(format!(
            "{}:{}",
            identity(path)?,
            sha256(&read_file(path, MAX_METADATA)?)
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(unsafe_path()),
    }
}

fn private_dir(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>, ApiError> {
    check_no_symlinks(path, false)?;
    let before = fs::symlink_metadata(path)?;
    if !before.is_file() || before.nlink() != 1 || before.len() > limit {
        return Err(unsafe_path());
    }
    let mut file = platform_native::filesystem::open_read(
        path,
        platform_native::filesystem::ReadOptions::NoLinksNonblocking,
    )?;
    if !same_file(&before, &file.metadata()?) {
        return Err(stale());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != before.len()
        || !same_file(&before, &file.metadata()?)
        || !same_file(&before, &fs::symlink_metadata(path)?)
    {
        return Err(stale());
    }
    Ok(bytes)
}
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.mode() == b.mode()
        && a.nlink() == b.nlink()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}
fn identity(path: &Path) -> Result<String, ApiError> {
    check_no_symlinks(path, false)?;
    let m = fs::symlink_metadata(path)?;
    Ok(format!("{}:{}", m.dev(), m.ino()))
}
fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn rename_new(from: &Path, to: &Path) -> Result<(), ApiError> {
    check_no_symlinks(from, false)?;
    check_no_symlinks(to, true)?;
    #[cfg(target_os = "macos")]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let from = CString::new(from.as_os_str().as_bytes()).map_err(|_| unsafe_path())?;
        let to = CString::new(to.as_os_str().as_bytes()).map_err(|_| unsafe_path())?;
        if unsafe {
            libc::renameatx_np(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        } != 0
        {
            return Err(error(
                "PROJECT_GIT_INITIALIZE_CONFLICT",
                "Git 根已出现或不能安全发布；保留现有目录。",
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (from, to);
        Err(error(
            "UNSUPPORTED_PLATFORM",
            "此 Git 初始化事务目前仅验证 macOS。",
        ))
    }
}
fn valid_relative(path: &str) -> Result<(), ApiError> {
    if path.is_empty()
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || Path::new(path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        Err(unsafe_path())
    } else {
        Ok(())
    }
}
fn valid_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && value.bytes().any(|byte| byte != b'0')
}
fn one_oid(bytes: &[u8]) -> Result<String, ApiError> {
    let value = std::str::from_utf8(bytes)
        .map_err(|_| output_invalid())?
        .strip_suffix('\n')
        .ok_or_else(output_invalid)?;
    if !valid_oid(value) {
        return Err(output_invalid());
    }
    Ok(value.into())
}
fn one_ref(bytes: &[u8]) -> Result<String, ApiError> {
    let value = std::str::from_utf8(bytes)
        .map_err(|_| output_invalid())?
        .strip_suffix('\n')
        .ok_or_else(output_invalid)?;
    if !value.starts_with("refs/heads/")
        || value.len() > 1024
        || value.ends_with('.')
        || value.contains("..")
        || value.contains("@{")
        || value
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c))
        || value
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
    {
        return Err(output_invalid());
    }
    Ok(value.into())
}
fn parse_versions(bytes: &[u8], maximum: usize) -> Result<Vec<GitVersion>, ApiError> {
    if bytes.len() > MAX_OUTPUT {
        return Err(limit());
    }
    if bytes.is_empty() {
        return Ok(vec![]);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| output_invalid())?;
    let fields: Vec<&str> = text
        .strip_suffix('\0')
        .ok_or_else(output_invalid)?
        .split('\0')
        .collect();
    if !fields.len().is_multiple_of(6) || fields.len() / 6 > maximum {
        return Err(output_invalid());
    }
    let mut versions = Vec::new();
    let mut seen = BTreeSet::new();
    for row in fields.as_chunks::<6>().0 {
        let parents: Vec<String> = if row[1].is_empty() {
            vec![]
        } else {
            row[1].split(' ').map(str::to_owned).collect()
        };
        let valid_text = |value: &str, maximum: usize| {
            value.len() <= maximum && !value.chars().any(char::is_control)
        };
        if !valid_oid(row[0])
            || !seen.insert(row[0])
            || parents.len() > 100
            || parents.iter().any(|oid| !valid_oid(oid))
            || !valid_text(row[2], 200)
            || !valid_text(row[3], 254)
            || !valid_text(row[5], 4000)
            || !valid_author_date(row[4])
        {
            return Err(output_invalid());
        }
        versions.push(GitVersion {
            oid: row[0].into(),
            parents,
            subject: row[5].into(),
            author: GitAuthor {
                name: row[2].into(),
                email: row[3].into(),
            },
            authored_at: row[4].into(),
        });
    }
    Ok(versions)
}
fn valid_author_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 25
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            10 => *byte == b'T',
            13 | 16 | 22 => *byte == b':',
            19 => matches!(*byte, b'+' | b'-'),
            _ => byte.is_ascii_digit(),
        })
}
fn validate_digest(value: &str) -> Result<(), ApiError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Err(input_error())
    } else {
        Ok(())
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}
fn input_error() -> ApiError {
    error(
        "PROJECT_GIT_INPUT_INVALID",
        "请提供当前预览、明确作者和有效的版本说明。",
    )
}
fn stale() -> ApiError {
    error(
        "PROJECT_GIT_STALE_PREVIEW",
        "HEAD、索引或开发文件已变化；请重新查看范围和改动后保存。",
    )
}
fn unsafe_path() -> ApiError {
    error(
        "PROJECT_GIT_UNSAFE_PATH",
        "Git 或开发文件包含链接、特殊文件或不安全路径；未跟随该路径。",
    )
}
fn limit() -> ApiError {
    error(
        "PROJECT_GIT_LIMIT",
        "项目 Git 预览或文件大小超过当前内部处理上限；未截断后保存。",
    )
}
fn output_invalid() -> ApiError {
    error(
        "PROJECT_GIT_OUTPUT_INVALID",
        "Git 返回无法识别的结构化结果；未猜测仓库状态。",
    )
}
fn git_failed() -> ApiError {
    error(
        "PROJECT_GIT_FAILED",
        "Git 本地操作失败；未回显可能包含私人信息的工具输出。",
    )
}
fn interrupted() -> ApiError {
    error(
        "PROJECT_GIT_INTERRUPTED",
        "Git 操作已收到停止请求；提交点前不会移动版本，提交结果以任务记录为准。",
    )
}
fn recovery() -> ApiError {
    error(
        "PROJECT_GIT_RECOVERY_REQUIRED",
        "此 Git 操作的引用或索引结果需要核对；已保留本次现场，不会重新提交或覆盖当前内容。",
    )
}
fn blocked(view: &ProjectGitStatus) -> ApiError {
    error(
        "PROJECT_GIT_SCOPE_BLOCKED",
        "当前仓库或保存范围尚不支持写入；请查看完整范围与问题列表。",
    )
    .detail("issues", json!(view.issues))
    .detail("scope", json!(view.scope))
}

fn git_command(tools: &GitToolchain, private: &Path, project: Option<&ProjectHandle>) -> Command {
    let mut command = Command::new(&tools.git.path);
    command
        .env_clear()
        .current_dir(project.map_or(private, |project| project.root.as_path()))
        .env("PATH", &tools.path)
        .env("LC_ALL", "C")
        .env("HOME", private.join("home"))
        .env("XDG_CONFIG_HOME", private.join("home"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env(
            "GIT_CEILING_DIRECTORIES",
            private.parent().unwrap_or(private),
        )
        .env("GIT_DISCOVERY_ACROSS_FILESYSTEM", "0")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_PROTOCOL_FROM_USER", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/usr/bin/false")
        .env("GIT_SSH", "/usr/bin/false")
        .env("GIT_SSH_COMMAND", "/usr/bin/false")
        .env("SSH_ASKPASS", "/usr/bin/false")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::owned_process::prepare_command(&mut command);
    if let Some(path) = &tools.exec_path {
        command.env("GIT_EXEC_PATH", path);
    }
    command.arg("--no-pager");
    for setting in [
        "protocol.allow=never",
        "credential.helper=",
        "credential.interactive=false",
        "core.askPass=/usr/bin/false",
        "core.fsmonitor=false",
        "core.untrackedCache=false",
        "core.attributesFile=/dev/null",
        "core.excludesFile=/dev/null",
        "core.autocrlf=false",
        "commit.gpgSign=false",
        "tag.gpgSign=false",
        "gpg.program=/usr/bin/false",
        "gc.auto=0",
        "maintenance.auto=false",
        "core.fsync=all",
        "core.fsyncMethod=fsync",
    ] {
        command.arg("-c").arg(setting);
    }
    command.arg("-c").arg(format!(
        "core.hooksPath={}",
        private.join("empty-hooks").display()
    ));
    if let Some(project) = project {
        command
            .arg(format!("--git-dir={}", project.root.join(".git").display()))
            .arg(format!("--work-tree={}", project.root.display()));
    }
    command
}
fn supported_version(value: &str) -> bool {
    ["2.54.0", "2.56.0"].into_iter().any(|version| {
        value == version
            || value
                .strip_prefix(version)
                .and_then(|rest| rest.strip_prefix(" (Apple Git-")?.strip_suffix(')'))
                .is_some_and(|number| {
                    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
                })
    })
}
struct Output {
    code: i32,
    stdout: Vec<u8>,
}
impl Output {
    fn success(self) -> Result<Vec<u8>, ApiError> {
        if self.code == 0 {
            Ok(self.stdout)
        } else {
            Err(git_failed())
        }
    }
}
async fn watch_stop(receiver: &mut watch::Receiver<bool>) {
    loop {
        if *receiver.borrow() || receiver.changed().await.is_err() {
            return;
        }
    }
}
async fn read_output(
    mut stream: impl AsyncRead + Unpin,
    keep: bool,
    maximum: usize,
) -> Result<Vec<u8>, ApiError> {
    let mut bytes = Vec::new();
    let mut total = 0usize;
    let mut buffer = [0u8; 8192];
    loop {
        let n = stream.read(&mut buffer).await.map_err(|_| git_failed())?;
        if n == 0 {
            return Ok(bytes);
        }
        total = total.checked_add(n).ok_or_else(limit)?;
        if total > maximum {
            return Err(limit());
        }
        if keep {
            bytes.extend_from_slice(&buffer[..n]);
        }
    }
}

#[cfg(test)]
#[path = "../project_git_fixture.rs"]
mod fixture;

#[cfg(test)]
#[path = "../project_git_tests.rs"]
mod tests;
