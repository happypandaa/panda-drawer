//! Persistent, project-local protection for an interrupted source-only restore.
//! This module never changes runtime identity, maintenance state or project files.
use crate::{
    projects::{check_no_symlinks, valid_id},
    ApiError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

const ROOT: &str = ".platform/project-state/source-restore";
// Internal metadata inspection budgets, not product storage quotas.
const MAX_JOURNAL: u64 = 4 * 1024 * 1024;
const MAX_TASKS: usize = 1000;
const MAX_PROJECT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_WORKSPACE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_WORKSPACE_ENTRIES: usize = 4096;
const LONGEST_PHASE: usize = "not-applied".len();
// No capacity is persisted or retained after publication. Serializing the scan
// and directory publication closes concurrent admissions from different projects.
static PUBLICATION_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy)]
struct PublicationLimits {
    project_bytes: u64,
    project_tasks: usize,
    workspace_bytes: u64,
    workspace_entries: usize,
}

/// Keep this permit until the prepared directory's exclusive publication and
/// parent-directory sync finish. It must not be held across an await point.
#[must_use = "hold the permit until the prepared directory has been published and synced"]
pub(crate) struct SourceRestorePublicationPermit {
    _publication_lock: MutexGuard<'static, ()>,
}

/// `max_journal_bytes` is the largest exact serialized journal over every phase,
/// including terminal phases. The transaction, which owns that format, computes
/// it before calling this function. This function does not create any files.
pub(crate) fn reserve_publication(
    workspace: &Path,
    project_id: &str,
    task_id: &str,
    max_journal_bytes: u64,
) -> Result<SourceRestorePublicationPermit, ApiError> {
    reserve_publication_with_limits(
        workspace,
        project_id,
        task_id,
        max_journal_bytes,
        PublicationLimits {
            project_bytes: MAX_PROJECT_BYTES,
            project_tasks: MAX_TASKS,
            workspace_bytes: MAX_WORKSPACE_BYTES,
            workspace_entries: MAX_WORKSPACE_ENTRIES,
        },
    )
}

fn reserve_publication_with_limits(
    workspace: &Path,
    project_id: &str,
    task_id: &str,
    max_journal_bytes: u64,
    limits: PublicationLimits,
) -> Result<SourceRestorePublicationPermit, ApiError> {
    if !valid_id(project_id) || !valid_task(task_id) {
        return Err(invalid());
    }
    if max_journal_bytes == 0 || max_journal_bytes > MAX_JOURNAL {
        return Err(capacity());
    }
    // The lock protects no in-memory accounting: after a panic, a new full scan
    // safely recovers any published directory instead of keeping a stale quota.
    let publication_lock = PUBLICATION_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    check_publication_capacity(workspace, project_id, task_id, max_journal_bytes, limits).map_err(
        |error| {
            if error.code == "PROJECT_SOURCE_RESTORE_LIMIT" {
                capacity()
            } else {
                error
            }
        },
    )?;
    Ok(SourceRestorePublicationPermit {
        _publication_lock: publication_lock,
    })
}

