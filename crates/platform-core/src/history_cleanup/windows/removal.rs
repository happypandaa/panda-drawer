//! Project-owned removal. Existing ownership selects the root; an inventory of
//! exact names and full identities bounds each actual native unlink.
use crate::{file_identity::PersistedIdentity, ApiError};
use platform_native::windows::{files as native, removal};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    File,
    Directory,
    Reparse,
}
struct Entry {
    relative: PathBuf,
    identity: native::FileIdentity,
    kind: Kind,
    bytes: u64,
    readonly: bool,
    target: Option<PathBuf>,
    digest: Option<String>,
}

fn blocked(path: &Path, message: impl std::fmt::Display) -> ApiError {
    ApiError::new("CLEANUP_ITEM_PROTECTED", "目录移除未完成；已保留剩余内容。")
        .detail("remainingPath", path.to_string_lossy().into_owned())
        .detail("cause", message.to_string())
}

fn observed(path: &Path, relative: PathBuf) -> Result<Entry, ApiError> {
    let opened = removal::inspect(path)?;
    let metadata = opened.file.metadata()?;
    let kind = if platform_native::filesystem::is_reparse(&metadata) {
        Kind::Reparse
    } else if metadata.is_dir() {
        Kind::Directory
    } else if metadata.is_file() {
        Kind::File
    } else {
        return Err(blocked(path, "Unsupported special file"));
    };
    let target = if kind == Kind::Reparse {
        Some(fs::read_link(path)?)
    } else {
        None
    };
    Ok(Entry {
        relative,
        identity: opened.identity,
        bytes: if kind == Kind::File {
            metadata.len()
        } else {
            0
        },
        readonly: metadata.permissions().readonly(),
        kind,
        target,
        digest: None,
    })
}

fn hash_file(path: &Path, expected: &Entry) -> Result<String, ApiError> {
    // Historical payloads admit hard links on Mac. Bind and hash just this
    // original name; another name never grants authority to its external owner.
    let mut file = native::open_read_file_allowing_hard_links(path)?;
    let matches_file = |file: &fs::File| -> Result<bool, ApiError> {
        let identity = native::identity(file)?;
        let metadata = file.metadata()?;
        Ok(expected.identity.same_object(&identity)
            && !identity.directory
            && metadata.is_file()
            && !platform_native::filesystem::is_reparse(&metadata)
            && metadata.len() == expected.bytes
            && metadata.permissions().readonly() == expected.readonly)
    };
    if !matches_file(&file)? {
        return Err(blocked(path, "File changed before cleanup content read"));
    }
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 128 * 1024];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    if !matches_file(&file)? {
        return Err(blocked(path, "File changed during cleanup content read"));
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn inventory_value(entries: &[Entry]) -> Value {
    Value::Array(entries.iter().map(|entry| {
        let kind = match &entry.kind { Kind::File => "file", Kind::Directory => "directory", Kind::Reparse => "symlink" };
        json!({"path":entry.relative,"kind":kind,
            "identity":{"device":entry.identity.volume_serial,
                "inode":u64::from_le_bytes(entry.identity.file_id[..8].try_into().unwrap()),
                "windowsFileId":entry.identity.file_id},
            "bytes":entry.bytes,"digest":entry.digest,"target":entry.target,"readOnly":entry.readonly})
    }).collect())
}

fn matches(path: &Path, expected: &Entry) -> Result<(), ApiError> {
    let actual = observed(path, expected.relative.clone())?;
    if !expected.identity.same_object(&actual.identity)
        || expected.kind != actual.kind
        || expected.bytes != actual.bytes
        || expected.readonly != actual.readonly
        || expected.target != actual.target
    {
        return Err(blocked(
            path,
            "Cleanup entry identity, type, size or permissions changed",
        ));
    }
    Ok(())
}

fn scan(
    root: &Path,
    directory: &Path,
    volume: u64,
    entries: &mut Vec<Entry>,
    contents: bool,
) -> Result<(), ApiError> {
    let relative = directory
        .strip_prefix(root)
        .map_err(|e| blocked(directory, e))?;
    if relative.components().count() > 256 {
        return Err(blocked(directory, "Cleanup depth exceeds 256"));
    }
    let _pin = native::pin_directory(directory)?;
    let mut children = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        if entries.len() >= 500_000 {
            return Err(blocked(
                directory,
                "Cleanup inventory exceeds 500000 entries",
            ));
        }
        let path = child.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|e| blocked(&path, e))?
            .to_path_buf();
        if relative.to_str().is_none() {
            return Err(blocked(&path, "Cleanup filename is not valid text"));
        }
        let mut entry = observed(&path, relative)?;
        if entry.identity.volume_serial != volume {
            return Err(blocked(&path, "Nested volume or changed filesystem"));
        }
        let is_directory = entry.kind == Kind::Directory;
        if contents && entry.kind == Kind::File {
            entry.digest = Some(hash_file(&path, &entry)?);
        }
        entries.push(entry);
        let index = entries.len() - 1;
        if is_directory {
            scan(root, &path, volume, entries, contents)?;
        }
        matches(&path, &entries[index])?;
    }
    Ok(())
}

fn wire(value: u64) -> Value {
    if value <= 9_007_199_254_740_991 {
        json!(value)
    } else {
        json!({"$int64":value.to_string()})
    }
}

