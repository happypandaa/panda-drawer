//! Source-only, explicitly recoverable worktree changes. These records are an
//! operation journal, not a second version history or a database backup.
use super::*;
use crate::source_restore_guard as guard;
use std::os::fd::AsRawFd;
use unicode_normalization::UnicodeNormalization;

const JOURNAL_LIMIT: u64 = 4 * 1024 * 1024;
const MIN_FREE: u64 = 64 * 1024 * 1024;
const RESTORE_POLICY: &str = "project-source-restore-v1";

impl GitSourceRestoreRecoverInput {
    pub(super) fn task_kind(&self) -> &'static str {
        match self.action {
            RestoreAction::Continue => "gitSourceRestoreContinue",
            RestoreAction::Undo => "gitSourceRestoreUndo",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stamp {
    descriptor: RestoreDescriptor,
    mode: u32,
    device: u64,
    inode: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlannedFile {
    oid: String,
    descriptor: RestoreDescriptor,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    root_identity: String,
    manifest_sha256: String,
    repository_digest: String,
    index: Option<String>,
    head: String,
    branch: String,
    branch_ref: Option<String>,
    metadata_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestorePlan {
    policy: String,
    project_id: String,
    target_commit: String,
    snapshot_digest: String,
    binding: Binding,
    before: BTreeMap<String, RestoreDescriptor>,
    target: BTreeMap<String, PlannedFile>,
    changes: Vec<RestoreChange>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreEntry {
    path: String,
    before: Option<Stamp>,
    after: Option<Stamp>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreJournal {
    schema_version: u32,
    operation: String,
    project_id: String,
    task_id: String,
    phase: String,
    plan_digest: String,
    plan: RestorePlan,
    entries: Vec<RestoreEntry>,
    git: ExecutableInfo,
}
impl ProjectGitManager {
    pub async fn source_restore_preview(
        &self,
        id: &str,
        input: GitSourceRestorePreviewInput,
        stop: watch::Receiver<bool>,
    ) -> Result<GitSourceRestorePreview, ApiError> {
        validate_target(&input.target_commit)?;
        self.available()?;
        let lease = Arc::new(
            self.store
                .reserve_transient_operation(id, "source-restore-preview")?,
        );
        guard::check_development_available(self.paths.workspace_root(), id)?;
        let project = self.store.project(id)?;
        let (_sender, cancel) = watch::channel(false);
        let mut context = Context::new(stop, cancel)?;
        context.bind(self.store.processes(), lease);
        let _gate = read_gate(&project, &mut context).await?;
        let tools = context.resolve(&self.git).await?;
        let plan = self
            .restore_plan(&project, &input.target_commit, &tools, &mut context)
            .await?;
        preview(&project, &plan, &tools)
    }
    pub async fn source_restore_apply(
        self: &Arc<Self>,
        id: &str,
        input: GitSourceRestoreApplyInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        validate_target(&input.target_commit)?;
        validate_target(&input.expected_head)?;
        validate_digest(&input.expected_snapshot)?;
        validate_digest(&input.expected_plan_digest)?;
        one_ref(format!("{}\n", input.expected_branch).as_bytes()).map_err(|_| input_error())?;
        self.submit(id, Operation::RestoreApply(input), stop)
    }
    pub async fn source_restore_recover(
        self: &Arc<Self>,
        id: &str,
        input: GitSourceRestoreRecoverInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        validate_digest(&input.expected_plan_digest)?;
        validate_digest(&input.expected_journal_digest)?;
        self.submit(id, Operation::RestoreRecover(input), stop)
    }
    pub async fn source_restore_status(
        &self,
        id: &str,
        stop: watch::Receiver<bool>,
    ) -> Result<GitSourceRestoreRecoveryStatus, ApiError> {
        self.available()?;
        let lease = Arc::new(
            self.store
                .reserve_transient_operation(id, "source-restore-status")?,
        );
        let project = self.store.project(id)?;
        let (_sender, cancel) = watch::channel(false);
        let mut context = Context::new(stop, cancel)?;
        context.bind(self.store.processes(), lease);
        let _gate = read_gate(&project, &mut context).await?;
        let state = guard::inspect(self.paths.workspace_root(), id);
        let blocked = !state.issues.is_empty() || state.pending.len() > 1;
        let mut result = GitSourceRestoreRecoveryStatus {
            state: "clear".into(),
            operations: vec![],
            issues: state.issues,
            database: database_boundary(),
        };
        for pending in state.pending {
            context.checkpoint()?;
            if blocked {
                result.operations.push(RecoveryOperation {
                    restore_task_id: pending.task_id,
                    target_commit: String::new(),
                    phase: pending.phase,
                    plan_digest: pending.plan_digest,
                    journal_digest: pending.journal_digest,
                    can_continue: false,
                    can_undo: false,
                    changes: vec![],
                    issues: vec![],
                });
                continue;
            }
            let root = self.restore_root(id, &pending.task_id);
            let inspected = load_journal(&root, id, &pending.task_id).and_then(|journal| {
                let changes = classify_all(&project, &root, &journal)?;
                let mut issues = Vec::new();
                for result in [
                    verify_binding(&project, &journal.plan.binding),
                    verify_no_git_locks(&project, &journal.plan.binding),
                    verify_mixed_source(&project, &journal, &changes),
                ] {
                    if let Err(error) = result {
                        issues.push(error);
                    }
                }
                Ok((journal, changes, issues))
            });
            match inspected {
                Ok((journal, changes, issues)) => {
                    let unknown = !issues.is_empty();
                    result.operations.push(RecoveryOperation {
                        restore_task_id: pending.task_id,
                        target_commit: journal.plan.target_commit,
                        phase: pending.phase,
                        plan_digest: pending.plan_digest,
                        journal_digest: pending.journal_digest,
                        can_continue: !unknown,
                        can_undo: !unknown,
                        changes,
                        issues,
                    });
                }
                Err(error) => result.operations.push(RecoveryOperation {
                    restore_task_id: pending.task_id,
                    target_commit: String::new(),
                    phase: pending.phase,
                    plan_digest: pending.plan_digest,
                    journal_digest: pending.journal_digest,
                    can_continue: false,
                    can_undo: false,
                    changes: vec![],
                    issues: vec![error],
                }),
            }
        }
        if !result.issues.is_empty()
            || result.operations.len() > 1
            || result
                .operations
                .iter()
                .any(|operation| !operation.issues.is_empty())
        {
            result.state = "blocked".into();
            for operation in &mut result.operations {
                operation.can_continue = false;
                operation.can_undo = false;
            }
        } else if !result.operations.is_empty() {
            result.state = "pending".into();
        }
        Ok(result)
    }
    fn restore_root(&self, id: &str, task: &str) -> PathBuf {
        self.paths
            .workspace_root()
            .join(".platform/project-state/source-restore")
            .join(id)
            .join(task)
    }
    async fn restore_plan(
        &self,
        project: &ProjectHandle,
        target: &str,
        tools: &GitToolchain,
        context: &mut Context,
    ) -> Result<RestorePlan, ApiError> {
        let mut snapshot = inspect(project, Some(tools), context).await?;
        self.add_recovery_issue(&project.manifest.id, &mut snapshot.view)?;
        finalize_capabilities(&mut snapshot.view);
        if !snapshot.view.can_save
            || !snapshot.view.changes.is_empty()
            || head_oid(&snapshot.view).is_none()
        {
            return Err(restore_error(
                "PROJECT_SOURCE_RESTORE_NOT_CLEAN",
                "只可从干净、已保存且范围明确的源码开始取回；未覆盖当前改动、暂存或未知文件。",
            ));
        }
        let binding = bind(project, &snapshot.view)?;
        let target_files = read_target(project, target, tools, context).await?;
        let before: BTreeMap<_, _> = snapshot
            .files
            .iter()
            .map(|(path, file)| (path.clone(), descriptor(file)))
            .collect();
        let paths = before
            .keys()
            .chain(target_files.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        validate_collisions(paths.iter().map(String::as_str))?;
        let mut changes = Vec::new();
        for path in paths {
            let old = before.get(&path).cloned();
            let new = target_files.get(&path).map(|file| file.descriptor.clone());
            if old != new {
                changes.push(RestoreChange {
                    path,
                    kind: if old.is_none() {
                        "add"
                    } else if new.is_none() {
                        "delete"
                    } else {
                        "modify"
                    }
                    .into(),
                    before: old,
                    after: new,
                });
            }
        }
        if changes.iter().any(|change| change.path == "project.json") {
            return Err(restore_error(
                "PROJECT_SOURCE_RESTORE_MANIFEST_CHANGED",
                "目标版本的 project.json 必须与当前文件逐字节一致；本步不改运行声明或项目身份。",
            ));
        }
        let verify = inspect(project, Some(tools), context).await?;
        if verify.view.snapshot_digest != snapshot.view.snapshot_digest {
            return Err(stale());
        }
        let plan = RestorePlan {
            policy: RESTORE_POLICY.into(),
            project_id: project.manifest.id.clone(),
            target_commit: target.into(),
            snapshot_digest: snapshot.view.snapshot_digest,
            binding,
            before,
            target: target_files,
            changes,
        };
        validate_target_policy(project, &plan, tools, context).await?;
        if serde_json::to_vec(&plan)?.len() as u64 > JOURNAL_LIMIT / 2 {
            return Err(restore_limit());
        }
        Ok(plan)
    }
}
fn preview(
    project: &ProjectHandle,
    plan: &RestorePlan,
    tools: &GitToolchain,
) -> Result<GitSourceRestorePreview, ApiError> {
    Ok(GitSourceRestorePreview {
        repository_root: project.root.clone(),
        head: GitHead {
            oid: Some(plan.binding.head.clone()),
            branch: Some(plan.binding.branch.clone()),
        },
        target_commit: plan.target_commit.clone(),
        snapshot_digest: plan.snapshot_digest.clone(),
        plan_digest: plan_digest(plan)?,
        changes: plan.changes.clone(),
        total_files: plan.target.len(),
        total_bytes: plan.target.values().map(|file| file.descriptor.size).sum(),
        can_apply: !plan.changes.is_empty(),
        issues: if plan.changes.is_empty() {
            vec![restore_error(
                "PROJECT_SOURCE_RESTORE_NO_CHANGES",
                "目标版本与当前源码范围一致；没有需要取回的文件。 ",
            )]
        } else {
            vec![]
        },
        git: tools.git.clone(),
        database: database_boundary(),
    })
}
fn descriptor(file: &SourceFile) -> RestoreDescriptor {
    RestoreDescriptor {
        sha256: file.digest.clone(),
        size: file.size,
        executable: file.mode == "100755",
    }
}
fn validate_target(value: &str) -> Result<(), ApiError> {
    if value.len() == 40 && valid_oid(value) {
        Ok(())
    } else {
        Err(input_error())
    }
}
fn plan_digest(plan: &RestorePlan) -> Result<String, ApiError> {
    Ok(sha256(&serde_json::to_vec(plan)?))
}
async fn read_gate(
    project: &ProjectHandle,
    context: &mut Context,
) -> Result<Arc<tokio::sync::OwnedRwLockReadGuard<()>>, ApiError> {
    context.checkpoint()?;
    tokio::select! { biased;
        _ = watch_stop(&mut context.stop) => Err(interrupted()),
        _ = watch_stop(&mut context.cancel) => Err(interrupted()),
        _ = tokio::time::sleep_until(context.deadline) => Err(restore_error("PROJECT_SOURCE_RESTORE_TIMEOUT", "源码取回超过内部时间限制。")),
        guard = project.operation_gate.clone().read_owned() => {
            let guard = Arc::new(guard);
            context.phase(guard.clone());
            Ok(guard)
        },
    }
}
fn bind(project: &ProjectHandle, status: &ProjectGitStatus) -> Result<Binding, ApiError> {
    let head = status.repository.head.as_ref().ok_or_else(output_invalid)?;
    let branch = head.branch.clone().ok_or_else(output_invalid)?;
    Ok(Binding {
        root_identity: identity(&project.root)?,
        manifest_sha256: sha256(&read_file(
            &project.root.join("project.json"),
            MAX_METADATA,
        )?),
        repository_digest: repository_digest(&project.root.join(".git"))?,
        index: index_fingerprint(&project.root.join(".git/index"))?,
        head: head.oid.clone().ok_or_else(output_invalid)?,
        branch_ref: index_fingerprint(&project.root.join(".git").join(&branch))?,
        metadata_digest: binding_metadata(project, &branch)?,
        branch,
    })
}
fn verify_binding(project: &ProjectHandle, binding: &Binding) -> Result<(), ApiError> {
    project.check()?;
    let git = project.root.join(".git");
    if identity(&project.root)? != binding.root_identity
        || sha256(&read_file(
            &project.root.join("project.json"),
            MAX_METADATA,
        )?) != binding.manifest_sha256
        || repository_digest(&git)? != binding.repository_digest
        || index_fingerprint(&git.join("index"))? != binding.index
        || index_fingerprint(&git.join(&binding.branch))? != binding.branch_ref
        || binding_metadata(project, &binding.branch)? != binding.metadata_digest
        || read_file(&git.join("HEAD"), 4096)? != format!("ref: {}\n", binding.branch).as_bytes()
    {
        return Err(restore_changed());
    }
    Ok(())
}
fn binding_metadata(project: &ProjectHandle, branch: &str) -> Result<String, ApiError> {
    let mut values = BTreeMap::new();
    for path in [
        "HEAD",
        "config",
        "index",
        "packed-refs",
        "shallow",
        "info/attributes",
        "info/exclude",
        branch,
    ] {
        let absolute = project.root.join(".git").join(path);
        check_no_symlinks(&absolute, true)?;
        let metadata = match fs::symlink_metadata(&absolute) {
            Ok(value) if value.is_file() && value.nlink() == 1 => Some(format!(
                "{}:{}:{}:{}:{}:{}:{}:{}",
                value.dev(),
                value.ino(),
                value.mode(),
                value.len(),
                value.mtime(),
                value.mtime_nsec(),
                value.ctime(),
                value.ctime_nsec()
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            _ => return Err(restore_changed()),
        };
        values.insert(path, metadata);
    }
    Ok(sha256(&serde_json::to_vec(&values)?))
}
fn verify_step_binding(project: &ProjectHandle, binding: &Binding) -> Result<(), ApiError> {
    project.check()?;
    if identity(&project.root)? != binding.root_identity
        || binding_metadata(project, &binding.branch)? != binding.metadata_digest
        || sha256(&read_file(
            &project.root.join("project.json"),
            MAX_METADATA,
        )?) != binding.manifest_sha256
    {
        return Err(restore_changed());
    }
    verify_no_git_locks(project, binding)
}
fn restore_error(code: &str, message: &str) -> ApiError {
    error(code, message)
}
fn restore_changed() -> ApiError {
    restore_error(
        "PROJECT_SOURCE_RESTORE_CHANGED",
        "源码、仓库或保留现场出现计划之外的状态；未覆盖第三方内容，请核对现场后再明确继续或撤销。",
    )
}
fn restore_limit() -> ApiError {
    restore_error(
        "PROJECT_SOURCE_RESTORE_LIMIT",
        "源码取回超过文件、字节或现场大小限制；没有截断后继续。",
    )
}

#[derive(Clone)]
struct Blob {
    path: String,
    oid: String,
    size: u64,
    executable: bool,
}
async fn read_target(
    project: &ProjectHandle,
    target: &str,
    tools: &GitToolchain,
    context: &mut Context,
) -> Result<BTreeMap<String, PlannedFile>, ApiError> {
    let mut command = git_command(tools, context.root(), Some(project));
    command.args(["cat-file", "-t", target]);
    if context.run(command, vec![]).await?.success()? != b"commit\n" {
        return Err(restore_error(
            "PROJECT_SOURCE_RESTORE_TARGET_INVALID",
            "请选择本机已有的完整提交号；不接受标签、分支、缩写或其他对象。",
        ));
    }
    let mut command = git_command(tools, context.root(), Some(project));
    command.args(["ls-tree", "-r", "-l", "-z", "--full-tree", target]);
    let bytes = context.run(command, vec![]).await?.success()?;
    let roots = included_roots(&project.manifest);
    let mut blobs = Vec::new();
    let mut total = 0u64;
    for raw in bytes.split(|byte| *byte == 0).filter(|raw| !raw.is_empty()) {
        let text = std::str::from_utf8(raw).map_err(|_| output_invalid())?;
        let (header, path) = text.split_once('\t').ok_or_else(output_invalid)?;
        let fields: Vec<_> = header.split_ascii_whitespace().collect();
        if fields.len() != 4
            || !matches!(fields[0], "100644" | "100755")
            || fields[1] != "blob"
            || !valid_oid(fields[2])
        {
            return Err(restore_error(
                "PROJECT_SOURCE_RESTORE_TREE_UNSUPPORTED",
                "目标版本含链接、子模块或其他不支持的树条目；不会部分取回。",
            ));
        }
        valid_relative(path)?;
        if data_versions::is_data_path(path) {
            continue;
        }
        if path.len() > 1024 || path.split('/').count() > 32 {
            return Err(restore_limit());
        }
        if protected(path, &project.manifest).is_some() || !included(path, &roots) {
            return Err(restore_error("PROJECT_SOURCE_RESTORE_SCOPE_UNSUPPORTED", "目标版本含保护范围或尚未适配的文件；未读取这些文件内容，也不会只取回一部分后声称完整。 ").detail("path", path));
        }
        let size = fields[3].parse::<u64>().map_err(|_| output_invalid())?;
        total = total.checked_add(size).ok_or_else(restore_limit)?;
        if size > MAX_BLOB || total > MAX_BYTES || blobs.len() >= MAX_FILES {
            return Err(restore_limit());
        }
        blobs.push(Blob {
            path: path.into(),
            oid: fields[2].into(),
            size,
            executable: fields[0] == "100755",
        });
    }
    validate_collisions(blobs.iter().map(|blob| blob.path.as_str()))?;
    if !blobs.iter().any(|blob| blob.path == "project.json") {
        return Err(restore_error(
            "PROJECT_SOURCE_RESTORE_MANIFEST_CHANGED",
            "目标版本没有相同的 project.json；不能在当前实例取回。",
        ));
    }
    let target_root = context.root().join("restore-target");
    private_dir(&target_root)?;
    check_space(&target_root, total)?;
    let result = materialize_blobs(project, tools, context, &blobs, &target_root).await?;
    if read_file(&target_root.join("project.json"), MAX_METADATA)?
        != read_file(&project.root.join("project.json"), MAX_METADATA)?
    {
        return Err(restore_error(
            "PROJECT_SOURCE_RESTORE_MANIFEST_CHANGED",
            "目标版本的 project.json 与当前文件不同；本步不会替换项目身份或运行声明。",
        ));
    }
    Ok(result)
}
async fn materialize_blobs(
    project: &ProjectHandle,
    tools: &GitToolchain,
    context: &mut Context,
    blobs: &[Blob],
    destination: &Path,
) -> Result<BTreeMap<String, PlannedFile>, ApiError> {
    context.checkpoint()?;
    let blobs = blobs.to_vec();
    let destination = destination.to_path_buf();
    let stop = context.stop.clone();
    let cancel = context.cancel.clone();
    let deadline = context.deadline;
    #[cfg(test)]
    let mut writer_pause = context.writer_pause.take();
    let mut command = git_command(tools, context.root(), Some(project));
    command.args(["cat-file", "--batch"]);
    let run = context.start(command, move |pipes| {
        let mut stdin = BlockingPipe::new(pipes.stdin);
        let mut stdout = BlockingPipe::new(pipes.stdout);
        let internal_stop = pipes.stop;
        crate::owned_process::IoPlan::new(async move {
            tokio::task::spawn_blocking(move || {
                let check = || {
                    process_io::checkpoint(&stop, &cancel, deadline)?;
                    if *internal_stop.borrow() {
                        return Err(interrupted());
                    }
                    Ok::<_, ApiError>(())
                };
                let mut files = BTreeMap::new();
                let mut buffer = vec![0u8; 64 * 1024];
                for blob in blobs {
                    check()?;
                    // Interleave requests and responses so neither bounded
                    // facade can deadlock behind a large request list/payload.
                    stdin
                        .write_all(format!("{}\n", blob.oid).as_bytes())
                        .map_err(|_| git_failed())?;
                    stdin.flush().map_err(|_| git_failed())?;
                    process_io::expect_blocking_line(
                        &mut stdout,
                        format!("{} blob {}\n", blob.oid, blob.size).as_bytes(),
                    )?;
                    let path = destination.join(&blob.path);
                    private_dir(path.parent().ok_or_else(unsafe_path)?)?;
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(if blob.executable { 0o755 } else { 0o644 })
                        .custom_flags(libc::O_NOFOLLOW)
                        .open(&path)?;
                    #[cfg(test)]
                    if let Some((entered, release)) = writer_pause.take() {
                        let _ = entered.send(());
                        let _ = release.recv();
                    }
                    let mut remaining = blob.size;
                    let mut digest = Sha256::new();
                    while remaining != 0 {
                        check()?;
                        let count = remaining.min(buffer.len() as u64) as usize;
                        stdout
                            .read_exact(&mut buffer[..count])
                            .map_err(|_| output_invalid())?;
                        digest.update(&buffer[..count]);
                        file.write_all(&buffer[..count])?;
                        remaining -= count as u64;
                    }
                    let mut delimiter = [0u8; 1];
                    stdout
                        .read_exact(&mut delimiter)
                        .map_err(|_| output_invalid())?;
                    if delimiter[0] != b'\n' {
                        return Err(output_invalid());
                    }
                    file.sync_all()?;
                    files.insert(
                        blob.path.clone(),
                        PlannedFile {
                            oid: blob.oid,
                            descriptor: RestoreDescriptor {
                                sha256: format!("{:x}", digest.finalize()),
                                size: blob.size,
                                executable: blob.executable,
                            },
                        },
                    );
                }
                drop(stdin);
                let mut extra = [0u8; 1];
                if stdout.read(&mut extra).map_err(|_| git_failed())? != 0 {
                    return Err(output_invalid());
                }
                Ok::<_, ApiError>(files)
            })
            .await
            .map_err(|_| {
                restore_error(
                    "PROJECT_SOURCE_RESTORE_WORKER_FAILED",
                    "读取目标源码的文件工作者未正常收尾；保留现场待核对。",
                )
            })?
        })
        .drain("stderr", async move {
            read_output(pipes.stderr, false, 64 * 1024)
                .await
                .map(|_| ())
        })
    })?;
    let output = context.wait(run).await?;
    if !output.status.success() {
        return Err(git_failed());
    }
    Ok(output.value)
}
async fn validate_target_policy(
    project: &ProjectHandle,
    plan: &RestorePlan,
    tools: &GitToolchain,
    context: &mut Context,
) -> Result<(), ApiError> {
    let target = context.root().join("restore-target");
    let private_index = context.root().join("absent-policy-index");
    let mut command = git_command(tools, context.root(), Some(project));
    command
        .arg(format!("--work-tree={}", target.display()))
        .env("GIT_INDEX_FILE", &private_index)
        .args([
            "check-attr",
            "-z",
            "--stdin",
            "filter",
            "text",
            "eol",
            "ident",
            "working-tree-encoding",
        ]);
    let mut input = Vec::new();
    for path in plan.target.keys() {
        input.extend_from_slice(path.as_bytes());
        input.push(0);
    }
    let output = context.run(command, input).await?.success()?;
    let fields: Vec<_> = output.split(|byte| *byte == 0).collect();
    if fields.last() != Some(&b"".as_slice()) || !(fields.len() - 1).is_multiple_of(3) {
        return Err(output_invalid());
    }
    if fields[..fields.len() - 1]
        .chunks(3)
        .any(|row| !matches!(row[2], b"unspecified" | b"unset"))
    {
        return Err(restore_error(
            "PROJECT_SOURCE_RESTORE_ATTRIBUTES_UNSUPPORTED",
            "目标版本含内容转换属性；本步不运行过滤器，也不会取回后冒称可按标准范围保存。",
        ));
    }
    let added: Vec<_> = plan
        .changes
        .iter()
        .filter(|change| change.before.is_none())
        .map(|change| change.path.as_str())
        .collect();
    if !added.is_empty() {
        let mut command = git_command(tools, context.root(), Some(project));
        command
            .arg(format!("--work-tree={}", target.display()))
            .env("GIT_INDEX_FILE", private_index)
            .args(["check-ignore", "--no-index", "-z", "--stdin"]);
        let mut input = Vec::new();
        for path in &added {
            input.extend_from_slice(path.as_bytes());
            input.push(0);
        }
        let output = context.run(command, input).await?;
        if !matches!(output.code, 0 | 1) {
            return Err(git_failed());
        }
        if !output.stdout.is_empty() {
            return Err(restore_error("PROJECT_SOURCE_RESTORE_IGNORED_ADDITION", "目标版本将新增目前未跟踪且被目标忽略规则排除的文件；请先适配范围，避免取回后不能完整保存。"));
        }
    }
    Ok(())
}
pub(super) fn validate_collisions<'a>(
    paths: impl Iterator<Item = &'a str>,
) -> Result<(), ApiError> {
    let mut ancestors = BTreeMap::<String, String>::new();
    let mut files = BTreeSet::new();
    for path in paths {
        valid_relative(path)?;
        let mut prefix = String::new();
        for part in path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let folded = prefix
                .nfd()
                .flat_map(char::to_lowercase)
                .collect::<String>();
            if ancestors.get(&folded).is_some_and(|old| old != &prefix)
                || (prefix != path && files.contains(&folded))
            {
                return Err(restore_error(
                    "PROJECT_SOURCE_RESTORE_PATH_COLLISION",
                    "目标与当前路径存在大小写、Unicode 或文件/目录冲突；本步不替换目录结构。",
                ));
            }
            ancestors.insert(folded, prefix.clone());
        }
        let folded = path.nfd().flat_map(char::to_lowercase).collect::<String>();
        if !files.insert(folded.clone())
            || ancestors
                .range(format!("{folded}/")..)
                .next()
                .is_some_and(|(key, _)| key.starts_with(&format!("{folded}/")))
        {
            return Err(restore_error(
                "PROJECT_SOURCE_RESTORE_PATH_COLLISION",
                "目标或当前路径重复或存在文件/目录冲突。",
            ));
        }
    }
    Ok(())
}
fn check_space(path: &Path, needed: u64) -> Result<(), ApiError> {
    if fs2::available_space(path)? < needed.saturating_add(MIN_FREE) {
        return Err(restore_error(
            "PROJECT_SOURCE_RESTORE_SPACE",
            "可用空间不足，未开始替换源码；已有内容和现场保留。",
        ));
    }
    Ok(())
}

fn stamp(path: &Path) -> Result<Option<Stamp>, ApiError> {
    check_no_symlinks(path, true)?;
    let before = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let bytes = read_file(path, MAX_BLOB)?;
    let after = fs::symlink_metadata(path)?;
    if !same_file(&before, &after) {
        return Err(restore_changed());
    }
    Ok(Some(Stamp {
        descriptor: RestoreDescriptor {
            sha256: sha256(&bytes),
            size: before.len(),
            executable: before.mode() & 0o111 != 0,
        },
        mode: before.mode() & 0o7777,
        device: before.dev(),
        inode: before.ino(),
    }))
}
fn slot(root: &Path, index: usize) -> PathBuf {
    root.join("slots").join(format!("{index:05}"))
}
fn validate_descriptor(value: &RestoreDescriptor) -> Result<(), ApiError> {
    validate_digest(&value.sha256)?;
    if value.size > MAX_BLOB {
        return Err(restore_limit());
    }
    Ok(())
}
fn load_journal(root: &Path, id: &str, task: &str) -> Result<RestoreJournal, ApiError> {
    let bytes = read_file(&root.join("journal.json"), JOURNAL_LIMIT)?;
    let journal: RestoreJournal = serde_json::from_slice(&bytes).map_err(|_| restore_changed())?;
    if journal.schema_version != 1
        || journal.operation != "sourceRestore"
        || journal.project_id != id
        || journal.task_id != task
        || journal.plan.project_id != id
        || journal.plan.policy != RESTORE_POLICY
        || plan_digest(&journal.plan)? != journal.plan_digest
        || !matches!(
            journal.phase.as_str(),
            "prepared"
                | "applying"
                | "applied"
                | "undoing"
                | "completed"
                | "undone"
                | "not-applied"
        )
    {
        return Err(restore_changed());
    }
    validate_target(&journal.plan.target_commit)?;
    validate_target(&journal.plan.binding.head)?;
    one_ref(format!("{}\n", journal.plan.binding.branch).as_bytes())?;
    validate_digest(&journal.plan.binding.manifest_sha256)?;
    validate_digest(&journal.plan.binding.metadata_digest)?;
    validate_digest(&journal.plan.snapshot_digest)?;
    let manifest = journal
        .plan
        .before
        .get("project.json")
        .ok_or_else(restore_changed)?;
    if manifest.sha256 != journal.plan.binding.manifest_sha256
        || journal
            .plan
            .target
            .get("project.json")
            .map(|file| &file.descriptor)
            != Some(manifest)
    {
        return Err(restore_changed());
    }
    if journal.plan.before.len() > MAX_FILES || journal.plan.target.len() > MAX_FILES {
        return Err(restore_limit());
    }
    let mut total_before = 0u64;
    let mut total_after = 0u64;
    for descriptor in journal.plan.before.values() {
        validate_descriptor(descriptor)?;
        total_before = total_before
            .checked_add(descriptor.size)
            .ok_or_else(restore_limit)?;
    }
    for file in journal.plan.target.values() {
        validate_descriptor(&file.descriptor)?;
        validate_target(&file.oid)?;
        total_after = total_after
            .checked_add(file.descriptor.size)
            .ok_or_else(restore_limit)?;
    }
    if total_before > MAX_BYTES || total_after > MAX_BYTES {
        return Err(restore_limit());
    }
    let paths = journal
        .plan
        .before
        .keys()
        .chain(journal.plan.target.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    validate_collisions(paths.iter().map(String::as_str))?;
    let mut changes = Vec::new();
    for path in paths {
        let before = journal.plan.before.get(&path).cloned();
        let after = journal
            .plan
            .target
            .get(&path)
            .map(|file| file.descriptor.clone());
        if before != after {
            changes.push(RestoreChange {
                path,
                kind: if before.is_none() {
                    "add"
                } else if after.is_none() {
                    "delete"
                } else {
                    "modify"
                }
                .into(),
                before,
                after,
            });
        }
    }
    if changes.is_empty()
        || changes.iter().any(|change| change.path == "project.json")
        || serde_json::to_value(&changes)? != serde_json::to_value(&journal.plan.changes)?
        || changes.len() != journal.entries.len()
    {
        return Err(restore_changed());
    }
    for (change, entry) in changes.iter().zip(&journal.entries) {
        if change.path != entry.path
            || change.before.as_ref() != entry.before.as_ref().map(|value| &value.descriptor)
            || change.after.as_ref() != entry.after.as_ref().map(|value| &value.descriptor)
        {
            return Err(restore_changed());
        }
        for stamp in [&entry.before, &entry.after].into_iter().flatten() {
            if stamp.mode > 0o7777 || (stamp.mode & 0o111 != 0) != stamp.descriptor.executable {
                return Err(restore_changed());
            }
        }
    }
    Ok(journal)
}
fn write_journal(root: &Path, journal: &RestoreJournal) -> Result<(), ApiError> {
    if serde_json::to_vec_pretty(journal)?.len() as u64 > JOURNAL_LIMIT {
        return Err(restore_limit());
    }
    atomic_json(&root.join("journal.json"), journal)
}
fn maximum_journal_bytes(journal: &RestoreJournal) -> Result<u64, ApiError> {
    let mut candidate = journal.clone();
    let mut maximum = 0;
    for phase in [
        "prepared",
        "applying",
        "applied",
        "undoing",
        "completed",
        "undone",
        "not-applied",
    ] {
        candidate.phase = phase.into();
        maximum = maximum.max(serde_json::to_vec_pretty(&candidate)?.len() as u64);
    }
    if maximum > JOURNAL_LIMIT {
        return Err(restore_limit());
    }
    Ok(maximum)
}
fn classify_all(
    project: &ProjectHandle,
    root: &Path,
    journal: &RestoreJournal,
) -> Result<Vec<RecoveryChange>, ApiError> {
    let roots = included_roots(&project.manifest);
    for path in journal.plan.before.keys().chain(journal.plan.target.keys()) {
        if !included(path, &roots) || protected(path, &project.manifest).is_some() {
            return Err(restore_changed());
        }
    }
    check_no_symlinks(root, false)?;
    let slots = root.join("slots");
    check_no_symlinks(&slots, false)?;
    for entry in fs::read_dir(&slots)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| restore_changed())?;
        let index = name.parse::<usize>().map_err(|_| restore_changed())?;
        if index >= journal.entries.len() || name != format!("{index:05}") {
            return Err(restore_changed());
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(unsafe_path());
        }
    }
    let mut result = Vec::new();
    for (index, entry) in journal.entries.iter().enumerate() {
        let state = classify_entry(project, root, index, entry).unwrap_or("unknown");
        result.push(RecoveryChange {
            path: entry.path.clone(),
            kind: journal.plan.changes[index].kind.clone(),
            state: state.into(),
        });
    }
    Ok(result)
}
fn classify_entry(
    project: &ProjectHandle,
    root: &Path,
    index: usize,
    entry: &RestoreEntry,
) -> Result<&'static str, ApiError> {
    let live = stamp(&checked_relative(&project.root, &entry.path, true)?)?;
    let held = stamp(&slot(root, index))?;
    Ok(if live == entry.before && held == entry.after {
        "before"
    } else if live == entry.after && held == entry.before {
        "after"
    } else {
        "unknown"
    })
}
fn verify_mixed_source(
    project: &ProjectHandle,
    journal: &RestoreJournal,
    changes: &[RecoveryChange],
) -> Result<(), ApiError> {
    if changes.iter().any(|change| change.state == "unknown") {
        return Err(restore_changed());
    }
    let scan = scan_source(
        &project.root,
        &project.manifest,
        &included_roots(&project.manifest),
    )?;
    if !scan.issues.is_empty() || !scan.unclassified.is_empty() || !scan.markers.is_empty() {
        return Err(restore_changed());
    }
    let actual: BTreeMap<_, _> = scan
        .files
        .iter()
        .map(|(path, file)| (path.clone(), descriptor(file)))
        .collect();
    let mut expected = journal.plan.before.clone();
    for (entry, state) in journal.entries.iter().zip(changes) {
        if state.state == "after" {
            if let Some(after) = &entry.after {
                expected.insert(entry.path.clone(), after.descriptor.clone());
            } else {
                expected.remove(&entry.path);
            }
        }
    }
    if actual != expected {
        return Err(restore_changed());
    }
    Ok(())
}
fn open_directory(path: &Path) -> Result<File, ApiError> {
    check_no_symlinks(path, false)?;
    Ok(OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?)
}
/// Both modes preserve whichever object was present at the syscall boundary.
/// Never unlink a displaced file, including after a failed verification.
#[cfg(target_os = "macos")]
pub(super) fn move_preserving(from: &Path, to: &Path, exchange: bool) -> Result<(), ApiError> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let from_parent = open_directory(from.parent().ok_or_else(unsafe_path)?)?;
    let to_parent = open_directory(to.parent().ok_or_else(unsafe_path)?)?;
    if from_parent.metadata()?.dev() != to_parent.metadata()?.dev() {
        return Err(restore_error(
            "PROJECT_SOURCE_RESTORE_CROSS_VOLUME",
            "源码与恢复现场必须位于同一卷；未使用复制覆盖作为替代。",
        ));
    }
    let from_name = CString::new(from.file_name().ok_or_else(unsafe_path)?.as_bytes())
        .map_err(|_| unsafe_path())?;
    let to_name = CString::new(to.file_name().ok_or_else(unsafe_path)?.as_bytes())
        .map_err(|_| unsafe_path())?;
    if unsafe {
        libc::renameatx_np(
            from_parent.as_raw_fd(),
            from_name.as_ptr(),
            to_parent.as_raw_fd(),
            to_name.as_ptr(),
            if exchange {
                libc::RENAME_SWAP
            } else {
                libc::RENAME_EXCL
            },
        )
    } != 0
    {
        return Err(restore_error(
            "PROJECT_SOURCE_RESTORE_MOVE_FAILED",
            "原子源码变更未完成；现有文件与本次现场均保留，请重新核对。",
        ));
    }
    from_parent.sync_all()?;
    to_parent.sync_all()?;
    Ok(())
}
#[cfg(not(target_os = "macos"))]
pub(super) fn move_preserving(_from: &Path, _to: &Path, _exchange: bool) -> Result<(), ApiError> {
    Err(restore_error(
        "UNSUPPORTED_PLATFORM",
        "源码取回事务目前仅验证 macOS。",
    ))
}
pub(super) fn ensure_live_parent(root: &Path, path: &str) -> Result<(), ApiError> {
    valid_relative(path)?;
    let parent = Path::new(path).parent().ok_or_else(unsafe_path)?;
    let mut current = root.to_path_buf();
    for part in parent.components() {
        current.push(part.as_os_str());
        check_no_symlinks(&current, true)?;
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() => (),
            Ok(_) => return Err(restore_changed()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
                open_directory(current.parent().ok_or_else(unsafe_path)?)?.sync_all()?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

impl ProjectGitManager {
    pub(super) async fn source_restore_apply_inner(
        &self,
        project: &ProjectHandle,
        task_id: &str,
        input: GitSourceRestoreApplyInput,
        context: &mut Context,
    ) -> Result<Value, ApiError> {
        self.tasks.phase(task_id, "source-restore-preview")?;
        let tools = context.resolve(&self.git).await?;
        let plan = self
            .restore_plan(project, &input.target_commit, &tools, context)
            .await?;
        if plan.snapshot_digest != input.expected_snapshot
            || plan_digest(&plan)? != input.expected_plan_digest
            || plan.binding.head != input.expected_head
            || plan.binding.branch != input.expected_branch
        {
            return Err(stale());
        }
        if plan.changes.is_empty() {
            return Err(restore_error(
                "PROJECT_SOURCE_RESTORE_NO_CHANGES",
                "目标源码与当前一致；未创建空取回任务。",
            ));
        }
        self.tasks.phase(task_id, "source-restore-materialize")?;
        let staging_parent = self
            .paths
            .workspace_root()
            .join(".platform/project-state/source-restore-staging")
            .join(&project.manifest.id);
        private_dir(&staging_parent)?;
        let staging = staging_parent.join(task_id);
        fs::create_dir(&staging)?;
        private_dir(&staging.join("slots"))?;
        if fs::metadata(&staging)?.dev() != fs::metadata(&project.root)?.dev() {
            return Err(restore_error(
                "PROJECT_SOURCE_RESTORE_CROSS_VOLUME",
                "源码与恢复现场不在同一卷；未使用复制覆盖替代原子操作。",
            ));
        }
        let mut entries = Vec::new();
        for (index, change) in plan.changes.iter().enumerate() {
            context.checkpoint()?;
            let before = stamp(&checked_relative(&project.root, &change.path, true)?)?;
            if before.as_ref().map(|value| &value.descriptor) != change.before.as_ref() {
                return Err(stale());
            }
            let after = if let Some(expected) = &change.after {
                check_space(&staging, expected.size)?;
                let bytes = read_file(
                    &context.root().join("restore-target").join(&change.path),
                    MAX_BLOB,
                )?;
                if sha256(&bytes) != expected.sha256 || bytes.len() as u64 != expected.size {
                    return Err(restore_changed());
                }
                let path = slot(&staging, index);
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(if expected.executable { 0o755 } else { 0o644 })
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&path)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
                stamp(&path)?
            } else {
                None
            };
            entries.push(RestoreEntry {
                path: change.path.clone(),
                before,
                after,
            });
        }
        let mut journal = RestoreJournal {
            schema_version: 1,
            operation: "sourceRestore".into(),
            project_id: project.manifest.id.clone(),
            task_id: task_id.into(),
            phase: "prepared".into(),
            plan_digest: input.expected_plan_digest,
            plan,
            entries,
            git: tools.git,
        };
        write_journal(&staging, &journal)?;
        open_directory(&staging.join("slots"))?.sync_all()?;
        open_directory(&staging)?.sync_all()?;
        verify_binding(project, &journal.plan.binding)?;
        verify_no_git_locks(project, &journal.plan.binding)?;
        let before = classify_all(project, &staging, &journal)?;
        verify_mixed_source(project, &journal, &before)?;
        if before.iter().any(|change| change.state != "before") {
            return Err(restore_changed());
        }
        context.checkpoint()?;
        let root = self.restore_root(&project.manifest.id, task_id);
        // Publish a complete prepared directory at once. A preparation crash never
        // leaves an empty or journal-less task inside the development guard root.
        {
            // No await while held: admission and the same-volume publication form
            // one short section across projects, including future phase growth.
            let _publication = guard::reserve_publication(
                self.paths.workspace_root(),
                &project.manifest.id,
                task_id,
                maximum_journal_bytes(&journal)?,
            )?;
            private_dir(root.parent().ok_or_else(unsafe_path)?)?;
            if let Err(error) = move_preserving(&staging, &root, false) {
                if root.try_exists().unwrap_or(true) {
                    return Err(error
                        .outcome("unknown")
                        .detail("restoreTaskId", task_id.to_owned())
                        .detail("pending", true));
                }
                return Err(error);
            }
        }
        self.drive_restore(
            project,
            task_id,
            &mut journal,
            RestoreAction::Continue,
            "source-restore",
            context,
        )
        .await
    }
    pub(super) async fn source_restore_recover_inner(
        &self,
        project: &ProjectHandle,
        task_id: &str,
        input: GitSourceRestoreRecoverInput,
        context: &mut Context,
    ) -> Result<Value, ApiError> {
        let restore_task = input.restore_task_id.clone();
        self.source_restore_recover_verified(project, task_id, input, context)
            .await
            .map_err(|error| {
                if error.details.get("pending").is_some() {
                    error
                } else {
                    error
                        .outcome("not_committed")
                        .detail("restoreTaskId", restore_task)
                        .detail("pending", true)
                }
            })
    }
    async fn source_restore_recover_verified(
        &self,
        project: &ProjectHandle,
        task_id: &str,
        input: GitSourceRestoreRecoverInput,
        context: &mut Context,
    ) -> Result<Value, ApiError> {
        let permit = guard::validate_recovery(
            self.paths.workspace_root(),
            &project.manifest.id,
            &input.restore_task_id,
            &input.expected_plan_digest,
            &input.expected_journal_digest,
        )?;
        permit.revalidate(self.paths.workspace_root(), &project.manifest.id)?;
        let root = self.restore_root(&project.manifest.id, &input.restore_task_id);
        let mut journal = load_journal(&root, &project.manifest.id, &input.restore_task_id)?;
        verify_binding(project, &journal.plan.binding)?;
        verify_no_git_locks(project, &journal.plan.binding)?;
        let changes = classify_all(project, &root, &journal)?;
        verify_mixed_source(project, &journal, &changes)?;
        let operation = if input.action == RestoreAction::Undo {
            "source-restore-undo"
        } else {
            "source-restore-continue"
        };
        self.drive_restore(
            project,
            task_id,
            &mut journal,
            input.action,
            operation,
            context,
        )
        .await
    }
    async fn drive_restore(
        &self,
        project: &ProjectHandle,
        task_id: &str,
        journal: &mut RestoreJournal,
        direction: RestoreAction,
        operation: &str,
        context: &mut Context,
    ) -> Result<Value, ApiError> {
        let retained_root = self.restore_root(&project.manifest.id, &journal.task_id);
        let root = retained_root.as_path();
        self.tasks
            .phase(
                task_id,
                if direction == RestoreAction::Undo {
                    "source-restore-undo"
                } else {
                    "source-restore-apply"
                },
            )
            .map_err(|error| {
                error
                    .outcome("unknown")
                    .detail("restoreTaskId", journal.task_id.clone())
                    .detail("pending", true)
            })?;
        let outcome =
            apply_entries(project, root, journal, direction, context, |_, _| Ok(())).await;
        if let Err(error) = outcome {
            let verified = verify_binding(project, &journal.plan.binding)
                .and_then(|_| verify_no_git_locks(project, &journal.plan.binding))
                .and_then(|_| classify_all(project, root, journal))
                .and_then(|changes| {
                    verify_mixed_source(project, journal, &changes)?;
                    Ok(changes)
                });
            if direction == RestoreAction::Continue
                && verified
                    .as_ref()
                    .is_ok_and(|changes| changes.iter().all(|change| change.state == "before"))
            {
                if let Ok(warning) = persist_terminal(root, journal, "not-applied", write_journal) {
                    return Err(error
                        .outcome("not_committed")
                        .detail("restoreTaskId", journal.task_id.clone())
                        .detail("durabilityWarning", json!(warning))
                        .detail("pending", false));
                }
            }
            return Err(error
                .outcome("unknown")
                .detail("restoreTaskId", journal.task_id.clone())
                .detail("pending", true));
        }
        self.tasks
            .phase(task_id, "source-restore-verify")
            .map_err(|error| {
                error
                    .outcome("unknown")
                    .detail("restoreTaskId", journal.task_id.clone())
                    .detail("pending", true)
            })?;
        let finish = (|| {
            verify_binding(project, &journal.plan.binding)?;
            verify_no_git_locks(project, &journal.plan.binding)?;
            let changes = classify_all(project, root, journal)?;
            verify_mixed_source(project, journal, &changes)?;
            let expected = if direction == RestoreAction::Undo {
                "before"
            } else {
                "after"
            };
            if changes.iter().any(|change| change.state != expected) {
                return Err(restore_changed());
            }
            persist_terminal(
                root,
                journal,
                if direction == RestoreAction::Undo {
                    "undone"
                } else {
                    "completed"
                },
                write_journal,
            )
        })();
        let warning = finish.map_err(|error| {
            error
                .outcome("unknown")
                .detail("restoreTaskId", journal.task_id.clone())
                .detail("pending", true)
        })?;
        let result = json!({"operation":operation,"operationOutcome":"committed","restoreTaskId":journal.task_id,"state":journal.phase,"pending":false,"targetCommit":journal.plan.target_commit,"head":journal.plan.binding.head,"branch":journal.plan.binding.branch,"paths":journal.entries.iter().map(|entry| &entry.path).collect::<Vec<_>>(),"database":database_boundary(),"issues":warning.iter().collect::<Vec<_>>(),"durabilityWarning":warning,"completedAt":now()});
        Ok(result)
    }
}
async fn apply_entries(
    project: &ProjectHandle,
    root: &Path,
    journal: &mut RestoreJournal,
    direction: RestoreAction,
    context: &mut Context,
    mut checkpoint: impl FnMut(usize, bool) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    verify_binding(project, &journal.plan.binding)?;
    verify_no_git_locks(project, &journal.plan.binding)?;
    let states = classify_all(project, root, journal)?;
    verify_mixed_source(project, journal, &states)?;
    context.checkpoint()?;
    journal.phase = if direction == RestoreAction::Undo {
        "undoing"
    } else {
        "applying"
    }
    .into();
    write_journal(root, journal)?;
    let order: Vec<usize> = if direction == RestoreAction::Undo {
        (0..journal.entries.len()).rev().collect()
    } else {
        (0..journal.entries.len()).collect()
    };
    for index in order {
        tokio::task::yield_now().await;
        context.checkpoint()?;
        checkpoint(index, false)?;
        verify_step_binding(project, &journal.plan.binding)?;
        let state = classify_entry(project, root, index, &journal.entries[index])?;
        if state == "unknown" {
            return Err(restore_changed());
        }
        let expected = if direction == RestoreAction::Undo {
            "after"
        } else {
            "before"
        };
        if state != expected {
            continue;
        }
        let entry = &journal.entries[index];
        let live = checked_relative(&project.root, &entry.path, true)?;
        let held = slot(root, index);
        ensure_live_parent(&project.root, &entry.path)?;
        match (entry.before.is_some(), entry.after.is_some(), direction) {
            (true, true, _) => move_preserving(&held, &live, true)?,
            (false, true, RestoreAction::Continue) | (true, false, RestoreAction::Undo) => {
                move_preserving(&held, &live, false)?
            }
            (true, false, RestoreAction::Continue) | (false, true, RestoreAction::Undo) => {
                move_preserving(&live, &held, false)?
            }
            _ => return Err(restore_changed()),
        }
        checkpoint(index, true)?;
        verify_step_binding(project, &journal.plan.binding)?;
        let after = classify_entry(project, root, index, &journal.entries[index])?;
        let expected = if direction == RestoreAction::Undo {
            "before"
        } else {
            "after"
        };
        if after != expected {
            return Err(restore_changed());
        }
    }
    if direction == RestoreAction::Continue {
        journal.phase = "applied".into();
        write_journal(root, journal)?;
    }
    Ok(())
}
fn verify_no_git_locks(project: &ProjectHandle, binding: &Binding) -> Result<(), ApiError> {
    for relative in [
        "HEAD.lock".to_owned(),
        "index.lock".into(),
        "packed-refs.lock".into(),
        format!("{}.lock", binding.branch),
    ] {
        let path = project.root.join(".git").join(relative);
        check_no_symlinks(&path, true)?;
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            _ => {
                return Err(restore_error(
                    "PROJECT_SOURCE_RESTORE_GIT_BUSY",
                    "发现外部 Git 操作锁；源码取回已停止，未删除或替换该锁。",
                ))
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "project_git_restore_tests.rs"]
mod tests;

/// atomic_json may publish its rename before its final directory fsync fails.
/// A read-back of the exact terminal document acknowledges the current commit
/// instead of advertising a recovery entry that the guard correctly sees as done.
fn persist_terminal(
    root: &Path,
    journal: &mut RestoreJournal,
    phase: &str,
    writer: impl FnOnce(&Path, &RestoreJournal) -> Result<(), ApiError>,
) -> Result<Option<ApiError>, ApiError> {
    journal.phase = phase.into();
    match writer(root, journal) {
        Ok(()) => Ok(None),
        Err(error) => {
            let actual = load_journal(root, &journal.project_id, &journal.task_id);
            if actual.is_ok_and(|actual| {
                serde_json::to_value(&actual).ok() == serde_json::to_value(&*journal).ok()
            }) {
                Ok(Some(restore_error("PROJECT_SOURCE_RESTORE_DURABILITY_UNCONFIRMED", "源码状态及最终记录已读回核对；最后一次目录同步未获确认，无法承诺断电耐久性。现场继续保留。")))
            } else {
                Err(error)
            }
        }
    }
}
