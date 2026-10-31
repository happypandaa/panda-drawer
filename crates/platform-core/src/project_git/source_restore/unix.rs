use super::*;
use std::os::fd::AsRawFd;

pub(super) fn validate_path(_: &str) -> Result<(), ApiError> {
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Stamp {
    pub(super) descriptor: RestoreDescriptor,
    pub(super) mode: u32,
    pub(super) device: u64,
    pub(super) inode: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RestoreJournal {
    pub(super) schema_version: u32,
    pub(super) operation: String,
    pub(super) project_id: String,
    pub(super) task_id: String,
    pub(super) phase: String,
    pub(super) plan_digest: String,
    pub(super) plan: RestorePlan,
    pub(super) entries: Vec<RestoreEntry>,
    pub(super) git: ExecutableInfo,
}
pub(super) fn binding_metadata(project: &ProjectHandle, branch: &str) -> Result<String, ApiError> {
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
pub(super) fn stamp(path: &Path) -> Result<Option<Stamp>, ApiError> {
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
pub(super) fn open_directory(path: &Path) -> Result<File, ApiError> {
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

pub(super) fn create_file(path: &Path, executable: bool) -> Result<File, ApiError> {
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(if executable { 0o755 } else { 0o644 })
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?)
}
pub(super) fn create_staging(path: &Path) -> Result<(), ApiError> {
    Ok(fs::create_dir(path)?)
}
pub(super) fn same_volume(a: &Path, b: &Path) -> Result<bool, ApiError> {
    Ok(fs::metadata(a)?.dev() == fs::metadata(b)?.dev())
}
pub(super) fn executable(value: bool) -> bool {
    value
}
pub(super) fn valid_stamp(stamp: &Stamp) -> bool {
    stamp.mode <= 0o7777 && (stamp.mode & 0o111 != 0) == stamp.descriptor.executable
}
pub(super) fn ordinary_single_file(path: &Path, metadata: &fs::Metadata) -> Result<bool, ApiError> {
    let _ = path;
    Ok(metadata.is_file() && metadata.nlink() == 1)
}
pub(super) fn verify_journal_root(_: &Path, _: &RestoreJournal) -> Result<(), ApiError> {
    Ok(())
}
pub(super) fn require_terminal_ready(_: &Path, _: &RestoreJournal) -> Result<(), ApiError> {
    Ok(())
}
pub(super) fn classify_journal_entry(
    project: &ProjectHandle,
    root: &Path,
    index: usize,
    entry: &RestoreEntry,
    _: &RestoreJournal,
) -> Result<&'static str, ApiError> {
    classify_entry(project, root, index, entry)
}
pub(super) fn verify_mixed_source_at(
    project: &ProjectHandle,
    _: &Path,
    journal: &RestoreJournal,
    changes: &[RecoveryChange],
) -> Result<(), ApiError> {
    verify_mixed_source(project, journal, changes)
}
pub(super) fn stabilize_pending(
    _: &ProjectHandle,
    _: &Path,
    _: &mut RestoreJournal,
) -> Result<(), ApiError> {
    Ok(())
}
pub(super) fn new_journal(
    root: &Path,
    task: &str,
    plan_digest: String,
    plan: RestorePlan,
    entries: Vec<RestoreEntry>,
    git: ExecutableInfo,
) -> Result<RestoreJournal, ApiError> {
    let _ = root;
    Ok(RestoreJournal {
        schema_version: 1,
        operation: "sourceRestore".into(),
        project_id: plan.project_id.clone(),
        task_id: task.into(),
        phase: "prepared".into(),
        plan_digest,
        plan,
        entries,
        git,
    })
}
pub(super) fn move_prepared(from: &Path, to: &Path, _: &RestoreJournal) -> Result<(), ApiError> {
    move_preserving(from, to, false)
}
pub(super) fn move_entry(
    project: &ProjectHandle,
    root: &Path,
    journal: &mut RestoreJournal,
    index: usize,
    direction: RestoreAction,
    live: &Path,
    held: &Path,
) -> Result<(), ApiError> {
    let _ = (project, root);
    let entry = &journal.entries[index];
    match (entry.before.is_some(), entry.after.is_some(), direction) {
        (true, true, _) => move_preserving(held, live, true)?,
        (false, true, RestoreAction::Continue) | (true, false, RestoreAction::Undo) => {
            move_preserving(held, live, false)?
        }
        (true, false, RestoreAction::Continue) | (false, true, RestoreAction::Undo) => {
            move_preserving(live, held, false)?
        }
        _ => return Err(restore_changed()),
    }
    Ok(())
}
