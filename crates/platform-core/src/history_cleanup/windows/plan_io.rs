//! File facts and removal authorization for an already accepted cleanup plan.
use super::*;
use platform_native::windows::files as native;

pub(super) fn read_json(path: &Path) -> Result<Value, ApiError> {
    check_no_symlinks(path, false)?;
    let file = native::open_read_file_allowing_hard_links(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > 64_000_000 {
        return Err(error("Invalid cleanup metadata"));
    }
    Ok(serde_json::from_reader(file)?)
}

pub(super) fn verify_parents(item: &Candidate) -> Result<(), ApiError> {
    for (path, expected) in &item.parents {
        if directory_identity(path)? != *expected {
            return Err(error("Cleanup ancestor directory was replaced"));
        }
    }
    if directory_identity(&item.path)?
        != *item
            .root_identity
            .as_ref()
            .ok_or_else(|| error("Missing cleanup root identity"))?
    {
        return Err(error("Cleanup root directory was replaced"));
    }
    Ok(())
}

pub(super) fn remove_tree(item: &Candidate) -> Result<Value, ApiError> {
    verify_parents(item)?;
    let identity = item
        .root_identity
        .as_ref()
        .ok_or_else(|| error("Missing cleanup root identity"))?;
    let expected = PersistedIdentity {
        device: identity.device,
        inode: identity.inode,
        volume_uuid: None,
        windows_file_id: Some(identity.windows_file_id),
    };
    let snapshot = serde_json::to_value(&item.entries)?;
    // The native remover retains its exact root and parent namespace while
    // invoking this check, then rechecks each bound file's digest before unlink.
    removal::remove_checked(&item.path, &expected, |observed| {
        verify_parents(item)?;
        if observed != &snapshot {
            return Err(error("Content changed immediately before removal"));
        }
        Ok(())
    })
}

pub(super) fn retired_cleanup_status_impl(
    paths: &RuntimePaths,
    migration_id: &str,
) -> Result<Option<Value>, ApiError> {
    if !valid_id(migration_id) {
        return Err(error("Invalid migration identity"));
    }
    let journal = read_json(
        &paths
            .migrations_dir()
            .join(migration_id)
            .join("journal.json"),
    )?;
    let Some(source) = journal["sourcePath"].as_str().map(PathBuf::from) else {
        return Ok(None);
    };
    if !matches!(fs::symlink_metadata(&source), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(None);
    }
    let expected: PersistedIdentity = serde_json::from_value(journal["sourceIdentity"].clone())?;
    if !expected
        .absent_on_expected_volume(&source)
        .map_err(|failure| error(failure.to_string()))?
    {
        return Ok(None);
    }
    Ok(Some(
        json!({"completed":false,"status":"absent","accountingComplete":false}),
    ))
}
