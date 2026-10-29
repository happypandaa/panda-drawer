//! Logical filesystem accounting and narrowly scoped, restart-safe cleanup.
//! Scanning never opens SQLite, executes project code or reads attachment bodies.
use crate::{
    builds::{self, BuildManager},
    file_identity::PersistedIdentity,
    files,
    maintenance::MaintenanceManager,
    projects::{check_no_symlinks, valid_id, validate_manifest},
    ApiError, ProjectManifest, ProjectStore, RuntimePaths, TaskRecord,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering as Cmp,
    collections::{BTreeMap, HashMap, HashSet},
    ffi::{CStr, CString},
    fs::{self, File},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinHandle;
use uuid::Uuid;

const SCHEMA: u32 = 1;
const CATEGORIES: [&str; 7] = [
    "source",
    "output",
    "attachments",
    "database",
    "dependencies",
    "cache",
    "other",
];
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn int(n: u64) -> Value {
    if n > 9_007_199_254_740_991 {
        json!({"$int64":n.to_string()})
    } else {
        json!(n)
    }
}
fn problem(path: impl AsRef<Path>, reason: &str, message: impl std::fmt::Display) -> Value {
    json!({"path":path.as_ref(),"reason":reason,"message":message.to_string()})
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ApiError> {
    check_no_symlinks(path, false)?;
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RootIdentity {
    path: PathBuf,
    device: u64,
    inode: u64,
}
fn root_identity(path: &Path) -> Result<RootIdentity, ApiError> {
    check_no_symlinks(path, false)?;
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() {
        return Err(ApiError::new(
            "UNSAFE_PATH",
            "Expected an ordinary directory",
        ));
    }
    Ok(RootIdentity {
        path: path.canonicalize()?,
        device: m.dev(),
        inode: m.ino(),
    })
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Mapping {
    source: String,
    output: String,
    database: String,
    attachments: String,
    docs: Vec<String>,
}
impl From<&ProjectManifest> for Mapping {
    fn from(m: &ProjectManifest) -> Self {
        Self {
            source: m.source_dir.clone(),
            output: m.output_dir.clone(),
            database: m.database.clone(),
            attachments: m.attachments_dir.clone(),
            docs: m
                .docs
                .as_object()
                .into_iter()
                .flat_map(|v| v.values())
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Binding {
    workspace_root_identity: RootIdentity,
    project_root_identity: RootIdentity,
    root_generation: String,
    mapping: Option<Mapping>,
    invalidation: Value,
    #[serde(default)]
    window_views_signature: Option<String>,
}
#[derive(Default, Clone)]
struct Counter {
    bytes: u64,
    files: u64,
}
impl Counter {
    fn add(&mut self, size: u64) {
        self.bytes = self.bytes.saturating_add(size);
        self.files = self.files.saturating_add(1);
    }
    fn value(&self) -> Value {
        json!({"bytes":int(self.bytes),"fileCount":int(self.files)})
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    path: String,
    bytes: u64,
    modified: u64,
    device: u64,
    inode: u64,
    kind: u32,
}
struct Walk {
    fingerprint: [u8; 32],
    issues: Vec<Value>,
    entries: u64,
}
impl Walk {
    fn new() -> Self {
        Self {
            fingerprint: [0; 32],
            issues: vec![],
            entries: 0,
        }
    }
    fn note(&mut self, e: &Entry) {
        let digest = Sha256::digest(
            format!(
                "{}\0{}\0{}\0{}\0{}\0{}",
                e.path, e.bytes, e.modified, e.device, e.inode, e.kind
            )
            .as_bytes(),
        );
        for (a, b) in self.fingerprint.iter_mut().zip(digest) {
            *a ^= b;
        }
        self.entries += 1;
        if self.entries.is_multiple_of(256) {
            std::thread::yield_now();
        }
    }
    fn signature(&self) -> String {
        format!(
            "{}:{}",
            URL_SAFE_NO_PAD.encode(self.fingerprint),
            self.entries
        )
    }
}
fn name_c(name: &[u8]) -> Result<CString, ApiError> {
    CString::new(name).map_err(|_| ApiError::new("INVALID_PATH", "NUL in filename"))
}
fn child_dir(parent: &File, name: &[u8]) -> Result<File, ApiError> {
    let name = name_c(name)?;
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
fn stat_at(parent: &File, name: &[u8]) -> Result<libc::stat, ApiError> {
    let name = name_c(name)?;
    let mut st = std::mem::MaybeUninit::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            st.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { st.assume_init() })
}
fn directory_names(dir: &File, mut visit: impl FnMut(&[u8])) -> Result<(), ApiError> {
    let fd = unsafe { libc::dup(dir.as_raw_fd()) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        unsafe { libc::close(fd) };
        return Err(std::io::Error::last_os_error().into());
    }
    loop {
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error() = 0;
        }
        #[cfg(not(target_os = "macos"))]
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            let error = std::io::Error::last_os_error();
            unsafe { libc::closedir(stream) };
            return if error.raw_os_error() == Some(0) {
                Ok(())
            } else {
                Err(error.into())
            };
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            visit(name);
        }
    }
}
fn walk(root: &Path, excluded: &[PathBuf], visit: impl FnMut(&Entry)) -> Walk {
    walk_cancellable(root, excluded, &|| false, visit)
}
fn walk_cancellable(
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
        let before = dir.metadata().ok().and_then(|m| m.modified().ok());
        let enumeration = directory_names(dir, |name| {
            if cancelled() {
                return;
            }
            let Ok(name) = std::str::from_utf8(name) else {
                out.issues.push(problem(
                    relative,
                    "invalid_filename",
                    "Filename is not UTF-8",
                ));
                return;
            };
            let path = relative.join(name);
            if excluded.iter().any(|p| path == *p || path.starts_with(p)) {
                return;
            }
            let st = match stat_at(dir, name.as_bytes()) {
                Ok(m) => m,
                Err(e) => {
                    out.issues.push(problem(&path, "unreadable", e));
                    return;
                }
            };
            let kind = st.st_mode as u32 & libc::S_IFMT as u32;
            #[cfg(target_os = "macos")]
            let modified = (st.st_mtime.max(0) as u64)
                .saturating_mul(1000)
                .saturating_add((st.st_mtime_nsec.max(0) as u64) / 1_000_000);
            #[cfg(not(target_os = "macos"))]
            let modified = (st.st_mtime.max(0) as u64)
                .saturating_mul(1000)
                .saturating_add((st.st_mtime_nsec.max(0) as u64) / 1_000_000);
            let entry = Entry {
                path: path.to_string_lossy().into_owned(),
                bytes: st.st_size.max(0) as u64,
                modified,
                device: st.st_dev as u64,
                inode: st.st_ino,
                kind,
            };
            out.note(&entry);
            if kind == libc::S_IFREG as u32 {
                visit(&entry);
            } else if kind == libc::S_IFDIR as u32 {
                match child_dir(dir, name.as_bytes()) {
                    Ok(child) => descend(&child, &path, excluded, out, cancelled, visit),
                    Err(e) => out.issues.push(problem(&path, "unreadable", e)),
                }
            } else {
                out.issues.push(problem(
                    &path,
                    if kind == libc::S_IFLNK as u32 {
                        "symlink_not_counted"
                    } else {
                        "special_file_not_counted"
                    },
                    "Only ordinary files contribute logical bytes",
                ));
            }
        });
        if let Err(e) = enumeration {
            out.issues.push(problem(relative, "unreadable", e));
        }
        if before != dir.metadata().ok().and_then(|m| m.modified().ok()) {
            out.issues.push(problem(
                relative,
                "changed_during_scan",
                "Directory changed while enumerating",
            ));
        }
    }
    let mut out = Walk::new();
    match files::root(root) {
        Ok(fd) => descend(
            &fd,
            Path::new(""),
            excluded,
            &mut out,
            cancelled,
            &mut visit,
        ),
        Err(e) => out.issues.push(problem(root, "unreadable", e)),
    };
    out
}
fn in_root(path: &str, root: &str) -> bool {
    Path::new(path).starts_with(root)
}
fn category(path: &str, m: Option<&Mapping>) -> &'static str {
    let Some(m) = m else {
        return "other";
    };
    if in_root(path, &m.attachments) {
        "attachments"
    } else if path == m.database
        || ["-wal", "-shm", "-journal"]
            .iter()
            .any(|s| path == format!("{}{s}", m.database))
    {
        "database"
    } else if in_root(path, &format!("{}/node_modules", m.source)) {
        "dependencies"
    } else if in_root(path, &m.output) {
        "output"
    } else if in_root(path, &m.source)
        || in_root(path, "meta")
        || in_root(path, "docs")
        || path == "project.json"
        || path == "README.md"
        || m.docs.iter().any(|p| p == path)
    {
        "source"
    } else {
        "other"
    }
}
fn upload_staging(path: &str) -> bool {
    Path::new(path).parent() == Some(Path::new(".platform/uploads"))
        && Path::new(path)
            .file_name()
            .and_then(|v| v.to_str())
            .and_then(|s| s.strip_prefix(".upload-"))
            .is_some_and(|s| s.len() == 32 && s.bytes().all(|c| c.is_ascii_hexdigit()))
}
fn scope_scan(
    root: &Path,
    scope: Value,
    excluded: &[PathBuf],
    classify: impl Fn(&str) -> String,
) -> Value {
    let started = now();
    let identity = root_identity(root).ok();
    let mut counts: BTreeMap<String, Counter> = BTreeMap::new();
    let mut total = Counter::default();
    let mut first = walk(root, excluded, |e| {
        total.add(e.bytes);
        counts.entry(classify(&e.path)).or_default().add(e.bytes);
    });
    let second = walk(root, excluded, |_| {});
    if first.signature() != second.signature() {
        first.issues.push(problem(
            root,
            "changed_during_scan",
            "Filesystem metadata changed between passes",
        ));
    }
    if identity.is_none() || root_identity(root).ok() != identity {
        first.issues.push(problem(
            root,
            "root_changed",
            "Root identity cannot be confirmed",
        ));
    }
    first.issues.extend(second.issues);
    json!({"scope":scope,"root":root,"scanId":Uuid::new_v4().to_string(),"startedAt":started,"completedAt":now(),"complete":first.issues.is_empty(),"stale":false,"totalBytes":if identity.is_some(){int(total.bytes)}else{Value::Null},"fileCount":if identity.is_some(){int(total.files)}else{Value::Null},"categories":counts.into_iter().map(|(k,v)|(k,v.value())).collect::<BTreeMap<_,_>>(),"issues":first.issues,"excludedRoots":excluded,"measurement":"logicalFileBytes","nonAtomic":true})
}

fn window_views_root(paths: &RuntimePaths, id: &str) -> PathBuf {
    paths
        .data_root()
        .join("web-project-ownership")
        .join(id)
        .join("artifact-views")
}
fn optional_child_dir(parent: &File, name: &[u8]) -> Result<Option<File>, ApiError> {
    let name = name_c(name)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(error.into())
        }
    } else {
        Ok(Some(unsafe { File::from_raw_fd(fd) }))
    }
}
fn window_views_directory(paths: &RuntimePaths, id: &str) -> Result<Option<File>, ApiError> {
    let mut directory = files::root(paths.data_root())?;
    for name in [
        b"web-project-ownership".as_slice(),
        id.as_bytes(),
        b"artifact-views",
    ] {
        let Some(child) = optional_child_dir(&directory, name)? else {
            return Ok(None);
        };
        directory = child;
    }
    Ok(Some(directory))
}
fn optional_stat_at(parent: &File, name: &[u8]) -> Result<Option<libc::stat>, ApiError> {
    let name = name_c(name)?;
    let mut stat = std::mem::MaybeUninit::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } == 0
    {
        Ok(Some(unsafe { stat.assume_init() }))
    } else {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(error.into())
        }
    }
}
fn window_metadata(metadata: &fs::Metadata) -> (u64, u64, u32, u64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.mode(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
    )
}
/// Control views are immutable after publication. Only the root, view container
/// and receipt metadata are needed to invalidate a cached payload measurement.
fn window_views_signature(paths: &RuntimePaths, id: &str) -> Result<String, ApiError> {
    let Some(directory) = window_views_directory(paths, id)? else {
        return Ok("absent".into());
    };
    let before = window_metadata(&directory.metadata()?);
    let mut digest = Sha256::new();
    digest.update(format!("{before:?}"));
    let mut names = vec![];
    directory_names(&directory, |name| names.push(name.to_vec()))?;
    names.sort();
    for name in names {
        let stat = stat_at(&directory, &name)?;
        digest.update((name.len() as u64).to_be_bytes());
        digest.update(&name);
        digest.update(format!(
            "{:?}",
            (
                stat.st_dev,
                stat.st_ino,
                stat.st_mode,
                stat.st_size,
                stat.st_mtime,
                stat.st_mtime_nsec
            )
        ));
        if stat.st_mode as u32 & libc::S_IFMT as u32 == libc::S_IFDIR as u32 {
            let container = child_dir(&directory, &name)?;
            let metadata = window_metadata(&container.metadata()?);
            if metadata.0 != stat.st_dev as u64 || metadata.1 != stat.st_ino {
                return Err(ApiError::new(
                    "STORAGE_SCAN_INVALIDATED",
                    "Window artifact container identity changed",
                ));
            }
            if let Some(receipt) = optional_stat_at(&container, b"view.json")? {
                if receipt.st_mode as u32 & libc::S_IFMT as u32 != libc::S_IFREG as u32 {
                    return Err(ApiError::new(
                        "UNSAFE_PATH",
                        "Window artifact receipt is not an ordinary file",
                    ));
                }
                digest.update(format!(
                    "{:?}",
                    (
                        receipt.st_dev,
                        receipt.st_ino,
                        receipt.st_mode,
                        receipt.st_size,
                        receipt.st_mtime,
                        receipt.st_mtime_nsec
                    )
                ));
            } else {
                digest.update(b"receipt-missing");
            }
            if window_metadata(&container.metadata()?) != metadata {
                return Err(ApiError::new(
                    "STORAGE_SCAN_INVALIDATED",
                    "Window artifact container changed while inspecting its receipt",
                ));
            }
        }
    }
    if window_metadata(&directory.metadata()?) != before {
        return Err(ApiError::new(
            "STORAGE_SCAN_INVALIDATED",
            "Window artifact root changed while inspecting containers",
        ));
    }
    Ok(URL_SAFE_NO_PAD.encode(digest.finalize()))
}
fn window_views_scan(
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
    let identity = match directory.metadata() {
        Ok(metadata) => (metadata.dev(), metadata.ino()),
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
        .and_then(|directory| directory.metadata().ok())
        .map(|metadata| (metadata.dev(), metadata.ino()));
    if final_identity != Some(identity) {
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

type RootGenerations = HashMap<String, (RootIdentity, Option<String>, String)>;
struct StorageRunner {
    project_id: String,
    handle: JoinHandle<()>,
}
#[cfg(test)]
#[derive(Default)]
struct ScanPause {
    entered: AtomicBool,
    released: std::sync::Mutex<bool>,
    wake: std::sync::Condvar,
}
#[cfg(test)]
impl ScanPause {
    fn wait(&self) {
        self.entered.store(true, Ordering::Release);
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.wake.wait(released).unwrap();
        }
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}
pub(crate) struct StorageManager {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    maintenance: Arc<MaintenanceManager>,
    instance: String,
    stopping: AtomicBool,
    scans: Mutex<HashMap<String, String>>,
    runners: Mutex<Vec<StorageRunner>>,
    forgotten: Mutex<HashSet<String>>,
    installation: Option<PathBuf>,
    generations: Mutex<RootGenerations>,
    readers: Arc<tokio::sync::RwLock<()>>,
    #[cfg(test)]
    scan_pause: Mutex<Option<Arc<ScanPause>>>,
    #[cfg(test)]
    aggregate_pause: Mutex<Option<Arc<ScanPause>>>,
}
impl StorageManager {
    pub fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        maintenance: Arc<MaintenanceManager>,
    ) -> Result<Arc<Self>, ApiError> {
        let installation = std::env::current_exe().ok().and_then(|p| {
            p.ancestors()
                .find(|a| a.extension().is_some_and(|v| v == "app"))
                .map(Path::to_path_buf)
        });
        let manager = Arc::new(Self {
            paths,
            store,
            tasks,
            maintenance,
            instance: Uuid::new_v4().to_string(),
            stopping: AtomicBool::new(false),
            scans: Mutex::new(HashMap::new()),
            runners: Mutex::new(vec![]),
            forgotten: Mutex::new(HashSet::new()),
            installation,
            generations: Mutex::new(HashMap::new()),
            readers: Arc::new(tokio::sync::RwLock::new(())),
            #[cfg(test)]
            scan_pause: Mutex::new(None),
            #[cfg(test)]
            aggregate_pause: Mutex::new(None),
        });
        Ok(manager)
    }
    fn cache(&self, id: &str) -> PathBuf {
        self.paths
            .workspace_root()
            .join(".platform/cache/storage/projects")
            .join(format!("{id}.json"))
    }
    fn binding(&self, id: &str) -> Result<(Binding, Vec<Value>), ApiError> {
        self.require_project_available(id)?;
        self.store.check_workspace_available()?;
        let (root, manifest) = self.store.metadata_root(id)?;
        let mut issues = vec![];
        let mapping = match manifest {
            Some(m) => match validate_manifest(&m, id, &root) {
                Ok(()) => Some(Mapping::from(&m)),
                Err(e) => {
                    issues.push(problem(root.join("project.json"), "invalid_manifest", e));
                    None
                }
            },
            None => {
                issues.push(problem(
                    root.join("project.json"),
                    "invalid_manifest",
                    "Classification manifest is unavailable",
                ));
                None
            }
        };
        let invalidation = read_json::<Value>(
            &self
                .paths
                .workspace_root()
                .join(".platform/cache/storage/invalidations/projects")
                .join(format!("{id}.json")),
        )
        .unwrap_or(Value::Null);
        let project_identity = root_identity(&root)?;
        let registration_switch = self.store.registered_epoch(id)?;
        let mut generations = self.generations.lock().unwrap();
        self.require_project_available(id)?;
        if root_identity(&root)? != project_identity
            || self.store.registered_epoch(id)? != registration_switch
        {
            return Err(ApiError::new(
                "STORAGE_SCAN_INVALIDATED",
                "项目身份已变化，未保留旧的空间扫描状态。",
            ));
        }
        let generation = generations.entry(id.into()).or_insert_with(|| {
            (
                project_identity.clone(),
                registration_switch.clone(),
                Uuid::new_v4().to_string(),
            )
        });
        if generation.0 != project_identity || generation.1 != registration_switch {
            *generation = (
                project_identity.clone(),
                registration_switch,
                Uuid::new_v4().to_string(),
            );
        }
        let root_generation = generation.2.clone();
        let window_views_signature = match window_views_signature(&self.paths, id) {
            Ok(signature) => Some(signature),
            Err(error) => {
                issues.push(problem(
                    window_views_root(&self.paths, id),
                    "window_views_unavailable",
                    error,
                ));
                None
            }
        };
        Ok((
            Binding {
                workspace_root_identity: root_identity(self.paths.workspace_root())?,
                project_root_identity: project_identity,
                root_generation,
                mapping,
                invalidation,
                window_views_signature,
            },
            issues,
        ))
    }
    fn empty(id: &str, reason: &str) -> Value {
        json!({"scope":{"kind":"project","projectId":id},"status":"not_scanned","scanId":null,"startedAt":null,"completedAt":null,"complete":false,"stale":true,"totalBytes":null,"categories":null,"issues":[],"invalidationReasons":[reason]})
    }
    pub async fn info(self: &Arc<Self>, id: &str) -> Result<Value, ApiError> {
        let (binding, issues) = self.binding(id)?;
        let cached = read_json::<Value>(&self.cache(id))
            .ok()
            .filter(|v| v["schemaVersion"] == SCHEMA && v["scope"]["projectId"] == id);
        let mut response=cached.as_ref().and_then(|v|v.get("result")).cloned().unwrap_or_else(||json!({"currentProject":Self::empty(id,"missing_or_invalid_cache"),"relatedHistory":null}));
        let old = cached
            .as_ref()
            .and_then(|v| v.get("binding"))
            .and_then(|v| serde_json::from_value::<Binding>(v.clone()).ok());
        let stale = old.as_ref() != Some(&binding) || response["currentProject"]["stale"] == true;
        if stale {
            response["currentProject"]["stale"] = true.into();
            response["currentProject"]["invalidationReasons"] =
                json!(["root_generation_mapping_or_known_content_changed"]);
        }
        response["scope"] = json!({"kind":"project","projectId":id});
        response["root"] = json!(binding.project_root_identity.path);
        if !issues.is_empty() {
            response["classificationIssues"] = json!(issues);
        }
        if stale {
            if let Ok(task) = self.submit_scan(id).await {
                response["activeTask"] = json!(task);
            }
        }
        Ok(response)
    }
    pub async fn submit_scan(self: &Arc<Self>, id: &str) -> Result<TaskRecord, ApiError> {
        let mut runners = self.runners.lock().unwrap();
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        let (binding, issues) = self.binding(id)?;
        let mut scans = self.scans.lock().unwrap();
        if let Some(task) = scans.get(id) {
            if let Ok(t) = self.tasks.task(task) {
                if !t.status.is_final() {
                    return Ok(t);
                }
            }
        }
        let record = self.tasks.create_readonly_operation(id, "storageScan")?;
        scans.insert(id.into(), record.task_id.clone());
        let manager = self.clone();
        let task = record.clone();
        let id = id.to_owned();
        let runner_project = id.clone();
        let runner = tokio::spawn(async move {
            let _ = manager.tasks.phase(&task.task_id, "scanning");
            let worker = manager.clone();
            let worker_id = id.clone();
            let b = binding.clone();
            let result =
                tokio::task::spawn_blocking(move || worker.scan_project(&worker_id, b, issues))
                    .await
                    .map_err(|e| ApiError::new("STORAGE_SCAN_FAILED", e.to_string()))
                    .and_then(|v| v);
            let _ = manager.tasks.finish(&task.task_id, result);
            manager.scans.lock().unwrap().remove(&id);
        });
        runners.push(StorageRunner {
            project_id: runner_project,
            handle: runner,
        });
        Ok(record)
    }
    fn scan_project(
        &self,
        id: &str,
        binding: Binding,
        mut issues: Vec<Value>,
    ) -> Result<Value, ApiError> {
        let root = &binding.project_root_identity.path;
        let started = now();
        let mut counts: BTreeMap<&str, Counter> = CATEGORIES
            .into_iter()
            .map(|k| (k, Counter::default()))
            .collect();
        let mut assets = Counter::default();
        let mut main = Counter::default();
        let mut companions = Counter::default();
        let mut uploads = Counter::default();
        let safe_cache = self
            .cleanup_candidates(id, &binding, ["buildCache"].as_slice())
            .unwrap_or_default()
            .0
            .into_iter()
            .filter_map(|(_, p)| p.strip_prefix(root).ok().map(Path::to_path_buf))
            .collect::<Vec<_>>();
        let cancelled =
            || self.stopping.load(Ordering::Acquire) || self.forgotten.lock().unwrap().contains(id);
        let mut first = walk_cancellable(root, &[], &cancelled, |e| {
            let key = if upload_staging(&e.path)
                || safe_cache.iter().any(|p| Path::new(&e.path).starts_with(p))
            {
                "cache"
            } else {
                category(&e.path, binding.mapping.as_ref())
            };
            counts.get_mut(key).unwrap().add(e.bytes);
            if upload_staging(&e.path) {
                uploads.add(e.bytes);
            }
            if let Some(m) = &binding.mapping {
                if key == "source"
                    && [
                        format!("{}/public", m.source),
                        format!("{}/src/assets", m.source),
                    ]
                    .iter()
                    .any(|p| in_root(&e.path, p))
                {
                    assets.add(e.bytes);
                }
                if key == "database" {
                    if e.path == m.database {
                        main.add(e.bytes)
                    } else {
                        companions.add(e.bytes)
                    }
                }
            }
        });
        let second = walk(root, &[], |_| {});
        if first.signature() != second.signature() {
            issues.push(problem(
                root,
                "changed_during_scan",
                "File metadata changed while scanning",
            ));
        }
        issues.append(&mut first.issues);
        issues.extend(second.issues);
        let (window_views_root, window_views, window_view_issues) =
            window_views_scan(&self.paths, id, &cancelled);
        let window_views_complete = window_view_issues.is_empty();
        issues.extend(window_view_issues);
        let output = counts.get_mut("output").unwrap();
        output.bytes = output.bytes.saturating_add(window_views.bytes);
        output.files = output.files.saturating_add(window_views.files);
        let mut categories = counts
            .iter()
            .map(|(k, c)| (k.to_string(), c.value()))
            .collect::<serde_json::Map<_, _>>();
        categories.get_mut("source").unwrap()["fixedAssets"] = assets.value();
        categories.get_mut("output").unwrap()["windowViews"] = json!({"root":window_views_root,"bytes":int(window_views.bytes),"fileCount":int(window_views.files),"complete":window_views_complete});
        categories.get_mut("database").unwrap()["main"] = main.value();
        categories.get_mut("database").unwrap()["companions"] = companions.value();
        categories.get_mut("cache").unwrap()["uploadStaging"] = json!({"bytes":int(uploads.bytes),"fileCount":int(uploads.files),"protected":true,"reason":"Upload staging is not a cleanup category; active or unclassified residues remain protected"});
        let total = counts.values().fold(0u64, |a, c| a.saturating_add(c.bytes));
        let file_count = counts.values().fold(0u64, |a, c| a.saturating_add(c.files));
        let current = json!({"scope":{"kind":"project","projectId":id},"root":root,"scanId":Uuid::new_v4().to_string(),"startedAt":started,"completedAt":now(),"complete":issues.is_empty(),"stale":false,"totalBytes":int(total),"fileCount":int(file_count),"categories":categories,"issues":issues,"nonAtomic":true,"measurement":"logicalFileBytes"});
        let result = json!({"currentProject":current,"relatedHistory":self.history(id)});
        #[cfg(test)]
        if let Some(pause) = self.scan_pause.lock().unwrap().clone() {
            pause.wait();
        }
        if self.stopping.load(Ordering::Acquire)
            || self.binding(id).map(|v| v.0).ok() != Some(binding.clone())
        {
            return Err(ApiError::new(
                "STORAGE_SCAN_INVALIDATED",
                "Project root, mapping or known content changed; scan was not published",
            )
            .detail("partialResult", result));
        }
        let envelope = json!({"schemaVersion":SCHEMA,"scope":{"kind":"project","projectId":id},"workspaceRootIdentity":binding.workspace_root_identity,"projectRootIdentity":binding.project_root_identity,"rootGeneration":binding.root_generation,"binding":binding,"result":result});
        check_no_symlinks(&self.cache(id), true)?;
        builds::atomic_json(&self.cache(id), &envelope)?;
        // Known mutations may invalidate between the previous check and rename.
        if self.stopping.load(Ordering::Acquire)
            || self.binding(id).map(|v| v.0).ok() != Some(binding)
        {
            let _ = fs::remove_file(self.cache(id));
            return Err(ApiError::new(
                "STORAGE_SCAN_INVALIDATED",
                "Root changed during scan publication",
            ));
        }
        Ok(result)
    }
    fn history(&self, id: &str) -> Value {
        let mut entries = vec![];
        let mut issues = vec![];
        let workspace = self.paths.workspace_root();
        let operations = workspace.join(".platform/operations");
        if let Ok(dirs) = fs::read_dir(&operations) {
            for entry in dirs.flatten() {
                let mut metadata = None;
                for name in ["journal.json", "snapshot.json"] {
                    let path = entry.path().join(name);
                    if path.is_file() {
                        match read_json::<Value>(&path) {
                            Ok(v) if v["projectId"] == id || v["sourceProjectId"] == id => {
                                metadata = Some(v);
                                break;
                            }
                            Ok(_) => {}
                            Err(e) => issues.push(problem(path, "unreadable_operation_record", e)),
                        }
                    }
                }
                if let Some(m) = metadata {
                    let mut value = scope_scan(
                        &entry.path(),
                        json!({"kind":"operation","projectId":id}),
                        &[],
                        |_| "operations".into(),
                    );
                    value["taskId"] = m["taskId"].clone();
                    value["protected"] = true.into();
                    value["protectionReason"] = if m["resolved"] == true {
                        "Retained operation evidence; no general cleanup permission".into()
                    } else {
                        "Unresolved or protected operation".into()
                    };
                    entries.push(value);
                }
            }
        }
        let tasks = workspace.join(".platform/tasks");
        if let Ok(dirs) = fs::read_dir(&tasks) {
            for dir in dirs.flatten() {
                let task_id = dir.file_name().to_string_lossy().into_owned();
                if let Ok(task) = self.tasks.task(&task_id) {
                    if task.project_id.as_deref() != Some(id) {
                        continue;
                    }
                    let mut value = scope_scan(
                        &dir.path(),
                        json!({"kind":"task","projectId":id}),
                        &[],
                        |_| "taskRecordsAndLogs".into(),
                    );
                    value["taskId"] = json!(task_id);
                    value["protected"] = true.into();
                    value["protectionReason"] = if task.status.is_final() {
                        "Task record is retained; only recognized log files may be cleaned".into()
                    } else {
                        "Active task records and logs are protected".into()
                    };
                    value["cleanupCategories"] = if task.status.is_final() {
                        json!(["logs"])
                    } else {
                        json!([])
                    };
                    entries.push(value);
                }
            }
        }
        json!({"entries":entries,"issues":issues,"includedInCurrentProject":false,"referenceOnly":true})
    }
    pub async fn files(&self, id: &str, query: Value) -> Result<Value, ApiError> {
        let reader = self.readers.clone().read_owned().await;
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Storage operations are paused",
            ));
        }
        let (binding, _) = self.binding(id)?;
        let expected = binding.clone();
        let request = FileQuery::parse(query)?;
        let cursor = request
            .cursor
            .as_ref()
            .map(|v| decode_cursor(v))
            .transpose()?;
        let filter = request.filter_key();
        let instance = self.instance.clone();
        let result=tokio::task::spawn_blocking(move|| {
            let _reader=reader;
            let started=now();let mapping=binding.mapping.as_ref().ok_or_else(||ApiError::new("INVALID_PROJECT_MANIFEST","Attachment mapping is unavailable"))?;
            let root=binding.project_root_identity.path.join(&mapping.attachments);
            let initial=root_identity(&root)?;
            let key=serde_json::to_string(&binding)?;
            if cursor.as_ref().is_some_and(|c|c.filter!=filter || c.binding!=key || c.instance!=instance){return Err(invalid_cursor());}
            let mut selected:Vec<Entry>=vec![];
            let mut first=walk(&root,&[],|entry| {
                if !entry.path.starts_with(&request.prefix) || !entry.path.to_lowercase().contains(&request.query.to_lowercase()) || request.kind.as_ref().is_some_and(|k|k!=display_type(&entry.path)) {return;}
                if let Some(c)=&cursor {if compare_entry(entry,&c.last,&request)!=Cmp::Greater{return;}}
                let index=selected.binary_search_by(|old|compare_entry(old,entry,&request)).unwrap_or_else(|i|i);
                if index<=request.limit {selected.insert(index,entry.clone());if selected.len()>request.limit+1{selected.pop();}}
            });
            let second=walk(&root,&[],|_|{});
            let signature=first.signature();
            if cursor.as_ref().is_some_and(|c|c.signature!=signature) {return Err(invalid_cursor());}
            if signature!=second.signature() {first.issues.push(problem(&root,"changed_during_scan","Attachments changed during pagination; start again"));}
            first.issues.extend(second.issues);
            if root_identity(&root).ok()!=Some(initial){return Err(invalid_cursor());}
            let more=selected.len()>request.limit;selected.truncate(request.limit);
            let next=if more && first.issues.is_empty() {selected.last().map(|last|URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Cursor{filter,signature,binding:key,instance,last:last.clone()}).unwrap()))} else {None};
            let rows=selected.into_iter().map(|e|json!({"path":e.path,"size":int(e.bytes),"mediaType":files::mime(&e.path),"modifiedAt":e.modified,"displayType":display_type(&e.path)})).collect::<Vec<_>>();
            Ok::<_,ApiError>(json!({"files":rows,"nextCursor":next,"complete":first.issues.is_empty(),"issues":first.issues,"scope":{"kind":"attachments"},"startedAt":started,"completedAt":now(),"nonAtomic":true}))
        }).await.map_err(|e|ApiError::new("STORAGE_FILES_FAILED",e.to_string()))??;
        // A new root or known upload invalidates even an otherwise stable page.
        let current = self.binding(id)?.0;
        if current != expected {
            return Err(invalid_cursor());
        }
        Ok(result)
    }
    fn protection(&self, id: &str, except: Option<&str>) -> Result<(), ApiError> {
        let workspace = self.paths.workspace_root();
        let publication = workspace
            .join(".platform/project-state/builds")
            .join(format!("{id}.publication.json"));
        check_no_symlinks(&publication, true)?;
        if publication.exists() {
            return Err(ApiError::new(
                "RECOVERY_REQUIRED",
                "An unresolved artifact publication protects project caches",
            ));
        }
        let operations = workspace.join(".platform/operations");
        check_no_symlinks(&operations, true)?;
        if operations.exists() {
            for entry in fs::read_dir(operations)? {
                let path = entry?.path();
                if path.file_name().and_then(|n| n.to_str()) == except {
                    continue;
                }
                {
                    let file = path.join("journal.json");
                    if file.exists() {
                        let value: Value = read_json(&file)?;
                        if value["resolved"] != true
                            && (value["projectId"] == id || value["sourceProjectId"] == id)
                        {
                            return Err(ApiError::new(
                                "RECOVERY_REQUIRED",
                                "An unresolved operation protects this project's files",
                            )
                            .detail("taskId", value["taskId"].clone()));
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn cleanup_candidates(
        &self,
        id: &str,
        binding: &Binding,
        categories: &[&str],
    ) -> Result<CleanupCandidates, ApiError> {
        let mapping = binding.mapping.as_ref().ok_or_else(|| {
            ApiError::new(
                "INVALID_PROJECT_MANIFEST",
                "Cleanup requires validated project paths",
            )
        })?;
        let root = &binding.project_root_identity.path;
        let mut paths = vec![];
        let mut skipped = vec![];
        if categories.contains(&"dependencies") {
            let path = root.join(&mapping.source).join("node_modules");
            if fs::symlink_metadata(&path).is_ok() {
                paths.push(("dependencies".into(), path));
            }
        }
        if categories.contains(&"buildCache") {
            let builds = root.join(".platform/builds");
            check_no_symlinks(&builds, true)?;
            if builds.is_dir() {
                for dir in fs::read_dir(&builds)? {
                    let dir = dir?;
                    let task = dir.file_name().to_string_lossy().into_owned();
                    match self.tasks.task(&task) {
                        Ok(t)
                            if t.project_id.as_deref() == Some(id)
                                && t.task_type == "build"
                                && t.status.is_final() =>
                        {
                            for name in ["dist", "previous", "publication", "restore"] {
                                let path = dir.path().join(name);
                                if fs::symlink_metadata(&path).is_ok() {
                                    paths.push(("buildCache".into(), path));
                                }
                            }
                        }
                        _ => skipped.push(problem(
                            dir.path(),
                            "protected_or_unknown_task",
                            "Only known completed build cache objects are eligible",
                        )),
                    }
                }
            }
        }
        if categories.contains(&"logs") {
            let tasks = self.paths.workspace_root().join(".platform/tasks");
            check_no_symlinks(&tasks, true)?;
            if tasks.is_dir() {
                for dir in fs::read_dir(tasks)? {
                    let dir = dir?;
                    let task = dir.file_name().to_string_lossy().into_owned();
                    if let Ok(t) = self.tasks.task(&task) {
                        if t.project_id.as_deref() != Some(id) {
                            continue;
                        }
                        if !t.status.is_final() {
                            skipped.push(problem(
                                dir.path(),
                                "active_task",
                                "Active task logs are protected",
                            ));
                            continue;
                        }
                        for name in ["head.log", "tail.log", "log.json"] {
                            let path = dir.path().join(name);
                            if fs::symlink_metadata(&path).is_ok() {
                                paths.push(("logs".into(), path));
                            }
                        }
                    }
                }
            }
        }
        Ok((paths, skipped))
    }
    pub async fn submit_cleanup(
        self: &Arc<Self>,
        id: &str,
        body: Value,
    ) -> Result<TaskRecord, ApiError> {
        let mut runners = self.runners.lock().unwrap();
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Storage operations are paused",
            ));
        }
        let input: CleanupInput = serde_json::from_value(body)?;
        if input.categories.is_empty()
            || input
                .categories
                .iter()
                .any(|c| !matches!(c.as_str(), "dependencies" | "buildCache" | "logs"))
        {
            return Err(ApiError::new(
                "INVALID_CLEANUP_CATEGORY",
                "Choose dependencies, buildCache or logs; arbitrary paths are not accepted",
            ));
        }
        self.binding(id)?;
        self.store
            .control(id)?
            .check(false, input.maintenance_id.as_deref(), false)?;
        self.protection(id, None)?;
        let (record, lease) = self.tasks.create_operation(id, "storageCleanup")?;
        let manager = self.clone();
        let task = record.clone();
        let id = id.to_owned();
        let runner_project = id.clone();
        let runner = tokio::spawn(async move {
            let _lease = lease;
            let _gate = manager.store.gate(&id).write_owned().await;
            let _ = manager.tasks.phase(&task.task_id, "checking-cleanup");
            let worker = manager.clone();
            let worker_id = id.clone();
            let task_id = task.task_id.clone();
            let outcome =
                tokio::task::spawn_blocking(move || worker.cleanup(&worker_id, &task_id, &input))
                    .await
                    .map_err(|e| ApiError::new("STORAGE_CLEANUP_FAILED", e.to_string()))
                    .and_then(|v| v);
            match outcome {
                Ok(result) => {
                    let _ = manager.tasks.finish(&task.task_id, Ok(result));
                }
                Err(error) => {
                    let result=error.details.get("partialResult").cloned().unwrap_or_else(||json!({"deleted":[],"skipped":[],"failures":[error.clone()],"logicalBytes":0,"fileCount":0,"operationOutcome":"not_started"}));
                    let _ = manager.tasks.finish_partial(&task.task_id, result, error);
                }
            }
            let _ = MaintenanceManager::invalidate_storage(manager.paths.workspace_root(), &id);
            manager.maintenance.notify(&id);
        });
        runners.push(StorageRunner {
            project_id: runner_project,
            handle: runner,
        });
        Ok(record)
    }
    fn cleanup(&self, id: &str, task: &str, input: &CleanupInput) -> Result<Value, ApiError> {
        let (binding, _) = self.binding(id)?;
        if self.store.registered_epoch(id)?.is_none() {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Cleanup requires the registered project directory",
            ));
        }
        self.store
            .control(id)?
            .check(false, input.maintenance_id.as_deref(), false)?;
        self.protection(id, Some(task))?;
        let selected = input
            .categories
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let (candidates, skipped) = self.cleanup_candidates(id, &binding, &selected)?;
        let mut result = json!({"deleted":[],"skipped":skipped,"failures":[],"logicalBytes":0,"fileCount":0,"operationOutcome":"in_progress"});
        let mut bytes = 0u64;
        let mut count = 0u64;
        for (category, target) in candidates {
            if self.binding(id).map(|v| v.0).ok() != Some(binding.clone()) {
                result["failures"].as_array_mut().unwrap().push(problem(
                    &target,
                    "root_changed",
                    "Cleanup root or classification changed",
                ));
                break;
            }
            self.store
                .control(id)?
                .check(false, input.maintenance_id.as_deref(), false)
                .map_err(|e| e.detail("partialResult", result.clone()))?;
            self.protection(id, Some(task))
                .map_err(|e| e.detail("partialResult", result.clone()))?;
            let mut removed = Deletion::default();
            delete_whitelisted(&target, &mut removed);
            bytes = bytes.saturating_add(removed.bytes);
            count = count.saturating_add(removed.files);
            if removed.files > 0 || removed.links > 0 {
                result["deleted"].as_array_mut().unwrap().push(json!({"category":category,"path":target,"fileCount":int(removed.files),"logicalBytes":int(removed.bytes),"linkCount":removed.links}));
            }
            result["failures"]
                .as_array_mut()
                .unwrap()
                .extend(removed.issues);
            result["logicalBytes"] = int(bytes);
            result["fileCount"] = int(count);
        }
        let failed = !result["failures"].as_array().unwrap().is_empty();
        result["operationOutcome"] = if failed {
            "partial".into()
        } else {
            "completed".into()
        };
        result["measurement"] = "removedLogicalFileBytes".into();
        result["physicalBytesFreed"] = Value::Null;
        if failed {
            Err(ApiError::new(
                "STORAGE_CLEANUP_PARTIAL",
                "Some objects were retained; inspect the completed deletions and errors",
            )
            .detail("partialResult", result))
        } else {
            Ok(result)
        }
    }
    pub async fn workspace_info(&self) -> Result<Value, ApiError> {
        let reader = self.readers.clone().read_owned().await;
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Storage operations are paused",
            ));
        }
        self.store.check_workspace_available()?;
        self.require_aggregate_available()?;
        let paths = self.paths.clone();
        let installation = self.installation.clone();
        let generation = self.instance.clone();
        #[cfg(test)]
        let pause = self.aggregate_pause.lock().unwrap().clone();
        tokio::task::spawn_blocking(move || {
            let _reader = reader;
            #[cfg(test)]
            if let Some(pause) = pause {
                pause.wait();
            }
            workspace_scan(&paths, installation.as_deref(), &generation)
        })
        .await
        .map_err(|e| ApiError::new("STORAGE_SCAN_FAILED", e.to_string()))?
    }
    pub(crate) fn request_stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }
    pub async fn quiesce(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.request_stop();
        let runners = std::mem::take(&mut *self.runners.lock().unwrap());
        let issues = crate::shutdown::join_all(
            "storage",
            runners.into_iter().map(|runner| runner.handle).collect(),
        )
        .await;
        let _drained = self.readers.write().await;
        issues
    }
    pub fn resume(&self) {
        self.stopping.store(false, Ordering::Release);
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.quiesce().await
    }
    fn require_project_available(&self, id: &str) -> Result<(), ApiError> {
        if self.forgotten.lock().unwrap().contains(id) {
            return Err(ApiError::new(
                "PROJECT_DELETED",
                "项目正在删除或已经删除，空间扫描已停止。",
            ));
        }
        Ok(())
    }
    fn require_aggregate_available(&self) -> Result<(), ApiError> {
        for id in self.forgotten.lock().unwrap().iter() {
            let pending = if id.starts_with("prj_") {
                !self.store.deleted_project_identity_absent(id)?
                    || self
                        .paths
                        .data_root()
                        .join("web-project-ownership")
                        .join(id)
                        .exists()
            } else {
                self.paths.data_root().join("node-apps").join(id).exists()
                    || self
                        .paths
                        .data_root()
                        .join("node-project-ownership")
                        .join(id)
                        .exists()
            };
            if pending {
                return Err(ApiError::new(
                    "STORAGE_SCAN_INVALIDATED",
                    "项目正在删除，汇总将在删除结束后重新统计。",
                ));
            }
        }
        Ok(())
    }
    /// Called before the deletion worker takes its project write gate. Waiting
    /// here joins the real blocking scan and its final task write, not merely a
    /// task status. Other projects' scans and cleanup drafts remain intact.
    pub(crate) async fn forget_deleted_project(&self, id: &str) -> Result<Vec<String>, ApiError> {
        if !(id.starts_with("prj_") && valid_id(id)
            || Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id))
        {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        self.forgotten.lock().unwrap().insert(id.into());
        let selected = {
            let mut runners = self.runners.lock().unwrap();
            let mut selected = vec![];
            let mut keep = vec![];
            for runner in std::mem::take(&mut *runners) {
                if runner.project_id == id {
                    selected.push(runner.handle)
                } else {
                    keep.push(runner)
                }
            }
            *runners = keep;
            selected
        };
        let issues = crate::shutdown::join_all("storage-deletion", selected).await;
        if !issues.is_empty() {
            return Err(
                ApiError::new("STORAGE_SCAN_STOP_FAILED", "空间扫描尚未确认结束。")
                    .detail("issues", json!(issues)),
            );
        }
        let _aggregate = self.readers.write().await;
        self.scans.lock().unwrap().remove(id);
        self.generations.lock().unwrap().remove(id);
        let roots = crate::workspace::local_cleanup_roots(
            &self
                .paths
                .selected_for_cleanup()
                .map_err(|error| ApiError::new("STORAGE_CACHE_UNAVAILABLE", error.to_string()))?,
        )?;
        let mut paths = vec![self.paths.data_root().join("cache/storage/platform.json")];
        for root in roots {
            let storage = root.join(".platform/cache/storage");
            paths.extend([
                storage.join("projects").join(format!("{id}.json")),
                storage
                    .join("invalidations/projects")
                    .join(format!("{id}.json")),
                storage.join("workspace.json"),
                storage.join("invalidations/workspace.json"),
            ]);
        }
        for path in paths {
            remove_storage_cache(&path)?;
        }
        let ids = self.tasks.storage_cache_task_ids(id);
        self.tasks.forget_storage_cache_tasks(id, &ids)?;
        Ok(ids)
    }
    pub(crate) fn finish_deleted_project(&self, id: &str) {
        self.scans.lock().unwrap().remove(id);
        self.generations.lock().unwrap().remove(id);
        self.forgotten.lock().unwrap().remove(id);
    }
}

