//! Persistent, project-local protection for an interrupted data-only restore.
//! The guard never opens the live database or clears/replays a retained operation.
use crate::{
    file_identity::PersistedIdentity,
    projects::DataRestoreRegistrationPlan,
    projects::{check_no_symlinks, valid_id},
    ApiError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

const ROOT: &str = ".platform/project-state/data-restore";
// Internal metadata inspection budgets, not product storage quotas.
const MAX_JOURNAL: u64 = 8 * 1024 * 1024;
const MAX_TASKS: usize = 1000;
const MAX_PROJECT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_WORKSPACE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_WORKSPACE_ENTRIES: usize = 4096;
const LONGEST_PHASE: usize = "undo-committing".len();
const TRANSACTION_RESERVED: u64 = 2 * 1024 * 1024 + 128 * 1024;
// No capacity is persisted or retained after publication. Serializing the scan
// and directory publication closes concurrent admissions from different projects.
static PUBLICATION_LOCK: Mutex<()> = Mutex::new(());

/// Publish a complete prepared directory before any live-data movement. No
/// arbitrary directory or pre-existing journal is adopted or recursively removed.
pub(crate) fn publish_prepared<T: Serialize>(
    workspace: &Path,
    staging: &Path,
    journal: &T,
) -> Result<PathBuf, ApiError> {
    let bytes = serde_json::to_vec_pretty(journal).map_err(|_| invalid())?;
    if bytes.len() as u64 > MAX_JOURNAL {
        return Err(limit());
    }
    let raw: DataRestoreJournalHeader = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let header = journal_header(&bytes, &raw.project_id, &raw.task_id)?;
    if header.phase != "prepared"
        || !staging.starts_with(workspace.join(".platform"))
        || !header
            .operation_identity
            .matches_path(staging)
            .map_err(|_| invalid())?
    {
        return Err(invalid());
    }
    header
        .registration_plan
        .verify_root(workspace, &header.project_id)?;
    if header
        .registration_plan
        .epoch_state(workspace, &header.project_id, staging)?
        != crate::projects::DataRestoreEpochState::Before
    {
        return Err(changed());
    }
    let maximum = if let Some(plan) = &header.transaction_plan {
        plan.verify(&staging.join("plan.json"), MAX_JOURNAL)?;
        // Reserve immutable plan plus maximum active journal and completion.
        plan.bytes
            .checked_add(TRANSACTION_RESERVED)
            .ok_or_else(capacity)?
    } else {
        bytes.len() as u64 + (LONGEST_PHASE - header.phase.len()) as u64
    };
    let _permit = if header.transaction_plan.is_some() {
        reserve_publication_with_limits(
            workspace,
            &header.project_id,
            &header.task_id,
            maximum,
            PublicationLimits {
                project_bytes: MAX_PROJECT_BYTES,
                project_tasks: MAX_TASKS,
                workspace_bytes: MAX_WORKSPACE_BYTES,
                workspace_entries: MAX_WORKSPACE_ENTRIES,
                incoming_entries: 130,
            },
        )?
    } else {
        reserve_publication(workspace, &header.project_id, &header.task_id, maximum)?
    };
    let root = workspace
        .join(ROOT)
        .join(&header.project_id)
        .join(&header.task_id);
    let workspace_fd = crate::files::root(workspace)?;
    crate::files::parent(
        &workspace_fd,
        &format!("{ROOT}/{}/.publication-probe", header.project_id),
        true,
    )?;
    let staging_fd = crate::files::root(staging)?;
    let observed = PersistedIdentity::capture(staging).map_err(|_| changed())?;
    let opened = staging_fd.metadata().map_err(|_| invalid())?;
    if !header.operation_identity.matches_captured(&observed)
        || opened.dev() != observed.device
        || opened.ino() != observed.inode
    {
        return Err(changed());
    }
    if let Some(identity) = &header.journal_identity {
        if identity.is_legacy()
            || !identity
                .matches_path(&staging.join("journal.json"))
                .map_err(|_| changed())?
        {
            return Err(changed());
        }
        let observed = read_journal(
            &staging.join("journal.json"),
            &mut ReadBudget {
                remaining_bytes: MAX_JOURNAL,
                remaining_entries: 1,
            },
            MAX_JOURNAL,
        )?;
        if observed != bytes {
            return Err(changed());
        }
        crate::files::read(&staging_fd, "journal.json")?
            .sync_all()
            .map_err(|_| invalid())?;
    } else {
        let mut file = crate::files::create(&staging_fd, "journal.json").map_err(|_| changed())?;
        file.write_all(&bytes).map_err(|_| invalid())?;
        file.sync_all().map_err(|_| invalid())?;
    }
    staging_fd.sync_all().map_err(|_| invalid())?;
    let publication = (|| {
        move_directory_exclusive(staging, &root)?;
        if !header
            .operation_identity
            .matches_path(&root)
            .map_err(|_| changed())?
        {
            return Err(changed());
        }
        let observed = read_journal(
            &root.join("journal.json"),
            &mut ReadBudget {
                remaining_bytes: MAX_JOURNAL,
                remaining_entries: 1,
            },
            MAX_JOURNAL,
        )?;
        if observed != bytes
            || header
                .registration_plan
                .epoch_state(workspace, &header.project_id, &root)?
                != crate::projects::DataRestoreEpochState::Before
        {
            return Err(changed());
        }
        let mut parent = root.parent();
        while let Some(path) = parent.filter(|path| path.starts_with(workspace)) {
            crate::files::root(path)?.sync_all()?;
            parent = path.parent();
        }
        Ok(root.clone())
    })();
    publication.map_err(|error: ApiError| {
        if root.try_exists().unwrap_or(true) {
            error
                .detail("pending", true)
                .detail("businessBlocked", true)
                .detail("retainedPath", root.display().to_string())
        } else {
            error
        }
    })
}

#[cfg(target_os = "macos")]
fn move_directory_exclusive(from: &Path, to: &Path) -> Result<(), ApiError> {
    use std::{
        ffi::CString,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
    };
    let left = crate::files::root(from.parent().ok_or_else(invalid)?)?;
    let right = crate::files::root(to.parent().ok_or_else(invalid)?)?;
    if left.metadata().map_err(|_| invalid())?.dev()
        != right.metadata().map_err(|_| invalid())?.dev()
    {
        return Err(invalid());
    }
    let a =
        CString::new(from.file_name().ok_or_else(invalid)?.as_bytes()).map_err(|_| invalid())?;
    let b = CString::new(to.file_name().ok_or_else(invalid)?.as_bytes()).map_err(|_| invalid())?;
    if unsafe {
        libc::renameatx_np(
            left.as_raw_fd(),
            a.as_ptr(),
            right.as_raw_fd(),
            b.as_ptr(),
            libc::RENAME_EXCL,
        )
    } != 0
    {
        return Err(changed());
    }
    left.sync_all().map_err(|_| invalid())?;
    right.sync_all().map_err(|_| invalid())?;
    Ok(())
}
#[cfg(not(target_os = "macos"))]
fn move_directory_exclusive(_from: &Path, _to: &Path) -> Result<(), ApiError> {
    Err(ApiError::new(
        "UNSUPPORTED_PLATFORM",
        "数据取回现场发布目前仅验证macOS。",
    ))
}

#[derive(Clone, Copy)]
struct PublicationLimits {
    project_bytes: u64,
    project_tasks: usize,
    workspace_bytes: u64,
    workspace_entries: usize,
    incoming_entries: usize,
}

/// Keep this permit until the prepared directory's exclusive publication and
/// parent-directory sync finish. It must not be held across an await point.
#[must_use = "hold the permit until the prepared directory has been published and synced"]
pub(crate) struct DataRestorePublicationPermit {
    _publication_lock: MutexGuard<'static, ()>,
}

/// `max_journal_bytes` is the largest exact serialized journal over every phase.
/// The transaction, which owns that format, computes
/// it before calling this function. This function does not create any files.
pub(crate) fn reserve_publication(
    workspace: &Path,
    project_id: &str,
    task_id: &str,
    max_journal_bytes: u64,
) -> Result<DataRestorePublicationPermit, ApiError> {
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
            incoming_entries: 0,
        },
    )
}

