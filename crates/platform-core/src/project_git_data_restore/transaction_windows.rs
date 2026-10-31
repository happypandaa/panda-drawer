//! Native immutable metadata publication and sealed receipts.
use super::*;
use platform_native::windows::files as native;
use std::{
    io::{Seek, SeekFrom},
    os::windows::fs::OpenOptionsExt,
};

fn open_writable_control(path: &Path) -> Result<File, ApiError> {
    let _parent = native::pin_directory(path.parent().ok_or_else(invalid)?)?;
    // GENERIC_READ | GENERIC_WRITE | READ_CONTROL | FILE_WRITE_ATTRIBUTES.
    // Attribute changes and all flushes stay on this original file handle.
    let file = OpenOptions::new()
        .access_mode(0x8000_0000 | 0x4000_0000 | 0x0002_0000 | 0x100)
        .share_mode(7)
        .custom_flags(0x0020_0000)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || platform_native::filesystem::is_reparse(&metadata)
        || native::identity(&file)?.links != 1
    {
        return Err(changed());
    }
    native::verify_private(&file)?;
    Ok(file)
}
pub(super) fn reserve_proof_private(proof: &FileProof) -> bool {
    !proof.read_only
}
pub(super) fn metadata_writable(file: &File, metadata: &fs::Metadata) -> Result<bool, ApiError> {
    native::verify_private(file)?;
    Ok(!metadata.permissions().readonly())
}
pub(super) fn control_sealed(path: &Path, terminal: bool) -> Result<bool, ApiError> {
    let file = native::open_read_file(path)?;
    native::verify_private(&file)?;
    let read_only = file.metadata()?.permissions().readonly();
    let _ = terminal;
    Ok(read_only)
}
pub(super) fn reserve_permissions(
    proof: &FileProof,
    observed: &FileProof,
    path: &Path,
) -> Result<bool, ApiError> {
    native::verify_private(&native::open_read_file(path)?)?;
    Ok(proof.read_only == observed.read_only)
}
pub(super) fn canonical_permissions(
    proof: &FileProof,
    path: &Path,
    name: &str,
) -> Result<bool, ApiError> {
    let sealed = control_sealed(path, name == "terminal")?;
    Ok(sealed == proof.read_only || (name == "terminal" && sealed))
}
pub(super) fn sync_parent(path: &Path) -> Result<(), ApiError> {
    native::sync_directory(&files::root(path)?)?;
    Ok(())
}
pub(super) fn visible_state(
    plan: &DataRestoreRegistrationPlan,
    workspace: &Path,
    id: &str,
    operation: &Path,
) -> Result<DataRestoreEpochState, ApiError> {
    plan.visible_state_at(workspace, id, operation)
}
pub(super) fn extend_exchange_names(
    workspace: &Path,
    operation: &Path,
    plan: &Plan,
    names: &mut BTreeSet<String>,
) -> Result<(), ApiError> {
    let entries = plan.registration_plan.exchange_metadata(
        workspace,
        &plan.project_id,
        operation,
        &plan.operation_identity,
    )?;
    if !entries.is_empty() {
        // Authenticate the selected tuple and the other immutable image before
        // accepting its precisely missing slot and exact extra journal names.
        plan.registration_plan
            .epoch_state(workspace, &plan.project_id, operation)?;
        for name in ["registration-apply.json", "registration-undo.json"] {
            if !operation.join(name).try_exists()? {
                names.remove(name);
            }
        }
        for (name, _) in entries {
            names.insert(name);
        }
    }
    Ok(())
}
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
        let sealed = control_sealed(&canonical, name == "terminal")?;
        if !identity.matches_captured(&opened_identity)
            || !opened_identity
                .matches_path(&canonical)
                .map_err(|_| changed())?
            || (name != "terminal" && sealed)
        {
            return Err(changed());
        }
        if !sealed {
            let writable = open_writable_control(&canonical)?;
            if !identity
                .matches_open_runtime(&writable)
                .map_err(|_| changed())?
            {
                return Err(changed());
            }
            writable.sync_all()?;
        }
        sync_parent(&operation.join("metadata-attempts"))?;
        sync_parent(&operation.join("metadata-reserve"))?;
        sync_parent(operation)?;
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
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .share_mode(7)
            .custom_flags(0x0020_0000)
            .open(&candidate)?;
        let metadata = file.metadata()?;
        let expected = plan.metadata_reserve.get(name).ok_or_else(invalid)?;
        let observed = PersistedIdentity::from_file(&file).map_err(|_| changed())?;
        native::verify_private(&file)?;
        if !expected.identity.matches_captured(&observed)
            || metadata.len() > 16 * 1024
            || native::identity(&file)?.links != 1
            || metadata.permissions().readonly() != expected.read_only
        {
            return Err(changed());
        }
        file
    };
    faults.at("after-metadata-candidate-create")?;
    let observed_identity = PersistedIdentity::capture(&candidate).map_err(|_| changed())?;
    let metadata = output.metadata()?;
    if observed_identity.is_legacy()
        || !observed_identity
            .matches_open_runtime(&output)
            .map_err(|_| changed())?
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
    native::sync_directory(&parent)?;
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
    let terminal: Terminal = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let directory = files::root(operation)?;
    let path = operation.join("terminal.json");
    let mut file = native::open_read_file(&path)?;
    native::verify_private(&file)?;
    let identity = PersistedIdentity::from_file(&file).map_err(|_| changed())?;
    let mut actual = vec![];
    (&mut file).take(16 * 1024 + 1).read_to_end(&mut actual)?;
    if !terminal.record_identity.matches_captured(&identity)
        || !identity.matches_path(&path).map_err(|_| changed())?
        || actual != bytes
    {
        return Err(changed());
    }
    let sealed = file.metadata()?.permissions().readonly();
    if !sealed {
        drop(file);
        file = open_writable_control(&path)?;
        if !identity
            .matches_open_runtime(&file)
            .map_err(|_| changed())?
        {
            return Err(changed());
        }
        file.sync_all()?;
    }
    sync_parent(&operation.join("metadata-attempts"))?;
    sync_parent(&operation.join("metadata-reserve"))?;
    native::sync_directory(&directory)?;
    faults.at("after-terminal-namespace-sync-before-seal")?;
    check()?;
    if !sealed {
        let mut permissions = file.metadata()?.permissions();
        permissions.set_readonly(true);
        file.set_permissions(permissions)?;
    }
    faults.at("after-terminal-seal-before-file-sync")?;
    file.seek(SeekFrom::Start(0))?;
    actual.clear();
    (&mut file).take(16 * 1024 + 1).read_to_end(&mut actual)?;
    if !file.metadata()?.permissions().readonly()
        || native::identity(&file)?.links != 1
        || !identity
            .matches_open_runtime(&file)
            .map_err(|_| changed())?
        || !identity.matches_path(&path).map_err(|_| changed())?
        || actual != bytes
    {
        return Err(changed());
    }
    // A retry of an already sealed receipt uses read access. Windows cannot
    // flush a read-only reopen; new seals flush the original RW handle after
    // updating attributes. Directory fsync has no Win32 equivalent.
    if !sealed {
        file.sync_all()?;
    }
    faults.at("after-terminal-seal-file-sync")?;
    Ok(())
}