fn check_publication_capacity(
    workspace: &Path,
    project_id: &str,
    task_id: &str,
    max_journal_bytes: u64,
    limits: PublicationLimits,
) -> Result<(), ApiError> {
    let mut budget = ReadBudget {
        remaining_bytes: limits.workspace_bytes,
        remaining_entries: limits.workspace_entries,
    };
    // Charge the new task before reading any existing content. A new project
    // directory is charged below only if it is not already in the directory scan.
    budget.entry()?;
    budget.bytes(max_journal_bytes)?;
    if limits.project_tasks == 0 || max_journal_bytes > limits.project_bytes {
        return Err(capacity());
    }
    let root = workspace.join(ROOT);
    let mut target_present = false;
    if directory_present(&root)? {
        for project in fs::read_dir(&root).map_err(|_| invalid())? {
            budget.entry()?;
            let project = project.map_err(|_| invalid())?;
            let id = project.file_name().into_string().map_err(|_| invalid())?;
            if !valid_id(&id) || !directory_present(&project.path())? {
                return Err(invalid());
            }
            let target = id == project_id;
            target_present |= target;
            let mut project_bytes = if target { max_journal_bytes } else { 0 };
            let mut project_tasks = usize::from(target);
            for entry in fs::read_dir(project.path()).map_err(|_| invalid())? {
                project_tasks += 1;
                if project_tasks > limits.project_tasks {
                    return Err(capacity());
                }
                budget.entry()?;
                let entry = entry.map_err(|_| invalid())?;
                let task = entry.file_name().into_string().map_err(|_| invalid())?;
                if !valid_task(&task) || !directory_present(&entry.path())? {
                    return Err(invalid());
                }
                if target && task == task_id {
                    return Err(changed());
                }
                let bytes = read_journal(
                    &entry.path().join("journal.json"),
                    &mut budget,
                    limits.project_bytes.saturating_sub(project_bytes),
                )?;
                let header = journal_header(&bytes, &id, &task)?;
                let pending = pending_phase(&header.phase);
                if target && pending {
                    return Err(ApiError::new(
                        "PROJECT_SOURCE_RESTORE_PENDING",
                        "已有源码取回操作尚未核对完成；请先处理对应操作。",
                    )
                    .detail("taskId", task));
                }
                // The transaction changes only the phase after publication.
                // Account for an in-flight journal's longest future spelling,
                // even when another project finishes while this scan proceeds.
                let growth = if pending {
                    (LONGEST_PHASE - header.phase.len()) as u64
                } else {
                    0
                };
                if bytes.len() as u64 + growth > MAX_JOURNAL {
                    return Err(capacity());
                }
                budget.bytes(growth)?;
                project_bytes += bytes.len() as u64 + growth;
                if project_bytes > limits.project_bytes {
                    return Err(capacity());
                }
            }
        }
    }
    if !target_present {
        budget.entry()?;
    }
    Ok(())
}

struct ReadBudget {
    remaining_bytes: u64,
    remaining_entries: usize,
}
impl ReadBudget {
    fn entry(&mut self) -> Result<(), ApiError> {
        if self.remaining_entries == 0 {
            return Err(limit());
        }
        self.remaining_entries -= 1;
        Ok(())
    }
    fn bytes(&mut self, bytes: u64) -> Result<(), ApiError> {
        self.remaining_bytes = self.remaining_bytes.checked_sub(bytes).ok_or_else(limit)?;
        Ok(())
    }
}

/// The transaction owns the remaining journal fields. All shared fields are required.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceRestoreJournalHeader {
    pub schema_version: u32,
    pub operation: String,
    pub project_id: String,
    pub task_id: String,
    pub phase: String,
    pub plan_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSourceRestore {
    pub task_id: String,
    pub phase: String,
    pub plan_digest: String,
    pub journal_digest: String,
}

/// Reading an affected project's recovery state remains possible even when corrupt.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRestoreState {
    pub pending: Vec<PendingSourceRestore>,
    pub issues: Vec<ApiError>,
    /// Operations whose journal reached a terminal phase. Their retained
    /// originals are no longer needed to continue or undo anything.
    #[serde(skip)]
    pub terminal: Vec<String>,
}
impl SourceRestoreState {
    pub(crate) fn check(&self) -> Result<(), ApiError> {
        if let Some(issue) = self.issues.first() {
            return Err(issue.clone());
        }
        if let Some(pending) = self.pending.first() {
            return Err(ApiError::new(
                "PROJECT_SOURCE_RESTORE_PENDING",
                "源码取回尚未核对完成；请处理对应操作后再继续开发。当前网页和业务数据仍可使用。",
            )
            .detail("taskId", pending.task_id.clone())
            .detail("planDigest", pending.plan_digest.clone())
            .detail("journalDigest", pending.journal_digest.clone()));
        }
        Ok(())
    }
}

/// Not deserializable and not constructible by other modules. It only admits the
/// exact retained operation after the caller confirmed its current two digests.
pub(crate) struct SourceRestoreRecoveryPermit {
    workspace: PathBuf,
    project_id: String,
    task_id: String,
    plan_digest: String,
    journal_digest: String,
}
impl SourceRestoreRecoveryPermit {
    pub(crate) fn revalidate(&self, workspace: &Path, project_id: &str) -> Result<(), ApiError> {
        if workspace != self.workspace || project_id != self.project_id {
            return Err(changed());
        }
        let state = inspect(workspace, project_id);
        if !state.issues.is_empty() || state.pending.len() != 1 {
            return Err(changed());
        }
        let pending = &state.pending[0];
        if pending.task_id != self.task_id
            || pending.plan_digest != self.plan_digest
            || pending.journal_digest != self.journal_digest
        {
            return Err(changed());
        }
        Ok(())
    }
}

