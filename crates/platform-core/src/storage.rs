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
    collections::{BTreeMap, HashMap},
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
fn walk(root: &Path, excluded: &[PathBuf], mut visit: impl FnMut(&Entry)) -> Walk {
    fn descend(
        dir: &File,
        relative: &Path,
        excluded: &[PathBuf],
        out: &mut Walk,
        visit: &mut dyn FnMut(&Entry),
    ) {
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
                    Ok(child) => descend(&child, &path, excluded, out, visit),
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
        Ok(fd) => descend(&fd, Path::new(""), excluded, &mut out, &mut visit),
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

type RootGenerations = HashMap<String, (RootIdentity, Option<String>, String)>;
pub(crate) struct StorageManager {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    maintenance: Arc<MaintenanceManager>,
    instance: String,
    stopping: AtomicBool,
    scans: Mutex<HashMap<String, String>>,
    runners: Mutex<Vec<JoinHandle<()>>>,
    installation: Option<PathBuf>,
    generations: Mutex<RootGenerations>,
    readers: Arc<tokio::sync::RwLock<()>>,
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
            installation,
            generations: Mutex::new(HashMap::new()),
            readers: Arc::new(tokio::sync::RwLock::new(())),
        });
        manager.recover_cleanup()?;
        Ok(manager)
    }
    fn cache(&self, id: &str) -> PathBuf {
        self.paths
            .workspace_root()
            .join(".platform/cache/storage/projects")
            .join(format!("{id}.json"))
    }
    fn binding(&self, id: &str) -> Result<(Binding, Vec<Value>), ApiError> {
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
        let registration_switch = self.store.session_generation(id)?;
        let mut generations = self.generations.lock().unwrap();
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
        Ok((
            Binding {
                workspace_root_identity: root_identity(self.paths.workspace_root())?,
                project_root_identity: project_identity,
                root_generation,
                mapping,
                invalidation,
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
        runners.push(runner);
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
        let mut first = walk(root, &[], |e| {
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
        let mut categories = counts
            .iter()
            .map(|(k, c)| (k.to_string(), c.value()))
            .collect::<serde_json::Map<_, _>>();
        categories.get_mut("source").unwrap()["fixedAssets"] = assets.value();
        categories.get_mut("database").unwrap()["main"] = main.value();
        categories.get_mut("database").unwrap()["companions"] = companions.value();
        categories.get_mut("cache").unwrap()["uploadStaging"] = json!({"bytes":int(uploads.bytes),"fileCount":int(uploads.files),"protected":true,"reason":"Upload staging is not a cleanup category; active or unclassified residues remain protected"});
        let total = counts.values().fold(0u64, |a, c| a.saturating_add(c.bytes));
        let current = json!({"scope":{"kind":"project","projectId":id},"root":root,"scanId":Uuid::new_v4().to_string(),"startedAt":started,"completedAt":now(),"complete":issues.is_empty(),"stale":false,"totalBytes":int(total),"categories":categories,"issues":issues,"nonAtomic":true,"measurement":"logicalFileBytes"});
        let result = json!({"currentProject":current,"relatedHistory":self.history(id)});
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
                for name in [
                    "journal.json",
                    "lifecycle.json",
                    "snapshot.json",
                    "cleanup.json",
                ] {
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
                for name in ["journal.json", "lifecycle.json"] {
                    let file = path.join(name);
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
        runners.push(runner);
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
        let mut journal = CleanupJournal {
            schema_version: SCHEMA,
            project_id: id.into(),
            task_id: task.into(),
            resolved: false,
            result: json!({"deleted":[],"skipped":skipped,"failures":[],"logicalBytes":0,"fileCount":0,"operationOutcome":"in_progress"}),
            in_flight: None,
        };
        let path = self.cleanup_journal(task);
        check_no_symlinks(&path, true)?;
        builds::atomic_json(&path, &journal)?;
        let mut bytes = 0u64;
        let mut count = 0u64;
        for (category, target) in candidates {
            if self.binding(id).map(|v| v.0).ok() != Some(binding.clone()) {
                journal.result["failures"]
                    .as_array_mut()
                    .unwrap()
                    .push(problem(
                        &target,
                        "root_changed",
                        "Cleanup root or classification changed",
                    ));
                break;
            }
            self.store
                .control(id)?
                .check(false, input.maintenance_id.as_deref(), false)
                .map_err(|e| e.detail("partialResult", journal.result.clone()))?;
            self.protection(id, Some(task))
                .map_err(|e| e.detail("partialResult", journal.result.clone()))?;
            journal.in_flight = Some(target.clone());
            builds::atomic_json(&path, &journal)
                .map_err(|e| e.detail("partialResult", journal.result.clone()))?;
            let mut removed = Deletion::default();
            delete_whitelisted(&target, &mut removed);
            bytes = bytes.saturating_add(removed.bytes);
            count = count.saturating_add(removed.files);
            if removed.files > 0 || removed.links > 0 {
                journal.result["deleted"].as_array_mut().unwrap().push(json!({"category":category,"path":target,"fileCount":int(removed.files),"logicalBytes":int(removed.bytes),"linkCount":removed.links}));
            }
            journal.result["failures"]
                .as_array_mut()
                .unwrap()
                .extend(removed.issues);
            journal.result["logicalBytes"] = int(bytes);
            journal.result["fileCount"] = int(count);
            journal.in_flight = None;
            builds::atomic_json(&path, &journal)
                .map_err(|e| e.detail("partialResult", journal.result.clone()))?;
        }
        let failed = !journal.result["failures"].as_array().unwrap().is_empty();
        journal.result["operationOutcome"] = if failed {
            "partial".into()
        } else {
            "completed".into()
        };
        journal.result["measurement"] = "removedLogicalFileBytes".into();
        journal.result["physicalBytesFreed"] = Value::Null;
        journal.resolved = true;
        builds::atomic_json(&path, &journal)
            .map_err(|e| e.detail("partialResult", journal.result.clone()))?;
        if failed {
            Err(ApiError::new(
                "STORAGE_CLEANUP_PARTIAL",
                "Some objects were retained; inspect the completed deletions and errors",
            )
            .detail("partialResult", journal.result))
        } else {
            Ok(journal.result)
        }
    }
    fn cleanup_journal(&self, task: &str) -> PathBuf {
        self.paths
            .workspace_root()
            .join(".platform/operations")
            .join(task)
            .join("cleanup.json")
    }
    fn recover_cleanup(&self) -> Result<(), ApiError> {
        let root = self.paths.workspace_root().join(".platform/operations");
        check_no_symlinks(&root, true)?;
        if root.is_dir() {
            for dir in fs::read_dir(root)? {
                let path = dir?.path().join("cleanup.json");
                if !path.is_file() {
                    continue;
                }
                let mut journal: CleanupJournal = read_json(&path)?;
                if journal.schema_version != SCHEMA
                    || !valid_id(&journal.project_id)
                    || !valid_id(&journal.task_id)
                    || path
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(|v| v.to_str())
                        != Some(&journal.task_id)
                {
                    return Err(ApiError::new(
                        "STORAGE_CLEANUP_JOURNAL_INVALID",
                        "Cleanup journal identity is invalid",
                    ));
                }
                if !journal.resolved {
                    journal.result["operationOutcome"] = "interrupted".into();
                    journal.result["automaticRetry"] = false.into();
                    if let Some(p) = &journal.in_flight {
                        journal.result["failures"].as_array_mut().ok_or_else(||ApiError::new("STORAGE_CLEANUP_JOURNAL_INVALID","Missing cleanup results"))?.push(problem(p,"interrupted_object","This object may have been partially deleted; inspect it before an explicit retry"));
                    }
                    journal.resolved = true;
                    builds::atomic_json(&path, &journal)?;
                }
                self.tasks
                    .reconcile_operation_result(&journal.task_id, journal.result)?;
            }
        }
        Ok(())
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
        let paths = self.paths.clone();
        let installation = self.installation.clone();
        let generation = self.instance.clone();
        tokio::task::spawn_blocking(move || {
            let _reader = reader;
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
        let issues = crate::shutdown::join_all("storage", runners).await;
        let _drained = self.readers.write().await;
        issues
    }
    pub fn resume(&self) {
        self.stopping.store(false, Ordering::Release);
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.quiesce().await
    }
}

type CleanupCandidates = (Vec<(String, PathBuf)>, Vec<Value>);
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CleanupInput {
    categories: Vec<String>,
    maintenance_id: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CleanupJournal {
    schema_version: u32,
    project_id: String,
    task_id: String,
    resolved: bool,
    result: Value,
    in_flight: Option<PathBuf>,
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
                            // A durable cleanup receipt explains the missing old copy.
                            // Keep the migration proof; do not count an unknown phantom root.
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
    let mut measured = vec![];
    for (kind, root) in &roots {
        let exclusions = roots
            .iter()
            .filter(|(_, p)| p != root && p.starts_with(root))
            .filter_map(|(_, p)| p.strip_prefix(root).ok().map(Path::to_path_buf))
            .collect::<Vec<_>>();
        let value = scope_scan(root, json!({"kind":kind}), &exclusions, |p| {
            if kind == "workspace" {
                workspace_category(p)
            } else if kind == "applicationData" && in_root(p, "node-apps") {
                // Installed Node apps are user projects, not platform settings.
                "nodeApps".into()
            } else {
                kind.clone()
            }
        });
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
    // These project rows are references into the workspace scope, never added
    // again to its total or to knownLocalTotal.
    let mut project_rows = vec![];
    {
        for (id, path) in crate::projects::enumerate_project_roots(paths.workspace_root())? {
            if !path.try_exists()? {
                continue;
            }
            let mut row = scope_scan(&path, json!({"kind":"project","projectId":id}), &[], |_| {
                "project".into()
            });
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
                    } else {
                        "other".into()
                    }
                },
            );
            row["referenceOnly"] = true.into();
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
