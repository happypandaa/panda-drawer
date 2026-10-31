//! Clear only a verified manager UDF's children after its actual native exit.
//! Keeping the original empty root preserves the user's frozen deletion plan.
use platform_core::ApiError;
use platform_native::windows::{files as native, removal};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, PartialEq, Eq)]
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
}
fn pending(path: &Path, cause: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        "MANAGER_PROFILE_CLEANUP_UNCONFIRMED",
        "管理器浏览器存储清理尚未完成，已保留剩余内容。",
    )
    .detail("remainingPath", path.to_string_lossy().into_owned())
    .detail("cause", cause.to_string())
}
fn observe(path: &Path, relative: PathBuf) -> Result<Entry, ApiError> {
    let opened = removal::inspect(path)?;
    let metadata = opened.file.metadata()?;
    let kind = if platform_native::filesystem::is_reparse(&metadata) {
        Kind::Reparse
    } else if metadata.is_dir() {
        Kind::Directory
    } else if metadata.is_file() {
        Kind::File
    } else {
        return Err(pending(path, "Unsupported manager profile entry"));
    };
    let target = if kind == Kind::Reparse {
        Some(fs::read_link(path)?)
    } else {
        None
    };
    let bytes = if kind == Kind::File {
        metadata.len()
    } else {
        0
    };
    Ok(Entry {
        relative,
        identity: opened.identity,
        kind,
        bytes,
        readonly: metadata.permissions().readonly(),
        target,
    })
}
fn same(path: &Path, original: &Entry) -> Result<(), ApiError> {
    let actual = observe(path, original.relative.clone())?;
    if !original.identity.same_object(&actual.identity)
        || original.kind != actual.kind
        || original.bytes != actual.bytes
        || original.readonly != actual.readonly
        || original.target != actual.target
    {
        return Err(pending(path, "Manager profile entry changed"));
    }
    Ok(())
}
fn scan(
    root: &Path,
    directory: &Path,
    volume: u64,
    entries: &mut Vec<Entry>,
) -> Result<(), ApiError> {
    if directory
        .strip_prefix(root)
        .map_err(|error| pending(directory, error))?
        .components()
        .count()
        > 256
    {
        return Err(pending(directory, "Cleanup depth exceeds 256"));
    }
    let _pin = native::pin_directory(directory)?;
    let mut children = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        if entries.len() >= 500_000 {
            return Err(pending(
                directory,
                "Cleanup inventory exceeds 500000 entries",
            ));
        }
        let path = child.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|error| pending(&path, error))?
            .to_path_buf();
        let entry = observe(&path, relative)?;
        if entry.identity.volume_serial != volume {
            return Err(pending(&path, "Manager profile nested volume changed"));
        }
        let directory = entry.kind == Kind::Directory;
        entries.push(entry);
        let index = entries.len() - 1;
        if directory {
            scan(root, &path, volume, entries)?;
        }
        same(&path, &entries[index])?;
    }
    Ok(())
}
fn unlink(path: &Path, identity: &native::FileIdentity) -> Result<(), ApiError> {
    let mut waits = [20, 40, 80, 160, 320, 640, 1000].into_iter();
    loop {
        match removal::remove(path, identity) {
            Ok(()) => break,
            Err(error) if matches!(error.raw_os_error(), Some(5 | 32 | 33)) => {
                let delay = waits.next().ok_or_else(|| pending(path, &error))?;
                std::thread::sleep(Duration::from_millis(delay));
            }
            Err(error) => return Err(pending(path, error)),
        }
    }
    for delay in [0, 20, 40, 80, 160, 320, 640, 1000] {
        if delay != 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if matches!(error.raw_os_error(), Some(5 | 32 | 33)) => (),
            Err(error) => return Err(pending(path, error)),
            Ok(_) => {
                if !identity.same_object(&removal::inspect(path)?.identity) {
                    return Err(pending(
                        path,
                        "Replacement appeared after manager cleanup request",
                    ));
                }
            }
        }
    }
    Err(pending(
        path,
        "Manager profile entry remains delete-pending",
    ))
}
pub(super) fn clear_contents(path: &Path, expected: &native::FileIdentity) -> Result<(), ApiError> {
    // Only this already-owned root's real DELETE sharing is checked. This
    // does not inspect ancestors' ACLs or preflight every descendant.
    removal::verify_removal_access(path, expected)?;
    let root = native::pin_directory(path)?;
    if !expected.same_object(&native::identity(&root.file)?) {
        return Err(pending(path, "Manager root identity changed"));
    }
    let (path, _scope) = native::directory_scope(&root.file)?;
    let mut entries = vec![];
    scan(&path, &path, expected.volume_serial, &mut entries)?;
    let by_path = entries
        .iter()
        .map(|entry| (entry.relative.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut ordered = entries.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        right
            .relative
            .components()
            .count()
            .cmp(&left.relative.components().count())
            .then(right.relative.cmp(&left.relative))
    });
    for entry in ordered {
        let mut parent = path.clone();
        let mut relative = PathBuf::new();
        let mut pins = vec![];
        for component in entry
            .relative
            .parent()
            .unwrap_or(Path::new(""))
            .components()
        {
            let std::path::Component::Normal(name) = component else {
                return Err(pending(&path, "Invalid cleanup parent"));
            };
            parent.push(name);
            relative.push(name);
            let pin = native::pin_directory(&parent)?;
            let original = by_path
                .get(&relative)
                .ok_or_else(|| pending(&parent, "Parent absent from inventory"))?;
            if !original.identity.same_object(&native::identity(&pin.file)?) {
                return Err(pending(&parent, "Manager profile parent changed"));
            }
            pins.push(pin);
        }
        let selected = path.join(&entry.relative);
        same(&selected, entry)?;
        unlink(&selected, &entry.identity)?;
    }
    if !expected.same_object(&native::identity(&root.file)?)
        || fs::read_dir(&path)?.next().transpose()?.is_some()
    {
        return Err(pending(
            &path,
            "Manager original root changed or new content remains",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_manager_contents_clear_keeps_original_root_and_external_hardlink() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-manager-clear-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let directory = root.join("manager");
        native::create_private_directory(&directory).unwrap();
        fs::create_dir(directory.join("nested")).unwrap();
        fs::write(directory.join("nested/value"), b"private browser fixture").unwrap();
        let outside = root.join("external-original");
        fs::hard_link(directory.join("nested/value"), &outside).unwrap();
        native::set_read_only(&outside, true).unwrap();
        let identity = native::open_no_reparse(&directory).unwrap().identity;
        clear_contents(&directory, &identity).unwrap();
        assert!(identity.same_object(&native::open_no_reparse(&directory).unwrap().identity));
        assert!(fs::read_dir(&directory).unwrap().next().is_none());
        assert_eq!(fs::read(&outside).unwrap(), b"private browser fixture");
        assert!(fs::metadata(&outside).unwrap().permissions().readonly());
    }
    #[test]
    fn retained_root_pin_and_wrong_full_id_do_not_clear_any_manager_bytes() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-manager-clear-held-")
            .tempdir()
            .unwrap();
        let directory = temporary.path().canonicalize().unwrap().join("manager");
        native::create_private_directory(&directory).unwrap();
        fs::write(directory.join("value"), b"retain").unwrap();
        let pin = native::pin_directory(&directory).unwrap();
        let identity = native::identity(&pin.file).unwrap();
        assert!(clear_contents(&directory, &identity).is_err());
        assert_eq!(fs::read(directory.join("value")).unwrap(), b"retain");
        drop(pin);
        let mut wrong = identity.clone();
        wrong.file_id[15] ^= 1;
        assert!(clear_contents(&directory, &wrong).is_err());
        assert_eq!(fs::read(directory.join("value")).unwrap(), b"retain");
        clear_contents(&directory, &identity).unwrap();
    }
}