pub(crate) fn validate_recovery(
    workspace: &Path,
    project_id: &str,
    task_id: &str,
    expected_plan_digest: &str,
    expected_journal_digest: &str,
) -> Result<SourceRestoreRecoveryPermit, ApiError> {
    if !valid_id(project_id)
        || !valid_task(task_id)
        || !valid_digest(expected_plan_digest)
        || !valid_digest(expected_journal_digest)
    {
        return Err(changed());
    }
    let permit = SourceRestoreRecoveryPermit {
        workspace: workspace.into(),
        project_id: project_id.into(),
        task_id: task_id.into(),
        plan_digest: expected_plan_digest.into(),
        journal_digest: expected_journal_digest.into(),
    };
    permit.revalidate(workspace, project_id)?;
    Ok(permit)
}

pub(crate) fn check_development_available(
    workspace: &Path,
    project_id: &str,
) -> Result<(), ApiError> {
    inspect(workspace, project_id).check()
}

/// Projects that have retained source-restore operation directories.
pub(crate) fn operation_projects(workspace: &Path) -> Result<Vec<String>, ApiError> {
    let root = workspace.join(ROOT);
    if !directory_present(&root)? {
        return Ok(vec![]);
    }
    let mut ids = vec![];
    for entry in fs::read_dir(&root).map_err(|_| invalid())? {
        let id = entry
            .map_err(|_| invalid())?
            .file_name()
            .into_string()
            .map_err(|_| invalid())?;
        if !valid_id(&id) {
            return Err(invalid());
        }
        ids.push(id);
    }
    ids.sort();
    Ok(ids)
}

/// Directories of this project's terminal operations. Any pending or
/// unreadable operation of the same project keeps all of them protected.
pub(crate) fn terminal_operations(
    workspace: &Path,
    project_id: &str,
) -> Result<Vec<PathBuf>, ApiError> {
    let state = inspect(workspace, project_id);
    state.check()?;
    Ok(state
        .terminal
        .iter()
        .map(|task| workspace.join(ROOT).join(project_id).join(task))
        .collect())
}

pub(crate) fn inspect(workspace: &Path, project_id: &str) -> SourceRestoreState {
    inspect_with_budget(
        workspace,
        project_id,
        &mut ReadBudget {
            remaining_bytes: MAX_PROJECT_BYTES,
            remaining_entries: MAX_TASKS,
        },
    )
}

fn inspect_with_budget(
    workspace: &Path,
    project_id: &str,
    budget: &mut ReadBudget,
) -> SourceRestoreState {
    let mut state = SourceRestoreState::default();
    if let Err(error) = inspect_project(workspace, project_id, &mut state, budget) {
        state.issues.push(error);
    }
    state.pending.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    state
}

/// Used before pausing a workspace and by historical-content protection checks.
pub(crate) fn check_workspace_available(workspace: &Path) -> Result<(), ApiError> {
    check_workspace_with_budget(
        workspace,
        &mut ReadBudget {
            remaining_bytes: MAX_WORKSPACE_BYTES,
            remaining_entries: MAX_WORKSPACE_ENTRIES,
        },
    )
}
fn check_workspace_with_budget(workspace: &Path, budget: &mut ReadBudget) -> Result<(), ApiError> {
    let root = workspace.join(ROOT);
    if !directory_present(&root)? {
        return Ok(());
    }
    for entry in fs::read_dir(&root).map_err(|_| invalid())? {
        budget.entry()?;
        let entry = entry.map_err(|_| invalid())?;
        let id = entry.file_name().into_string().map_err(|_| invalid())?;
        if !valid_id(&id) {
            return Err(invalid());
        }
        inspect_with_budget(workspace, &id, budget)
            .check()
            .map_err(|error| error.detail("projectId", id))?;
    }
    Ok(())
}