fn retryable(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

fn unlink(path: &Path, expected: &native::FileIdentity) -> Result<(), ApiError> {
    // Keep this exact parent namespace pinned through retries and the final
    // missing-name receipt, including when unlinking the root itself.
    let _parent = native::pin_directory(
        path.parent()
            .ok_or_else(|| blocked(path, "Removal target has no parent"))?,
    )?;
    // Retry only Windows occupancy errors, bounded to about 2.3 seconds.
    let mut waits = [20, 40, 80, 160, 320, 640, 1000].into_iter();
    loop {
        match removal::remove(path, expected) {
            Ok(()) => break,
            Err(error) if retryable(&error) => {
                let Some(ms) = waits.next() else {
                    return Err(blocked(path, error)
                        .detail("reason", "file_in_use")
                        .detail("hint", "有文件正在被使用，关闭相关程序后重试。"));
                };
                std::thread::sleep(Duration::from_millis(ms));
            }
            Err(error) => return Err(blocked(path, error)),
        }
    }
    // SetDisposition is a request, not an absence receipt. Never re-delete a
    // name after success: an unrelated replacement may now occupy that name.
    for ms in [0, 20, 40, 80, 160, 320, 640, 1000] {
        if ms != 0 {
            std::thread::sleep(Duration::from_millis(ms));
        }
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if retryable(&error) => (),
            Err(error) => return Err(blocked(path, error)),
            Ok(_) => {
                if !expected.same_object(&removal::inspect(path)?.identity) {
                    return Err(blocked(path, "Replacement appeared after deletion request"));
                }
            }
        }
    }
    Err(
        blocked(path, "Deletion request has not produced an absent name")
            .detail("reason", "file_in_use")
            .detail("hint", "有文件正在被使用，关闭相关程序后重试。"),
    )
}

pub(super) fn remove(path: &Path, expected: &PersistedIdentity) -> Result<Value, ApiError> {
    remove_with_hook(path, expected, || Ok(()))
}

pub(super) fn remove_checked(
    path: &Path,
    expected: &PersistedIdentity,
    verify: impl FnOnce(&Value) -> Result<(), ApiError>,
) -> Result<Value, ApiError> {
    remove_impl(path, expected, true, |entries| {
        verify(&inventory_value(entries))
    })
}

fn remove_with_hook(
    path: &Path,
    expected: &PersistedIdentity,
    after_scan: impl FnOnce() -> Result<(), ApiError>,
) -> Result<Value, ApiError> {
    remove_impl(path, expected, false, |_| after_scan())
}

fn remove_impl(
    path: &Path,
    expected: &PersistedIdentity,
    contents: bool,
    after_scan: impl FnOnce(&[Entry]) -> Result<(), ApiError>,
) -> Result<Value, ApiError> {
    let root = native::pin_directory(path)?;
    if !expected.matches_open_runtime(&root.file)? {
        return Err(blocked(path, "Permanent deletion target identity changed"));
    }
    let identity = native::identity(&root.file)?;
    let (path, scope) = native::directory_scope(&root.file)?;
    let mut entries = Vec::new();
    scan(&path, &path, identity.volume_serial, &mut entries, contents)?;
    after_scan(&entries)?;
    // Match Mac's second complete content scan after the migration callback.
    // An unknown new name or same-size edit cannot be accepted by re-inventory.
    if entries.iter().any(|entry| entry.digest.is_some()) {
        let mut current = Vec::new();
        scan(&path, &path, identity.volume_serial, &mut current, true)?;
        if inventory_value(&current) != inventory_value(&entries) {
            return Err(blocked(&path, "Content changed immediately before removal"));
        }
    }
    let by_path = entries
        .iter()
        .map(|entry| (entry.relative.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut ordered = entries.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| {
        b.relative
            .components()
            .count()
            .cmp(&a.relative.components().count())
            .then(b.relative.cmp(&a.relative))
    });
    let mut removed_bytes = 0u64;
    let mut removed_files = 0u64;
    let mut removed_entries = 0u64;
    let result = (|| {
        for entry in ordered {
            let mut parent = path.clone();
            let mut relative = PathBuf::new();
            let mut pins = Vec::new();
            for component in entry
                .relative
                .parent()
                .unwrap_or(Path::new(""))
                .components()
            {
                let Component::Normal(name) = component else {
                    return Err(blocked(&path, "Invalid inventory parent"));
                };
                parent.push(name);
                relative.push(name);
                let selected = native::pin_directory(&parent)?;
                let original = by_path
                    .get(&relative)
                    .ok_or_else(|| blocked(&parent, "Parent absent from inventory"))?;
                if !original
                    .identity
                    .same_object(&native::identity(&selected.file)?)
                {
                    return Err(blocked(&parent, "Cleanup parent identity changed"));
                }
                pins.push(selected);
            }
            let target = path.join(&entry.relative);
            matches(&target, entry)?;
            if let Some(digest) = &entry.digest {
                if &hash_file(&target, entry)? != digest {
                    return Err(blocked(&target, "Cleanup file changed before removal"));
                }
                matches(&target, entry)?;
            }
            unlink(&target, &entry.identity)?;
            removed_bytes = removed_bytes
                .checked_add(entry.bytes)
                .ok_or_else(|| blocked(&path, "Removal accounting overflow"))?;
            if entry.kind != Kind::Directory {
                removed_files += 1;
            }
            removed_entries += 1;
        }
        if fs::read_dir(&path)?.next().transpose()?.is_some() {
            return Err(blocked(&path, "New unplanned content remains"));
        }
        Ok(())
    })();
    drop(scope);
    drop(root);
    let result = result.and_then(|()| unlink(&path, &identity));
    let summary = json!({"removedLogicalBytes":wire(removed_bytes),"removedFileCount":wire(removed_files),"removedEntryCount":wire(removed_entries),"completed":result.is_ok(),"accountingComplete":result.is_ok(),"countsAreLowerBounds":result.is_err(),"remainingPath":if result.is_err(){Some(&path)}else{None}});
    result
        .map(|()| summary.clone())
        .map_err(|error| error.detail("cleanupResult", summary))
}

#[cfg(test)]
#[path = "removal_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "checked_tests.rs"]
mod checked_tests;
