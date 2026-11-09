//! Descriptor-bound publication into an existing user directory. A small
//! receipt per entry is durable before its first write; cleanup never adopts
//! an entire user subtree, including the temporary destination-volume stage.
use super::{Binding, SelectedDirectory};
use crate::{file_identity::PersistedIdentity, files, ApiError};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, CString},
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt},
        },
    },
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

const MAX_ENTRIES: usize = 50_000;
const MAX_BYTES: u64 = 512 * 1024 * 1024;
const MIN_FREE: u64 = 64 * 1024 * 1024;
const HEADER: &str = "selected-publication.json";
const WORKING: &str = "selected-working.json";
const ENTRIES: &str = "selected-entries";
const STAGE_REMOVAL: &str = "selected-stage-removal.json";

fn failure(message: &str) -> ApiError {
    ApiError::new("SOURCE_SELECTED_DIRECTORY_CHANGED", message)
}
fn nonempty() -> ApiError {
    ApiError::new(
        "SOURCE_SELECTED_DIRECTORY_NOT_EMPTY",
        "请选择一个空文件夹，或新建一个。",
    )
}
fn io(error: std::io::Error) -> ApiError {
    files::error(error)
}
fn name(value: &str) -> Result<CString, ApiError> {
    if value.is_empty()
        || value.contains('/')
        || matches!(value, "." | "..")
        || value.chars().any(char::is_control)
    {
        return Err(failure("所选文件夹中的条目名称无效。"));
    }
    CString::new(value).map_err(|_| failure("所选文件夹中的条目名称无效。"))
}

struct Anchor {
    path: PathBuf,
    file: File,
    identity: PersistedIdentity,
}
impl Anchor {
    fn open(path: &Path) -> Result<Self, ApiError> {
        let file = files::root(path)?;
        let identity = PersistedIdentity::from_file(&file).map_err(|e| failure(&e.to_string()))?;
        Ok(Self {
            path: path.into(),
            file,
            identity,
        })
    }
    fn verify(&self) -> Result<(), ApiError> {
        let current = files::root(&self.path)?;
        let actual = PersistedIdentity::from_file(&current).map_err(|e| failure(&e.to_string()))?;
        if !self.identity.matches_captured(&actual) {
            return Err(failure("所选文件夹或发布暂存目录已被替换，请保留现场。"));
        }
        Ok(())
    }
    fn child(&self, value: &str) -> Result<Self, ApiError> {
        self.verify()?;
        let descriptor = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name(value)?.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(io(std::io::Error::last_os_error()));
        }
        let file = unsafe { File::from_raw_fd(descriptor) };
        let identity = PersistedIdentity::from_file(&file).map_err(|e| failure(&e.to_string()))?;
        let child = Self {
            path: self.path.join(value),
            file,
            identity,
        };
        self.verify()?;
        child.verify()?;
        Ok(child)
    }
}

fn stat(parent: &File, value: &str) -> Result<Option<libc::stat>, ApiError> {
    let value = name(value)?;
    let mut output = std::mem::MaybeUninit::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            value.as_ptr(),
            output.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } == 0
    {
        return Ok(Some(unsafe { output.assume_init() }));
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::NotFound {
        Ok(None)
    } else {
        Err(io(error))
    }
}
fn names(directory: &File) -> Result<Vec<String>, ApiError> {
    let fd = unsafe { libc::dup(directory.as_raw_fd()) };
    if fd < 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        unsafe {
            libc::close(fd);
        }
        return Err(io(std::io::Error::last_os_error()));
    }
    struct Listing(*mut libc::DIR);
    impl Drop for Listing {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let listing = Listing(stream);
    unsafe {
        libc::rewinddir(listing.0);
    }
    let mut output = Vec::new();
    loop {
        unsafe {
            *libc::__error() = 0;
        }
        let entry = unsafe { libc::readdir(listing.0) };
        if entry.is_null() {
            let issue = std::io::Error::last_os_error();
            if issue.raw_os_error().unwrap_or(0) != 0 {
                return Err(io(issue));
            }
            break;
        }
        let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        let value =
            std::str::from_utf8(bytes).map_err(|_| failure("所选文件夹中的条目名称无效。"))?;
        name(value)?;
        if output.len() >= MAX_ENTRIES {
            return Err(failure("应用文件过多，未继续发布。"));
        }
        output.push(value.into());
    }
    output.sort();
    Ok(output)
}
fn identity(root: &Anchor, metadata: &libc::stat) -> PersistedIdentity {
    PersistedIdentity {
        device: metadata.st_dev as u64,
        inode: metadata.st_ino,
        volume_uuid: if metadata.st_dev as u64 == root.identity.device {
            root.identity.volume_uuid.clone()
        } else {
            None
        },
        windows_file_id: None,
    }
}
fn revision(path: &Path, identity: &PersistedIdentity) -> Result<String, ApiError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            path,
            identity.deletion_revision_value()
        ))?)
    ))
}
fn selected_root(selection: &SelectedDirectory) -> Result<Anchor, ApiError> {
    let root = Anchor::open(&selection.path)?;
    if revision(&root.path, &root.identity)? != selection.revision {
        return Err(failure("所选文件夹已变化，请重新选择空文件夹。"));
    }
    Ok(root)
}
fn empty(root: &Anchor) -> Result<(), ApiError> {
    for value in names(&root.file)? {
        if value != ".DS_Store" {
            return Err(nonempty());
        }
        let file = files::read(&root.file, &value)?;
        if !file.metadata()?.is_file() {
            return Err(nonempty());
        }
    }
    root.verify()
}
pub(super) fn inspect(path: &Path) -> Result<SelectedDirectory, ApiError> {
    if !path.is_absolute()
        || path.parent().is_none()
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err(failure("请选择明确的本机绝对路径。"));
    }
    let root = Anchor::open(path)?;
    empty(&root)?;
    Ok(SelectedDirectory {
        path: path.into(),
        revision: revision(path, &root.identity)?,
    })
}
pub(super) fn validate(selection: &SelectedDirectory) -> Result<PersistedIdentity, ApiError> {
    let root = selected_root(selection)?;
    empty(&root)?;
    Ok(root.identity)
}

