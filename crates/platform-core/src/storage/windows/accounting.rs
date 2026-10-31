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

fn window_views_directory(paths: &RuntimePaths, id: &str) -> Result<Option<File>, ApiError> {
    let mut directory = native::directory_file(paths.data_root())?;
    for name in ["web-project-ownership", id, "artifact-views"] {
        let (path, _scope) = path_at(&directory, OsStr::new(name))?;
        directory = match native::directory_file(&path) {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
    }
    Ok(Some(directory))
}
fn window_metadata(file: &File) -> Result<(u64, [u8; 16], u32, u64, u64), ApiError> {
    let metadata = file.metadata()?;
    let identity = native::identity(file)?;
    Ok((
        identity.volume_serial,
        identity.file_id,
        metadata.file_attributes(),
        metadata.len(),
        metadata.last_write_time(),
    ))
}
pub(super) fn window_views_signature(paths: &RuntimePaths, id: &str) -> Result<String, ApiError> {
    let Some(directory) = window_views_directory(paths, id)? else {
        return Ok("absent".into());
    };
    let before = window_metadata(&directory)?;
    let mut digest = Sha256::new();
    digest.update(format!("{before:?}"));
    let mut children = names(&directory)?;
    children.sort();
    for name in children {
        let (path, _scope) = path_at(&directory, &name)?;
        let observed = removal::inspect(&path)?;
        let metadata = observed.file.metadata()?;
        let bytes = utf16(&name);
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
        digest.update(format!("{:?}", window_metadata(&observed.file)?));
        if kind(&metadata) == EntryKind::Directory {
            let container = child_dir(&directory, &name)?;
            let before = window_metadata(&container)?;
            if before.0 != observed.identity.volume_serial || before.1 != observed.identity.file_id
            {
                return Err(ApiError::new(
                    "STORAGE_SCAN_INVALIDATED",
                    "Window artifact container identity changed",
                ));
            }
            let (receipt, _scope) = path_at(&container, OsStr::new("view.json"))?;
            match removal::inspect(&receipt) {
                Ok(receipt) => {
                    if kind(&receipt.file.metadata()?) != EntryKind::File {
                        return Err(ApiError::new(
                            "UNSAFE_PATH",
                            "Window artifact receipt is not an ordinary file",
                        ));
                    }
                    digest.update(format!("{:?}", window_metadata(&receipt.file)?));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    digest.update(b"receipt-missing")
                }
                Err(error) => return Err(error.into()),
            }
            if window_metadata(&container)? != before {
                return Err(ApiError::new(
                    "STORAGE_SCAN_INVALIDATED",
                    "Window artifact container changed while inspecting its receipt",
                ));
            }
        }
    }
    if window_metadata(&directory)? != before {
        return Err(ApiError::new(
            "STORAGE_SCAN_INVALIDATED",
            "Window artifact root changed while inspecting containers",
        ));
    }
    Ok(URL_SAFE_NO_PAD.encode(digest.finalize()))
}
pub(super) fn window_views_scan(
    paths: &RuntimePaths,
    id: &str,
    cancelled: &dyn Fn() -> bool,
) -> (PathBuf, Counter, Vec<Value>) {
    let root = window_views_root(paths, id);
    let mut count = Counter::default();
    let mut issues = vec![];
    let directory = match window_views_directory(paths, id) {
        Ok(Some(directory)) => directory,
        Ok(None) => {
            if !matches!(window_views_directory(paths, id), Ok(None)) {
                issues.push(problem(
                    &root,
                    "changed_during_scan",
                    "Window artifact directory appeared while scanning",
                ));
            }
            return (root, count, issues);
        }
        Err(error) => {
            issues.push(problem(&root, "window_views_unavailable", error));
            return (root, count, issues);
        }
    };
    let identity = match native::identity(&directory) {
        Ok(identity) => identity,
        Err(error) => {
            issues.push(problem(&root, "window_views_unavailable", error));
            return (root, count, issues);
        }
    };
    let mut first = walk_cancellable(&root, &[], cancelled, |entry| count.add(entry.bytes));
    let second = walk_cancellable(&root, &[], cancelled, |_| {});
    if first.signature() != second.signature() {
        issues.push(problem(
            &root,
            "changed_during_scan",
            "Window artifact metadata changed between passes",
        ));
    }
    let final_identity = window_views_directory(paths, id)
        .ok()
        .flatten()
        .and_then(|directory| native::identity(&directory).ok());
    if !final_identity.is_some_and(|current| identity.same_object(&current)) {
        issues.push(problem(
            &root,
            "root_changed",
            "Window artifact directory identity changed while scanning",
        ));
    }
    issues.append(&mut first.issues);
    issues.extend(second.issues);
    for issue in &mut issues {
        if let Some(path) = issue["path"].as_str() {
            if !Path::new(path).is_absolute() {
                issue["path"] = json!(root.join(path));
            }
        }
    }
    (root, count, issues)
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
pub(super) fn executable_stamp(path: &Path) -> Result<Value, ApiError> {
    let file = native::open_read_file_allowing_hard_links(path)?;
    let metadata = file.metadata()?;
    let identity = native::identity(&file)?;
    Ok(
        json!({"device":identity.volume_serial,"fileId":identity.file_id,"bytes":metadata.len(),"modified":modified(&metadata)}),
    )
}
