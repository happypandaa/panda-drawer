//! Native counterpart of the storage cleanup whitelist. Only selected cache
//! roots enter this module; ordinary project content is never inferred here.
use super::*;
use platform_native::windows::{files as native, removal};
use std::{ffi::OsStr, time::Duration};

fn current_kind(metadata: &fs::Metadata) -> EntryKind {
    if platform_native::filesystem::is_reparse(metadata) {
        EntryKind::Reparse
    } else if metadata.is_dir() {
        EntryKind::Directory
    } else if metadata.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}
fn in_selected_link_root(path: &Path, allowed: &Path) -> bool {
    let Ok(link) = fs::read_link(path) else {
        return false;
    };
    let target = if link.is_absolute() {
        link
    } else {
        path.parent().unwrap().join(link)
    };
    let mut normalized = PathBuf::new();
    for component in target.components() {
        match component {
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return false;
                }
            }
            std::path::Component::CurDir => (),
            other => normalized.push(other.as_os_str()),
        }
    }
    // The native read_link target can use a verbatim prefix while a caller used
    // DOS spelling, or vice versa. Convert only exact lexical Win32 round trips.
    let target =
        platform_native::windows::api_paths::conventional_path(&normalized).unwrap_or(normalized);
    let allowed = platform_native::windows::api_paths::conventional_path(allowed)
        .unwrap_or_else(|_| allowed.to_owned());
    target.starts_with(allowed)
}
fn require_absent(path: &Path, expected: &native::FileIdentity) -> Result<(), ApiError> {
    for delay in [0, 20, 40, 80, 160, 320, 640, 1000] {
        if delay != 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if matches!(error.raw_os_error(), Some(5 | 32 | 33)) => (),
            Err(error) => return Err(error.into()),
            Ok(_) => {
                if !expected.same_object(&removal::inspect(path)?.identity) {
                    return Err(ApiError::new(
                        "STORAGE_CLEANUP_CHANGED",
                        "A replacement appeared after the deletion request",
                    ));
                }
            }
        }
    }
    Err(ApiError::new(
        "STORAGE_CLEANUP_IN_USE",
        "The selected object is still in use; close it and retry",
    ))
}
fn unlink(path: &Path, expected: &native::FileIdentity) -> Result<(), ApiError> {
    let mut waits = [20, 40, 80, 160, 320, 640, 1000].into_iter();
    loop {
        match removal::remove(path, expected) {
            Ok(()) => break,
            Err(error) if matches!(error.raw_os_error(), Some(5 | 32 | 33)) => {
                let Some(delay) = waits.next() else {
                    return Err(error.into());
                };
                std::thread::sleep(Duration::from_millis(delay));
            }
            Err(error) => return Err(error.into()),
        }
    }
    require_absent(path, expected)
}
pub(super) fn delete_whitelisted(path: &Path, out: &mut Deletion) {
    fn remove(parent: &File, name: &OsStr, path: &Path, allowed: &Path, out: &mut Deletion) {
        if path
            .strip_prefix(allowed)
            .is_ok_and(|relative| relative.components().count() > 256)
        {
            out.issues.push(problem(
                path,
                "depth_limit_retained",
                "Directory is too deep for safe cleanup",
            ));
            return;
        }
        let (actual_parent, _scope) = match native::directory_scope(parent) {
            Ok(value) => value,
            Err(error) => {
                out.issues.push(problem(path, "not_deleted", error));
                return;
            }
        };
        let target = actual_parent.join(name);
        let observed = match removal::inspect(&target) {
            Ok(value) => value,
            Err(error) => {
                out.issues.push(problem(path, "not_deleted", error));
                return;
            }
        };
        let metadata = match observed.file.metadata() {
            Ok(value) => value,
            Err(error) => {
                out.issues.push(problem(path, "not_deleted", error));
                return;
            }
        };
        let kind = current_kind(&metadata);
        if kind == EntryKind::Directory {
            let directory = match native::directory_file(&target) {
                Ok(directory) => directory,
                Err(error) => {
                    out.issues.push(problem(path, "not_deleted", error));
                    return;
                }
            };
            match native::identity(&directory) {
                Ok(identity) if observed.identity.same_object(&identity) => (),
                _ => {
                    out.issues.push(problem(
                        path,
                        "changed_object_retained",
                        "Directory changed before cleanup could inspect it",
                    ));
                    return;
                }
            }
            let children = match fs::read_dir(&target) {
                Ok(children) => children,
                Err(error) => {
                    out.issues.push(problem(path, "not_deleted", error));
                    return;
                }
            };
            for child in children {
                match child {
                    Ok(child) => remove(
                        &directory,
                        &child.file_name(),
                        &path.join(child.file_name()),
                        allowed,
                        out,
                    ),
                    Err(error) => {
                        out.issues.push(problem(path, "not_deleted", error));
                        return;
                    }
                }
            }
            drop(directory);
        } else if kind == EntryKind::Reparse {
            if !in_selected_link_root(&target, allowed) {
                out.issues.push(problem(
                    path,
                    "unsafe_link_retained",
                    "Link target is outside the exact cleanup root or cannot be read",
                ));
                return;
            }
        } else if kind != EntryKind::File {
            out.issues.push(problem(
                path,
                "special_file_retained",
                "Unsupported file kind",
            ));
            return;
        }
        let current = match removal::inspect(&target) {
            Ok(value) => value,
            Err(error) => {
                out.issues.push(problem(path, "not_deleted", error));
                return;
            }
        };
        let current_metadata = match current.file.metadata() {
            Ok(value) => value,
            Err(error) => {
                out.issues.push(problem(path, "not_deleted", error));
                return;
            }
        };
        if !observed.identity.same_object(&current.identity)
            || kind != current_kind(&current_metadata)
            || metadata.permissions().readonly() != current_metadata.permissions().readonly()
        {
            out.issues.push(problem(
                path,
                "changed_object_retained",
                "Object changed during cleanup",
            ));
            return;
        }
        let expected = current.identity.clone();
        // Our observation handles must not keep a successful disposition pending.
        // The actual parent namespace remains pinned by the caller and _scope.
        drop(current);
        drop(observed);
        match unlink(&target, &expected) {
            Ok(()) if kind == EntryKind::File => {
                out.bytes = out.bytes.saturating_add(metadata.len());
                out.files += 1;
            }
            Ok(()) if kind == EntryKind::Reparse => out.links += 1,
            Ok(()) => (),
            Err(error) => out.issues.push(problem(path, "not_deleted", error)),
        }
    }
    if let Err(error) = check_no_symlinks(path, true) {
        out.issues
            .push(problem(path, "unsafe_root_retained", error));
        return;
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    match native::directory_file(parent) {
        Ok(parent) => {
            remove(&parent, name, path, path, out);
            let _ = native::sync_directory(&parent);
        }
        Err(error) => out.issues.push(problem(path, "not_deleted", error)),
    }
}