fn read_bytes(root: &Anchor, value: &str) -> Result<Option<Vec<u8>>, ApiError> {
    root.verify()?;
    if stat(&root.file, value)?.is_none() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    files::read(&root.file, value)?
        .take(65_537)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 65_536 {
        return Err(failure("发布记录无效，请保留现场。"));
    }
    Ok(Some(bytes))
}
fn read<T: DeserializeOwned>(root: &Anchor, value: &str) -> Result<Option<T>, ApiError> {
    read_bytes(root, value)?
        .map(|bytes| {
            serde_json::from_slice(&bytes).map_err(|_| failure("发布记录无效，请保留现场。"))
        })
        .transpose()
}
fn save<T: Serialize>(root: &Anchor, value: &str, record: &T) -> Result<(), ApiError> {
    root.verify()?;
    let previous = read_bytes(root, value)?;
    let temporary = format!(".selected-{}", Uuid::new_v4().simple());
    let mut file = files::create(&root.file, &temporary)?;
    file.write_all(&serde_json::to_vec(record)?)?;
    file.sync_all()?;
    root.verify()?;
    if read_bytes(root, value)? != previous {
        return Err(failure("发布记录已被其他操作修改，请保留现场。"));
    }
    if unsafe {
        libc::renameat(
            root.file.as_raw_fd(),
            name(&temporary)?.as_ptr(),
            root.file.as_raw_fd(),
            name(value)?.as_ptr(),
        )
    } != 0
    {
        return Err(io(std::io::Error::last_os_error()));
    }
    root.file.sync_all()?;
    root.verify()
}
fn mkdir(root: &Anchor, value: &str) -> Result<Anchor, ApiError> {
    root.verify()?;
    if unsafe { libc::mkdirat(root.file.as_raw_fd(), name(value)?.as_ptr(), 0o700) } != 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    root.file.sync_all()?;
    root.child(value)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Working {
    version: u32,
    binding: Binding,
    selection: SelectedDirectory,
    identity: PersistedIdentity,
}
pub(crate) fn prepare(
    receipt: &Path,
    binding: &Binding,
    selection: &SelectedDirectory,
    copy: impl FnOnce() -> Result<PathBuf, ApiError>,
) -> Result<PathBuf, ApiError> {
    let receipt = Anchor::open(receipt)?;
    if let Some(saved) = read::<Working>(&receipt, WORKING)? {
        if saved.version != 1 || saved.binding != *binding || saved.selection != *selection {
            return Err(failure("源码准备记录与本次添加不符，请保留现场。"));
        }
        let working = receipt.child("selected-working")?;
        if !saved.identity.matches_captured(&working.identity) {
            return Err(failure("源码准备目录已被替换，请保留现场。"));
        }
        return Ok(working.path);
    }
    let working_path = copy()?;
    let working = receipt.child("selected-working")?;
    if working.path != working_path {
        return Err(failure("源码准备目录与本次添加不符。"));
    }
    save(
        &receipt,
        WORKING,
        &Working {
            version: 1,
            binding: binding.clone(),
            selection: selection.clone(),
            identity: working.identity,
        },
    )?;
    Ok(working_path)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    File,
    Directory,
    Link,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    path: String,
    kind: Kind,
    identity: Option<PersistedIdentity>,
    digest: Option<String>,
    ready: bool,
    #[serde(default)]
    removal: Option<Removal>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Removal {
    location: String,
    name: String,
    parent_identity: PersistedIdentity,
    identity: Option<PersistedIdentity>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Publication {
    version: u32,
    binding: Binding,
    selection: SelectedDirectory,
    root_identity: PersistedIdentity,
    original_ds_store: Option<PersistedIdentity>,
    stage_name: String,
    stage_identity: Option<PersistedIdentity>,
    phase: String,
}
fn require_binding(
    record: &Publication,
    source_id: &str,
    app_id: &str,
    lease_id: &str,
    selection: &SelectedDirectory,
) -> Result<(), ApiError> {
    if record.version != 1
        || record.binding.source_id != source_id
        || record.binding.app_id != app_id
        || record.binding.lease_id != lease_id
        || record.selection != *selection
        || !record.stage_name.starts_with(".panda-install-")
        || name(&record.stage_name).is_err()
        || !matches!(
            record.phase.as_str(),
            "copying" | "ready" | "publishing" | "published" | "cleaned"
        )
    {
        return Err(failure("发布记录与本次添加不符，请保留现场。"));
    }
    Ok(())
}
fn records(receipt: &Anchor) -> Result<(Anchor, Vec<(String, Entry)>), ApiError> {
    let directory = receipt.child(ENTRIES)?;
    let mut entries = Vec::new();
    let mut paths = BTreeSet::new();
    for value in names(&directory.file)? {
        if !value.ends_with(".json")
            || value.len() != 13
            || !value[..8].bytes().all(|b| b.is_ascii_digit())
        {
            return Err(failure("发布条目记录无效，请保留现场。"));
        }
        let entry: Entry =
            read(&directory, &value)?.ok_or_else(|| failure("发布条目记录缺失，请保留现场。"))?;
        files::parts(&entry.path)?;
        if !paths.insert(entry.path.clone()) {
            return Err(failure("发布条目记录无效，请保留现场。"));
        }
        entries.push((value, entry));
    }
    Ok((directory, entries))
}
fn kind(metadata: &libc::stat) -> Result<Kind, ApiError> {
    match metadata.st_mode & libc::S_IFMT {
        libc::S_IFREG if metadata.st_nlink == 1 => Ok(Kind::File),
        libc::S_IFDIR => Ok(Kind::Directory),
        libc::S_IFLNK => Ok(Kind::Link),
        _ => Err(failure("应用包含特殊文件或共享硬链接，未发布。")),
    }
}
fn read_link(parent: &File, value: &str) -> Result<Vec<u8>, ApiError> {
    let mut bytes = vec![0u8; 4096];
    let count = unsafe {
        libc::readlinkat(
            parent.as_raw_fd(),
            name(value)?.as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
        )
    };
    if count < 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    if count as usize == bytes.len() {
        return Err(failure("依赖链接目标过长，未发布。"));
    }
    bytes.truncate(count as usize);
    Ok(bytes)
}
fn hash_file(mut file: File) -> Result<String, ApiError> {
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut count = 0u64;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > MAX_BYTES {
            return Err(failure("应用文件过大，未继续发布。"));
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn matches(root: &Anchor, entry: &Entry, content: bool) -> Result<bool, ApiError> {
    let (parent, leaf) = match files::parent(&root.file, &entry.path, false) {
        Ok(value) => value,
        Err(e) if e.code == "FILE_NOT_FOUND" => return Ok(false),
        Err(e) => return Err(e),
    };
    let Some(metadata) = stat(&parent, &leaf)? else {
        return Ok(false);
    };
    if kind(&metadata).ok() != Some(entry.kind)
        || !entry
            .identity
            .as_ref()
            .is_some_and(|expected| expected.matches_captured(&identity(root, &metadata)))
    {
        return Ok(false);
    }
    if content {
        let digest = match entry.kind {
            Kind::File => Some(hash_file(files::read(&root.file, &entry.path)?)?),
            Kind::Link => Some(format!("{:x}", Sha256::digest(read_link(&parent, &leaf)?))),
            Kind::Directory => None,
        };
        if entry.digest != digest {
            return Ok(false);
        }
    }
    Ok(true)
}
fn require_root(
    root: &Anchor,
    publication: &Publication,
    entries: &[(String, Entry)],
) -> Result<(), ApiError> {
    root.verify()?;
    for value in names(&root.file)? {
        if value == publication.stage_name {
            if !publication.stage_identity.as_ref().is_some_and(|expected| {
                stat(&root.file, &value)
                    .ok()
                    .flatten()
                    .is_some_and(|s| expected.matches_captured(&identity(root, &s)))
            }) {
                return Err(failure("发布暂存目录已变化，请保留现场。"));
            }
        } else if value == ".DS_Store" {
            // Finder may create or replace its metadata during a download.
            // It never grants ownership and is never overwritten by a rename.
            let _ = files::read(&root.file, &value)?;
        } else if !entries
            .iter()
            .any(|(_, e)| e.path == value && matches(root, e, false).unwrap_or(false))
        {
            return Err(nonempty());
        }
    }
    Ok(())
}
fn verify_tree(root: &Anchor, entries: &[(String, Entry)]) -> Result<(), ApiError> {
    let mut children: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (_, entry) in entries {
        if !entry.ready || !matches(root, entry, true)? {
            return Err(failure("发布文件已变化，请保留现场。"));
        }
        let (parent, child) = entry
            .path
            .rsplit_once('/')
            .unwrap_or(("", entry.path.as_str()));
        children.entry(parent).or_default().insert(child);
        if entry.kind == Kind::Directory {
            children.entry(&entry.path).or_default();
        }
    }
    for (path, expected) in children {
        if path.is_empty() {
            continue;
        }
        let directory = files::root(&root.path.join(path))?;
        let actual: BTreeSet<String> = names(&directory)?.into_iter().collect();
        if actual.iter().map(String::as_str).collect::<BTreeSet<_>>() != expected {
            return Err(failure("发布目录出现了新的内容，请保留现场。"));
        }
    }
    root.verify()
}

fn copy_tree(
    working: &Anchor,
    input: &Anchor,
    stage: &Anchor,
    target: &Anchor,
    relative: &str,
    receipt: &Anchor,
    journal: &Anchor,
    publication: &Publication,
    entries: &mut Vec<(String, Entry)>,
    bytes: &mut u64,
    depth: usize,
) -> Result<(), ApiError> {
    if depth > 40 {
        return Err(failure("应用目录层级过深，未继续发布。"));
    }
    let before = names(&input.file)?;
    for value in &before {
        if entries.len() >= MAX_ENTRIES {
            return Err(failure("应用文件过多，未继续发布。"));
        }
        let path = if relative.is_empty() {
            value.clone()
        } else {
            format!("{relative}/{value}")
        };
        let metadata =
            stat(&input.file, value)?.ok_or_else(|| failure("准备好的应用文件已变化，未发布。"))?;
        let item_kind = kind(&metadata)?;
        let link = if item_kind == Kind::Link {
            let target = read_link(&input.file, value)?;
            let link_path = Path::new(std::ffi::OsStr::from_bytes(&target));
            if !path.split('/').any(|part| part == "node_modules")
                || link_path.is_absolute()
                || !fs::canonicalize(input.path.join(value))
                    .is_ok_and(|resolved| resolved.starts_with(&working.path))
            {
                return Err(failure("依赖链接必须是指向项目文件夹内部的相对链接。"));
            }
            Some(target)
        } else {
            None
        };
        let mut entry = Entry {
            path: path.clone(),
            kind: item_kind,
            identity: None,
            digest: None,
            ready: false,
            removal: None,
        };
        let record_name = format!("{:08}.json", entries.len());
        save(journal, &record_name, &entry)?;
        target.verify()?;
        let mut output = None;
        let child = if item_kind == Kind::Directory {
            Some(mkdir(target, value)?)
        } else if let Some(link) = &link {
            let link = CString::new(link.as_slice()).map_err(|_| failure("依赖链接目标无效。"))?;
            if unsafe {
                libc::symlinkat(
                    link.as_ptr(),
                    target.file.as_raw_fd(),
                    name(value)?.as_ptr(),
                )
            } != 0
            {
                return Err(io(std::io::Error::last_os_error()));
            }
            None
        } else {
            output = Some(files::create(&target.file, value)?);
            None
        };
        entry.identity = Some(if let Some(output) = &output {
            PersistedIdentity::from_file(output).map_err(|e| failure(&e.to_string()))?
        } else if let Some(child) = &child {
            child.identity.clone()
        } else {
            let written = stat(&target.file, value)?
                .ok_or_else(|| failure("刚创建的发布条目已变化，请保留现场。"))?;
            if kind(&written)? != Kind::Link
                || read_link(&target.file, value)?.as_slice() != link.as_ref().unwrap().as_slice()
            {
                return Err(failure("刚创建的发布条目已变化，请保留现场。"));
            }
            identity(stage, &written)
        });
        save(journal, &record_name, &entry)?;
        let entry_index = entries.len();
        entries.push((record_name.clone(), entry.clone()));
        if item_kind == Kind::Directory {
            let source_child = input.child(value)?;
            copy_tree(
                working,
                &source_child,
                stage,
                child.as_ref().unwrap(),
                &path,
                receipt,
                journal,
                publication,
                entries,
                bytes,
                depth + 1,
            )?;
        } else if let Some(link) = link {
            entry.digest = Some(format!("{:x}", Sha256::digest(link)));
        } else {
            let mut input_file = files::read(&input.file, value)?;
            let before_file = input_file.metadata()?;
            require_space(&target.file, before_file.len())?;
            *bytes = bytes
                .checked_add(before_file.len())
                .ok_or_else(|| failure("应用文件过大，未继续发布。"))?;
            if *bytes > MAX_BYTES {
                return Err(failure("应用文件过大，未继续发布。"));
            }
            let output = output.as_mut().unwrap();
            let mut copied = 0u64;
            let mut digest = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let count = input_file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                copied += count as u64;
                if copied > before_file.len() {
                    return Err(failure("复制时应用文件变化，未发布。"));
                }
                output.write_all(&buffer[..count])?;
                digest.update(&buffer[..count]);
            }
            let after = input_file.metadata()?;
            if copied != before_file.len()
                || before_file.dev() != after.dev()
                || before_file.ino() != after.ino()
                || before_file.mtime() != after.mtime()
                || before_file.mtime_nsec() != after.mtime_nsec()
                || before_file.len() != after.len()
            {
                return Err(failure("复制时应用文件变化，未发布。"));
            }
            output.set_permissions(fs::Permissions::from_mode(before_file.mode() & 0o777))?;
            output.sync_all()?;
            entry.digest = Some(format!("{:x}", digest.finalize()));
        }
        target.file.sync_all()?;
        let current =
            stat(&input.file, value)?.ok_or_else(|| failure("复制时应用文件变化，未发布。"))?;
        if current.st_dev != metadata.st_dev
            || current.st_ino != metadata.st_ino
            || current.st_mode != metadata.st_mode
            || current.st_mtime != metadata.st_mtime
            || current.st_mtime_nsec != metadata.st_mtime_nsec
        {
            return Err(failure("复制时应用文件变化，未发布。"));
        }
        entry.ready = true;
        save(journal, &record_name, &entry)?;
        entries[entry_index].1 = entry;
        receipt.verify()?;
        // Root additions are checked again before the first public rename.
        let _ = publication;
    }
    if before != names(&input.file)? {
        return Err(failure("复制时应用目录变化，未发布。"));
    }
    Ok(())
}

fn require_space(directory: &File, bytes: u64) -> Result<(), ApiError> {
    let mut space = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::fstatvfs(directory.as_raw_fd(), space.as_mut_ptr()) } != 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    let space = unsafe { space.assume_init() };
    let block = if space.f_frsize == 0 {
        space.f_bsize
    } else {
        space.f_frsize
    };
    if u128::from(space.f_bavail) * u128::from(block) < u128::from(bytes) + u128::from(MIN_FREE) {
        return Err(failure("所选磁盘可用空间不足，未继续发布。"));
    }
    Ok(())
}

fn move_exclusive(from: &File, from_name: &str, to: &File, to_name: &str) -> Result<(), ApiError> {
    if unsafe {
        libc::renameatx_np(
            from.as_raw_fd(),
            name(from_name)?.as_ptr(),
            to.as_raw_fd(),
            name(to_name)?.as_ptr(),
            libc::RENAME_EXCL,
        )
    } != 0
    {
        return Err(failure(
            "所选文件夹已有内容或发布失败，原内容与本次现场均保留。",
        ));
    }
    to.sync_all()?;
    from.sync_all()?;
    Ok(())
}
fn open_stage(root: &Anchor, publication: &Publication) -> Result<Anchor, ApiError> {
    let stage = root.child(&publication.stage_name)?;
    if !publication
        .stage_identity
        .as_ref()
        .is_some_and(|expected| expected.matches_captured(&stage.identity))
    {
        return Err(failure("发布暂存目录身份不符，未清理任何内容。"));
    }
    Ok(stage)
}
pub(crate) fn publish(
    receipt_path: &Path,
    binding: &Binding,
    selection: &SelectedDirectory,
) -> Result<PathBuf, ApiError> {
    let receipt = Anchor::open(receipt_path)?;
    let saved: Working = read(&receipt, WORKING)?.ok_or_else(|| failure("源码准备记录缺失。"))?;
    if saved.version != 1 || saved.binding != *binding || saved.selection != *selection {
        return Err(failure("源码准备记录与本次添加不符，请保留现场。"));
    }
    let working = receipt.child("selected-working")?;
    if !saved.identity.matches_captured(&working.identity) {
        return Err(failure("源码准备目录已被替换，请保留现场。"));
    }
    let manifest = crate::panda_manifest::PandaManifest::load(&working.path)
        .map_err(|e| failure(&e.to_string()))?;
    if manifest.agent.is_none() {
        return Err(failure("只有声明 agent 规则的应用才能安装到所选文件夹。"));
    }
    manifest
        .inspect(&working.path)
        .map_err(|e| failure(&e.to_string()))?;
    let root = selected_root(selection)?;
    let mut publication = if let Some(publication) = read::<Publication>(&receipt, HEADER)? {
        require_binding(
            &publication,
            &binding.source_id,
            &binding.app_id,
            &binding.lease_id,
            selection,
        )?;
        if publication.binding != *binding
            || !publication.root_identity.matches_captured(&root.identity)
        {
            return Err(failure("发布记录与所选文件夹不符，请保留现场。"));
        }
        publication
    } else {
        empty(&root)?;
        let original_ds_store = stat(&root.file, ".DS_Store")?.map(|m| identity(&root, &m));
        let mut publication = Publication {
            version: 1,
            binding: binding.clone(),
            selection: selection.clone(),
            root_identity: root.identity.clone(),
            original_ds_store,
            stage_name: format!(".panda-install-{}", Uuid::new_v4().simple()),
            stage_identity: None,
            phase: "copying".into(),
        };
        save(&receipt, HEADER, &publication)?;
        let stage = mkdir(&root, &publication.stage_name)?;
        publication.stage_identity = Some(stage.identity);
        save(&receipt, HEADER, &publication)?;
        mkdir(&receipt, ENTRIES)?;
        publication
    };
    let (journal, mut entries) = records(&receipt)?;
    if publication.phase == "published" {
        verify_tree(&root, &entries)?;
        require_root(&root, &publication, &entries)?;
        return Ok(root.path);
    }
    let stage = open_stage(&root, &publication)?;
    if publication.phase == "copying" {
        if !entries.is_empty() || !names(&stage.file)?.is_empty() {
            return Err(failure("源码复制尚未完成，请清理本次临时材料后重新添加。"));
        }
        require_space(&stage.file, 0)?;
        let mut copied_bytes = 0;
        copy_tree(
            &working,
            &working,
            &stage,
            &stage,
            "",
            &receipt,
            &journal,
            &publication,
            &mut entries,
            &mut copied_bytes,
            0,
        )?;
        verify_tree(&stage, &entries)?;
        publication.phase = "ready".into();
        save(&receipt, HEADER, &publication)?;
    }
    if !matches!(
        publication.phase.as_str(),
        "ready" | "publishing" | "published"
    ) {
        return Err(failure("发布状态不能继续，请保留现场。"));
    }
    publication.phase = "publishing".into();
    save(&receipt, HEADER, &publication)?;
    require_root(&root, &publication, &entries)?;
    for (_, entry) in entries.iter().filter(|(_, e)| !e.path.contains('/')) {
        root.verify()?;
        if matches(&root, entry, true)? {
            continue;
        }
        if !matches(&stage, entry, true)? {
            return Err(failure("发布条目身份不符，原内容已保留。"));
        }
        move_exclusive(&stage.file, &entry.path, &root.file, &entry.path)?;
    }
    verify_tree(&root, &entries)?;
    require_root(&root, &publication, &entries)?;
    if !names(&stage.file)?.is_empty() {
        return Err(failure("发布暂存目录出现了新的内容，请保留现场。"));
    }
    remove_stage(&root, &receipt, &publication)?;
    publication.phase = "published".into();
    save(&receipt, HEADER, &publication)?;
    Ok(root.path)
}

pub(crate) fn reconcile(
    receipt_path: &Path,
    source_id: &str,
    app_id: &str,
    lease_id: &str,
    selection: &SelectedDirectory,
) -> Result<Option<PathBuf>, ApiError> {
    let receipt = Anchor::open(receipt_path)?;
    let Some(mut publication) = read::<Publication>(&receipt, HEADER)? else {
        return Ok(None);
    };
    require_binding(&publication, source_id, app_id, lease_id, selection)?;
    if !matches!(publication.phase.as_str(), "publishing" | "published") {
        return Ok(None);
    }
    let root = selected_root(selection)?;
    let (_, entries) = records(&receipt)?;
    if entries.is_empty() {
        return Ok(None);
    }
    if entries
        .iter()
        .any(|(_, entry)| !matches(&root, entry, true).unwrap_or(false))
    {
        return Ok(None);
    }
    verify_tree(&root, &entries)?;
    if stat(&root.file, &publication.stage_name)?.is_some() {
        let stage = open_stage(&root, &publication)?;
        if !names(&stage.file)?.is_empty() {
            return Err(failure("发布暂存目录出现了新的内容，请保留现场。"));
        }
    }
    if stat(&root.file, &publication.stage_name)?.is_some()
        || stat(&receipt.file, STAGE_REMOVAL)?.is_some()
    {
        remove_stage(&root, &receipt, &publication)?;
    }
    require_root(&root, &publication, &entries)?;
    publication.phase = "published".into();
    save(&receipt, HEADER, &publication)?;
    Ok(Some(root.path))
}

fn parent_anchor(root: &Anchor, path: &str) -> Result<(Anchor, String), ApiError> {
    let (file, leaf) = files::parent(&root.file, path, false)?;
    let identity = PersistedIdentity::from_file(&file).map_err(|e| failure(&e.to_string()))?;
    let parent = Path::new(path).parent().unwrap_or(Path::new(""));
    let anchor = Anchor {
        path: root.path.join(parent),
        file,
        identity,
    };
    anchor.verify()?;
    Ok((anchor, leaf))
}

fn remove_entry(
    root: &Anchor,
    location: &str,
    journal: &Anchor,
    record_name: &str,
    entry: &mut Entry,
    before_rename: &mut impl FnMut(&Path) -> Result<(), ApiError>,
) -> Result<bool, ApiError> {
    root.verify()?;
    if entry
        .removal
        .as_ref()
        .is_some_and(|removal| removal.location != location)
    {
        return Ok(false);
    }
    if entry.removal.is_none() && !matches(root, entry, entry.ready)? {
        return Ok(false);
    }
    let (parent, leaf) = parent_anchor(root, &entry.path)?;
    if entry.removal.is_none() {
        entry.removal = Some(Removal {
            location: location.into(),
            name: format!(".panda-delete-entry-{}", Uuid::new_v4()),
            parent_identity: parent.identity.clone(),
            identity: None,
        });
        save(journal, record_name, entry)?;
        let quarantine = mkdir(&parent, &entry.removal.as_ref().unwrap().name)?;
        entry.removal.as_mut().unwrap().identity = Some(quarantine.identity);
        save(journal, record_name, entry)?;
    }
    let removal = entry.removal.as_ref().unwrap().clone();
    if !removal.name.starts_with(".panda-delete-entry-")
        || name(&removal.name).is_err()
        || !removal.parent_identity.matches_captured(&parent.identity)
        || removal.identity.is_none()
    {
        return Err(failure("清理暂存身份无法核对，已保留相关内容。"));
    }
    if stat(&parent.file, &removal.name)?.is_none() {
        if stat(&parent.file, &leaf)?.is_none() {
            entry.removal = None;
            save(journal, record_name, entry)?;
            return Ok(true);
        }
        return Err(failure("清理暂存目录已变化，已保留相关内容。"));
    }
    let quarantine = parent.child(&removal.name)?;
    if !removal
        .identity
        .as_ref()
        .unwrap()
        .matches_captured(&quarantine.identity)
    {
        return Err(failure("清理暂存目录已变化，已保留相关内容。"));
    }
    if stat(&quarantine.file, "payload")?.is_none() {
        let Some(current) = stat(&parent.file, &leaf)? else {
            if !names(&quarantine.file)?.is_empty() {
                return Err(failure("清理暂存目录出现了新的内容，已保留现场。"));
            }
            remove_empty_quarantine(&parent, &quarantine, &removal.name)?;
            entry.removal = None;
            save(journal, record_name, entry)?;
            return Ok(true);
        };
        if !entry
            .identity
            .as_ref()
            .is_some_and(|expected| expected.matches_captured(&identity(root, &current)))
        {
            return Ok(false);
        }
        before_rename(&parent.path.join(&leaf))?;
        parent.verify()?;
        quarantine.verify()?;
        move_exclusive(&parent.file, &leaf, &quarantine.file, "payload")?;
    }
    let staged = Entry {
        path: "payload".into(),
        removal: None,
        ..entry.clone()
    };
    let owns_payload = matches(&quarantine, &staged, entry.ready).unwrap_or(false);
    if !owns_payload || stat(&parent.file, &leaf)?.is_some() {
        // A substituted entry can have crossed the rename boundary. Restore it
        // exclusively; never unlink it or overwrite a newly occupied name.
        if stat(&parent.file, &leaf)?.is_none() {
            let _ = move_exclusive(&quarantine.file, "payload", &parent.file, &leaf);
        }
        if names(&quarantine.file)?.is_empty() {
            remove_empty_quarantine(&parent, &quarantine, &removal.name)?;
            entry.removal = None;
            save(journal, record_name, entry)?;
        }
        return Ok(false);
    }
    quarantine.verify()?;
    if !matches(&quarantine, &staged, entry.ready)? {
        return Err(failure("清理暂存中的条目已被替换，已保留现场。"));
    }
    let flag = if entry.kind == Kind::Directory {
        libc::AT_REMOVEDIR
    } else {
        0
    };
    if unsafe { libc::unlinkat(quarantine.file.as_raw_fd(), c"payload".as_ptr(), flag) } != 0 {
        let issue = std::io::Error::last_os_error();
        if issue.raw_os_error() == Some(libc::ENOTEMPTY) {
            let _ = move_exclusive(&quarantine.file, "payload", &parent.file, &leaf);
            if names(&quarantine.file)?.is_empty() {
                remove_empty_quarantine(&parent, &quarantine, &removal.name)?;
                entry.removal = None;
                save(journal, record_name, entry)?;
            }
            return Ok(false);
        }
        return Err(io(issue));
    }
    quarantine.file.sync_all()?;
    remove_empty_quarantine(&parent, &quarantine, &removal.name)?;
    entry.removal = None;
    save(journal, record_name, entry)?;
    Ok(true)
}
fn remove_empty_quarantine(
    parent: &Anchor,
    quarantine: &Anchor,
    value: &str,
) -> Result<(), ApiError> {
    parent.verify()?;
    quarantine.verify()?;
    if !names(&quarantine.file)?.is_empty() {
        return Err(failure("清理暂存目录出现了新的内容，已保留现场。"));
    }
    if unsafe {
        libc::unlinkat(
            parent.file.as_raw_fd(),
            name(value)?.as_ptr(),
            libc::AT_REMOVEDIR,
        )
    } != 0
    {
        return Err(io(std::io::Error::last_os_error()));
    }
    parent.file.sync_all()?;
    Ok(())
}
fn remove_stage(
    root: &Anchor,
    receipt: &Anchor,
    publication: &Publication,
) -> Result<(), ApiError> {
    let mut entry = match read::<Entry>(receipt, STAGE_REMOVAL)? {
        Some(entry) => entry,
        None => {
            let entry = Entry {
                path: publication.stage_name.clone(),
                kind: Kind::Directory,
                identity: publication.stage_identity.clone(),
                digest: None,
                ready: true,
                removal: None,
            };
            save(receipt, STAGE_REMOVAL, &entry)?;
            entry
        }
    };
    if entry.path != publication.stage_name
        || entry.kind != Kind::Directory
        || !entry
            .identity
            .as_ref()
            .zip(publication.stage_identity.as_ref())
            .is_some_and(|(a, b)| a.matches_captured(b))
    {
        return Err(failure("发布暂存目录身份不符，未清理任何内容。"));
    }
    if entry.removal.is_none() && stat(&root.file, &entry.path)?.is_none() {
        return Ok(());
    }
    if !remove_entry(
        root,
        "root",
        receipt,
        STAGE_REMOVAL,
        &mut entry,
        &mut |_| Ok(()),
    )? {
        return Err(
            failure("发布暂存目录出现了新增或替换内容，已保留现场。").detail(
                "remainingPaths",
                serde_json::json!([root.path.join(&entry.path)]),
            ),
        );
    }
    Ok(())
}
fn exists(root: &Anchor, path: &str) -> Result<bool, ApiError> {
    match files::parent(&root.file, path, false) {
        Ok((parent, leaf)) => Ok(stat(&parent, &leaf)?.is_some()),
        Err(e) if e.code == "FILE_NOT_FOUND" => Ok(false),
        Err(e) => Err(e),
    }
}
pub(crate) fn cleanup(
    receipt_path: &Path,
    source_id: &str,
    app_id: &str,
    lease_id: &str,
    selection: &SelectedDirectory,
) -> Result<(), ApiError> {
    let receipt = Anchor::open(receipt_path)?;
    let Some(mut publication) = read::<Publication>(&receipt, HEADER)? else {
        return Ok(());
    };
    require_binding(&publication, source_id, app_id, lease_id, selection)?;
    if publication.phase == "cleaned" {
        return Ok(());
    }
    let root = selected_root(selection)?;
    let stage = if stat(&root.file, &publication.stage_name)?.is_some() {
        Some(open_stage(&root, &publication)?)
    } else {
        None
    };
    let (journal, mut entries) = if stat(&receipt.file, ENTRIES)?.is_some() {
        records(&receipt)?
    } else {
        if stage.as_ref().is_some_and(|stage| {
            !names(&stage.file)
                .map(|names| names.is_empty())
                .unwrap_or(false)
        }) {
            return Err(failure("发布条目记录缺失，已保留所选文件夹中的内容。"));
        }
        (mkdir(&receipt, ENTRIES)?, Vec::new())
    };
    entries.sort_by_key(|(_, entry)| std::cmp::Reverse(entry.path.matches('/').count()));
    let mut remaining = Vec::new();
    for (record_name, entry) in &mut entries {
        let mut removed = false;
        if let Some(stage) = &stage {
            removed = remove_entry(
                stage,
                "stage",
                &journal,
                record_name,
                entry,
                &mut |_| Ok(()),
            )?;
        }
        if !removed && !(entry.path == ".DS_Store" && publication.original_ds_store.is_some()) {
            removed = remove_entry(&root, "root", &journal, record_name, entry, &mut |_| Ok(()))?;
        }
        if !removed
            && (exists(&root, &entry.path)?
                || stage
                    .as_ref()
                    .is_some_and(|s| exists(s, &entry.path).unwrap_or(true)))
        {
            remaining.push(root.path.join(&entry.path));
        }
        if let Some(removal) = &entry.removal {
            let base = if removal.location == "stage" {
                stage
                    .as_ref()
                    .map(|stage| &stage.path)
                    .unwrap_or(&root.path)
            } else {
                &root.path
            };
            remaining.push(
                base.join(Path::new(&entry.path).parent().unwrap_or(Path::new("")))
                    .join(&removal.name),
            );
        }
    }
    if let Some(stage) = stage {
        stage.verify()?;
        if names(&stage.file)?.is_empty() {
            remove_stage(&root, &receipt, &publication)?;
        } else {
            remaining.push(stage.path);
        }
    } else if stat(&receipt.file, STAGE_REMOVAL)?.is_some() {
        remove_stage(&root, &receipt, &publication)?;
    }
    if !remaining.is_empty() {
        return Err(failure(
            "部分文件已新增或替换，已保留这些内容和发布记录；所选文件夹不会删除。",
        )
        .detail("remainingPaths", serde_json::json!(remaining)));
    }
    root.verify()?;
    publication.phase = "cleaned".into();
    save(&receipt, HEADER, &publication)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Fixture {
        _temporary: tempfile::TempDir,
        base: PathBuf,
        receipt: PathBuf,
        selection: SelectedDirectory,
        binding: Binding,
    }
    impl Fixture {
        fn new() -> Self {
            let temporary = tempfile::Builder::new()
                .prefix("panda-disposable-selected-publication-")
                .tempdir()
                .unwrap();
            let base = temporary.path().canonicalize().unwrap();
            let root = base.join("user-folder");
            let receipt = base.join("private-receipt");
            fs::create_dir(&root).unwrap();
            fs::create_dir(&receipt).unwrap();
            fs::write(root.join(".DS_Store"), b"original Finder metadata").unwrap();
            let selection = inspect(&root).unwrap();
            let binding = Binding {
                source_id: format!("src_{}", Uuid::new_v4().simple()),
                app_id: Uuid::new_v4().to_string(),
                lease_id: format!("task_{}", Uuid::new_v4().simple()),
                digest: "a".repeat(64),
            };
            let working = receipt.join("selected-working");
            fs::create_dir(&working).unwrap();
            fs::create_dir(working.join(".git")).unwrap();
            fs::write(working.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
            fs::write(working.join("AGENTS.md"), "Disposable agent rules").unwrap();
            fs::write(working.join("panda.json"), r#"{"manifestVersion":2,"app":{"name":"Disposable selected app"},"web":{"root":".","entry":"index.html"},"agent":{"instructions":"AGENTS.md"}}"#).unwrap();
            fs::write(
                working.join("index.html"),
                "<!doctype html><p>Disposable</p>",
            )
            .unwrap();
            prepare(&receipt, &binding, &selection, || Ok(working)).unwrap();
            Self {
                _temporary: temporary,
                base,
                receipt,
                selection,
                binding,
            }
        }
        fn publish(&self) -> Result<PathBuf, ApiError> {
            publish(&self.receipt, &self.binding, &self.selection)
        }
        fn cleanup(&self) -> Result<(), ApiError> {
            cleanup(
                &self.receipt,
                &self.binding.source_id,
                &self.binding.app_id,
                &self.binding.lease_id,
                &self.selection,
            )
        }
    }

    #[test]
    fn selected_directory_revision_uses_stable_volume_identity() {
        let fixture = Fixture::new();
        let root = Anchor::open(&fixture.selection.path).unwrap();
        let mut remounted = root.identity.clone();
        remounted.device = remounted.device.wrapping_add(1);
        assert!(root.identity.volume_uuid.is_some());
        assert_eq!(
            revision(&root.path, &root.identity).unwrap(),
            revision(&root.path, &remounted).unwrap()
        );
        remounted.inode = remounted.inode.wrapping_add(1);
        assert_ne!(
            revision(&root.path, &root.identity).unwrap(),
            revision(&root.path, &remounted).unwrap()
        );
    }

    #[test]
    fn selected_directory_admission_ignores_only_finder_metadata_and_rejects_replacements() {
        let fixture = Fixture::new();
        validate(&fixture.selection).unwrap();
        for leaf in [".git", ".env", "normal-file"] {
            fs::write(fixture.selection.path.join(leaf), b"user material").unwrap();
            assert_eq!(
                inspect(&fixture.selection.path).unwrap_err().code,
                "SOURCE_SELECTED_DIRECTORY_NOT_EMPTY"
            );
            fs::remove_file(fixture.selection.path.join(leaf)).unwrap();
        }
        let old = fixture.base.join("original-user-folder");
        fs::rename(&fixture.selection.path, &old).unwrap();
        fs::create_dir(&fixture.selection.path).unwrap();
        fs::write(
            fixture.selection.path.join("new-user-file"),
            b"preserved replacement",
        )
        .unwrap();
        assert!(validate(&fixture.selection).is_err());
        assert!(fixture.publish().is_err());
        assert_eq!(
            fs::read(fixture.selection.path.join("new-user-file")).unwrap(),
            b"preserved replacement"
        );
        assert_eq!(
            fs::read(old.join(".DS_Store")).unwrap(),
            b"original Finder metadata"
        );
    }

    #[test]
    fn selected_publication_preserves_internal_npm_links_and_cleanup_keeps_user_root() {
        let fixture = Fixture::new();
        let working = fixture.receipt.join("selected-working");
        fs::create_dir_all(working.join("node_modules/.bin")).unwrap();
        fs::create_dir_all(working.join("node_modules/package/bin")).unwrap();
        fs::write(
            working.join("node_modules/package/bin/tool.js"),
            "console.log('disposable')",
        )
        .unwrap();
        symlink(
            "../package/bin/tool.js",
            working.join("node_modules/.bin/tool"),
        )
        .unwrap();
        assert_eq!(fixture.publish().unwrap(), fixture.selection.path);
        assert_eq!(
            fs::read_link(fixture.selection.path.join("node_modules/.bin/tool")).unwrap(),
            Path::new("../package/bin/tool.js")
        );
        // A lost reply reuses the same receipt, never creates another folder.
        assert_eq!(fixture.publish().unwrap(), fixture.selection.path);
        fixture.cleanup().unwrap();
        assert!(fixture.selection.path.is_dir());
        assert_eq!(
            fs::read(fixture.selection.path.join(".DS_Store")).unwrap(),
            b"original Finder metadata"
        );
        assert_eq!(fs::read_dir(&fixture.selection.path).unwrap().count(), 1);
    }

    #[test]
    fn selected_publication_rejects_external_and_non_dependency_links_without_deleting_targets() {
        for link in ["node_modules/.bin/external", "source-link"] {
            let fixture = Fixture::new();
            let working = fixture.receipt.join("selected-working");
            let outside = fixture.base.join("user-original");
            fs::write(&outside, b"external original must survive").unwrap();
            let path = working.join(link);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            symlink(&outside, &path).unwrap();
            assert!(fixture.publish().is_err());
            fixture.cleanup().unwrap();
            assert_eq!(
                fs::read(&outside).unwrap(),
                b"external original must survive"
            );
            assert_eq!(fs::read_dir(&fixture.selection.path).unwrap().count(), 1);
        }
    }

    #[test]
    fn selected_cleanup_preserves_added_children_and_replaced_public_files() {
        let fixture = Fixture::new();
        let working = fixture.receipt.join("selected-working");
        fs::create_dir(working.join("notes")).unwrap();
        fs::write(working.join("notes/ours.txt"), "platform material").unwrap();
        fixture.publish().unwrap();
        fs::write(
            fixture.selection.path.join("notes/user-added.txt"),
            "new user material",
        )
        .unwrap();
        fs::rename(
            fixture.selection.path.join("index.html"),
            fixture.base.join("original-index"),
        )
        .unwrap();
        fs::write(
            fixture.selection.path.join("index.html"),
            "replacement user material",
        )
        .unwrap();
        assert!(fixture.cleanup().is_err());
        assert_eq!(
            fs::read_to_string(fixture.selection.path.join("notes/user-added.txt")).unwrap(),
            "new user material"
        );
        assert_eq!(
            fs::read_to_string(fixture.selection.path.join("index.html")).unwrap(),
            "replacement user material"
        );
        assert!(!fixture.selection.path.join("notes/ours.txt").exists());
        assert!(fixture.selection.path.is_dir());
        assert_eq!(
            fs::read(fixture.selection.path.join(".DS_Store")).unwrap(),
            b"original Finder metadata"
        );
    }

    #[test]
    fn selected_cleanup_restores_a_substitution_between_identity_check_and_public_rename() {
        for replacement_is_link in [false, true] {
            let fixture = Fixture::new();
            fixture.publish().unwrap();
            let receipt = Anchor::open(&fixture.receipt).unwrap();
            let root = Anchor::open(&fixture.selection.path).unwrap();
            let (journal, mut entries) = records(&receipt).unwrap();
            let (record_name, entry) = entries
                .iter_mut()
                .find(|(_, entry)| entry.path == "index.html")
                .unwrap();
            let original = fixture.base.join("held-original");
            let foreign = fixture.base.join("foreign-original");
            fs::write(&foreign, "foreign link target").unwrap();
            let mut hook = |path: &Path| {
                fs::rename(path, &original)?;
                if replacement_is_link {
                    symlink(&foreign, path)?;
                } else {
                    fs::write(path, "replacement raced with cleanup")?;
                }
                Ok(())
            };
            assert!(!remove_entry(&root, "root", &journal, record_name, entry, &mut hook).unwrap());
            assert!(original.is_file());
            if replacement_is_link {
                assert_eq!(
                    fs::read_link(root.path.join("index.html")).unwrap(),
                    foreign
                );
            } else {
                assert_eq!(
                    fs::read_to_string(root.path.join("index.html")).unwrap(),
                    "replacement raced with cleanup"
                );
            }
            assert_eq!(
                fs::read_to_string(fixture.base.join("foreign-original")).unwrap(),
                "foreign link target"
            );
            assert!(!names(&root.file)
                .unwrap()
                .iter()
                .any(|name| name.starts_with(".panda-delete-entry-")));
        }
    }

    #[test]
    fn selected_reconcile_recognizes_a_fully_published_tree_after_a_lost_final_receipt() {
        let fixture = Fixture::new();
        fixture.publish().unwrap();
        let receipt = Anchor::open(&fixture.receipt).unwrap();
        let mut publication: Publication = read(&receipt, HEADER).unwrap().unwrap();
        publication.phase = "publishing".into();
        save(&receipt, HEADER, &publication).unwrap();
        assert_eq!(
            reconcile(
                &fixture.receipt,
                &fixture.binding.source_id,
                &fixture.binding.app_id,
                &fixture.binding.lease_id,
                &fixture.selection
            )
            .unwrap(),
            Some(fixture.selection.path.clone())
        );
        fixture.cleanup().unwrap();
    }
}
