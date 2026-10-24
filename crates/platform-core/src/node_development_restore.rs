//! Explicit source-only restoration in the author's one existing repository.
//! Retained operation payloads are recovery evidence, never another Git history.
use super::*;
use crate::source_restore_guard::{PendingSourceRestore, SourceRestoreState};
use fs2::FileExt;

const RESTORE_ROOT: &str = "local-projects-node-restores";
const MAX_JOURNAL: u64 = 4 * 1024 * 1024;
const MAX_OPERATIONS: usize = 1000;
const MAX_JOURNAL_TOTAL: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeRestoreScope {
    pub app_id: String,
    pub registration_digest: String,
    pub control_path: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeRestorePreviewInput {
    pub target_commit: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeRestoreApplyInput {
    pub target_commit: String,
    pub expected_snapshot: String,
    pub expected_plan_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeRestoreRecoverInput {
    pub operation_id: String,
    pub expected_plan_digest: String,
    pub expected_journal_digest: String,
    pub action: RestoreAction,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stamp {
    descriptor: RestoreDescriptor,
    device: u64,
    inode: u64,
    mode: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetFile {
    oid: String,
    descriptor: RestoreDescriptor,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    source_root: PathBuf,
    root_identity: String,
    git_identity: String,
    repository_digest: String,
    head: String,
    branch: String,
    branch_fingerprint: Option<String>,
    manifest_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Plan {
    scope: NodeRestoreScope,
    binding: Binding,
    target_commit: String,
    snapshot_digest: String,
    before: BTreeMap<String, Stamp>,
    target: BTreeMap<String, TargetFile>,
    changes: Vec<RestoreChange>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    path: String,
    before: Option<Stamp>,
    after: Option<Stamp>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    version: u32,
    operation_id: String,
    phase: String,
    operation_identity: String,
    plan_digest: String,
    plan: Plan,
    entries: Vec<Entry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ControlMarker {
    version: u32,
    scope: NodeRestoreScope,
    source_root: PathBuf,
    operation_id: String,
    plan_digest: String,
    evidence_digest: String,
    phase: String,
}
fn read_control(path: &Path) -> Result<Option<ControlMarker>, ApiError> {
    check_no_symlinks(path, true)?;
    if !path.try_exists()? {
        return Ok(None);
    }
    let marker: ControlMarker =
        serde_json::from_slice(&read_file(path, 64 * 1024)?).map_err(|_| changed())?;
    validate_scope(&marker.scope)?;
    if marker.version != 1
        || marker.scope.control_path != path
        || !valid_operation(&marker.operation_id)
        || !matches!(marker.phase.as_str(), "pending" | "completed" | "undone")
    {
        return Err(changed());
    }
    validate_digest(&marker.plan_digest)?;
    validate_digest(&marker.evidence_digest)?;
    Ok(Some(marker))
}
fn control_is_terminal(marker: &ControlMarker) -> Result<bool, ApiError> {
    if terminal(&marker.phase) {
        return Ok(true);
    }
    let path = operation(&marker.source_root, &marker.operation_id)?;
    let (journal, _) = read_journal(&path)?;
    if journal.plan.scope != marker.scope
        || journal.plan_digest != marker.plan_digest
        || evidence_digest(&journal)? != marker.evidence_digest
    {
        return Err(changed());
    }
    if !terminal(&journal.phase) {
        return Ok(false);
    }
    let states = classify(&marker.source_root, &path, &journal)?;
    let expected = if journal.phase == "undone" {
        "before"
    } else {
        "after"
    };
    if states.iter().any(|state| state != expected) {
        return Err(changed());
    }
    Ok(true)
}
pub(crate) fn require_control_clear(path: &Path) -> Result<(), ApiError> {
    if let Some(marker) = read_control(path)? {
        if !control_is_terminal(&marker)? {
            return Err(pending());
        }
    }
    Ok(())
}
fn verify_control(journal: &Journal) -> Result<(), ApiError> {
    match read_control(&journal.plan.scope.control_path)? {
        Some(marker)
            if marker.scope == journal.plan.scope
                && marker.source_root == journal.plan.binding.source_root
                && marker.operation_id == journal.operation_id
                && marker.plan_digest == journal.plan_digest
                && marker.evidence_digest == evidence_digest(journal)?
                && marker.phase == "pending" =>
        {
            Ok(())
        }
        None if journal.phase == "prepared" => Ok(()),
        _ => Err(changed()),
    }
}
fn write_control(journal: &Journal, phase: &str) -> Result<(), ApiError> {
    let path = &journal.plan.scope.control_path;
    if let Some(previous) = read_control(path)? {
        if previous.operation_id != journal.operation_id && !control_is_terminal(&previous)? {
            return Err(changed());
        }
        if previous.operation_id == journal.operation_id
            && (previous.scope != journal.plan.scope
                || previous.plan_digest != journal.plan_digest
                || previous.evidence_digest != evidence_digest(journal)?)
        {
            return Err(changed());
        }
    }
    atomic_json(
        path,
        &ControlMarker {
            version: 1,
            scope: journal.plan.scope.clone(),
            source_root: journal.plan.binding.source_root.clone(),
            operation_id: journal.operation_id.clone(),
            plan_digest: journal.plan_digest.clone(),
            evidence_digest: evidence_digest(journal)?,
            phase: phase.into(),
        },
    )
}
fn evidence_digest(journal: &Journal) -> Result<String, ApiError> {
    fingerprint(
        &json!({"version":journal.version,"operationId":journal.operation_id,"operationIdentity":journal.operation_identity,"planDigest":journal.plan_digest,"entries":journal.entries}),
    )
}

fn issue(code: &str, message: &str) -> ApiError {
    error(code, message)
}
fn changed() -> ApiError {
    issue(
        "NODE_SOURCE_RESTORE_CHANGED",
        "源码、仓库或恢复现场出现计划外变化；已停止并保留现场，不会覆盖第三方内容。",
    )
}
fn pending() -> ApiError {
    issue(
        "NODE_SOURCE_RESTORE_PENDING",
        "源码取回尚未处理完成；请核对后明确继续或撤回，再构建、改绑或保存版本。",
    )
}
fn terminal(phase: &str) -> bool {
    matches!(phase, "completed" | "undone")
}
fn valid_operation(value: &str) -> bool {
    value
        .strip_prefix("restore_")
        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
}
fn validate_scope(scope: &NodeRestoreScope) -> Result<(), ApiError> {
    if uuid::Uuid::parse_str(&scope.app_id).is_err()
        || !scope.control_path.is_absolute()
        || scope
            .control_path
            .file_name()
            .and_then(|name| name.to_str())
            != Some("source-restore.json")
        || scope
            .control_path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            != Some(scope.app_id.as_str())
    {
        return Err(input_error());
    }
    // Completed evidence outlives its owning instance's permitted deletion.
    // Missing control parents must not poison that author's retained repository.
    check_no_symlinks(scope.control_path.parent().ok_or_else(input_error)?, true)?;
    validate_digest(&scope.registration_digest)
}
fn operations(root: &Path) -> PathBuf {
    root.join(".git").join(RESTORE_ROOT)
}
fn operation(root: &Path, id: &str) -> Result<PathBuf, ApiError> {
    if !valid_operation(id) {
        return Err(input_error());
    }
    let path = operations(root).join(id);
    check_no_symlinks(&path, false)?;
    Ok(path)
}
fn fingerprint(value: &impl Serialize) -> Result<String, ApiError> {
    Ok(sha256(&serde_json::to_vec(value)?))
}
fn descriptor(file: &SourceFile) -> RestoreDescriptor {
    RestoreDescriptor {
        sha256: file.digest.clone(),
        size: file.size,
        executable: file.mode == "100755",
    }
}
fn stamp(path: &Path) -> Result<Option<Stamp>, ApiError> {
    check_no_symlinks(path, true)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let bytes = read_file(path, MAX_BLOB)?;
    let after = fs::symlink_metadata(path)?;
    if !same_file(&metadata, &after) {
        return Err(changed());
    }
    Ok(Some(Stamp {
        descriptor: RestoreDescriptor {
            sha256: sha256(&bytes),
            size: bytes.len() as u64,
            executable: metadata.mode() & 0o111 != 0,
        },
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
    }))
}
fn raw_snapshot(root: &Path) -> Result<BTreeMap<String, Stamp>, ApiError> {
    let scan = scan_node(root)?;
    if !scan.issues.is_empty() || !scan.unclassified.is_empty() || !scan.markers.is_empty() {
        return Err(changed());
    }
    scan.files
        .keys()
        .map(|path| {
            Ok((
                path.clone(),
                stamp(&checked_relative(root, path, false)?)?.ok_or_else(changed)?,
            ))
        })
        .collect()
}
fn binding(root: &Path, snapshot: &NodeSnapshot) -> Result<Binding, ApiError> {
    let head = snapshot
        .view
        .repository
        .head
        .as_ref()
        .ok_or_else(input_error)?;
    let branch = head.branch.clone().ok_or_else(input_error)?;
    Ok(Binding {
        source_root: root.into(),
        root_identity: node_root_identity(root)?,
        git_identity: identity(&root.join(".git"))?,
        repository_digest: repository_digest(&root.join(".git"))?,
        head: head.oid.clone().ok_or_else(input_error)?,
        branch_fingerprint: index_fingerprint(&root.join(".git").join(&branch))?,
        branch,
        manifest_digest: sha256(&read_file(&root.join("local-app.json"), MAX_METADATA)?),
    })
}
fn verify_binding(root: &Path, expected: &Binding) -> Result<(), ApiError> {
    let git = root.join(".git");
    if root != expected.source_root
        || node_root_identity(root)? != expected.root_identity
        || identity(&git)? != expected.git_identity
        || repository_digest(&git)? != expected.repository_digest
        || index_fingerprint(&git.join(&expected.branch))? != expected.branch_fingerprint
        || read_file(&git.join("HEAD"), 4096)? != format!("ref: {}\n", expected.branch).as_bytes()
        || sha256(&read_file(&root.join("local-app.json"), MAX_METADATA)?)
            != expected.manifest_digest
    {
        return Err(changed());
    }
    let mut issues = vec![];
    inspect_repository_files(&git, None, &mut issues)?;
    if !issues.is_empty() {
        return Err(changed());
    }
    Ok(())
}
fn read_journal(path: &Path) -> Result<(Journal, String), ApiError> {
    let bytes = read_file(&path.join("journal.json"), MAX_JOURNAL)?;
    let journal: Journal = serde_json::from_slice(&bytes).map_err(|_| changed())?;
    if journal.version != 1
        || !valid_operation(&journal.operation_id)
        || path.file_name().and_then(|v| v.to_str()) != Some(journal.operation_id.as_str())
        || !matches!(
            journal.phase.as_str(),
            "prepared" | "applying" | "undoing" | "completed" | "undone"
        )
        || identity(path)? != journal.operation_identity
        || fingerprint(&journal.plan)? != journal.plan_digest
        || journal.entries.len() != journal.plan.changes.len()
    {
        return Err(changed());
    }
    validate_scope(&journal.plan.scope)?;
    let source_root = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(changed)?;
    if source_root != journal.plan.binding.source_root
        || journal.plan.target_commit.len() != 40
        || !valid_oid(&journal.plan.target_commit)
        || journal.plan.binding.head.len() != 40
        || !valid_oid(&journal.plan.binding.head)
        || one_ref(format!("{}\n", journal.plan.binding.branch).as_bytes()).is_err()
        || journal.plan.changes.is_empty()
        || journal.plan.changes.len() > MAX_FILES
    {
        return Err(changed());
    }
    crate::project_git::source_restore::validate_collisions(
        journal
            .plan
            .changes
            .iter()
            .map(|change| change.path.as_str()),
    )?;
    for (entry, change) in journal.entries.iter().zip(&journal.plan.changes) {
        valid_relative(&entry.path)?;
        if entry.path != change.path
            || !source_path_allowed(&entry.path)
            || entry.before.as_ref() != journal.plan.before.get(&entry.path)
            || entry.before.as_ref().map(|s| &s.descriptor) != change.before.as_ref()
            || entry.after.as_ref().map(|s| &s.descriptor) != change.after.as_ref()
        {
            return Err(changed());
        }
    }
    Ok((journal, sha256(&bytes)))
}
fn write_journal(path: &Path, journal: &Journal) -> Result<(), ApiError> {
    reserved_journal_size(journal)?;
    atomic_json(&path.join("journal.json"), journal)
}
fn reserved_journal_size(journal: &Journal) -> Result<u64, ApiError> {
    let mut longest = journal.clone();
    longest.phase = "completed".into();
    let size = serde_json::to_vec_pretty(&longest)?.len() as u64;
    if size > MAX_JOURNAL {
        return Err(limit());
    }
    Ok(size)
}
fn check_capacity(count: usize, bytes: u64, reservation: u64) -> Result<(), ApiError> {
    if count >= MAX_OPERATIONS
        || bytes
            .checked_add(reservation)
            .is_none_or(|total| total > MAX_JOURNAL_TOTAL)
    {
        return Err(limit());
    }
    Ok(())
}
fn reserve_capacity(root: &Path, journal: &Journal) -> Result<(), ApiError> {
    let mut count = 0;
    let mut bytes = 0u64;
    for item in fs::read_dir(operations(root))? {
        let item = item?;
        if item.file_name() == "operation.guard" {
            continue;
        }
        let journal_path = item.path().join("journal.json");
        check_no_symlinks(&journal_path, false)?;
        read_journal(&item.path())?;
        count += 1;
        bytes = bytes
            .checked_add(fs::symlink_metadata(journal_path)?.len())
            .ok_or_else(limit)?;
    }
    check_capacity(count, bytes, reserved_journal_size(journal)?)
}

// Every partially applied/undone tree must remain readable by scan_node. Reserve
// the union, including existing empty/protected entries and all target parents;
// two independently admissible endpoint trees can otherwise exceed the scanner
// budget between an add and a later delete and strand their own recovery.
fn source_union_budget(
    existing: &BTreeSet<String>,
    before: &BTreeMap<String, u64>,
    target: &BTreeMap<String, u64>,
) -> Result<(), ApiError> {
    let mut entries = existing.clone();
    let mut sizes = before.clone();
    for (path, size) in target {
        valid_relative(path)?;
        let components: Vec<_> = path.split('/').collect();
        if components.len() > 65 || *size > MAX_BLOB {
            return Err(limit());
        }
        for end in 1..=components.len() {
            entries.insert(components[..end].join("/"));
            if entries.len() > MAX_FILES {
                return Err(limit());
            }
        }
        sizes
            .entry(path.clone())
            .and_modify(|before| *before = (*before).max(*size))
            .or_insert(*size);
    }
    let bytes = sizes.values().try_fold(0u64, |total, size| {
        total.checked_add(*size).ok_or_else(limit)
    })?;
    if entries.len() > MAX_FILES || bytes > MAX_BYTES {
        return Err(limit());
    }
    Ok(())
}

pub(crate) fn guard_state(root: &Path) -> SourceRestoreState {
    let mut state = SourceRestoreState::default();
    let result = (|| -> Result<(), ApiError> {
        let parent = operations(root);
        check_no_symlinks(&parent, true)?;
        if !parent.try_exists()? {
            return Ok(());
        }
        let mut journal_bytes = 0u64;
        let mut count = 0;
        for item in fs::read_dir(&parent)? {
            let item = item?;
            let name = item.file_name();
            if name == "operation.guard" {
                continue;
            }
            count += 1;
            if count > MAX_OPERATIONS {
                return Err(limit());
            }
            let name = name.to_str().ok_or_else(changed)?;
            if !valid_operation(name) {
                return Err(changed());
            }
            let journal_path = item.path().join("journal.json");
            check_no_symlinks(&journal_path, false)?;
            journal_bytes = journal_bytes
                .checked_add(fs::symlink_metadata(&journal_path)?.len())
                .ok_or_else(limit)?;
            if journal_bytes > MAX_JOURNAL_TOTAL {
                return Err(limit());
            }
            let (journal, digest) = read_journal(&item.path())?;
            if !terminal(&journal.phase) {
                state.pending.push(PendingSourceRestore {
                    task_id: journal.operation_id,
                    phase: journal.phase,
                    plan_digest: journal.plan_digest,
                    journal_digest: digest,
                });
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        state.issues.push(error);
    }
    state
}
pub(crate) fn require_clear(root: &Path) -> Result<(), ApiError> {
    let state = guard_state(root);
    if let Some(issue) = state.issues.into_iter().next() {
        return Err(issue);
    }
    if !state.pending.is_empty() {
        return Err(pending());
    }
    Ok(())
}
struct OperationLock {
    _file: File,
}
fn lock(root: &Path) -> Result<OperationLock, ApiError> {
    let git = root.join(".git");
    check_no_symlinks(&git, false)?;
    if !fs::symlink_metadata(&git)?.is_dir() {
        return Err(changed());
    }
    let parent = operations(root);
    check_no_symlinks(&parent, true)?;
    private_dir(&parent)?;
    let path = parent.join("operation.guard");
    check_no_symlinks(&path, true)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    if !file.metadata()?.is_file() || file.metadata()?.nlink() != 1 {
        return Err(changed());
    }
    file.try_lock_exclusive().map_err(|_| {
        issue(
            "NODE_SOURCE_RESTORE_BUSY",
            "此源码仓库已有取回操作，请稍后核对。",
        )
    })?;
    Ok(OperationLock { _file: file })
}

impl NodeGit {
    async fn restore_plan(
        &self,
        root: &Path,
        scope: &NodeRestoreScope,
        target: &str,
        context: &mut Context,
        tools: &GitToolchain,
    ) -> Result<Plan, ApiError> {
        validate_scope(scope)?;
        require_control_clear(&scope.control_path)?;
        if target.len() != 40 || !valid_oid(target) {
            return Err(input_error());
        }
        require_clear(root)?;
        let snapshot = inspect_node(root, Some(tools), context, None).await?;
        if !snapshot.view.can_save
            || !snapshot.view.changes.is_empty()
            || head_oid(&snapshot.view).is_none()
        {
            return Err(issue(
                "NODE_SOURCE_RESTORE_NOT_CLEAN",
                "请先保存并处理当前源码、暂存与冲突，再取回已保存版本。",
            ));
        }
        let bound = binding(root, &snapshot)?;
        let mut command = node_command(tools, context.root(), Some(root));
        command.args(["cat-file", "-t", target]);
        if context.run(command, vec![]).await?.success()? != b"commit\n" {
            return Err(input_error());
        }
        let mut command = node_command(tools, context.root(), Some(root));
        command.args(["ls-tree", "-r", "-z", "--full-tree", target]);
        let mut issues = vec![];
        let tree = parse_tree(
            &context.run(command, vec![]).await?.success()?,
            false,
            &mut issues,
        )?;
        if !issues.is_empty() || tree.len() > MAX_FILES {
            return Err(changed());
        }
        crate::project_git::source_restore::validate_collisions(tree.keys().map(String::as_str))?;
        let output = scan_node(root)?.output_dir;
        let mut targets = BTreeMap::new();
        let mut total = 0u64;
        for (path, file) in tree {
            if !source_path_allowed(&path)
                || output.as_ref().is_some_and(|output| {
                    path == *output || path.starts_with(&format!("{output}/"))
                })
            {
                return Err(issue(
                    "NODE_SOURCE_RESTORE_SCOPE_UNSUPPORTED",
                    "目标提交含受保护或未支持文件，未部分取回。",
                )
                .detail("path", path));
            }
            let mut command = node_command(tools, context.root(), Some(root));
            command.args(["cat-file", "-s", &file.1]);
            let size: u64 = std::str::from_utf8(&context.run(command, vec![]).await?.success()?)
                .map_err(|_| output_invalid())?
                .trim()
                .parse()
                .map_err(|_| output_invalid())?;
            total = total.checked_add(size).ok_or_else(limit)?;
            if size > MAX_BLOB || total > MAX_BYTES {
                return Err(limit());
            }
            let mut command = node_command(tools, context.root(), Some(root));
            command.args(["cat-file", "blob", &file.1]);
            let bytes = context.run(command, vec![]).await?.success()?;
            if bytes.len() as u64 != size {
                return Err(output_invalid());
            }
            if path == "local-app.json" && sha256(&bytes) != bound.manifest_digest {
                return Err(issue(
                    "NODE_SOURCE_RESTORE_MANIFEST_CHANGED",
                    "目标版本的 local-app.json 必须与当前声明逐字节一致。",
                ));
            }
            targets.insert(
                path,
                TargetFile {
                    oid: file.1,
                    descriptor: RestoreDescriptor {
                        sha256: sha256(&bytes),
                        size,
                        executable: file.0 == "100755",
                    },
                },
            );
        }
        if !targets.contains_key("local-app.json") {
            return Err(issue(
                "NODE_SOURCE_RESTORE_MANIFEST_CHANGED",
                "目标提交没有当前应用声明。",
            ));
        }
        let before = raw_snapshot(root)?;
        let scan = scan_node(root)?;
        source_union_budget(
            &scan.entries,
            &before
                .iter()
                .map(|(path, stamp)| (path.clone(), stamp.descriptor.size))
                .collect(),
            &targets
                .iter()
                .map(|(path, target)| (path.clone(), target.descriptor.size))
                .collect(),
        )?;
        if targets
            .keys()
            .any(|path| before.contains_key(path) && !snapshot.files.contains_key(path))
        {
            return Err(issue(
                "NODE_SOURCE_RESTORE_SCOPE_UNSUPPORTED",
                "目标版本会覆盖当前忽略的本机文件，未执行取回。",
            ));
        }
        for path in targets.keys() {
            if fs::symlink_metadata(checked_relative(root, path, true)?)
                .is_ok_and(|metadata| metadata.is_dir())
            {
                return Err(issue(
                    "NODE_SOURCE_RESTORE_SCOPE_UNSUPPORTED",
                    "目标源码与当前目录冲突，未执行取回。",
                ));
            }
        }
        let collisions: BTreeSet<_> = before.keys().chain(targets.keys()).cloned().collect();
        crate::project_git::source_restore::validate_collisions(
            collisions.iter().map(String::as_str),
        )?;
        let paths: BTreeSet<_> = snapshot
            .files
            .keys()
            .chain(targets.keys())
            .cloned()
            .collect();
        crate::project_git::source_restore::validate_collisions(paths.iter().map(String::as_str))?;
        let changes = paths
            .into_iter()
            .filter_map(|path| {
                let old = snapshot.files.get(&path).map(descriptor);
                let new = targets.get(&path).map(|v| v.descriptor.clone());
                (old != new).then(|| RestoreChange {
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
                })
            })
            .collect();
        verify_binding(root, &bound)?;
        let verify = inspect_node(root, Some(tools), context, None).await?;
        if snapshot.view.snapshot_digest != verify.view.snapshot_digest
            || raw_snapshot(root)? != before
        {
            return Err(changed());
        }
        let plan = Plan {
            scope: scope.clone(),
            binding: bound,
            target_commit: target.into(),
            snapshot_digest: snapshot.view.snapshot_digest,
            before,
            target: targets,
            changes,
        };
        if serde_json::to_vec(&plan)?.len() as u64 > MAX_JOURNAL / 2 {
            return Err(limit());
        }
        Ok(plan)
    }
    pub async fn restore_preview(
        &self,
        root: &Path,
        scope: &NodeRestoreScope,
        input: NodeRestorePreviewInput,
    ) -> Result<Value, ApiError> {
        let (mut context, _senders) = self.context()?;
        let tools = context.resolve(&self.environment).await?;
        let plan = self
            .restore_plan(root, scope, &input.target_commit, &mut context, &tools)
            .await?;
        Ok(
            json!({"targetCommit":plan.target_commit,"head":plan.binding.head,"branch":plan.binding.branch,"snapshotDigest":plan.snapshot_digest,"planDigest":fingerprint(&plan)?,"changes":plan.changes,"canApply":!plan.changes.is_empty(),"repositoryRoot":root,"database":node_boundary()}),
        )
    }
    pub async fn restore_apply(
        &self,
        root: &Path,
        scope: &NodeRestoreScope,
        input: NodeRestoreApplyInput,
    ) -> Result<Value, ApiError> {
        self.restore_apply_with(root, scope, input, |_, _| Ok(()))
            .await
    }
    async fn restore_apply_with(
        &self,
        root: &Path,
        scope: &NodeRestoreScope,
        input: NodeRestoreApplyInput,
        mut checkpoint: impl FnMut(usize, bool) -> Result<(), ApiError>,
    ) -> Result<Value, ApiError> {
        #[cfg(not(target_os = "macos"))]
        return Err(issue(
            "UNSUPPORTED_PLATFORM",
            "Node 源码取回目前仅验证 macOS。",
        ));
        let (mut context, _senders) = self.context()?;
        let tools = context.resolve(&self.environment).await?;
        let plan = self
            .restore_plan(root, scope, &input.target_commit, &mut context, &tools)
            .await?;
        if input.expected_snapshot != plan.snapshot_digest
            || input.expected_plan_digest != fingerprint(&plan)?
        {
            return Err(changed());
        }
        if plan.changes.is_empty() {
            return Err(issue(
                "NODE_SOURCE_RESTORE_NO_CHANGES",
                "目标源码与当前相同，未创建空操作。",
            ));
        }
        let operation_lock = Arc::new(lock(root)?);
        context.retain(operation_lock);
        require_clear(root)?;
        verify_binding(root, &plan.binding)?;
        if raw_snapshot(root)? != plan.before {
            return Err(changed());
        }
        let id = format!("restore_{}", uuid::Uuid::new_v4());
        let staging_parent = root
            .join(".git")
            .join("local-projects-node-restore-preparing");
        check_no_symlinks(&staging_parent, true)?;
        private_dir(&staging_parent)?;
        let retained = staging_parent.join(&id);
        fs::create_dir(&retained)?;
        fs::set_permissions(&retained, fs::Permissions::from_mode(0o700))?;
        let payload = retained.join("payload");
        fs::create_dir(&payload)?;
        let mut entries = vec![];
        for (index, change) in plan.changes.iter().enumerate() {
            context.checkpoint()?;
            let destination = payload.join(index.to_string());
            let after = if let Some(target) = plan.target.get(&change.path) {
                let mut command = node_command(&tools, context.root(), Some(root));
                command.args(["cat-file", "blob", &target.oid]);
                let bytes = context.run(command, vec![]).await?.success()?;
                if sha256(&bytes) != target.descriptor.sha256
                    || bytes.len() as u64 != target.descriptor.size
                {
                    return Err(changed());
                }
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(if target.descriptor.executable {
                        0o700
                    } else {
                        0o600
                    })
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&destination)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
                stamp(&destination)?
            } else {
                None
            };
            entries.push(Entry {
                path: change.path.clone(),
                before: plan.before.get(&change.path).cloned(),
                after,
            });
        }
        File::open(&payload)?.sync_all()?;
        let mut journal = Journal {
            version: 1,
            operation_id: id,
            phase: "prepared".into(),
            operation_identity: identity(&retained)?,
            plan_digest: fingerprint(&plan)?,
            plan,
            entries,
        };
        verify_binding(root, &journal.plan.binding)?;
        if raw_snapshot(root)? != journal.plan.before {
            return Err(changed());
        }
        write_journal(&retained, &journal)?;
        File::open(&retained)?.sync_all()?;
        reserve_capacity(root, &journal)?;
        let published = operations(root).join(&journal.operation_id);
        crate::project_git::source_restore::move_preserving(&retained, &published, false)?;
        write_control(&journal, "pending")?;
        drive(
            root,
            &published,
            &mut journal,
            RestoreAction::Continue,
            &mut context,
            &mut checkpoint,
        )
        .await
        .map_err(|error| {
            error
                .outcome("unknown")
                .detail("operationId", journal.operation_id.clone())
                .detail("evidencePath", json!(published))
        })
    }
    pub fn restore_status(&self, root: &Path, scope: &NodeRestoreScope) -> Result<Value, ApiError> {
        validate_scope(scope)?;
        let state = guard_state(root);
        let mut values = vec![];
        let mut issues = state.issues;
        if let Some(marker) = read_control(&scope.control_path)? {
            if !terminal(&marker.phase) {
                if marker.scope != *scope || marker.source_root != root {
                    issues.push(changed());
                } else {
                    match control_is_terminal(&marker) {
                        Ok(false)
                            if !state
                                .pending
                                .iter()
                                .any(|pending| pending.task_id == marker.operation_id) =>
                        {
                            issues.push(changed())
                        }
                        Err(error) => issues.push(error),
                        _ => (),
                    }
                }
            }
        }
        for pending in state.pending {
            let path = operation(root, &pending.task_id)?;
            let (journal, digest) = read_journal(&path)?;
            let result =
                verify_scope(root, scope, &journal).and_then(|()| classify(root, &path, &journal));
            let (changes, can_recover) = match result {
                Ok(changes) => (changes, true),
                Err(error) => {
                    issues.push(error);
                    (vec![], false)
                }
            };
            let changes: Vec<_> = journal
                .entries
                .iter()
                .zip(changes)
                .map(|(entry, state)| json!({"path":entry.path,"state":state}))
                .collect();
            values.push(json!({"operationId":journal.operation_id,"phase":journal.phase,"targetCommit":journal.plan.target_commit,"planDigest":journal.plan_digest,"journalDigest":digest,"canContinue":can_recover,"canUndo":can_recover,"changes":changes}));
        }
        Ok(
            json!({"state":if values.is_empty() && issues.is_empty(){"clear"}else{"needs-attention"},"operations":values,"issues":issues,"database":node_boundary()}),
        )
    }
    pub async fn restore_recover(
        &self,
        root: &Path,
        scope: &NodeRestoreScope,
        input: NodeRestoreRecoverInput,
    ) -> Result<Value, ApiError> {
        self.restore_recover_with(root, scope, input, |_, _| Ok(()))
            .await
    }
    async fn restore_recover_with(
        &self,
        root: &Path,
        scope: &NodeRestoreScope,
        input: NodeRestoreRecoverInput,
        mut checkpoint: impl FnMut(usize, bool) -> Result<(), ApiError>,
    ) -> Result<Value, ApiError> {
        let (mut context, _senders) = self.context()?;
        let operation_lock = Arc::new(lock(root)?);
        context.retain(operation_lock);
        let state = guard_state(root);
        if !state.issues.is_empty()
            || state.pending.len() != 1
            || state.pending[0].task_id != input.operation_id
        {
            return Err(changed());
        }
        let retained = operation(root, &input.operation_id)?;
        let (mut journal, digest) = read_journal(&retained)?;
        if terminal(&journal.phase)
            || input.expected_plan_digest != journal.plan_digest
            || input.expected_journal_digest != digest
        {
            return Err(changed());
        }
        verify_scope(root, scope, &journal)?;
        verify_control(&journal)?;
        write_control(&journal, "pending")?;
        drive(
            root,
            &retained,
            &mut journal,
            input.action,
            &mut context,
            &mut checkpoint,
        )
        .await
        .map_err(|error| {
            error
                .outcome("unknown")
                .detail("operationId", journal.operation_id.clone())
                .detail("evidencePath", json!(retained))
        })
    }
}
fn verify_scope(root: &Path, scope: &NodeRestoreScope, journal: &Journal) -> Result<(), ApiError> {
    if scope != &journal.plan.scope {
        return Err(changed());
    }
    verify_binding(root, &journal.plan.binding)
}
fn classify(root: &Path, retained: &Path, journal: &Journal) -> Result<Vec<String>, ApiError> {
    verify_binding(root, &journal.plan.binding)?;
    verify_control(journal)?;
    let mut states = vec![];
    let mut expected = journal.plan.before.clone();
    for (index, entry) in journal.entries.iter().enumerate() {
        let live = stamp(&checked_relative(root, &entry.path, true)?)?;
        let held = stamp(&retained.join("payload").join(index.to_string()))?;
        let before = live == entry.before && held == entry.after;
        let after = live == entry.after && held == entry.before;
        let state = if before {
            "before"
        } else if after {
            "after"
        } else {
            return Err(changed());
        };
        if state == "after" {
            if let Some(after) = &entry.after {
                expected.insert(entry.path.clone(), after.clone());
            } else {
                expected.remove(&entry.path);
            }
        }
        states.push(state.into());
    }
    if raw_snapshot(root)? != expected {
        return Err(changed());
    }
    Ok(states)
}
async fn drive(
    root: &Path,
    retained: &Path,
    journal: &mut Journal,
    direction: RestoreAction,
    context: &mut Context,
    checkpoint: &mut impl FnMut(usize, bool) -> Result<(), ApiError>,
) -> Result<Value, ApiError> {
    classify(root, retained, journal)?;
    journal.phase = if direction == RestoreAction::Undo {
        "undoing"
    } else {
        "applying"
    }
    .into();
    write_journal(retained, journal)?;
    let mut order: Vec<_> = (0..journal.entries.len()).collect();
    if direction == RestoreAction::Undo {
        order.reverse();
    }
    for index in order {
        context.checkpoint()?;
        checkpoint(index, false)?;
        let states = classify(root, retained, journal)?;
        let expected = if direction == RestoreAction::Undo {
            "after"
        } else {
            "before"
        };
        if states[index] != expected {
            continue;
        }
        let entry = &journal.entries[index];
        crate::project_git::source_restore::ensure_live_parent(root, &entry.path)?;
        let live = checked_relative(root, &entry.path, true)?;
        let held = retained.join("payload").join(index.to_string());
        match (entry.before.is_some(), entry.after.is_some(), direction) {
            (true, true, _) => {
                crate::project_git::source_restore::move_preserving(&held, &live, true)?
            }
            (false, true, RestoreAction::Continue) | (true, false, RestoreAction::Undo) => {
                crate::project_git::source_restore::move_preserving(&held, &live, false)?
            }
            (true, false, RestoreAction::Continue) | (false, true, RestoreAction::Undo) => {
                crate::project_git::source_restore::move_preserving(&live, &held, false)?
            }
            _ => return Err(changed()),
        }
        classify(root, retained, journal)?;
        checkpoint(index, true)?;
        write_journal(retained, journal)?;
        tokio::task::yield_now().await;
    }
    let states = classify(root, retained, journal)?;
    let expected = if direction == RestoreAction::Undo {
        "before"
    } else {
        "after"
    };
    if states.iter().any(|state| state != expected) {
        return Err(changed());
    }
    journal.phase = if direction == RestoreAction::Undo {
        "undone"
    } else {
        "completed"
    }
    .into();
    write_journal(retained, journal)?;
    let control_warning = write_control(journal, &journal.phase).is_err();
    Ok(
        json!({"controlUpdateWarning":control_warning,"operationId":journal.operation_id,"state":journal.phase,"operationOutcome":"committed","sourceOnly":true,"evidencePath":retained,"database":node_boundary()}),
    )
}

#[cfg(test)]
#[path = "node_development_restore_tests.rs"]
mod tests;
