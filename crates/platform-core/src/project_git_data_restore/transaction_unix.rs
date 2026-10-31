//! Original Unix control-file publication and sealing, mechanically retained.
use super::*;
pub(super) fn publish_record(
    operation: &Path,
    plan: &Plan,
    name: &str,
    build: impl Fn(PersistedIdentity) -> Result<Vec<u8>, ApiError>,
    faults: &mut Faults,
    check: &mut impl FnMut() -> Result<(), ApiError>,
) -> Result<Vec<u8>, ApiError> {
    let canonical = operation.join(format!("{name}.json"));
    if canonical.try_exists()? {
        let bytes = read_file(&canonical, 16 * 1024)?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let identity: PersistedIdentity =
            serde_json::from_value(value.get("recordIdentity").cloned().ok_or_else(invalid)?)
                .map_err(|_| invalid())?;
        if identity.is_legacy()
            || !identity.matches_path(&canonical).map_err(|_| changed())?
            || build(identity.clone())? != bytes
        {
            return Err(changed());
        }
        let file = files::read(&files::root(operation)?, &format!("{name}.json"))?;
        let opened_identity = PersistedIdentity::from_file(&file).map_err(|_| changed())?;
        let mode = file.metadata()?.mode() & 0o777;
        if !identity.matches_captured(&opened_identity)
            || !opened_identity
                .matches_path(&canonical)
                .map_err(|_| changed())?
            || !(mode == 0o600 || (name == "terminal" && mode == 0o400))
        {
            return Err(changed());
        }
        file.sync_all()?;
        files::root(&operation.join("metadata-attempts"))?.sync_all()?;
        files::root(&operation.join("metadata-reserve"))?.sync_all()?;
        files::root(operation)?.sync_all()?;
        return Ok(bytes);
    }
    let (count, _) = metadata_usage(operation)?;
    check()?;
    faults.at("before-metadata-candidate-create")?;
    let candidate = if count < 128 {
        operation
            .join("metadata-attempts")
            .join(format!("{name}-{}.json", uuid::Uuid::new_v4().simple()))
    } else {
        operation.join("metadata-reserve").join(name)
    };
    let parent = files::root(candidate.parent().ok_or_else(invalid)?)?;
    let file_name = candidate
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?;
    let mut output = if count < 128 {
        files::create(&parent, file_name)?
    } else {
        use std::{
            ffi::CString,
            os::fd::{AsRawFd, FromRawFd},
        };
        let file_name = CString::new(file_name).map_err(|_| invalid())?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                file_name.as_ptr(),
                libc::O_RDWR | libc::O_APPEND | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(changed());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        let expected = plan.metadata_reserve.get(name).ok_or_else(invalid)?;
        if metadata.ino() != expected.identity.inode
            || metadata.len() > 16 * 1024
            || metadata.nlink() != 1
            || metadata.mode() != expected.mode
        {
            return Err(changed());
        }
        file
    };
    faults.at("after-metadata-candidate-create")?;
    let observed_identity = PersistedIdentity::capture(&candidate).map_err(|_| changed())?;
    let metadata = output.metadata()?;
    if observed_identity.is_legacy()
        || observed_identity.device != metadata.dev()
        || observed_identity.inode != metadata.ino()
    {
        return Err(changed());
    }
    let identity = if count < 128 {
        observed_identity
    } else {
        let expected = &plan
            .metadata_reserve
            .get(name)
            .ok_or_else(invalid)?
            .identity;
        if !expected.matches_captured(&observed_identity) {
            return Err(changed());
        }
        expected.clone()
    };
    let bytes = build(identity)?;
    if bytes.len() > 16 * 1024 {
        return Err(limit());
    }
    let mut prefix = vec![];
    if count >= 128 {
        (&mut output).take(16 * 1024 + 1).read_to_end(&mut prefix)?;
        if prefix.len() as u64 != metadata.len() || !bytes.starts_with(&prefix) {
            return Err(changed().detail("candidateCapacity", 128));
        }
    }
    let remaining = &bytes[prefix.len()..];
    let split = remaining.len() / 2;
    output.write_all(&remaining[..split])?;
    faults.at("after-metadata-partial-write")?;
    output.write_all(&remaining[split..])?;
    faults.at("before-metadata-file-sync")?;
    output.sync_all()?;
    faults.at("after-metadata-file-sync")?;
    parent.sync_all()?;
    if read_file(&candidate, 16 * 1024)? != bytes {
        return Err(changed());
    }
    check()?;
    faults.at("before-metadata-canonical-publication")?;
    slot_files::move_exclusive_with(&candidate, &canonical, || {
        faults.at("after-metadata-visible-before-directory-sync")
    })?;
    faults.at("after-metadata-directory-sync")?;
    Ok(bytes)
}