fn inspect_project(
    workspace: &Path,
    project_id: &str,
    state: &mut SourceRestoreState,
    budget: &mut ReadBudget,
) -> Result<(), ApiError> {
    if !valid_id(project_id) {
        return Err(invalid());
    }
    let root = workspace.join(ROOT);
    if !directory_present(&root)? {
        return Ok(());
    }
    let project = root.join(project_id);
    if !directory_present(&project)? {
        return Ok(());
    }
    let bytes_before = budget.remaining_bytes;
    for (count, entry) in fs::read_dir(&project).map_err(|_| invalid())?.enumerate() {
        if count >= MAX_TASKS {
            return Err(limit());
        }
        budget.entry()?;
        let entry = entry.map_err(|_| invalid())?;
        let task_id = entry.file_name().into_string().map_err(|_| invalid())?;
        let result = (|| {
            if !valid_task(&task_id) || !directory_present(&entry.path())? {
                return Err(invalid());
            }
            let remaining = MAX_PROJECT_BYTES.saturating_sub(bytes_before - budget.remaining_bytes);
            let bytes = read_journal(&entry.path().join("journal.json"), budget, remaining)?;
            let header = journal_header(&bytes, project_id, &task_id)?;
            if pending_phase(&header.phase) {
                Ok(Some(PendingSourceRestore {
                    task_id: task_id.clone(),
                    phase: header.phase,
                    plan_digest: header.plan_digest,
                    journal_digest: format!("{:x}", Sha256::digest(&bytes)),
                }))
            } else {
                Ok(None)
            }
        })();
        match result {
            Ok(Some(pending)) => state.pending.push(pending),
            Ok(None) => state.terminal.push(task_id),
            Err(error) if error.code == "PROJECT_SOURCE_RESTORE_LIMIT" => return Err(error),
            Err(error) => state.issues.push(error.detail("taskId", task_id)),
        }
    }
    Ok(())
}

fn journal_header(
    bytes: &[u8],
    project_id: &str,
    task_id: &str,
) -> Result<SourceRestoreJournalHeader, ApiError> {
    let header: SourceRestoreJournalHeader =
        serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if header.schema_version != 1
        || header.operation != "sourceRestore"
        || header.project_id != project_id
        || header.task_id != task_id
        || !valid_digest(&header.plan_digest)
        || !(pending_phase(&header.phase)
            || matches!(
                header.phase.as_str(),
                "completed" | "undone" | "not-applied"
            ))
    {
        return Err(invalid());
    }
    Ok(header)
}

fn pending_phase(phase: &str) -> bool {
    matches!(phase, "prepared" | "applying" | "applied" | "undoing")
}

fn directory_present(path: &Path) -> Result<bool, ApiError> {
    check_no_symlinks(path, true).map_err(|_| invalid())?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(invalid()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(invalid()),
    }
}
fn read_journal(
    path: &Path,
    budget: &mut ReadBudget,
    project_remaining: u64,
) -> Result<Vec<u8>, ApiError> {
    check_no_symlinks(path, false).map_err(|_| invalid())?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| invalid())?;
    let before = file.metadata().map_err(|_| invalid())?;
    if !before.is_file() || before.nlink() != 1 {
        return Err(invalid());
    }
    if before.len() > MAX_JOURNAL || before.len() > project_remaining {
        return Err(limit());
    }
    budget.bytes(before.len())?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(before.len())
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    let after = file.metadata().map_err(|_| invalid())?;
    check_no_symlinks(path, false).map_err(|_| invalid())?;
    let current = fs::symlink_metadata(path).map_err(|_| invalid())?;
    if bytes.len() as u64 != before.len()
        || !same_file(&before, &after)
        || !same_file(&after, &current)
    {
        return Err(invalid());
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
fn valid_task(value: &str) -> bool {
    valid_id(value) && value.starts_with("task_")
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn invalid() -> ApiError {
    ApiError::new(
        "PROJECT_SOURCE_RESTORE_INVALID",
        "源码取回现场无法安全核验；保留现场并暂停开发操作，当前网页和业务数据不受此门禁影响。",
    )
}
fn changed() -> ApiError {
    ApiError::new(
        "PROJECT_SOURCE_RESTORE_CHANGED",
        "源码取回任务或现场摘要已变化，请重新读取并确认；未开始恢复操作。",
    )
}
fn limit() -> ApiError {
    ApiError::new(
        "PROJECT_SOURCE_RESTORE_LIMIT",
        "源码取回现场超过本次内部核验预算；未检查部分继续保留并阻止开发和迁移，这不是清理授权。",
    )
}
fn capacity() -> ApiError {
    ApiError::new(
        "PROJECT_SOURCE_RESTORE_CAPACITY",
        "本次源码取回会超过恢复记录的内部核验容量，尚未发布新取回现场；已有记录保持不变，这不是清理授权。",
    )
}

#[cfg(test)]
#[path = "source_restore_guard_tests.rs"]
mod tests;