fn remove_storage_cache(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            fs::remove_file(path)?;
            File::open(path.parent().unwrap())?.sync_all()?;
            Ok(())
        }
        Ok(_) => Err(ApiError::new("UNSAFE_PATH", "空间缓存不是普通文件。")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

type CleanupCandidates = (Vec<(String, PathBuf)>, Vec<Value>);
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CleanupInput {
    categories: Vec<String>,
    maintenance_id: Option<String>,
}
#[derive(Default)]
struct Deletion {
    bytes: u64,
    files: u64,
    links: u64,
    issues: Vec<Value>,
}
fn delete_whitelisted(path: &Path, out: &mut Deletion) {
    fn remove(parent: &File, name: &[u8], path: &Path, allowed: &Path, out: &mut Deletion) {
        if path
            .strip_prefix(allowed)
            .is_ok_and(|p| p.components().count() > 256)
        {
            out.issues.push(problem(
                path,
                "depth_limit_retained",
                "Directory is too deep for safe cleanup",
            ));
            return;
        }
        let st = match stat_at(parent, name) {
            Ok(st) => st,
            Err(e) => {
                out.issues.push(problem(path, "not_deleted", e));
                return;
            }
        };
        let kind = st.st_mode as u32 & libc::S_IFMT as u32;
        let is_dir = kind == libc::S_IFDIR as u32;
        if is_dir {
            match child_dir(parent, name) {
                Ok(dir) => {
                    if dir.metadata().map_or(true, |m| {
                        m.dev() != st.st_dev as u64 || m.ino() != st.st_ino
                    }) {
                        out.issues.push(problem(
                            path,
                            "changed_object_retained",
                            "Directory changed before cleanup could inspect it",
                        ));
                        return;
                    }
                    if let Err(e) = directory_names(&dir, |child| {
                        remove(
                            &dir,
                            child,
                            &path.join(std::ffi::OsStr::from_bytes(child)),
                            allowed,
                            out,
                        )
                    }) {
                        out.issues.push(problem(path, "not_deleted", e));
                        return;
                    }
                }
                Err(e) => {
                    out.issues.push(problem(path, "not_deleted", e));
                    return;
                }
            }
        } else if kind == libc::S_IFLNK as u32 {
            // Ordinary internal package links may be unlinked, never followed.
            let safe = fs::read_link(path).ok().is_some_and(|link| {
                let target = if link.is_absolute() {
                    link
                } else {
                    path.parent().unwrap().join(link)
                };
                let mut normalized = PathBuf::new();
                for part in target.components() {
                    match part {
                        std::path::Component::ParentDir => {
                            if !normalized.pop() {
                                return false;
                            }
                        }
                        std::path::Component::CurDir => {}
                        other => normalized.push(other.as_os_str()),
                    }
                }
                normalized.starts_with(allowed)
            });
            if !safe {
                out.issues.push(problem(
                    path,
                    "unsafe_link_retained",
                    "Link target is outside the exact cleanup root or cannot be read",
                ));
                return;
            }
        } else if kind != libc::S_IFREG as u32 {
            out.issues.push(problem(
                path,
                "special_file_retained",
                "Unsupported file kind",
            ));
            return;
        }
        let current = match stat_at(parent, name) {
            Ok(st) => st,
            Err(e) => {
                out.issues.push(problem(path, "not_deleted", e));
                return;
            }
        };
        if current.st_dev != st.st_dev
            || current.st_ino != st.st_ino
            || current.st_mode != st.st_mode
        {
            out.issues.push(problem(
                path,
                "changed_object_retained",
                "Object changed during cleanup",
            ));
            return;
        }
        let Ok(name) = name_c(name) else {
            return;
        };
        if unsafe {
            libc::unlinkat(
                parent.as_raw_fd(),
                name.as_ptr(),
                if is_dir { libc::AT_REMOVEDIR } else { 0 },
            )
        } != 0
        {
            out.issues.push(problem(
                path,
                "not_deleted",
                std::io::Error::last_os_error(),
            ));
        } else if kind == libc::S_IFREG as u32 {
            out.bytes = out.bytes.saturating_add(st.st_size.max(0) as u64);
            out.files += 1;
        } else if kind == libc::S_IFLNK as u32 {
            out.links += 1;
        }
    }
    if let Err(e) = check_no_symlinks(path, true) {
        out.issues.push(problem(path, "unsafe_root_retained", e));
        return;
    }
    let Some(parent) = path.parent() else {
        return;
    };
    let Some(name) = path.file_name() else {
        return;
    };
    match files::root(parent) {
        Ok(fd) => {
            remove(&fd, name.as_bytes(), path, path, out);
            let _ = fd.sync_all();
        }
        Err(e) => out.issues.push(problem(path, "not_deleted", e)),
    }
}

#[derive(Clone)]
struct FileQuery {
    kind: Option<String>,
    prefix: String,
    query: String,
    sort: String,
    order: String,
    limit: usize,
    cursor: Option<String>,
}
impl FileQuery {
    fn parse(v: Value) -> Result<Self, ApiError> {
        let obj = v
            .as_object()
            .ok_or_else(|| ApiError::new("INVALID_INPUT", "File query must be an object"))?;
        if obj.keys().any(|k| {
            !matches!(
                k.as_str(),
                "type" | "prefix" | "query" | "sort" | "order" | "limit" | "cursor"
            )
        }) {
            return Err(ApiError::new(
                "INVALID_INPUT",
                "Unsupported file query field",
            ));
        }
        let string = |k: &str, default: &str| {
            obj.get(k)
                .and_then(Value::as_str)
                .unwrap_or(default)
                .to_owned()
        };
        let kind = obj
            .get("type")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_owned);
        if kind.as_ref().is_some_and(|v| {
            !matches!(
                v.as_str(),
                "image" | "audio" | "video" | "document" | "other"
            )
        }) {
            return Err(ApiError::new("INVALID_INPUT", "Invalid attachment type"));
        }
        let prefix = string("prefix", "");
        if !prefix.is_empty() {
            files::parts(prefix.trim_end_matches('/'))?;
        }
        let sort = string("sort", "name");
        let order = string("order", "asc");
        if !matches!(sort.as_str(), "name" | "size" | "modifiedAt")
            || !matches!(order.as_str(), "asc" | "desc")
        {
            return Err(ApiError::new("INVALID_INPUT", "Invalid attachment sort"));
        }
        let limit = match obj.get("limit") {
            None => 100,
            Some(v) => v
                .as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                .ok_or_else(|| ApiError::new("INVALID_INPUT", "limit must be an integer"))?,
        };
        if limit == 0 || limit > 500 {
            return Err(ApiError::new(
                "INVALID_INPUT",
                "limit must be between 1 and 500",
            ));
        }
        Ok(Self {
            kind,
            prefix,
            query: string("query", ""),
            sort,
            order,
            limit: limit as usize,
            cursor: obj.get("cursor").and_then(Value::as_str).map(str::to_owned),
        })
    }
    fn filter_key(&self) -> String {
        serde_json::to_string(&json!([
            self.kind,
            self.prefix,
            self.query,
            self.sort,
            self.order,
            self.limit
        ]))
        .unwrap()
    }
}
fn display_type(path: &str) -> &'static str {
    let mime = files::mime(path);
    if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("audio/") {
        "audio"
    } else if mime.starts_with("video/") {
        "video"
    } else if mime.starts_with("text/")
        || mime == "application/pdf"
        || mime.contains("officedocument")
        || mime.contains("msword")
        || mime.contains("opendocument")
    {
        "document"
    } else {
        "other"
    }
}
fn compare_entry(a: &Entry, b: &Entry, q: &FileQuery) -> Cmp {
    let key = match q.sort.as_str() {
        "size" => a.bytes.cmp(&b.bytes),
        "modifiedAt" => a.modified.cmp(&b.modified),
        _ => a.path.cmp(&b.path),
    };
    let key = if q.order == "desc" {
        key.reverse()
    } else {
        key
    };
    key.then_with(|| a.path.cmp(&b.path))
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    filter: String,
    signature: String,
    binding: String,
    instance: String,
    last: Entry,
}
fn invalid_cursor() -> ApiError {
    ApiError::new(
        "STORAGE_CURSOR_INVALID",
        "Attachments or query changed; request the first page again",
    )
}
fn decode_cursor(text: &str) -> Result<Cursor, ApiError> {
    if text.len() > 32_000 {
        return Err(invalid_cursor());
    }
    let bytes = URL_SAFE_NO_PAD.decode(text).map_err(|_| invalid_cursor())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid_cursor())
}