pub(super) fn seal_terminal(
    operation: &Path,
    bytes: &[u8],
    faults: &mut Faults,
    check: &mut impl FnMut() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    use std::io::{Seek, SeekFrom};
    let terminal: Terminal = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let directory = files::root(operation)?;
    let mut file = files::read(&directory, "terminal.json")?;
    let identity = PersistedIdentity::from_file(&file).map_err(|_| changed())?;
    let before = file.metadata()?;
    let mut actual = vec![];
    (&mut file).take(16 * 1024 + 1).read_to_end(&mut actual)?;
    if !terminal.record_identity.matches_captured(&identity)
        || !identity
            .matches_path(&operation.join("terminal.json"))
            .map_err(|_| changed())?
        || before.nlink() != 1
        || !matches!(before.mode() & 0o777, 0o600 | 0o400)
        || actual != bytes
    {
        return Err(changed());
    }
    file.sync_all()?;
    files::root(&operation.join("metadata-attempts"))?.sync_all()?;
    files::root(&operation.join("metadata-reserve"))?.sync_all()?;
    directory.sync_all()?;
    faults.at("after-terminal-namespace-sync-before-seal")?;
    check()?;
    file.set_permissions(fs::Permissions::from_mode(0o400))?;
    faults.at("after-terminal-seal-before-file-sync")?;
    file.seek(SeekFrom::Start(0))?;
    actual.clear();
    (&mut file).take(16 * 1024 + 1).read_to_end(&mut actual)?;
    if file.metadata()?.mode() & 0o777 != 0o400
        || file.metadata()?.nlink() != 1
        || !identity.matches_captured(&PersistedIdentity::from_file(&file).map_err(|_| changed())?)
        || !identity
            .matches_path(&operation.join("terminal.json"))
            .map_err(|_| changed())?
        || actual != bytes
    {
        return Err(changed());
    }
    file.sync_all()?;
    faults.at("after-terminal-seal-file-sync")?;
    Ok(())
}
pub(super) fn reserve_proof_private(proof: &FileProof) -> bool {
    proof.mode & 0o777 == 0o600
}
pub(super) fn metadata_writable(_: &File, metadata: &fs::Metadata) -> Result<bool, ApiError> {
    Ok(metadata.mode() & 0o777 == 0o600)
}
pub(super) fn control_sealed(path: &Path, terminal: bool) -> Result<bool, ApiError> {
    let mode = fs::symlink_metadata(path)?.mode() & 0o777;
    if !terminal {
        return Ok(mode != 0o600);
    }
    if mode == 0o600 {
        Ok(false)
    } else if terminal && mode == 0o400 {
        Ok(true)
    } else {
        Err(changed())
    }
}
pub(super) fn reserve_permissions(
    proof: &FileProof,
    observed: &FileProof,
    _: &Path,
) -> Result<bool, ApiError> {
    Ok(proof.mode == observed.mode)
}
pub(super) fn canonical_permissions(
    proof: &FileProof,
    path: &Path,
    name: &str,
) -> Result<bool, ApiError> {
    Ok(fs::symlink_metadata(path)?.mode() == proof.mode
        || (name == "terminal" && fs::symlink_metadata(path)?.mode() & 0o777 == 0o400))
}
pub(super) fn sync_parent(path: &Path) -> Result<(), ApiError> {
    files::root(path)?.sync_all()?;
    Ok(())
}
pub(super) fn extend_exchange_names(
    _: &Path,
    _: &Path,
    _: &Plan,
    _: &mut BTreeSet<String>,
) -> Result<(), ApiError> {
    Ok(())
}
pub(super) fn visible_state(
    plan: &DataRestoreRegistrationPlan,
    workspace: &Path,
    id: &str,
    _: &Path,
) -> Result<DataRestoreEpochState, ApiError> {
    plan.visible_state(workspace, id)
}