fn reserve_publication_with_limits(
    workspace: &Path,
    project_id: &str,
    task_id: &str,
    max_journal_bytes: u64,
    limits: PublicationLimits,
) -> Result<DataRestorePublicationPermit, ApiError> {
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
            if error.code == "PROJECT_DATA_RESTORE_LIMIT" {
                capacity()
            } else {
                error
            }
        },
    )?;
    Ok(DataRestorePublicationPermit {
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
    for _ in 0..limits.incoming_entries {
        budget.entry()?;
    }
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
            let project_entries_before = budget.remaining_entries;
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
                let before_material = budget.remaining_bytes;
                let released = inspect_transaction_material(
                    &entry.path(),
                    &header,
                    &mut budget,
                    limits
                        .project_bytes
                        .saturating_sub(project_bytes + bytes.len() as u64),
                )?;
                let extra = before_material - budget.remaining_bytes;
                let pending = !released && pending_phase(&header.phase);
                if target && pending {
                    return Err(ApiError::new(
                        "PROJECT_DATA_RESTORE_PENDING",
                        "已有数据取回操作尚未核对完成；请先处理对应操作。",
                    )
                    .detail("taskId", task));
                }
                // The transaction changes only the phase after publication.
                // Account for an in-flight journal's longest future spelling,
                // even when another project finishes while this scan proceeds.
                let growth = if pending {
                    if let Some(proof) = &header.transaction_plan {
                        TRANSACTION_RESERVED
                            .saturating_sub(bytes.len() as u64 + extra.saturating_sub(proof.bytes))
                    } else {
                        (LONGEST_PHASE - header.phase.len()) as u64
                    }
                } else {
                    0
                };
                if pending && header.transaction_plan.is_some() {
                    let (count, _) =
                        crate::project_git::data_restore_metadata_usage(&entry.path())?;
                    for _ in count..128 {
                        budget.entry()?;
                    }
                }
                let project_entries = project_entries_before - budget.remaining_entries
                    + usize::from(target) * (limits.incoming_entries + 1);
                if project_entries > MAX_TASKS {
                    return Err(capacity());
                }
                if bytes.len() as u64 + growth > MAX_JOURNAL {
                    return Err(capacity());
                }
                budget.bytes(growth)?;
                project_bytes += bytes.len() as u64 + growth + extra;
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
pub(crate) struct DataRestoreJournalHeader {
    pub schema_version: u32,
    pub operation: String,
    pub project_id: String,
    pub task_id: String,
    pub phase: String,
    pub plan_digest: String,
    #[serde(
        deserialize_with = "crate::projects::data_restore_registration::strict_identity::deserialize"
    )]
    pub operation_identity: PersistedIdentity,
    pub registration_plan: DataRestoreRegistrationPlan,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_plan: Option<crate::projects::data_restore_registration::FileProof>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::projects::data_restore_registration::strict_identity::optional"
    )]
    pub journal_identity: Option<PersistedIdentity>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingDataRestore {
    pub task_id: String,
    pub phase: String,
    pub plan_digest: String,
    pub journal_digest: String,
}

