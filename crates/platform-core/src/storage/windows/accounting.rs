//! Native metadata-only storage walks. No file body, database or source command
//! is opened by accounting; Windows names and complete IDs remain native.
use super::*;
use platform_native::windows::{files as native, removal};
use std::{
    ffi::{OsStr, OsString},
    os::windows::{ffi::OsStrExt, fs::MetadataExt},
};

fn utf16(value: &OsStr) -> Vec<u8> {
    value.encode_wide().flat_map(u16::to_le_bytes).collect()
}
pub(super) fn root_identity(path: &Path) -> Result<RootIdentity, ApiError> {
    let directory = native::directory_file(path)?;
    let (path, _scope) = native::directory_scope(&directory)?;
    let identity = native::identity(&directory)?;
    Ok(RootIdentity {
        path,
        device: identity.volume_serial,
        inode: u64::from_le_bytes(identity.file_id[..8].try_into().unwrap()),
        file_id: identity.file_id,
    })
}

pub(super) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ApiError> {
    check_no_symlinks(path, false)?;
    // read_json on Mac permits ordinary hard-linked metadata. The no-reparse
    // HANDLE strengthens path binding without adding a single-link policy.
    let mut file = native::open_read_file_allowing_hard_links(path)?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}
fn kind(metadata: &fs::Metadata) -> EntryKind {
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
fn modified(metadata: &fs::Metadata) -> u64 {
    metadata
        .last_write_time()
        .saturating_sub(116_444_736_000_000_000)
        / 10_000
}
fn names(directory: &File) -> Result<Vec<OsString>, ApiError> {
    let (path, _scope) = native::directory_scope(directory)?;
    fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.file_name()).map_err(Into::into))
        .collect()
}
fn path_at(parent: &File, name: &OsStr) -> Result<(PathBuf, native::PinnedDirectory), ApiError> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(ApiError::new(
            "INVALID_PATH",
            "Expected one native filename",
        ));
    }
    let (parent, scope) = native::directory_scope(parent)?;
    Ok((parent.join(name), scope))
}
fn child_dir(parent: &File, name: &OsStr) -> Result<File, ApiError> {
    let (path, _scope) = path_at(parent, name)?;
    Ok(native::directory_file(&path)?)
}
fn relative_label(path: &Path) -> String {
    path.iter()
        .map(|part| {
            part.to_str()
                .expect("walk accepts UTF-8 names before joining them")
        })
        .collect::<Vec<_>>()
        .join("/")
}
pub(super) fn walk(root: &Path, excluded: &[PathBuf], visit: impl FnMut(&Entry)) -> Walk {
    walk_cancellable(root, excluded, &|| false, visit)
}
pub(super) fn walk_cancellable(
    root: &Path,
    excluded: &[PathBuf],
    cancelled: &dyn Fn() -> bool,
    mut visit: impl FnMut(&Entry),
) -> Walk {
    fn descend(
        dir: &File,
        relative: &Path,
        excluded: &[PathBuf],
        out: &mut Walk,
        cancelled: &dyn Fn() -> bool,
        visit: &mut dyn FnMut(&Entry),
    ) {
        if cancelled() {
            return;
        }
        if relative.components().count() > 256 {
            out.issues.push(problem(
                relative,
                "depth_limit",
                "Directory depth exceeds the safe scanner limit",
            ));
            return;
        }
        let before = dir
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        let (listing, _listing_scope) = match native::directory_scope(dir) {
            Ok(value) => value,
            Err(error) => {
                out.issues.push(problem(relative, "unreadable", error));
                return;
            }
        };
        let children = match fs::read_dir(listing) {
            Ok(children) => children,
            Err(error) => {
                out.issues.push(problem(relative, "unreadable", error));
                return;
            }
        };
        for child in children {
            if cancelled() {
                break;
            }
            let name = match child {
                Ok(child) => child.file_name(),
                Err(error) => {
                    out.issues.push(problem(relative, "unreadable", error));
                    continue;
                }
            };
            let Some(label) = name.to_str() else {
                out.issues.push(problem(
                    relative,
                    "invalid_filename",
                    "Filename is not UTF-8",
                ));
                continue;
            };
            let relative = relative.join(label);
            if excluded
                .iter()
                .any(|path| relative == *path || relative.starts_with(path))
            {
                continue;
            }
            let (path, _scope) = match path_at(dir, &name) {
                Ok(value) => value,
                Err(error) => {
                    out.issues.push(problem(&relative, "unreadable", error));
                    continue;
                }
            };
            let observed = match removal::inspect(&path) {
                Ok(value) => value,
                Err(error) => {
                    out.issues.push(problem(&relative, "unreadable", error));
                    continue;
                }
            };
            let metadata = match observed.file.metadata() {
                Ok(value) => value,
                Err(error) => {
                    out.issues.push(problem(&relative, "unreadable", error));
                    continue;
                }
            };
            let entry = Entry {
                path: relative_label(&relative),
                bytes: metadata.len(),
                modified: modified(&metadata),
                device: observed.identity.volume_serial,
                inode: u64::from_le_bytes(observed.identity.file_id[..8].try_into().unwrap()),
                file_id: observed.identity.file_id,
                kind: kind(&metadata),
            };
            out.note(&entry);
            // Keep sub-millisecond writes and native attribute changes in the
            // two-pass signature too. Public timestamps remain Unix millis.
            let metadata_digest = Sha256::digest(format!(
                "{}\0{}\0{}\0{}",
                entry.path,
                metadata.creation_time(),
                metadata.last_write_time(),
                metadata.file_attributes(),
            ));
            for (byte, digest) in out.fingerprint.iter_mut().zip(metadata_digest) {
                *byte ^= digest;
            }
            match entry.kind {
                EntryKind::File => visit(&entry),
                EntryKind::Directory => match child_dir(dir, &name) {
                    Ok(child) => descend(&child, &relative, excluded, out, cancelled, visit),
                    Err(error) => out.issues.push(problem(&relative, "unreadable", error)),
                },
                EntryKind::Reparse => out.issues.push(problem(
                    &relative,
                    "reparse_not_counted",
                    "Only ordinary files contribute logical bytes",
                )),
                EntryKind::Other => out.issues.push(problem(
                    &relative,
                    "special_file_not_counted",
                    "Only ordinary files contribute logical bytes",
                )),
            }
        }
        if before
            != dir
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
        {
            out.issues.push(problem(
                relative,
                "changed_during_scan",
                "Directory changed while enumerating",
            ));
        }
    }
    let mut out = Walk::new();
    match native::directory_file(root) {
        Ok(directory) => descend(
            &directory,
            Path::new(""),
            excluded,
            &mut out,
            cancelled,
            &mut visit,
        ),
        Err(error) => out.issues.push(problem(root, "unreadable", error)),
    }
    out
}

pub(super) fn validate_query_prefix(path: &str) -> Result<(), ApiError> {
    if path.is_empty()
        || path.len() > 2048
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| {
            part.is_empty() || matches!(part, "." | "..") || part.encode_utf16().count() > 255
        })
    {
        return Err(ApiError::new(
            "INVALID_PATH",
            "Path must be a relative, non-empty file path without dot segments",
        ));
    }
    Ok(())
}

pub(super) fn disk_identity(path: &Path) -> Option<u64> {
    native::open_no_reparse(path)
        .ok()
        .map(|file| file.identity.volume_serial)
}
