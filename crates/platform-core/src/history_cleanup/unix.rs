//! Bound native directory removal used by deletion and verified workspace cleanup.
use crate::{
    file_identity::PersistedIdentity,
    projects::{check_no_symlinks, valid_id},
    ApiError, RuntimePaths,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Identity {
    device: u64,
    inode: u64,
}
impl Identity {
    fn of(m: &fs::Metadata) -> Self {
        Self {
            device: m.dev(),
            inode: m.ino(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Entry {
    path: PathBuf,
    kind: String,
    identity: Identity,
    bytes: u64,
    digest: Option<String>,
    target: Option<PathBuf>,
    mode: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    id: String,
    kind: String,
    label: String,
    path: PathBuf,
    project_id: Option<String>,
    name: Option<String>,
    total_bytes: Option<u64>,
    file_count: Option<u64>,
    can_clean: bool,
    blocked_reasons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    root_identity: Option<Identity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    parents: Vec<(PathBuf, Identity)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    entries: Vec<Entry>,
    #[serde(default)]
    guards: BTreeMap<PathBuf, String>,
}
impl Candidate {
    fn new(id: String, kind: &str, label: &str, path: PathBuf) -> Self {
        Self {
            id,
            kind: kind.into(),
            label: label.into(),
            path,
            project_id: None,
            name: None,
            total_bytes: None,
            file_count: None,
            can_clean: false,
            blocked_reasons: vec![],
            root_identity: None,
            parents: vec![],
            entries: vec![],
            guards: BTreeMap::new(),
        }
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    removed_logical_bytes: u64,
    removed_file_count: u64,
    removed_paths: Vec<PathBuf>,
}
fn error(message: impl std::fmt::Display) -> ApiError {
    ApiError::new("WORKSPACE_CLEANUP_BLOCKED", message.to_string())
}

fn wire_u64(value: Option<u64>) -> Value {
    match value {
        None => Value::Null,
        Some(value) if value <= 9_007_199_254_740_991 => json!(value),
        Some(value) => json!({"$int64":value.to_string()}),
    }
}

fn read_json(path: &Path) -> Result<Value, ApiError> {
    check_no_symlinks(path, false)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > 64_000_000 {
        return Err(error("Invalid cleanup metadata"));
    }
    Ok(serde_json::from_reader(file)?)
}

fn hash_reader(file: &mut File) -> Result<String, ApiError> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 128 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn directory_identity(path: &Path) -> Result<Identity, ApiError> {
    check_no_symlinks(path, false)?;
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(error("Expected an ordinary directory"));
    }
    Ok(Identity::of(&meta))
}

fn normalized(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
}

fn parent_identities(path: &Path) -> Result<Vec<(PathBuf, Identity)>, ApiError> {
    if !normalized(path) {
        return Err(error("Unnormalized cleanup path"));
    }
    path.ancestors()
        .skip(1)
        .map(|path| Ok((path.to_owned(), directory_identity(path)?)))
        .collect()
}

fn scan(path: &Path) -> Result<Vec<Entry>, ApiError> {
    scan_with_content(path, true)
}

fn scan_with_content(path: &Path, contents: bool) -> Result<Vec<Entry>, ApiError> {
    let identity = directory_identity(path)?;
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    if Identity::of(&root.metadata()?) != identity {
        return Err(error("Cleanup root changed during scan"));
    }
    fn walk(
        directory: &File,
        relative: &Path,
        device: u64,
        out: &mut Vec<Entry>,
        contents: bool,
    ) -> Result<(), ApiError> {
        if relative.components().count() > 256 || out.len() > 500_000 {
            return Err(error("Cleanup inventory exceeds its depth or entry bound"));
        }
        let mut children = directory_names(directory)?;
        children.sort();
        for name in children {
            if out.len() >= 500_000 {
                return Err(error("Cleanup inventory exceeds its entry bound"));
            }
            let rel = relative.join(&name);
            if rel.to_str().is_none() {
                return Err(error("Non-UTF8 cleanup filename"));
            }
            let stat = stat_at(directory, &name)?;
            if stat.st_dev as u64 != device {
                return Err(error("Nested mount or changed filesystem"));
            }
            let mut entry = Entry {
                path: rel.clone(),
                kind: String::new(),
                identity: Identity {
                    device: stat.st_dev as u64,
                    inode: stat.st_ino,
                },
                bytes: 0,
                digest: None,
                target: None,
                mode: stat.st_mode as u32 & 0o777,
            };
            match stat.st_mode & libc::S_IFMT {
                libc::S_IFLNK => {
                    entry.kind = "symlink".into();
                    entry.target = Some(read_link_at(directory, &name)?);
                }
                libc::S_IFREG => {
                    entry.kind = "file".into();
                    entry.bytes = stat.st_size as u64;
                    if contents {
                        let mut file = open_file_at(directory, &name)?;
                        if Identity::of(&file.metadata()?) != entry.identity {
                            return Err(error("File changed while scanning"));
                        }
                        entry.digest = Some(hash_reader(&mut file)?);
                    }
                }
                libc::S_IFDIR => entry.kind = "directory".into(),
                _ => return Err(error("Unsupported special file in cleanup tree")),
            }
            if !stat_matches(&stat_at(directory, &name)?, &entry) {
                return Err(error("Entry changed during scan"));
            }
            let child = if entry.kind == "directory" {
                Some(open_directory_at(directory, &name)?)
            } else {
                None
            };
            if child.as_ref().is_some_and(|child| {
                child.metadata().is_err()
                    || child
                        .metadata()
                        .is_ok_and(|meta| Identity::of(&meta) != entry.identity)
            }) {
                return Err(error("Directory changed during scan"));
            }
            out.push(entry.clone());
            if let Some(child) = child {
                walk(&child, &rel, device, out, contents)?;
            }
            if !stat_matches(&stat_at(directory, &name)?, &entry) {
                return Err(error("Directory tree changed during scan"));
            }
        }
        Ok(())
    }
    let mut entries = vec![];
    walk(
        &root,
        Path::new(""),
        identity.device,
        &mut entries,
        contents,
    )?;
    if directory_identity(path)? != identity {
        return Err(error("Cleanup root changed during scan"));
    }
    Ok(entries)
}

fn directory_names(directory: &File) -> Result<Vec<PathBuf>, ApiError> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    let descriptor = unsafe { libc::dup(directory.as_raw_fd()) };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stream = unsafe { libc::fdopendir(descriptor) };
    if stream.is_null() {
        unsafe {
            libc::close(descriptor);
        }
        return Err(std::io::Error::last_os_error().into());
    }
    struct Stream(*mut libc::DIR);
    impl Drop for Stream {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let stream = Stream(stream);
    let mut names = vec![];
    loop {
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error() = 0;
        }
        #[cfg(not(target_os = "macos"))]
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let failure = std::io::Error::last_os_error();
            if failure.raw_os_error().unwrap_or(0) != 0 {
                return Err(failure.into());
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            names.push(PathBuf::from(std::ffi::OsStr::from_bytes(name)));
        }
    }
    Ok(names)
}

fn open_file_at(directory: &File, name: &Path) -> Result<File, ApiError> {
    let name = c_name(name)?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn read_link_at(directory: &File, name: &Path) -> Result<PathBuf, ApiError> {
    use std::os::unix::ffi::OsStringExt;
    let name = c_name(name)?;
    let mut bytes = vec![0; 256];
    loop {
        let count = unsafe {
            libc::readlinkat(
                directory.as_raw_fd(),
                name.as_ptr(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            )
        };
        if count < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if count as usize == bytes.len() {
            if bytes.len() > 1_000_000 {
                return Err(error("Unexpected symlink target size"));
            }
            bytes.resize(bytes.len() * 2, 0);
            continue;
        }
        bytes.truncate(count as usize);
        return Ok(std::ffi::OsString::from_vec(bytes).into());
    }
}

fn verify_parents(item: &Candidate) -> Result<(), ApiError> {
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

fn c_name(path: &Path) -> Result<CString, ApiError> {
    CString::new(path.as_os_str().as_bytes()).map_err(error)
}

fn open_directory_at(parent: &File, name: &Path) -> Result<File, ApiError> {
    let name = c_name(name)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn stat_at(parent: &File, name: &Path) -> Result<libc::stat, ApiError> {
    let name = c_name(name)?;
    let mut value = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            value.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { value.assume_init() })
}

fn stat_matches(stat: &libc::stat, entry: &Entry) -> bool {
    let kind = stat.st_mode & libc::S_IFMT;
    let wanted = match entry.kind.as_str() {
        "file" => libc::S_IFREG,
        "directory" => libc::S_IFDIR,
        "symlink" => libc::S_IFLNK,
        _ => return false,
    };
    stat.st_dev as u64 == entry.identity.device
        && stat.st_ino == entry.identity.inode
        && kind == wanted
        && stat.st_mode as u32 & 0o777 == entry.mode
        && (entry.kind != "file" || stat.st_size as u64 == entry.bytes)
}

fn relative_parent(
    root: &File,
    relative: &Path,
    entries: &BTreeMap<PathBuf, &Entry>,
) -> Result<File, ApiError> {
    let mut directory = root.try_clone()?;
    let mut accumulated = PathBuf::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(error("Unsafe inventory relative path"));
        };
        accumulated.push(name);
        directory = open_directory_at(&directory, Path::new(name))?;
        let expected = entries
            .get(&accumulated)
            .ok_or_else(|| error("Directory is absent from the confirmed inventory"))?;
        if Identity::of(&directory.metadata()?) != expected.identity {
            return Err(error("Cleanup parent directory changed"));
        }
    }
    Ok(directory)
}

pub(crate) fn remove_owned_directory(
    path: &Path,
    expected: &PersistedIdentity,
) -> Result<Value, ApiError> {
    remove_owned_directory_bound(path, expected, || Ok(()))
}

fn remove_owned_directory_bound(
    path: &Path,
    expected: &PersistedIdentity,
    after_bind: impl FnOnce() -> Result<(), ApiError>,
) -> Result<Value, ApiError> {
    remove_owned_directory_snapshot(path, expected, false, after_bind, |_| Ok(()))
}

pub(crate) fn remove_owned_directory_checked(
    path: &Path,
    expected: &PersistedIdentity,
    verify: impl FnOnce(&Value) -> Result<(), ApiError>,
) -> Result<Value, ApiError> {
    remove_owned_directory_snapshot(path, expected, true, || Ok(()), verify)
}

fn remove_owned_directory_snapshot(
    path: &Path,
    expected: &PersistedIdentity,
    content: bool,
    after_bind: impl FnOnce() -> Result<(), ApiError>,
    verify: impl FnOnce(&Value) -> Result<(), ApiError>,
) -> Result<Value, ApiError> {
    check_no_symlinks(path, false)?;
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let observed = PersistedIdentity::from_file(&root).map_err(error)?;
    if !expected.matches_captured(&observed) {
        return Err(error("Permanent deletion target identity changed"));
    }
    let mut item = Candidate::new(
        "permanent-delete".into(),
        "ownedDirectory",
        "平台项目目录",
        path.into(),
    );
    item.root_identity = Some(Identity::of(&root.metadata()?));
    after_bind()?;
    item.parents = parent_identities(path)?;
    verify_parents(&item)?;
    item.entries = scan_with_content(path, content)?;
    verify_parents(&item)?;
    verify(&serde_json::to_value(&item.entries)?)?;
    item.can_clean = true;
    let result = remove_tree_with_hook(&item, |_, _| Ok(()));
    drop(root);
    result
}

fn remove_tree_with_hook(
    item: &Candidate,
    mut after_remove: impl FnMut(&Progress, &Path) -> Result<(), ApiError>,
) -> Result<Value, ApiError> {
    verify_parents(item)?;
    let contents = item.entries.iter().any(|entry| entry.digest.is_some());
    if contents && scan(&item.path)? != item.entries {
        return Err(error("Content changed immediately before removal"));
    }
    let parent = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(item.path.parent().unwrap())?;
    let root = open_directory_at(&parent, Path::new(item.path.file_name().unwrap()))?;
    if Some(Identity::of(&root.metadata()?)) != item.root_identity {
        return Err(error("Cleanup root changed before opening"));
    }
    let by_path = item
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut progress = Progress::default();
    let mut ordered = item.entries.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| {
        b.path
            .components()
            .count()
            .cmp(&a.path.components().count())
            .then(b.path.cmp(&a.path))
    });
    let run = (|| -> Result<(), ApiError> {
        for entry in ordered {
            verify_parents(item)?;
            let directory = relative_parent(
                &root,
                entry.path.parent().unwrap_or(Path::new("")),
                &by_path,
            )?;
            let name = Path::new(entry.path.file_name().unwrap());
            if !stat_matches(&stat_at(&directory, name)?, entry) {
                return Err(error("Cleanup file identity or type changed"));
            }
            if entry.kind == "file" && entry.digest.is_some() {
                let name_c = c_name(name)?;
                let fd = unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        name_c.as_ptr(),
                        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if fd < 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                let mut file = unsafe { File::from_raw_fd(fd) };
                if Identity::of(&file.metadata()?) != entry.identity
                    || Some(hash_reader(&mut file)?) != entry.digest
                {
                    return Err(error("Cleanup file changed before removal"));
                }
            } else if entry.kind == "symlink"
                && read_link_at(&directory, name)? != *entry.target.as_ref().unwrap()
            {
                return Err(error("Cleanup symlink target changed"));
            }
            if !stat_matches(&stat_at(&directory, name)?, entry) {
                return Err(error("Cleanup entry changed before unlink"));
            }
            let name_c = c_name(name)?;
            let flags = if entry.kind == "directory" {
                libc::AT_REMOVEDIR
            } else {
                0
            };
            if unsafe { libc::unlinkat(directory.as_raw_fd(), name_c.as_ptr(), flags) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            progress.removed_logical_bytes = progress
                .removed_logical_bytes
                .checked_add(entry.bytes)
                .ok_or_else(|| error("Removal accounting exceeds supported integer range"))?;
            if entry.kind != "directory" {
                progress.removed_file_count += 1;
            }
            progress.removed_paths.push(entry.path.clone());
            after_remove(&progress, &item.path)?;
        }
        verify_parents(item)?;
        if fs::read_dir(&item.path)?.next().is_some() {
            return Err(error("New unplanned content remains; directory retained"));
        }
        let name = c_name(Path::new(item.path.file_name().unwrap()))?;
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        parent.sync_all()?;
        Ok(())
    })();
    let summary = json!({"removedLogicalBytes":wire_u64(Some(progress.removed_logical_bytes)),"removedFileCount":wire_u64(Some(progress.removed_file_count)),"removedEntryCount":wire_u64(Some(progress.removed_paths.len() as u64)),"completed":run.is_ok(),"accountingComplete":run.is_ok(),"countsAreLowerBounds":run.is_err(),"remainingPath":if run.is_err(){Some(&item.path)}else{None}});
    run.map(|()| summary.clone())
        .map_err(|failure| failure.detail("cleanupResult", summary))
}

pub(crate) fn retired_cleanup_status(
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

#[cfg(test)]
#[path = "../history_cleanup_tests.rs"]
mod tests;

#[cfg(test)]
fn remove_tree(item: &Candidate) -> Result<Value, ApiError> {
    remove_tree_with_hook(item, |_, _| Ok(()))
}