/// Reading an affected project's recovery state remains possible even when corrupt.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataRestoreState {
    pub pending: Vec<PendingDataRestore>,
    pub issues: Vec<ApiError>,
}
impl DataRestoreState {
    pub(crate) fn check(&self) -> Result<(), ApiError> {
        if let Some(issue) = self.issues.first() {
            return Err(issue.clone().detail("businessBlocked", true));
        }
        if let Some(pending) = self.pending.first() {
            return Err(ApiError::new(
                "PROJECT_DATA_RESTORE_PENDING",
                "数据取回尚未核对完成；请处理对应操作后再继续开发。业务数据和网页访问暂时停止。",
            )
            .detail("taskId", pending.task_id.clone())
            .detail("planDigest", pending.plan_digest.clone())
            .detail("journalDigest", pending.journal_digest.clone())
            .detail("businessBlocked", true));
        }
        Ok(())
    }
}

/// Not deserializable and not constructible by other modules. It only admits the
/// exact retained operation after the caller confirmed its current two digests.
#[derive(Clone)]
pub(crate) struct DataRestoreRecoveryPermit {
    workspace: PathBuf,
    project_id: String,
    task_id: String,
    plan_digest: String,
    journal_digest: String,
}
impl DataRestoreRecoveryPermit {
    pub(crate) fn retained(
        &self,
        workspace: &Path,
        project_id: &str,
    ) -> Result<(PathBuf, DataRestoreJournalHeader), ApiError> {
        self.revalidate(workspace, project_id)?;
        let root = workspace.join(ROOT).join(project_id).join(&self.task_id);
        let mut budget = ReadBudget {
            remaining_bytes: MAX_JOURNAL,
            remaining_entries: 1,
        };
        let bytes = read_journal(&root.join("journal.json"), &mut budget, MAX_JOURNAL)?;
        if format!("{:x}", Sha256::digest(&bytes)) != self.journal_digest {
            return Err(changed());
        }
        Ok((root, journal_header(&bytes, project_id, &self.task_id)?))
    }
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
) -> Result<DataRestoreRecoveryPermit, ApiError> {
    if !valid_id(project_id)
        || !valid_task(task_id)
        || !valid_digest(expected_plan_digest)
        || !valid_digest(expected_journal_digest)
    {
        return Err(changed());
    }
    let permit = DataRestoreRecoveryPermit {
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
pub(crate) fn check_business_available(workspace: &Path, project_id: &str) -> Result<(), ApiError> {
    inspect(workspace, project_id).check()
}

pub(crate) fn inspect(workspace: &Path, project_id: &str) -> DataRestoreState {
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
) -> DataRestoreState {
    let mut state = DataRestoreState::default();
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
    state: &mut DataRestoreState,
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
            let released = inspect_transaction_material(
                &entry.path(),
                &header,
                budget,
                MAX_PROJECT_BYTES.saturating_sub(bytes_before - budget.remaining_bytes),
            )?;
            if released {
                return Ok(None);
            }
            if pending_phase(&header.phase) {
                if let Some(identity) = &header.journal_identity {
                    if identity.is_legacy()
                        || !identity
                            .matches_path(&entry.path().join("journal.json"))
                            .map_err(|_| invalid())?
                    {
                        return Err(invalid());
                    }
                }
                if !header
                    .operation_identity
                    .matches_path(&entry.path())
                    .map_err(|_| invalid())?
                {
                    return Err(invalid());
                }
                header
                    .registration_plan
                    .verify_root(workspace, project_id)?;
                header
                    .registration_plan
                    .epoch_state(workspace, project_id, &entry.path())?;
                Ok(Some(PendingDataRestore {
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
            Ok(None) => (),
            Err(error) if error.code == "PROJECT_DATA_RESTORE_LIMIT" => return Err(error),
            Err(error) => state.issues.push(
                error
                    .detail("taskId", task_id)
                    .detail("retainedPath", entry.path().display().to_string()),
            ),
        }
    }
    Ok(())
}

fn inspect_transaction_material(
    root: &Path,
    header: &DataRestoreJournalHeader,
    budget: &mut ReadBudget,
    project_remaining: u64,
) -> Result<bool, ApiError> {
    let Some(proof) = &header.transaction_plan else {
        return Ok(false);
    };
    let start = budget.remaining_bytes;
    let plan = read_journal(&root.join("plan.json"), budget, project_remaining)?;
    if plan.len() as u64 != proof.bytes || format!("{:x}", Sha256::digest(&plan)) != proof.sha256 {
        return Err(invalid());
    }
    let mut optional = Vec::new();
    for name in ["completion.json", "terminal.json"] {
        let bytes = match fs::symlink_metadata(root.join(name)) {
            Ok(_) => Some(read_journal(
                &root.join(name),
                budget,
                project_remaining
                    .saturating_sub(start - budget.remaining_bytes)
                    .min(16 * 1024),
            )?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(invalid()),
        };
        optional.push(bytes);
    }
    let (count, metadata_bytes) = crate::project_git::data_restore_metadata_usage(root)?;
    if metadata_bytes > project_remaining.saturating_sub(start - budget.remaining_bytes) {
        return Err(limit());
    }
    budget.bytes(metadata_bytes)?;
    for _ in 0..count + 2 {
        budget.entry()?;
    }
    let released = crate::project_git::inspect_data_restore_completion(
        root,
        header,
        &plan,
        optional[0].as_deref(),
        optional[1].as_deref(),
    )?;
    if !released {
        proof.verify(&root.join("plan.json"), MAX_JOURNAL)?;
    }
    Ok(released)
}

fn journal_header(
    bytes: &[u8],
    project_id: &str,
    task_id: &str,
) -> Result<DataRestoreJournalHeader, ApiError> {
    let header: DataRestoreJournalHeader = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if header.schema_version != 1
        || header.operation != "dataRestore"
        || header.project_id != project_id
        || header.task_id != task_id
        || header.operation_identity.is_legacy()
        || !valid_digest(&header.plan_digest)
        // Terminal phase text alone never releases a gate; the transaction's
        // independent, immutable-plan-bound completion is checked by inspection.
        || !(pending_phase(&header.phase) || (header.transaction_plan.is_some() && matches!(header.phase.as_str(),"completed"|"undone")))
    {
        return Err(invalid());
    }
    header.registration_plan.validate_shape(project_id)?;
    Ok(header)
}

fn pending_phase(phase: &str) -> bool {
    matches!(
        phase,
        "prepared" | "applying" | "applied" | "undoing" | "committing" | "undo-committing"
    )
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
        "PROJECT_DATA_RESTORE_INVALID",
        "数据取回现场无法安全核验；保留现场并暂停业务、开发、删除和迁移。",
    )
}
fn changed() -> ApiError {
    ApiError::new(
        "PROJECT_DATA_RESTORE_CHANGED",
        "数据取回任务或现场摘要已变化，请重新读取并确认；未开始恢复操作。",
    )
}
fn limit() -> ApiError {
    ApiError::new(
        "PROJECT_DATA_RESTORE_LIMIT",
        "数据取回现场超过本次内部核验预算；未检查部分继续保留并阻止业务、开发、删除和迁移，这不是清理授权。",
    )
}
fn capacity() -> ApiError {
    ApiError::new(
        "PROJECT_DATA_RESTORE_CAPACITY",
        "本次数据取回会超过恢复记录的内部核验容量，尚未发布新取回现场；已有记录保持不变，这不是清理授权。",
    )
}

#[cfg(test)]
#[path = "data_restore_guard_tests.rs"]
mod tests;
