//! Explicit local source versions in the project's own standard Git repository.
//! No database snapshot, network access, checkout, global identity, user hook or
//! content filter is part of this slice. Paths reported here describe a bounded
//! save scope, never authorization to move a repository or replace user data.
use crate::{
    builds::{atomic_json, BuildManager},
    environment::ExecutableInfo,
    git_environment::{GitEnvironmentManager, GitToolchain},
    projects::{check_no_symlinks, checked_relative},
    ApiError, ProjectHandle, ProjectManifest, ProjectStore, RuntimePaths, TaskRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
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
    process::{Child, Command},
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

#[path = "node_development_git.rs"]
pub mod node_development_git;

#[path = "project_git_build_guard.rs"]
mod build_guard;
pub(crate) use build_guard::BuildGitGuard;

#[path = "project_git_restore.rs"]
mod source_restore;
pub use source_restore::{
    GitSourceRestoreApplyInput, GitSourceRestorePreview, GitSourceRestorePreviewInput,
    GitSourceRestoreRecoverInput, GitSourceRestoreRecoveryStatus, RecoveryChange,
    RecoveryOperation, RestoreAction, RestoreChange, RestoreDescriptor,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitInitInput {
    pub expected_snapshot: String,
    pub initial_branch: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitAuthor {
    pub name: String,
    pub email: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitSaveInput {
    #[serde(deserialize_with = "required_head")]
    pub expected_head: Option<String>,
    pub expected_snapshot: String,
    pub message: String,
    pub author: GitAuthor,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitCreateBranchInput {
    pub name: String,
    pub expected_head: String,
    pub expected_branch: String,
    pub expected_snapshot: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitVersion {
    pub oid: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author: GitAuthor,
    pub authored_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGitVersions {
    pub repository_root: PathBuf,
    pub head: GitHead,
    pub versions: Vec<GitVersion>,
    pub has_more: bool,
    pub limit: u32,
    pub git: ExecutableInfo,
    pub database: Value,
}
fn required_head<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHead {
    pub oid: Option<String>,
    pub branch: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitMarker {
    pub path: PathBuf,
    pub kind: String,
    pub scope: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepositoryState {
    pub state: String,
    pub root: Option<PathBuf>,
    pub markers: Vec<GitMarker>,
    pub head: Option<GitHead>,
    pub index_state: String,
    pub git: Option<ExecutableInfo>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitExcludedPath {
    pub path: String,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSourceScope {
    pub policy: String,
    pub included_roots: Vec<String>,
    pub included_paths: Vec<String>,
    pub excluded: Vec<GitExcludedPath>,
    pub unclassified_paths: Vec<String>,
    pub protected_tracked_paths: Vec<String>,
    pub complete: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitChange {
    pub path: String,
    pub kind: String,
    pub staged: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGitStatus {
    pub repository: GitRepositoryState,
    pub scope: GitSourceScope,
    pub changes: Vec<GitChange>,
    pub snapshot_digest: String,
    pub can_initialize: bool,
    pub can_save: bool,
    pub can_list_versions: bool,
    pub can_create_branch: bool,
    pub branch_creation_issue: Option<ApiError>,
    pub source_restore: crate::source_restore_guard::SourceRestoreState,
    pub database: Value,
    pub issues: Vec<ApiError>,
}
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
}
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
        let _operation = self
            .store
            .reserve_transient_operation(project_id, "git-status")?;
        let project = self.store.project(project_id)?;
        let (_sender, cancel) = watch::channel(false);
        let mut context = Context::new(stop, cancel)?;
        context.checkpoint()?;
        let _gate = tokio::select! {
            biased;
            _ = watch_stop(&mut context.stop) => return Err(interrupted()),
            _ = tokio::time::sleep_until(context.deadline) => return Err(error("PROJECT_GIT_TIMEOUT", "等待项目读取窗口超过内部时间限制。")),
            gate = project.operation_gate.read() => gate,
        };
        project.check()?;
        let tools = context.resolve(&self.git).await;
        let mut snapshot = inspect(&project, tools.as_ref().ok(), &mut context, None).await?;
        if let Err(error) = tools {
            snapshot.view.issues.push(error);
        }
        self.add_recovery_issue(project_id, &mut snapshot.view)?;
        snapshot.view.source_restore =
            crate::source_restore_guard::inspect(self.paths.workspace_root(), project_id);
        finalize_capabilities(&mut snapshot.view);
        Ok(snapshot.view)
    }
    pub async fn initialize(
        self: &Arc<Self>,
        project_id: &str,
        input: GitInitInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        validate_digest(&input.expected_snapshot)?;
        if input.initial_branch != "main" {
            return Err(error(
                "PROJECT_GIT_INPUT_INVALID",
                "首次初始化仅支持明确的 main 分支。",
            ));
        }
        self.submit(project_id, Operation::Initialize(input), stop)
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
        let _operation = self
            .store
            .reserve_transient_operation(project_id, "git-versions")?;
        let project = self.store.project(project_id)?;
        let (_sender, cancel) = watch::channel(false);
        let mut context = Context::new(stop, cancel)?;
        context.checkpoint()?;
        let _gate = tokio::select! {
            biased;
            _ = watch_stop(&mut context.stop) => return Err(interrupted()),
            _ = tokio::time::sleep_until(context.deadline) => return Err(error("PROJECT_GIT_TIMEOUT", "等待项目读取窗口超过内部时间限制。")),
            gate = project.operation_gate.read() => gate,
        };
        let tools = context.resolve(&self.git).await?;
        let mut snapshot = inspect(&project, Some(&tools), &mut context, None).await?;
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
        let verify = inspect(&project, Some(&tools), &mut context, None).await?;
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
    pub async fn create_branch(
        self: &Arc<Self>,
        project_id: &str,
        input: GitCreateBranchInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        validate_digest(&input.expected_snapshot)?;
        if !valid_oid(&input.expected_head)
            || validate_branch_name(&input.name).is_err()
            || one_ref(format!("{}\n", input.expected_branch).as_bytes()).is_err()
        {
            return Err(input_error());
        }
        self.submit(project_id, Operation::CreateBranch(input), stop)
    }
    pub async fn save(
        self: &Arc<Self>,
        project_id: &str,
        input: GitSaveInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        validate_digest(&input.expected_snapshot)?;
        if input
            .expected_head
            .as_ref()
            .is_some_and(|oid| !valid_oid(oid))
        {
            return Err(input_error());
        }
        if input.message.trim().is_empty()
            || input.message.len() > 4000
            || input
                .message
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return Err(input_error());
        }
        if input.author.name.trim().is_empty()
            || input.author.name.len() > 200
            || input
                .author
                .name
                .chars()
                .any(|c| c.is_control() || "<>".contains(c))
            || input.author.email.len() > 254
            || !input.author.email.contains('@')
            || input
                .author
                .email
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || "<>".contains(c))
        {
            return Err(input_error());
        }
        self.submit(project_id, Operation::Save(input), stop)
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
            Operation::Initialize(_) => "gitInitialize",
            Operation::Save(_) => "gitSave",
            Operation::CreateBranch(_) => "gitBranchCreate",
            Operation::RestoreApply(_) => "gitSourceRestoreApply",
            Operation::RestoreRecover(ref input) => input.task_kind(),
        };
        let (task, lease) = if let Operation::RestoreRecover(input) = &operation {
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
            let _lease = lease;
            let branch_operation = matches!(operation, Operation::CreateBranch(_));
            let restore_operation = matches!(
                operation,
                Operation::RestoreApply(_) | Operation::RestoreRecover(_)
            );
            let recovery_task = match &operation {
                Operation::RestoreRecover(input) => Some(input.restore_task_id.clone()),
                _ => None,
            };
            let result = manager
                .run(&project_id, &task_id, operation, stop, cancel)
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
                    if (branch_operation || restore_operation)
                        && error.details.get("operationOutcome").is_none()
                    {
                        error.outcome("not_committed")
                    } else {
                        error
                    }
                });
            if let Err(error) = &result {
                let _ = manager.tasks.append(
                    &task_id,
                    format!(
                        "{}: {}{}\n",
                        error.code,
                        error.message,
                        if branch_operation {
                            format!(
                                " [branch outcome: {}]",
                                error.details["operationOutcome"]
                                    .as_str()
                                    .unwrap_or("unknown")
                            )
                        } else {
                            String::new()
                        }
                    )
                    .as_bytes(),
                );
            }
            let _ = manager.tasks.finish(&task_id, result);
            manager.active.lock().unwrap().remove(&task_id);
        });
        let mut runners = self.runners.lock().unwrap();
        runners.retain(|r| !r.is_finished());
        runners.push(runner);
        Ok(task)
    }
    pub fn cancel(&self, task_id: &str) -> Result<TaskRecord, ApiError> {
        let task = self.tasks.task(task_id)?;
        if !matches!(
            task.task_type.as_str(),
            "gitInitialize"
                | "gitSave"
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
    pub async fn shutdown(&self) {
        {
            let _submission = self.submission.lock().unwrap();
            self.stopping.store(true, Ordering::Release);
            for sender in self.active.lock().unwrap().values() {
                let _ = sender.send(true);
            }
        }
        let runners = std::mem::take(&mut *self.runners.lock().unwrap());
        for runner in runners {
            let _ = runner.await;
        }
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
        stop: watch::Receiver<bool>,
        cancel: watch::Receiver<bool>,
    ) -> Result<Value, ApiError> {
        let project = self.store.project(project_id)?;
        let mut context = Context::new(stop, cancel)?;
        context.checkpoint()?;
        let _gate = tokio::select! {
            biased;
            _ = watch_stop(&mut context.stop) => return Err(interrupted()),
            _ = watch_stop(&mut context.cancel) => return Err(interrupted()),
            _ = tokio::time::sleep_until(context.deadline) => return Err(error("PROJECT_GIT_TIMEOUT", "等待项目读取窗口超过内部时间限制。")),
            gate = project.operation_gate.read() => gate,
        };
        project.check()?;
        self.store.control(project_id)?.check(false, None, false)?;
        self.tasks.phase(task_id, "git-preview")?;
        let operation = match operation {
            Operation::RestoreApply(input) => {
                return self
                    .source_restore_apply_inner(&project, task_id, input, &mut context)
                    .await
            }
            Operation::RestoreRecover(input) => {
                return self
                    .source_restore_recover_inner(&project, task_id, input, &mut context)
                    .await
            }
            other => other,
        };
        let tools = context.resolve(&self.git).await?;
        let mut snapshot = inspect(&project, Some(&tools), &mut context, None).await?;
        self.add_recovery_issue(project_id, &mut snapshot.view)?;
        finalize_capabilities(&mut snapshot.view);
        match operation {
            Operation::Initialize(input) => {
                self.initialize_inner(&project, task_id, input, snapshot, &tools, &mut context)
                    .await
            }
            Operation::Save(input) => {
                self.save_inner(&project, task_id, input, snapshot, &tools, &mut context)
                    .await
            }
            Operation::CreateBranch(input) => {
                self.create_branch_inner(&project, task_id, input, snapshot, &tools, &mut context)
                    .await
            }
            Operation::RestoreApply(_) | Operation::RestoreRecover(_) => {
                unreachable!("restore operations are dispatched before Git resolution")
            }
        }
    }
    async fn initialize_inner(
        &self,
        project: &ProjectHandle,
        task_id: &str,
        input: GitInitInput,
        snapshot: Snapshot,
        tools: &GitToolchain,
        context: &mut Context,
    ) -> Result<Value, ApiError> {
        if snapshot.view.snapshot_digest != input.expected_snapshot {
            return Err(stale());
        }
        if !snapshot.view.can_initialize {
            return Err(blocked(&snapshot.view));
        }
        self.tasks.phase(task_id, "git-initialize-candidate")?;
        let operation_root = self
            .journal_path(&project.manifest.id, task_id)
            .with_extension("files");
        private_dir(&operation_root)?;
        let candidate = operation_root.join("repository");
        let template = context.root().join("empty-template");
        private_dir(&template)?;
        let mut command = git_command(tools, context.root(), None);
        command
            .args([
                "init",
                "--quiet",
                "--initial-branch=main",
                "--object-format=sha1",
                "--template",
            ])
            .arg(&template)
            .arg(&candidate);
        context.run(command, Vec::new()).await?.success()?;
        // This owned metadata reinforces external git add's defaults. It is not
        // the enforcement mechanism: status/save inspect HEAD and index as well.
        let output_pattern: String = project
            .manifest
            .output_dir
            .chars()
            .flat_map(|ch| {
                if "\\!#[]*? ".contains(ch) {
                    vec!['\\', ch]
                } else {
                    vec![ch]
                }
            })
            .collect();
        let excludes = format!("/data/\n/.platform/\n/{output_pattern}/\n**/node_modules/\n**/.env*\n**/.npmrc\n**/.netrc\n**/credentials/\n**/configuration/\n**/.ssh/\n**/id_rsa\n**/id_ed25519\n**/id_ecdsa\n**/id_dsa\n**/*.pem\n**/*.key\n**/*.p12\n**/*.pfx\n**/.cache/\n**/.vite/\n**/.hg/\n**/.svn/\n**/.DS_Store\n");
        private_dir(&candidate.join(".git/info"))?;
        fs::write(candidate.join(".git/info/exclude"), excludes)?;
        sync_ordinary_tree(&candidate.join(".git"))?;
        let verify = inspect(project, Some(tools), context, None).await?;
        if verify.view.snapshot_digest != input.expected_snapshot {
            return Err(stale());
        }
        context.checkpoint()?;
        project.check()?;
        let journal_path = self.journal_path(&project.manifest.id, task_id);
        let mut journal = Journal {
            schema_version: 1,
            project_id: project.manifest.id.clone(),
            task_id: task_id.into(),
            operation: "initialize".into(),
            phase: "prepared".into(),
            old_head: None,
            new_head: None,
            branch: Some("refs/heads/main".into()),
            previous_branch: None,
            snapshot_digest: input.expected_snapshot,
            paths: snapshot.view.scope.included_paths.clone(),
        };
        atomic_json(&journal_path, &journal)?;
        // No-overwrite rename is the commit point. Existing .git of any kind wins.
        if let Err(error) = rename_new(&candidate.join(".git"), &project.root.join(".git")) {
            journal.phase = "not-committed".into();
            let _ = atomic_json(&journal_path, &journal);
            return Err(error);
        }
        let result = json!({"operation":"initialize","operationOutcome":"committed","repositoryRoot":project.root,"branch":"main","head":null,"scope":snapshot.view.scope,"database":database_boundary(),"git":tools.git,"completedAt":now()});
        if File::open(&project.root)
            .and_then(|file| file.sync_all())
            .is_err()
        {
            return Err(recovery().outcome("committed").detail("result", result));
        }
        journal.phase = "completed".into();
        atomic_json(&journal_path, &journal).map_err(|_| {
            recovery()
                .outcome("committed")
                .detail("result", result.clone())
        })?;
        // Remove only this operation's now-empty staging directory; no historical
        // or interrupted evidence is discovered and deleted here.
        let _ = fs::remove_dir_all(operation_root);
        Ok(result)
    }
    async fn save_inner(
        &self,
        project: &ProjectHandle,
        task_id: &str,
        input: GitSaveInput,
        snapshot: Snapshot,
        tools: &GitToolchain,
        context: &mut Context,
    ) -> Result<Value, ApiError> {
        if snapshot.view.snapshot_digest != input.expected_snapshot
            || head_oid(&snapshot.view) != input.expected_head.as_deref()
        {
            return Err(stale());
        }
        if !snapshot.view.can_save {
            return Err(blocked(&snapshot.view));
        }
        if snapshot.view.changes.is_empty() {
            return Err(error(
                "PROJECT_GIT_NO_CHANGES",
                "当前纳入范围没有可保存的改动；未创建空版本。",
            ));
        }
        let branch = snapshot
            .view
            .repository
            .head
            .as_ref()
            .and_then(|head| head.branch.clone())
            .ok_or_else(|| {
                error(
                    "PROJECT_GIT_DETACHED",
                    "当前处于分离 HEAD；本步不会选择或移动分支。",
                )
            })?;
        let git_dir = project.root.join(".git");
        let mut lock = IndexLock::acquire(&git_dir.join("index.lock"))?;
        let verify = inspect(project, Some(tools), context, Some(&lock)).await?;
        if verify.view.snapshot_digest != input.expected_snapshot {
            return Err(stale());
        }
        self.tasks.phase(task_id, "git-source-snapshot")?;
        let operation_root = self
            .journal_path(&project.manifest.id, task_id)
            .with_extension("files");
        private_dir(&operation_root)?;
        let source = operation_root.join("source");
        private_dir(&source)?;
        for (path, entry) in &snapshot.files {
            context.checkpoint()?;
            let bytes = read_file(&checked_relative(&project.root, path, false)?, MAX_BLOB)?;
            if sha256(&bytes) != entry.digest {
                return Err(stale());
            }
            let destination = source.join(path);
            private_dir(destination.parent().unwrap())?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(if entry.mode == "100755" { 0o700 } else { 0o600 })
                .open(&destination)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        self.tasks.phase(task_id, "git-write-objects")?;
        let index = operation_root.join("index");
        let mut command = git_command(tools, context.root(), Some(project));
        command
            .env("GIT_INDEX_FILE", &index)
            .args(["read-tree", "--empty"]);
        context.run(command, Vec::new()).await?.success()?;
        let hashes = hash_paths(
            tools,
            context,
            Some(project),
            &source,
            snapshot.files.keys(),
            true,
        )
        .await?;
        let mut index_input = Vec::new();
        for (path, entry) in &snapshot.files {
            let oid = hashes.get(path).ok_or_else(output_invalid)?;
            index_input.extend_from_slice(format!("{} {}\t", entry.mode, oid).as_bytes());
            index_input.extend_from_slice(path.as_bytes());
            index_input.push(0);
        }
        let mut command = git_command(tools, context.root(), Some(project));
        command
            .env("GIT_INDEX_FILE", &index)
            .args(["update-index", "-z", "--index-info"]);
        context.run(command, index_input).await?.success()?;
        let mut command = git_command(tools, context.root(), Some(project));
        command.env("GIT_INDEX_FILE", &index).arg("write-tree");
        let tree = one_oid(&context.run(command, Vec::new()).await?.success()?)?;
        let mut command = git_command(tools, context.root(), Some(project));
        command
            .env("GIT_INDEX_FILE", &index)
            .args(["commit-tree", &tree, "--no-gpg-sign"]);
        if let Some(parent) = &input.expected_head {
            command.args(["-p", parent]);
        }
        author_environment(&mut command, &input.author);
        let new_head = one_oid(
            &context
                .run(command, input.message.as_bytes().to_vec())
                .await?
                .success()?,
        )?;
        let verify = inspect(project, Some(tools), context, Some(&lock)).await?;
        if verify.view.snapshot_digest != input.expected_snapshot {
            return Err(stale());
        }
        project.check()?;
        context.checkpoint()?;
        let index_bytes = read_file(&index, MAX_METADATA)?;
        lock.write(&index_bytes)?;
        let journal_path = self.journal_path(&project.manifest.id, task_id);
        let mut journal = Journal {
            schema_version: 1,
            project_id: project.manifest.id.clone(),
            task_id: task_id.into(),
            operation: "save".into(),
            phase: "prepared".into(),
            old_head: input.expected_head.clone(),
            new_head: Some(new_head.clone()),
            branch: Some(branch.clone()),
            previous_branch: None,
            snapshot_digest: input.expected_snapshot.clone(),
            paths: snapshot.files.keys().cloned().collect(),
        };
        atomic_json(&journal_path, &journal)?;
        self.tasks.phase(task_id, "git-commit")?;
        // Git adds HEAD reflog updates itself, so an explicit symref-verify HEAD
        // alongside a branch update is rejected as a duplicate reference. Prepare
        // an update through HEAD instead, then inspect its symbolic target while
        // Git holds both HEAD and branch locks, before authorizing commit.
        let mut command = git_command(tools, context.root(), Some(project));
        command.args([
            "update-ref",
            "--create-reflog",
            "-m",
            "Local Projects: save source",
            "--stdin",
            "-z",
        ]);
        author_environment(&mut command, &input.author);
        lock.preserve = true;
        let failure = context
            .commit_ref(command, &new_head, input.expected_head.as_deref(), || {
                if read_file(&git_dir.join("HEAD"), 4096)? != format!("ref: {branch}\n").as_bytes()
                    || repository_digest(&git_dir)? != snapshot.repository_digest
                    || source_fingerprint(&scan_source(
                        &project.root,
                        &project.manifest,
                        &snapshot.view.scope.included_roots,
                    )?)? != snapshot.source_digest
                {
                    return Err(stale());
                }
                project.check()
            })
            .await;
        if let Err(error) = failure {
            if error.details["safeAbort"] == true {
                journal.phase = "not-committed".into();
                atomic_json(&journal_path, &journal)?;
                lock.preserve = false;
                return Err(error.outcome("not_committed"));
            }
            return Err(error
                .outcome("unknown")
                .detail("taskId", task_id.to_owned())
                .detail("candidateCommit", new_head));
        }
        journal.phase = "ref-committed".into();
        let result = json!({"operation":"save","operationOutcome":"committed","commit":new_head,"parent":input.expected_head,"branch":branch.strip_prefix("refs/heads/").unwrap_or(&branch),"message":input.message,"author":input.author,"scope":snapshot.view.scope,"paths":journal.paths,"changes":snapshot.view.changes,"snapshotDigest":input.expected_snapshot,"database":database_boundary(),"git":tools.git,"completedAt":now()});
        // After the ref commit point, cancellation must not skip index publication.
        if atomic_json(&journal_path, &journal).is_err()
            || lock.publish(&git_dir.join("index")).is_err()
        {
            return Err(recovery().outcome("committed").detail("result", result));
        }
        journal.phase = "completed".into();
        atomic_json(&journal_path, &journal).map_err(|_| {
            recovery()
                .outcome("committed")
                .detail("result", result.clone())
        })?;
        let _ = fs::remove_dir_all(operation_root);
        Ok(result)
    }
    async fn create_branch_inner(
        &self,
        project: &ProjectHandle,
        task_id: &str,
        input: GitCreateBranchInput,
        snapshot: Snapshot,
        tools: &GitToolchain,
        context: &mut Context,
    ) -> Result<Value, ApiError> {
        if !branch_version_supported(&tools.git.version) {
            return Err(branch_version_issue());
        }
        if snapshot.view.snapshot_digest != input.expected_snapshot
            || head_oid(&snapshot.view) != Some(input.expected_head.as_str())
            || snapshot
                .view
                .repository
                .head
                .as_ref()
                .and_then(|head| head.branch.as_deref())
                != Some(input.expected_branch.as_str())
        {
            return Err(stale());
        }
        if !snapshot.view.can_create_branch {
            return Err(blocked(&snapshot.view));
        }
        let git_dir = project.root.join(".git");
        let index_lock = IndexLock::acquire(&git_dir.join("index.lock"))?;
        let verify = inspect(project, Some(tools), context, Some(&index_lock)).await?;
        if verify.view.snapshot_digest != input.expected_snapshot {
            return Err(stale());
        }
        let new_branch = format!("refs/heads/{}", input.name);
        let mut command = git_command(tools, context.root(), Some(project));
        command.args(["show-ref", "--verify", "--quiet", &new_branch]);
        match context.run(command, Vec::new()).await?.code {
            1 => (),
            0 => {
                return Err(error(
                    "PROJECT_GIT_BRANCH_EXISTS",
                    "此分支已存在；不会覆盖或切换已有分支。",
                ))
            }
            _ => return Err(git_failed()),
        }
        // symref-update HEAD locks HEAD and the new branch. A simultaneous verify
        // of the old branch is rejected by Git as a duplicate HEAD reflog update.
        // Hold the old branch's standard lock ourselves, and recheck its OID.
        // The existing index is only locked; it is never refreshed or published.
        let old_lock = RefLock::acquire(&git_dir.join(format!("{}.lock", input.expected_branch)))?;
        let mut command = git_command(tools, context.root(), Some(project));
        command.args(["rev-parse", "--verify", &input.expected_branch]);
        if one_oid(&context.run(command, Vec::new()).await?.success()?)? != input.expected_head {
            return Err(stale());
        }
        let journal_path = self.journal_path(&project.manifest.id, task_id);
        let mut journal = Journal {
            schema_version: 1,
            project_id: project.manifest.id.clone(),
            task_id: task_id.into(),
            operation: "create-branch".into(),
            phase: "prepared".into(),
            old_head: Some(input.expected_head.clone()),
            new_head: Some(input.expected_head.clone()),
            branch: Some(new_branch.clone()),
            previous_branch: Some(input.expected_branch.clone()),
            snapshot_digest: input.expected_snapshot.clone(),
            paths: vec![],
        };
        atomic_json(&journal_path, &journal)?;
        self.tasks.phase(task_id, "git-branch-commit")?;
        let mut command = git_command(tools, context.root(), Some(project));
        command.args([
            "update-ref",
            "--create-reflog",
            "-m",
            "Local Projects: create customization branch",
            "--stdin",
            "-z",
        ]);
        // This identity labels only the local reflog; no authored commit is made.
        author_environment(
            &mut command,
            &GitAuthor {
                name: "Local Projects".into(),
                email: "local-projects@localhost".into(),
            },
        );
        let commands = format!(
            "start\0create {new_branch}\0{}\0option no-deref\0symref-update HEAD\0{new_branch}\0oid\0{}\0prepare\0",
            input.expected_head, input.expected_head,
        ).into_bytes();
        let failure = context
            .commit_transaction(command, commands, || {
                if !old_lock.matches()
                    || !index_lock.matches(&index_lock.path)
                    || index_fingerprint(&git_dir.join("index"))? != index_lock.original_index
                    || read_file(&git_dir.join("HEAD"), 4096)?
                        != format!("ref: {}\n", input.expected_branch).as_bytes()
                    || repository_digest(&git_dir)? != snapshot.repository_digest
                    || source_fingerprint(&scan_source(
                        &project.root,
                        &project.manifest,
                        &snapshot.view.scope.included_roots,
                    )?)? != snapshot.source_digest
                {
                    return Err(stale());
                }
                project.check()
            })
            .await;
        if let Err(error) = failure {
            if error.details["safeAbort"] == true {
                journal.phase = "not-committed".into();
                atomic_json(&journal_path, &journal)?;
                self.tasks.append(
                    task_id,
                    b"New branch was not committed; source, index and data were not replaced.\n",
                )?;
                return Err(error.outcome("not_committed"));
            }
            let _ = self.tasks.append(task_id, b"Branch transaction outcome is unknown; inspect the retained journal before another write.\n");
            return Err(error
                .outcome("unknown")
                .detail("taskId", task_id.to_owned())
                .detail("branch", new_branch));
        }
        // The acknowledged Git ref transaction is the commit point. Cancellation
        // after it cannot relabel the branch change as an uncommitted operation.
        let result = json!({"operation":"create-branch","operationOutcome":"committed","repositoryRoot":project.root,"head":input.expected_head,"previousBranch":input.expected_branch,"branch":new_branch,"name":input.name,"snapshotDigest":input.expected_snapshot,"database":database_boundary(),"git":tools.git,"completedAt":now()});
        journal.phase = "ref-committed".into();
        atomic_json(&journal_path, &journal).map_err(|_| {
            recovery()
                .outcome("committed")
                .detail("result", result.clone())
        })?;
        let verify_committed = || -> Result<(), ApiError> {
            if read_file(&git_dir.join("HEAD"), 4096)? != format!("ref: {new_branch}\n").as_bytes()
                || one_oid(&read_file(&git_dir.join(&new_branch), 4096)?)? != input.expected_head
                || index_fingerprint(&git_dir.join("index"))? != index_lock.original_index
                || source_fingerprint(&scan_source(
                    &project.root,
                    &project.manifest,
                    &snapshot.view.scope.included_roots,
                )?)? != snapshot.source_digest
            {
                return Err(recovery());
            }
            Ok(())
        };
        verify_committed().map_err(|_| {
            recovery()
                .outcome("committed")
                .detail("result", result.clone())
        })?;
        journal.phase = "completed".into();
        atomic_json(&journal_path, &journal).map_err(|_| {
            recovery()
                .outcome("committed")
                .detail("result", result.clone())
        })?;
        let _ = self.tasks.append(task_id, b"Created and switched to the new branch at the existing HEAD; source, index, data and output were not replaced.\n");
        Ok(result)
    }
}
enum Operation {
    Initialize(GitInitInput),
    Save(GitSaveInput),
    CreateBranch(GitCreateBranchInput),
    RestoreApply(GitSourceRestoreApplyInput),
    RestoreRecover(GitSourceRestoreRecoverInput),
}

#[derive(Clone, Debug, Serialize)]
struct SourceFile {
    mode: String,
    digest: String,
    size: u64,
}
struct Snapshot {
    view: ProjectGitStatus,
    files: BTreeMap<String, SourceFile>,
    source_digest: String,
    repository_digest: String,
}
fn database_boundary() -> Value {
    json!({"state":"not-supported","reason":"本次只保存列出的开发文件；活跃数据库、附件及数据快照均未纳入版本。"})
}
fn head_oid(view: &ProjectGitStatus) -> Option<&str> {
    view.repository
        .head
        .as_ref()
        .and_then(|head| head.oid.as_deref())
}
fn finalize_capabilities(view: &mut ProjectGitStatus) {
    let common = view.repository.git.is_some() && view.scope.complete && view.issues.is_empty();
    view.can_initialize = common && view.repository.state == "none";
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
    view.branch_creation_issue = view
        .repository
        .git
        .as_ref()
        .filter(|git| !branch_version_supported(&git.version))
        .map(|_| branch_version_issue());
    view.can_create_branch = view.can_save
        && head_oid(view).is_some()
        && view.changes.is_empty()
        && view.branch_creation_issue.is_none();
    if view.source_restore.check().is_err() {
        view.can_initialize = false;
        view.can_save = false;
        view.can_create_branch = false;
    }
}
async fn inspect(
    project: &ProjectHandle,
    tools: Option<&GitToolchain>,
    context: &mut Context,
    owned_lock: Option<&IndexLock>,
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
        database: database_boundary(),
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
        inspect_repository_files(&git_dir, owned_lock, &mut view.issues)?;
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
                } else if !included(path, &view.scope.included_roots) {
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
        &json!({"policy":POLICY,"root":project.root,"rootIdentity":identity(&project.root)?,"manifest":manifest,"repository":view.repository,"repositoryDigest":repo_digest,"index":index_entries,"head":head_entries,"files":scan.files,"scope":view.scope,"sourceFingerprint":source_fingerprint(&after)?}),
    )?);
    finalize_capabilities(&mut view);
    Ok(Snapshot {
        view,
        files: scan.files,
        source_digest: initial_source,
        repository_digest: repo_digest,
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
/// Detect repositories in the same development scope used by source versions,
/// without reading Git metadata or file contents. Legacy whole-project restore
/// must preserve nested repositories as well as the platform's root repository.
pub(crate) fn development_git_marker(
    root: &Path,
    manifest: &ProjectManifest,
) -> Result<Option<PathBuf>, ApiError> {
    let roots = included_roots(manifest);
    let mut pending = vec![PathBuf::new()];
    let mut count = 0;
    while let Some(relative) = pending.pop() {
        check_no_symlinks(&root.join(&relative), false)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(root.join(&relative))? {
            count += 1;
            if count > MAX_FILES * 2 {
                return Err(limit());
            }
            let entry = entry?;
            let name = entry.file_name().into_string().map_err(|_| unsafe_path())?;
            if name.chars().any(char::is_control) || name.contains('\\') {
                return Err(unsafe_path());
            }
            entries.push((name, entry.path()));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, location) in entries {
            let path = relative.join(&name);
            let label = path.to_str().ok_or_else(unsafe_path)?;
            // This matches scan_source's marker-before-exclusion rule. Parents
            // such as data and node_modules are excluded before being visited.
            if name.eq_ignore_ascii_case(".git") {
                return Ok(Some(location));
            }
            if protected(label, manifest).is_some()
                || (!included(label, &roots) && !is_ancestor(label, &roots))
            {
                continue;
            }
            let metadata = fs::symlink_metadata(&location)?;
            if metadata.is_dir() {
                pending.push(path);
            } else if !metadata.is_file() || metadata.nlink() != 1 {
                return Err(unsafe_path().detail("path", label));
            }
        }
    }
    Ok(None)
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
fn inspect_repository_files(
    root: &Path,
    owned_lock: Option<&IndexLock>,
    issues: &mut Vec<ApiError>,
) -> Result<(), ApiError> {
    check_no_symlinks(root, false)?;
    let mut count = 0usize;
    fn visit(
        root: &Path,
        path: &Path,
        owned: Option<&IndexLock>,
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
                visit(root, &path, owned, issues, count, depth + 1)?;
            } else if !meta.is_file() || meta.nlink() != 1 {
                return Err(unsafe_path());
            }
            let relative = path.strip_prefix(root).map_err(|_| unsafe_path())?;
            let label = relative.to_str().ok_or_else(unsafe_path)?;
            if label.ends_with(".lock") && !owned.is_some_and(|lock| lock.matches(&path)) {
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
    visit(root, root, owned_lock, issues, &mut count, 0)
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

struct IndexLock {
    path: PathBuf,
    file: File,
    dev: u64,
    ino: u64,
    preserve: bool,
    published: bool,
    original_index: Option<String>,
    prepared_digest: Option<String>,
}
impl IndexLock {
    fn acquire(path: &Path) -> Result<Self, ApiError> {
        check_no_symlinks(path, true)?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| {
                error(
                    "PROJECT_GIT_LOCKED",
                    "无法取得 Git 索引锁；不会删除已有锁文件。",
                )
            })?;
        let metadata = file.metadata()?;
        let mut lock = Self {
            path: path.to_path_buf(),
            file,
            dev: metadata.dev(),
            ino: metadata.ino(),
            preserve: false,
            published: false,
            original_index: None,
            prepared_digest: None,
        };
        lock.original_index = index_fingerprint(&path.with_file_name("index"))?;
        Ok(lock)
    }
    fn matches(&self, path: &Path) -> bool {
        path == self.path
            && fs::symlink_metadata(path).is_ok_and(|m| {
                m.is_file() && m.dev() == self.dev && m.ino() == self.ino && m.nlink() == 1
            })
    }
    fn write(&mut self, bytes: &[u8]) -> Result<(), ApiError> {
        if !self.matches(&self.path) {
            return Err(stale());
        }
        self.file.write_all(bytes)?;
        self.file.sync_all()?;
        self.prepared_digest = Some(sha256(bytes));
        Ok(())
    }
    fn publish(&mut self, index: &Path) -> Result<(), ApiError> {
        if !self.matches(&self.path) {
            return Err(recovery());
        }
        if index != self.path.with_file_name("index")
            || index_fingerprint(index)? != self.original_index
            || self.prepared_digest.as_deref()
                != Some(sha256(&read_file(&self.path, MAX_METADATA)?).as_str())
        {
            return Err(recovery());
        }
        fs::rename(&self.path, index)?;
        self.published = true;
        File::open(index.parent().unwrap())?.sync_all()?;
        Ok(())
    }
}
impl Drop for IndexLock {
    fn drop(&mut self) {
        if !self.preserve && !self.published && self.matches(&self.path) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct RefLock {
    path: PathBuf,
    _file: File,
    device: u64,
    inode: u64,
}
impl RefLock {
    fn acquire(path: &Path) -> Result<Self, ApiError> {
        check_no_symlinks(path, true)?;
        // Packed refs need not have loose parent directories yet. Create only
        // absent directories without altering permissions of existing Git paths.
        fs::create_dir_all(path.parent().ok_or_else(unsafe_path)?)?;
        check_no_symlinks(path, true)?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| {
                error(
                    "PROJECT_GIT_LOCKED",
                    "旧分支正被其他程序操作或无法安全锁定；未切换分支。",
                )
            })?;
        let metadata = file.metadata()?;
        Ok(Self {
            path: path.to_path_buf(),
            _file: file,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    fn matches(&self) -> bool {
        check_no_symlinks(&self.path, false).is_ok()
            && fs::symlink_metadata(&self.path).is_ok_and(|meta| {
                meta.is_file()
                    && meta.nlink() == 1
                    && meta.dev() == self.device
                    && meta.ino() == self.inode
            })
    }
}
impl Drop for RefLock {
    fn drop(&mut self) {
        if self.matches() {
            let _ = fs::remove_file(&self.path);
        }
    }
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
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
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
fn sync_ordinary_tree(root: &Path) -> Result<(), ApiError> {
    check_no_symlinks(root, false)?;
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            sync_ordinary_tree(&path)?;
        } else if metadata.is_file() && metadata.nlink() == 1 {
            File::open(path)?.sync_all()?;
        } else {
            return Err(unsafe_path());
        }
    }
    File::open(root)?.sync_all()?;
    Ok(())
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
fn validate_branch_name(value: &str) -> Result<(), ApiError> {
    if value.is_empty()
        || value.len() > 128
        || value == "HEAD"
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte))
        || one_ref(format!("refs/heads/{value}\n").as_bytes()).is_err()
    {
        Err(input_error())
    } else {
        Ok(())
    }
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

struct Context {
    _temporary: TempDir,
    root: PathBuf,
    stop: watch::Receiver<bool>,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
}
impl Context {
    fn new(stop: watch::Receiver<bool>, cancel: watch::Receiver<bool>) -> Result<Self, ApiError> {
        let temporary = tempfile::Builder::new()
            .prefix("local-projects-git-operation-")
            .tempdir()
            .map_err(|_| git_failed())?;
        let root = temporary.path().canonicalize().map_err(|_| unsafe_path())?;
        if root
            .to_str()
            .is_none_or(|path| path.contains(':') || path.chars().any(char::is_control))
        {
            return Err(unsafe_path());
        }
        private_dir(&root.join("empty-hooks"))?;
        private_dir(&root.join("home"))?;
        Ok(Self {
            _temporary: temporary,
            root,
            stop,
            cancel,
            deadline: Instant::now() + TIMEOUT,
        })
    }
    fn root(&self) -> &Path {
        &self.root
    }
    fn checkpoint(&self) -> Result<(), ApiError> {
        if *self.stop.borrow()
            || *self.cancel.borrow()
            || self.stop.has_changed().is_err()
            || self.cancel.has_changed().is_err()
        {
            return Err(interrupted());
        }
        if Instant::now() >= self.deadline {
            return Err(error(
                "PROJECT_GIT_TIMEOUT",
                "Git 本地操作超过 120 秒内部限制；结果以任务提交点记录为准。",
            ));
        }
        Ok(())
    }
    async fn resolve(
        &mut self,
        environment: &GitEnvironmentManager,
    ) -> Result<GitToolchain, ApiError> {
        self.checkpoint()?;
        let tools = tokio::select! {
            biased;
            _=watch_stop(&mut self.stop)=>return Err(interrupted()),
            _=watch_stop(&mut self.cancel)=>return Err(interrupted()),
            _=tokio::time::sleep_until(self.deadline)=>return Err(error("PROJECT_GIT_TIMEOUT","Git 工具核对超过内部时间限制。")),
            result=environment.resolve()=>result.map_err(|_|error("PROJECT_GIT_ENVIRONMENT_UNAVAILABLE","Git 工具不可用，请先检查工具设置；不会改用另一份 Git。"))?,
        };
        if !supported_version(&tools.git.version) {
            return Err(error(
                "PROJECT_GIT_VERSION_UNSUPPORTED",
                "本地版本操作目前只验证 Git 2.54.0 与 2.56.0；工具选择未改变。",
            ));
        }
        Ok(tools)
    }
    async fn commit_ref(
        &mut self,
        command: Command,
        new_head: &str,
        old_head: Option<&str>,
        validate: impl FnOnce() -> Result<(), ApiError> + Send,
    ) -> Result<(), ApiError> {
        let commands = format!(
            "start\0update HEAD\0{new_head}\0{}\0prepare\0",
            old_head.unwrap_or("0000000000000000000000000000000000000000")
        )
        .into_bytes();
        self.commit_transaction(command, commands, validate).await
    }
    async fn commit_transaction(
        &mut self,
        mut command: Command,
        commands: Vec<u8>,
        validate: impl FnOnce() -> Result<(), ApiError> + Send,
    ) -> Result<(), ApiError> {
        self.checkpoint()
            .map_err(|error| error.detail("safeAbort", true))?;
        let child = command
            .spawn()
            .map_err(|_| git_failed().detail("safeAbort", true))?;
        let mut process = Process {
            group: child.id(),
            child,
        };
        let mut stdin = process.child.stdin.take().ok_or_else(git_failed)?;
        let mut stdout = process.child.stdout.take().ok_or_else(git_failed)?;
        let stderr = process.child.stderr.take().ok_or_else(git_failed)?;
        let mut safe_abort = false;
        let mut commit_sent = false;
        let result = tokio::select! {
            biased;
            _ = watch_stop(&mut self.stop) => Err(interrupted()),
            _ = watch_stop(&mut self.cancel) => Err(interrupted()),
            _ = tokio::time::sleep_until(self.deadline) => Err(error("PROJECT_GIT_TIMEOUT", "Git 引用事务超过内部时间限制；已保留待核对现场。")),
            result = async {
                let (_, result) = tokio::try_join!(
                    read_output(stderr, false, 64 * 1024),
                    async {
                        stdin.write_all(&commands).await.map_err(|_| git_failed())?;
                        stdin.flush().await.map_err(|_| git_failed())?;
                        expect_line(&mut stdout, b"start: ok\n").await?;
                        expect_line(&mut stdout, b"prepare: ok\n").await?;
                        let validation = validate();
                        if let Err(error) = validation {
                            stdin.write_all(b"abort\0").await.map_err(|_| git_failed())?;
                            drop(stdin);
                            expect_line(&mut stdout, b"abort: ok\n").await?;
                            let remaining = read_output(stdout, true, 1024).await?;
                            let status = process.child.wait().await.map_err(|_| git_failed())?;
                            if !status.success() || !remaining.is_empty() { return Err(git_failed()); }
                            safe_abort = true;
                            return Ok::<Result<(), ApiError>, ApiError>(Err(error));
                        }
                        commit_sent = true;
                        stdin.write_all(b"commit\0").await.map_err(|_| git_failed())?;
                        drop(stdin);
                        expect_line(&mut stdout, b"commit: ok\n").await?;
                        let remaining = read_output(stdout, true, 1024).await?;
                        let status = process.child.wait().await.map_err(|_| git_failed())?;
                        if !status.success() || !remaining.is_empty() { return Err(git_failed()); }
                        Ok(Ok(()))
                    }
                )?;
                result
            } => result,
        };
        process.finish().await;
        // Until commit is sent, closing stdin/killing this private process aborts
        // Git's prepared transaction. After sending it, missing acknowledgement
        // is deliberately unknown and leaves the journal for human inspection.
        safe_abort |= !commit_sent;
        result.map_err(|error| {
            error
                .detail("safeAbort", safe_abort)
                .detail("commitSent", commit_sent)
        })
    }

    async fn run(&mut self, mut command: Command, input: Vec<u8>) -> Result<Output, ApiError> {
        self.checkpoint()?;
        if input.len() > MAX_OUTPUT {
            return Err(limit());
        }
        let child = command.spawn().map_err(|_| git_failed())?;
        let mut process = Process {
            group: child.id(),
            child,
        };
        let mut stdin = process.child.stdin.take().ok_or_else(git_failed)?;
        let stdout = process.child.stdout.take().ok_or_else(git_failed)?;
        let stderr = process.child.stderr.take().ok_or_else(git_failed)?;
        let result = tokio::select! {
            biased;
            _=watch_stop(&mut self.stop)=>Err(interrupted()),
            _=watch_stop(&mut self.cancel)=>Err(interrupted()),
            _=tokio::time::sleep_until(self.deadline)=>Err(error("PROJECT_GIT_TIMEOUT","Git 本地操作超过 120 秒内部限制；结果以任务提交点记录为准。")),
            result=async{
                let (_,stdout,_,status)=tokio::try_join!(
                    async{stdin.write_all(&input).await.map_err(|_|git_failed())?;drop(stdin);Ok::<(),ApiError>(())},
                    read_output(stdout,true,MAX_OUTPUT),read_output(stderr,false,64*1024),
                    async{process.child.wait().await.map_err(|_|git_failed())}
                )?;
                Ok(Output{code:status.code().unwrap_or(-1),stdout})
            }=>result,
        };
        process.finish().await;
        result
    }
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
        .kill_on_drop(true)
        .process_group(0);
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
fn author_environment(command: &mut Command, author: &GitAuthor) {
    command
        .env("GIT_AUTHOR_NAME", &author.name)
        .env("GIT_AUTHOR_EMAIL", &author.email)
        .env("GIT_COMMITTER_NAME", &author.name)
        .env("GIT_COMMITTER_EMAIL", &author.email);
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
fn branch_version_supported(value: &str) -> bool {
    value == "2.56.0"
}
fn branch_version_issue() -> ApiError {
    error("PROJECT_GIT_BRANCH_VERSION_UNSUPPORTED", "新建并切换分支的引用事务目前仅验证 Git 2.56.0；仍可使用当前 Git 查看版本和保存源码，不会自动换工具。")
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
        let _ = tokio::time::timeout(Duration::from_millis(250), self.child.wait()).await;
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}
async fn expect_line(
    stream: &mut (impl AsyncRead + Unpin),
    expected: &[u8],
) -> Result<(), ApiError> {
    let mut line = Vec::new();
    for _ in 0..1024 {
        let byte = stream.read_u8().await.map_err(|_| git_failed())?;
        line.push(byte);
        if byte == b'\n' {
            return if line == expected {
                Ok(())
            } else {
                Err(output_invalid())
            };
        }
    }
    Err(limit())
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
#[path = "project_git_tests.rs"]
mod tests;