fn workspace_category(path: &str) -> String {
    if in_root(path, "projects") {
        "projects"
    } else if in_root(path, ".platform/operations") {
        "operations"
    } else if in_root(path, ".platform/cache") {
        "sharedCache"
    } else if in_root(path, ".platform") {
        "control"
    } else {
        "other"
    }
    .into()
}
fn unknown_scope(kind: &str, root: Option<&Path>, reason: &str) -> Value {
    json!({"scope":{"kind":kind},"root":root,"status":"unknown","totalBytes":null,"fileCount":null,"startedAt":null,"completedAt":null,"complete":false,"stale":true,"issues":[{"reason":reason}]})
}
fn numeric(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| {
        v.get("$int64")
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok())
    })
}
struct NodeSourceStorage {
    path: PathBuf,
    ownership: &'static str,
    preserved_on_delete: Option<bool>,
    identity: Option<PersistedIdentity>,
    issue: Option<String>,
}
fn node_source_storage(
    paths: &RuntimePaths,
) -> Result<BTreeMap<String, NodeSourceStorage>, ApiError> {
    let node_root = paths.data_root().join("node-apps");
    check_no_symlinks(&node_root, true)?;
    let mut sources = BTreeMap::new();
    if !node_root.try_exists()? {
        return Ok(sources);
    }
    let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
        paths.data_root().into(),
        paths.workspace_root().into(),
    );
    for entry in fs::read_dir(&node_root)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if Uuid::parse_str(&id).is_err() || !entry.file_type()?.is_dir() {
            continue;
        }
        let Ok(value) = read_json::<Value>(&entry.path().join("registration.json")) else {
            continue;
        };
        let Ok(crate::node_registration::StoredNodeRegistration::Direct(record)) =
            crate::node_registration::decode_registration(value, &id, &node_root)
        else {
            continue;
        };
        let owned = ownership.owned_paths(&id).ok();
        let owned_source = owned
            .as_ref()
            .map(|paths| paths.iter().any(|path| path == &record.source_path));
        let verified = (|| -> Result<PersistedIdentity, ApiError> {
            let binding: crate::node_apps::development::NodeDevelopmentBinding =
                read_json(&entry.path().join("development-source.json"))?;
            if binding.source_root != record.source_path {
                return Err(ApiError::new(
                    "NODE_SOURCE_CHANGED",
                    "登记的项目文件夹与目录身份不符。",
                ));
            }
            binding.validate_directory()?;
            crate::node_apps::development::require_claimed_source_owner(
                paths.data_root(),
                &id,
                &record.source_path,
            )
        })();
        sources.insert(
            id,
            NodeSourceStorage {
                path: record.source_path,
                ownership: match owned_source {
                    Some(true) => "platform",
                    Some(false) => "external",
                    None => "unknown",
                },
                preserved_on_delete: owned_source.map(|owned| !owned),
                identity: verified.as_ref().ok().cloned(),
                issue: verified.err().map(|error| error.message),
            },
        );
    }
    Ok(sources)
}
fn workspace_scan(
    paths: &RuntimePaths,
    installation: Option<&Path>,
    generation: &str,
) -> Result<Value, ApiError> {
    let workspace = root_identity(paths.workspace_root())?;
    let data = root_identity(paths.data_root())?;
    let mut roots = vec![("workspace".to_owned(), workspace.path.clone())];
    if let Some(p) = installation {
        roots.push(("platformProgram".into(), p.to_path_buf()));
    }
    let mut missing = vec![];
    if paths.migrations_dir().is_dir() {
        for entry in fs::read_dir(paths.migrations_dir())? {
            let file = entry?.path().join("journal.json");
            if !file.is_file() {
                continue;
            }
            if let Some(id) = file
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
            {
                if valid_id(id)
                    && crate::history_cleanup::retired_cleanup_status(paths, id)?.is_some()
                {
                    if let Ok(record) = read_json::<Value>(&file) {
                        if record["sourcePath"].as_str().is_some_and(|source| {
                            matches!(fs::symlink_metadata(source), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
                        }) {
                            // Current volume inspection confirms that the old path is absent.
                            // Preserve migration proof without claiming a past deletion.
                            continue;
                        }
                    }
                }
            }
            match read_json::<Value>(&file) {
                Ok(journal) if journal["committed"] == true => {
                    if let Some(source) = journal["sourcePath"].as_str() {
                        let path = PathBuf::from(source);
                        if path != workspace.path && !roots.iter().any(|(_, old)| old == &path) {
                            match root_identity(&path) {
                                Ok(actual) => {
                                    let verified = serde_json::from_value::<PersistedIdentity>(
                                        journal["sourceIdentity"].clone(),
                                    )
                                    .is_ok_and(|expected| {
                                        expected.matches_path(&path).unwrap_or(false)
                                    });
                                    if verified {
                                        roots.push(("retainedWorkspace".into(), actual.path));
                                    } else {
                                        missing.push(unknown_scope(
                                            "retainedWorkspace",
                                            Some(&path),
                                            "retained_root_identity_changed",
                                        ));
                                    }
                                }
                                Err(_) => missing.push(unknown_scope(
                                    "retainedWorkspace",
                                    Some(&path),
                                    "retained_root_unavailable",
                                )),
                            }
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => missing.push(json!({"scope":{"kind":"retainedWorkspace"},"root":null,"totalBytes":null,"complete":false,"stale":true,"issues":[problem(file,"unreadable_migration_record",error)]})),
            }
        }
    }
    if !roots.iter().any(|(_, p)| p == &data.path) {
        roots.push(("applicationData".into(), data.path.clone()));
    }
    let node_sources = node_source_storage(paths)?;
    // A source already inside a known scope is a reference, not another total.
    // Independent author folders are measured as additional disjoint locations.
    for source in node_sources
        .values()
        .filter(|source| source.identity.is_some())
    {
        if !roots.iter().any(|(_, root)| source.path.starts_with(root)) {
            roots.push(("nodeSource".into(), source.path.clone()));
        }
    }
    for source in node_sources
        .values()
        .filter(|source| source.identity.is_none())
    {
        if !roots.iter().any(|(_, root)| source.path.starts_with(root)) {
            missing.push(unknown_scope(
                "nodeSource",
                Some(&source.path),
                "source_directory_unavailable_or_replaced",
            ));
        }
    }
    let mut measured = vec![];
    for (kind, root) in &roots {
        let exclusions = roots
            .iter()
            .filter(|(_, p)| p != root && p.starts_with(root))
            .filter_map(|(_, p)| p.strip_prefix(root).ok().map(Path::to_path_buf))
            .collect::<Vec<_>>();
        let mut value = scope_scan(root, json!({"kind":kind}), &exclusions, |p| {
            if kind == "workspace" {
                workspace_category(p)
            } else if kind == "applicationData" && in_root(p, "node-apps") {
                // Installed Node apps are user projects, not platform settings.
                "nodeApps".into()
            } else {
                kind.clone()
            }
        });
        if kind == "nodeSource"
            && node_sources
                .values()
                .find(|source| &source.path == root)
                .is_some_and(|source| {
                    source
                        .identity
                        .as_ref()
                        .is_none_or(|identity| !identity.matches_path(root).unwrap_or(false))
                })
        {
            value = unknown_scope(
                "nodeSource",
                Some(root),
                "source_directory_changed_during_scan",
            );
        }
        measured.push((kind.clone(), value));
    }
    let mut program = unknown_scope(
        "platformProgram",
        None,
        "running_executable_is_not_inside_an_app_bundle",
    );
    let mut current = unknown_scope("workspace", Some(paths.workspace_root()), "not_scanned");
    let mut other = missing;
    let mut disks = BTreeMap::new();
    for (kind, mut value) in measured {
        if let Some(root) = value["root"].as_str() {
            if let Ok(m) = fs::metadata(root) {
                disks.entry(m.dev()).or_insert_with(||json!({"device":m.dev(),"exampleRoot":root,"availableBytes":fs2::available_space(root).ok().map(int)}));
            }
        }
        value["rootGeneration"] = generation.into();
        if kind == "platformProgram" {
            program = value;
        } else if kind == "workspace" {
            current = value;
        } else {
            other.push(value);
        }
    }
    // Project rows reference content already measured in the workspace and
    // application-data scopes. Their window-view totals are not added again to
    // either disjoint root total or to knownLocalTotal.
    let mut project_rows = vec![];
    {
        for (id, path) in crate::projects::enumerate_project_roots(paths.workspace_root())? {
            if !path.try_exists()? {
                continue;
            }
            let mut row = scope_scan(&path, json!({"kind":"project","projectId":id}), &[], |_| {
                "project".into()
            });
            let (views_root, views, view_issues) = window_views_scan(paths, &id, &|| false);
            row["totalBytes"] = numeric(&row["totalBytes"])
                .map(|bytes| int(bytes.saturating_add(views.bytes)))
                .unwrap_or(Value::Null);
            row["fileCount"] = numeric(&row["fileCount"])
                .map(|files| int(files.saturating_add(views.files)))
                .unwrap_or(Value::Null);
            row["windowViews"] = json!({"root":views_root,"bytes":int(views.bytes),"fileCount":int(views.files),"complete":view_issues.is_empty(),"referenceOnly":true});
            row["categories"]["windowViews"] = views.value();
            if !view_issues.is_empty() {
                row["complete"] = false.into();
                row["issues"]
                    .as_array_mut()
                    .expect("scope scan issues are an array")
                    .extend(view_issues);
            }
            row["name"] = read_json::<Value>(&path.join("project.json"))
                .ok()
                .and_then(|v| v.get("name").cloned())
                .unwrap_or_else(|| json!(id));
            row["referenceOnly"] = true.into();
            project_rows.push(row);
        }
    }
    project_rows.sort_by(|a, b| {
        numeric(&b["totalBytes"])
            .cmp(&numeric(&a["totalBytes"]))
            .then_with(|| a["root"].as_str().cmp(&b["root"].as_str()))
    });
    current["projects"] = json!(project_rows);
    // Per-app references into the application-data scope, never added again.
    let mut node_rows = vec![];
    let node_root = paths.data_root().join("node-apps");
    if node_root.is_dir() {
        for entry in fs::read_dir(&node_root)? {
            let entry = entry?;
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if Uuid::parse_str(&id).is_err()
                || !entry.file_type()?.is_dir()
                || !entry.path().join("registration.json").is_file()
            {
                continue;
            }
            let mut row = scope_scan(
                &entry.path(),
                json!({"kind":"nodeApp","appId":id}),
                &[],
                |p| {
                    if in_root(p, "generations") {
                        "program".into()
                    } else if in_root(p, "data") {
                        "data".into()
                    } else if in_root(p, "configuration") {
                        "configuration".into()
                    } else if in_root(p, "logs") || (in_root(p, "runs") && p.ends_with(".log")) {
                        "logs".into()
                    } else if in_root(p, "cache") {
                        "cache".into()
                    } else {
                        "other".into()
                    }
                },
            );
            row["referenceOnly"] = true.into();
            let mut instance = row.clone();
            instance["scope"] = json!({"kind":"nodeInstance","appId":id});
            row["instance"] = instance;
            if let Some(source) = node_sources.get(&id) {
                let identity_valid = || {
                    source.identity.as_ref().is_some_and(|identity| {
                        identity.matches_path(&source.path).unwrap_or(false)
                    })
                };
                let mut folder = if identity_valid() {
                    scope_scan(
                        &source.path,
                        json!({"kind":"nodeProjectFolder","appId":id}),
                        &[],
                        |path| {
                            if path.split('/').any(|part| part == "node_modules") {
                                "dependencies".into()
                            } else if in_root(path, ".git") {
                                "git".into()
                            } else {
                                "source".into()
                            }
                        },
                    )
                } else {
                    let mut value = unknown_scope(
                        "nodeProjectFolder",
                        Some(&source.path),
                        "source_directory_unavailable_or_replaced",
                    );
                    if let Some(message) = &source.issue {
                        value["issues"] = json!([{"reason":"source_directory_unavailable_or_replaced","message":message}]);
                    }
                    value
                };
                if source.identity.is_some() && !identity_valid() {
                    folder = unknown_scope(
                        "nodeProjectFolder",
                        Some(&source.path),
                        "source_directory_changed_during_scan",
                    );
                }
                folder["ownership"] = json!(source.ownership);
                folder["preservedOnDelete"] = json!(source.preserved_on_delete);
                folder["referenceOnly"] = true.into();
                row["sourcePath"] = json!(source.path);
                row["sourceOwnership"] = json!(source.ownership);
                row["sourcePreservedOnDelete"] = json!(source.preserved_on_delete);
                row["totalBytes"] = match (
                    numeric(&row["instance"]["totalBytes"]),
                    numeric(&folder["totalBytes"]),
                ) {
                    (Some(instance), Some(source)) => int(instance.saturating_add(source)),
                    _ => Value::Null,
                };
                row["fileCount"] = match (
                    numeric(&row["instance"]["fileCount"]),
                    numeric(&folder["fileCount"]),
                ) {
                    (Some(instance), Some(source)) => int(instance.saturating_add(source)),
                    _ => Value::Null,
                };
                row["complete"] =
                    json!(row["instance"]["complete"] == true && folder["complete"] == true);
                row["categories"]["projectFolder"] =
                    json!({"bytes":folder["totalBytes"],"fileCount":folder["fileCount"]});
                row["projectFolder"] = folder;
            }
            node_rows.push(row);
        }
    }
    node_rows.sort_by(|a, b| numeric(&b["totalBytes"]).cmp(&numeric(&a["totalBytes"])));
    let mut total = 0u64;
    let mut complete = true;
    for value in std::iter::once(&current)
        .chain(std::iter::once(&program))
        .chain(other.iter())
    {
        match numeric(&value["totalBytes"]) {
            Some(n) => total = total.saturating_add(n),
            None => complete = false,
        }
        complete &= value["complete"] == true;
    }
    let workspace_cache = paths
        .workspace_root()
        .join(".platform/cache/storage/workspace.json");
    check_no_symlinks(&workspace_cache, true)?;
    builds::atomic_json(
        &workspace_cache,
        &json!({"schemaVersion":SCHEMA,"scope":{"kind":"workspace"},"workspaceRootIdentity":workspace,"rootGeneration":generation,"result":current}),
    )?;
    if let Some(root) = installation {
        let cache = paths.data_root().join("cache/storage/platform.json");
        check_no_symlinks(&cache, true)?;
        let executable = std::env::current_exe()
            .ok()
            .and_then(|p| fs::metadata(p).ok())
            .map(
                |m| json!({"device":m.dev(),"inode":m.ino(),"bytes":m.len(),"modified":m.mtime()}),
            );
        builds::atomic_json(
            &cache,
            &json!({"schemaVersion":SCHEMA,"scope":{"kind":"platformProgram"},"installationRootIdentity":root_identity(root).ok(),"installedBuild":executable,"version":crate::APP_VERSION,"result":program}),
        )?;
    }
    Ok(
        json!({"platformProgram":program,"workspace":current,"nodeApps":node_rows,"otherLocalLocations":other,"knownLocalTotal":{"totalBytes":int(total),"complete":complete,"nonAtomic":true,"description":"Known disjoint locations measured at their stated times"},"disks":disks.into_values().collect::<Vec<_>>()}),
    )
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
