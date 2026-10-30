//! Explicit, scoped removal. Accepted plans live only in this manager; each new
//! preview rechecks current ownership. No per-file deletion history is written.
use crate::{
    builds::BuildManager,
    file_identity::PersistedIdentity,
    projects::{check_no_symlinks, valid_id},
    ApiError, ProjectStore, RuntimePaths, TaskRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
use std::{
    collections::{BTreeMap, BTreeSet},
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
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinHandle;
use uuid::Uuid;

const CONFIRMATION: &str = "永久清理所选内容";
const PLAN_TTL: u64 = 10 * 60 * 1000;

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
    fn public(&self) -> Value {
        json!({"id":self.id,"kind":self.kind,"label":self.label,"path":self.path,
            "projectId":self.project_id,"name":self.name,"totalBytes":wire_u64(self.total_bytes),
            "fileCount":wire_u64(self.file_count),"canClean":self.can_clean,"blockedReasons":self.blocked_reasons})
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    schema_version: u32,
    plan_id: String,
    instance_id: String,
    workspace_root: PathBuf,
    workspace_identity: Identity,
    expires_at: u64,
    items: Vec<Candidate>,
    task_id: Option<String>,
    selected_ids: Vec<String>,
    preview_task_id: String,
}
#[derive(Deserialize)]
struct DerivedCleanupResult {
    items: Vec<DerivedCleanupItem>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DerivedCleanupItem {
    id: String,
    project_id: Option<String>,
    path: PathBuf,
}
fn deleted_scope_matches(
    owner: Option<&str>,
    candidate: &str,
    path: &Path,
    id: &str,
    source_ids: &[String],
    roots: &[PathBuf],
) -> bool {
    if let Some(owner) = owner {
        return owner == id;
    }
    source_ids.iter().any(|source| source == candidate)
        || roots
            .iter()
            .any(|root| path.starts_with(root) || root.starts_with(path))
}
impl Plan {
    fn public(&self) -> Value {
        json!({"planId":self.plan_id,"expiresAt":self.expires_at,"workspaceRoot":self.workspace_root,
            "confirmationText":CONFIRMATION,"items":self.items.iter().map(Candidate::public).collect::<Vec<_>>(),
            "preserved":["当前项目及其源码、数据库、附件和发布版本","Node 项目的项目文件夹、数据和配置","未完成的网页项目取回和恢复资料以及操作日志","固定应用身份、设置、运行记录及迁移记录"],
            "warning":"永久清理不能撤销。取回前的原件清理后无法再找回那次取回之前的数据。请先停止外部 AI、编辑器或数据库工具对所选目录的访问。"})
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SubmitInput {
    plan_id: String,
    item_ids: Vec<String>,
    confirmation: String,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    removed_logical_bytes: u64,
    removed_file_count: u64,
    removed_paths: Vec<PathBuf>,
}
struct WorkspaceGuard(Arc<ProjectStore>);
impl Drop for WorkspaceGuard {
    fn drop(&mut self) {
        self.0.resume_workspace();
    }
}

pub(crate) struct CleanupManager {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    instance_id: String,
    admission: Mutex<()>,
    running: Mutex<Vec<JoinHandle<()>>>,
    stopping: AtomicBool,
    plans: Mutex<BTreeMap<String, Plan>>,
    latest: Mutex<Option<String>>,
    deleting: Mutex<BTreeSet<String>>,
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn error(message: impl std::fmt::Display) -> ApiError {
    ApiError::new("WORKSPACE_CLEANUP_BLOCKED", message.to_string())
}
/// A user-facing reason for keeping an item; it is shown verbatim.
fn protected(message: &str) -> ApiError {
    ApiError::new("CLEANUP_ITEM_PROTECTED", message)
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

impl CleanupManager {
    pub(crate) fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        instance_id: String,
    ) -> Result<Arc<Self>, ApiError> {
        Ok(Arc::new(Self {
            paths,
            store,
            tasks,
            instance_id,
            admission: Mutex::new(()),
            running: Mutex::new(vec![]),
            stopping: AtomicBool::new(false),
            plans: Mutex::new(BTreeMap::new()),
            latest: Mutex::new(None),
            deleting: Mutex::new(BTreeSet::new()),
        }))
    }
    fn ensure_running(&self) -> Result<(), ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"))
        } else if !self.deleting.lock().unwrap().is_empty() {
            Err(ApiError::new(
                "PROJECT_DELETION_PENDING",
                "项目正在删除，请在删除结束后重新预览清理范围。",
            ))
        } else {
            Ok(())
        }
    }
    pub(crate) fn preview(self: &Arc<Self>) -> Result<TaskRecord, ApiError> {
        let mut running = self.running.lock().unwrap();
        self.ensure_running()?;
        self.store.check_workspace_available()?;
        let task = self
            .tasks
            .create_workspace_operation("workspaceCleanupPreview")?;
        let id = task.task_id.clone();
        let manager = self.clone();
        let runner = tokio::spawn(async move {
            let worker = manager.clone();
            let task_id = id.clone();
            let result = tokio::task::spawn_blocking(move || {
                worker.tasks.phase(&task_id, "inspecting")?;
                let items = worker.candidates()?;
                let plan = Plan {
                    schema_version: 1,
                    plan_id: format!("cleanup_plan_{}", Uuid::new_v4().simple()),
                    instance_id: worker.instance_id.clone(),
                    workspace_root: worker.paths.workspace_root().into(),
                    workspace_identity: directory_identity(worker.paths.workspace_root())?,
                    expires_at: now_ms() + PLAN_TTL,
                    items,
                    task_id: None,
                    selected_ids: vec![],
                    preview_task_id: task_id.clone(),
                };
                let public = plan.public();
                let mut plans = worker.plans.lock().unwrap();
                plans.retain(|_, plan| plan.expires_at >= now_ms() || plan.task_id.is_some());
                plans.insert(plan.plan_id.clone(), plan);
                Ok(public)
            })
            .await
            .map_err(error)
            .and_then(|result| result);
            let _ = manager.tasks.finish(&id, result);
        });
        running.push(runner);
        Ok(task)
    }
    pub(crate) fn submit(self: &Arc<Self>, input: Value) -> Result<TaskRecord, ApiError> {
        let mut running = self.running.lock().unwrap();
        self.ensure_running()?;
        let input: SubmitInput = serde_json::from_value(input)?;
        if !input.plan_id.starts_with("cleanup_plan_")
            || !valid_id(&input.plan_id)
            || input.confirmation != CONFIRMATION
            || input.item_ids.is_empty()
        {
            return Err(ApiError::new(
                "INVALID_CLEANUP_CONFIRMATION",
                "Select specific preview items and enter the exact permanent-cleanup confirmation",
            ));
        }
        let _admission = self.admission.lock().unwrap();
        let mut plans = self.plans.lock().unwrap();
        let mut plan = plans.get(&input.plan_id).cloned().ok_or_else(|| {
            ApiError::new(
                "CLEANUP_PLAN_EXPIRED",
                "清理预览已结束或软件已重开，请重新扫描后确认。",
            )
        })?;
        let selected: BTreeSet<_> = input.item_ids.into_iter().collect();
        let selected_ids = selected.iter().cloned().collect::<Vec<_>>();
        if plan.schema_version != 1 || plan.plan_id != input.plan_id {
            return Err(error("Invalid cleanup plan identity"));
        }
        if let Some(id) = &plan.task_id {
            if plan.selected_ids != selected_ids {
                return Err(ApiError::new(
                    "CLEANUP_PLAN_CONSUMED",
                    "This plan was consumed with a different selection",
                ));
            }
            return self.tasks.task(id);
        }
        if plan.instance_id != self.instance_id
            || plan.expires_at < now_ms()
            || plan.workspace_root != self.paths.workspace_root()
            || plan.workspace_identity != directory_identity(self.paths.workspace_root())?
        {
            return Err(ApiError::new(
                "CLEANUP_PLAN_EXPIRED",
                "The plan expired or its runtime/workspace changed; preview again",
            ));
        }
        if selected.iter().any(|id| {
            !plan
                .items
                .iter()
                .any(|item| &item.id == id && item.can_clean)
        }) {
            return Err(error("Selection includes an unknown or protected item"));
        }
        self.store.begin_workspace_cleanup()?;
        let guard = WorkspaceGuard(self.store.clone());
        if self.store.has_active_tasks() {
            return Err(ApiError::new(
                "WORKSPACE_BUSY",
                "Wait for active project operations before cleanup",
            ));
        }
        let task = self.tasks.create_workspace_operation("workspaceCleanup")?;
        plan.task_id = Some(task.task_id.clone());
        plan.selected_ids = selected_ids;
        plans.insert(plan.plan_id.clone(), plan.clone());
        drop(plans);
        *self.latest.lock().unwrap() = Some(task.task_id.clone());
        let manager = self.clone();
        let id = task.task_id.clone();
        let runner = tokio::spawn(async move {
            let worker = manager.clone();
            let task_id = id.clone();
            let failure_items = plan
                .items
                .iter()
                .filter(|item| plan.selected_ids.contains(&item.id))
                .map(Candidate::public)
                .collect::<Vec<_>>();
            let result = tokio::task::spawn_blocking(move || {
                let _guard = guard;
                worker.execute(&task_id, &plan)
            })
            .await
            .map_err(error)
            .and_then(|result| result);
            match result {
                Ok(value) if value["completed"] == true => {
                    let _ = manager.tasks.finish(&id, Ok(value));
                }
                Ok(value) => {
                    let _ = manager.tasks.finish_partial(
                        &id,
                        value,
                        ApiError::new(
                            "WORKSPACE_CLEANUP_INCOMPLETE",
                            "Some selected content was retained; inspect actual item results",
                        ),
                    );
                }
                Err(failure) => {
                    let value = json!({"completed":false,"error":failure,"items":failure_items});
                    let _ = manager.tasks.finish_partial(&id, value, failure);
                }
            }
        });
        running.push(runner);
        Ok(task)
    }
    pub(crate) fn latest_task(&self) -> Result<Option<TaskRecord>, ApiError> {
        self.latest
            .lock()
            .unwrap()
            .as_ref()
            .map(|id| self.tasks.task(id))
            .transpose()
    }
    pub(crate) async fn forget_deleted_project(
        &self,
        id: &str,
        source_ids: &[String],
        roots: &[PathBuf],
    ) -> Result<Vec<String>, ApiError> {
        if !(id.starts_with("prj_") && valid_id(id)
            || Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id))
            || source_ids
                .iter()
                .any(|source| !valid_id(source) || !source.starts_with("src_"))
            || roots.iter().any(|root| !normalized(root))
        {
            return Err(error("Invalid project cleanup ownership"));
        }
        self.deleting.lock().unwrap().insert(id.into());
        let running = std::mem::take(&mut *self.running.lock().unwrap());
        let issues = crate::shutdown::join_all("history-cleanup-deletion", running).await;
        if !issues.is_empty() {
            return Err(error("History inspection or cleanup has not stopped")
                .detail("issues", json!(issues)));
        }
        // Only typed candidate ownership and known scope paths are inspected.
        // Nested strings in another project's registration are never references.
        let mut ids = vec![];
        let mut plan_ids = vec![];
        for (plan_id, plan) in self.plans.lock().unwrap().iter() {
            let matches = plan.items.iter().any(|item| {
                deleted_scope_matches(
                    item.project_id.as_deref(),
                    &item.id,
                    &item.path,
                    id,
                    source_ids,
                    roots,
                )
            });
            if matches {
                plan_ids.push(plan_id.clone());
                ids.push(plan.preview_task_id.clone());
                if let Some(task) = &plan.task_id {
                    ids.push(task.clone());
                }
            }
        }
        for task in self.tasks.history_cleanup_tasks() {
            if task
                .result
                .and_then(|result| serde_json::from_value::<DerivedCleanupResult>(result).ok())
                .is_some_and(|result| {
                    result.items.iter().any(|item| {
                        normalized(&item.path)
                            && deleted_scope_matches(
                                item.project_id.as_deref(),
                                &item.id,
                                &item.path,
                                id,
                                source_ids,
                                roots,
                            )
                    })
                })
            {
                ids.push(task.task_id);
            }
        }
        ids.sort();
        ids.dedup();
        crate::workspace::local_cleanup_roots(&self.paths.selected_for_cleanup().map_err(error)?)?;
        self.tasks.forget_owned_tasks(&ids)?;
        self.plans
            .lock()
            .unwrap()
            .retain(|key, _| !plan_ids.contains(key));
        let mut latest = self.latest.lock().unwrap();
        if latest.as_ref().is_some_and(|task| ids.contains(task)) {
            *latest = None;
        }
        Ok(ids)
    }
    pub(crate) fn finish_deleted_project(&self, id: &str) {
        self.deleting.lock().unwrap().remove(id);
    }
    pub(crate) fn request_stop(&self) {
        let _submission = self.running.lock().unwrap();
        self.stopping.store(true, Ordering::Release);
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.request_stop();
        let runners = std::mem::take(&mut *self.running.lock().unwrap());
        crate::shutdown::join_all("historyCleanup", runners).await
    }
    fn candidates(&self) -> Result<Vec<Candidate>, ApiError> {
        let mut items = vec![];
        // Migration sources are finalized automatically after a verified
        // reopen, with retries bound to their migration ID. History owns only
        // completed source/data restore originals.
        self.restore_candidates(&mut items)?;
        items.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.id.cmp(&b.id)));
        Ok(items)
    }
    /// Retained originals of finished Git data/source restores. Unfinished
    /// operations are listed as protected so the user can see why they stay.
    fn restore_candidates(&self, items: &mut Vec<Candidate>) -> Result<(), ApiError> {
        let workspace = self.paths.workspace_root();
        for scope in ["data", "source"] {
            let projects = if scope == "data" {
                crate::data_restore_guard::operation_projects(workspace)?
            } else {
                crate::source_restore_guard::operation_projects(workspace)?
            };
            for project in projects {
                let name = self
                    .store
                    .project(&project)
                    .ok()
                    .map(|handle| handle.manifest.name.clone());
                let directory = workspace
                    .join(format!(".platform/project-state/{scope}-restore"))
                    .join(&project);
                let mut tasks = vec![];
                for entry in fs::read_dir(&directory)? {
                    let entry = entry?;
                    if let Some(task) = entry.file_name().to_str() {
                        if task.starts_with("task_") && valid_id(task) {
                            tasks.push((task.to_owned(), entry.path()));
                        }
                    }
                }
                tasks.sort();
                for (task, path) in tasks {
                    let label = if scope == "data" {
                        "数据取回前的原件"
                    } else {
                        "源码取回前的原件"
                    };
                    let mut item = Candidate::new(
                        format!("{scope}restore_{task}"),
                        "restoreOriginal",
                        label,
                        path,
                    );
                    item.project_id = Some(project.clone());
                    item.name = name.clone();
                    self.inspect(&mut item, |item| {
                        self.validate_restore(item, scope, &project)
                    });
                    items.push(item);
                }
            }
        }
        Ok(())
    }
    fn validate_restore(
        &self,
        item: &mut Candidate,
        scope: &str,
        project: &str,
    ) -> Result<(), ApiError> {
        let workspace = self.paths.workspace_root();
        let terminal = if scope == "data" {
            crate::data_restore_guard::terminal_operations(workspace, project)
        } else {
            crate::source_restore_guard::terminal_operations(workspace, project)
        }
        .map_err(|_| protected("这个项目还有未完成或无法核对的取回操作，全部取回现场继续保留"))?;
        if !terminal.contains(&item.path) {
            return Err(protected("这次取回尚未结束，原件仍用于继续或撤回"));
        }
        self.check_references(Some(project), None)
            .map_err(|_| protected("这个项目还有未完成的操作，取回现场继续保留"))
    }
    fn inspect(
        &self,
        item: &mut Candidate,
        validate: impl FnOnce(&mut Candidate) -> Result<(), ApiError>,
    ) {
        let result = (|| {
            self.validate_location(item)?;
            item.root_identity = Some(directory_identity(&item.path)?);

            item.parents = parent_identities(&item.path)?;
            item.entries = scan(&item.path)?;
            item.total_bytes = Some(item.entries.iter().try_fold(0u64, |total, entry| {
                total.checked_add(entry.bytes).ok_or_else(|| {
                    error("Cleanup logical byte total exceeds the supported integer range")
                })
            })?);
            item.file_count = Some(
                item.entries
                    .iter()
                    .filter(|entry| entry.kind != "directory")
                    .count() as u64,
            );
            validate(item)
        })();
        match result {
            Ok(()) => item.can_clean = true,
            Err(failure) => item
                .blocked_reasons
                .push(format!("{}: {}", failure.code, failure.message)),
        }
    }
    fn validate_location(&self, item: &Candidate) -> Result<(), ApiError> {
        let path = &item.path;
        if !normalized(path)
            || path.parent().is_none()
            || path
                .components()
                .any(|part| part.as_os_str().to_string_lossy().ends_with(".app"))
        {
            return Err(error("Unsafe cleanup location"));
        }
        let workspace = self.paths.workspace_root();
        let allowed = match item.kind.as_str() {
            "restoreOriginal" => [
                workspace.join(".platform/project-state/data-restore"),
                workspace.join(".platform/project-state/source-restore"),
            ],
            _ => return Err(error("Unknown historical cleanup category")),
        };
        if !allowed
            .iter()
            .any(|root| path.starts_with(root) && path != root)
        {
            return Err(error("Payload is outside its recorded owner directory"));
        }
        check_no_symlinks(path, false)
    }
    fn check_references(
        &self,
        project_id: Option<&str>,
        except: Option<&str>,
    ) -> Result<(), ApiError> {
        match project_id {
            Some(id) => crate::project_git::check_data_operations_available(
                self.paths.workspace_root(),
                id,
            )?,
            None => {
                crate::project_git::check_data_workspace_available(self.paths.workspace_root())?
            }
        }
        match project_id {
            Some(id) => crate::source_restore_guard::check_development_available(
                self.paths.workspace_root(),
                id,
            )?,
            None => {
                crate::source_restore_guard::check_workspace_available(self.paths.workspace_root())?
            }
        }
        match project_id {
            Some(id) => crate::data_restore_guard::check_development_available(
                self.paths.workspace_root(),
                id,
            )?,
            None => {
                crate::data_restore_guard::check_workspace_available(self.paths.workspace_root())?
            }
        }
        let operations =
            match fs::read_dir(self.paths.workspace_root().join(".platform/operations")) {
                Ok(entries) => Some(entries),
                Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => None,
                Err(failure) => return Err(failure.into()),
            };
        for directory in operations.into_iter().flatten() {
            let directory = directory?;
            if directory.file_name().to_str() == except {
                continue;
            }
            for file in ["journal.json"] {
                let path = directory.path().join(file);
                if !path.try_exists()? {
                    continue;
                }
                let journal = read_json(&path)?;
                if journal["resolved"] != true
                    && project_id.is_none_or(|id| {
                        journal["projectId"] == id || journal["sourceProjectId"] == id
                    })
                {
                    return Err(error(
                        "An unresolved operation still protects this historical content",
                    ));
                }
            }
        }
        Ok(())
    }
    fn execute(&self, task: &str, plan: &Plan) -> Result<Value, ApiError> {
        self.tasks.phase(task, "revalidating")?;
        let fresh = self.candidates()?;
        let mut selected = vec![];
        for id in &plan.selected_ids {
            let planned = plan
                .items
                .iter()
                .find(|item| &item.id == id)
                .ok_or_else(|| error("Unknown planned item"))?;
            let current = fresh
                .iter()
                .find(|item| &item.id == id)
                .ok_or_else(|| error("Planned content disappeared; preview again"))?;
            if !current.can_clean
                || current.path != planned.path
                || current.root_identity != planned.root_identity
                || current.parents != planned.parents
                || current.entries != planned.entries
                || current.guards != planned.guards
            {
                return Err(ApiError::new("CLEANUP_PLAN_CHANGED", "Selected content or protection evidence changed after preview; no deletion started").detail("itemId", id.clone()).detail("blockedReasons",json!(current.blocked_reasons)));
            }
            selected.push(planned);
        }
        let mut results = vec![];
        let mut stopped = false;
        for item in selected {
            if stopped {
                results.push(json!({"id":item.id,"projectId":item.project_id,"kind":item.kind,"path":item.path,"status":"skipped","completed":false,"reason":"A preceding item needs attention"}));
                continue;
            }
            self.tasks.phase(task, &format!("cleaning-{}", item.kind))?;
            let operation = remove_tree(item);
            let result = match operation {
                Ok(removed) => {
                    json!({"id":item.id,"projectId":item.project_id,"kind":item.kind,"path":item.path,"status":"removed","completed":true,"removed":removed,"completedAt":now_ms()})
                }
                Err(failure) => {
                    stopped = true;
                    json!({"id":item.id,"projectId":item.project_id,"kind":item.kind,"path":item.path,"status":"failed","completed":false,"error":failure})
                }
            };
            results.push(result);
        }
        let result = json!({"planId":plan.plan_id,"workspaceRoot":plan.workspace_root,"completed":!stopped,"items":results,"completedAt":now_ms()});
        Ok(result)
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
fn remove_tree(item: &Candidate) -> Result<Value, ApiError> {
    remove_tree_with_hook(item, |_, _| Ok(()))
}

/// Explicit permanent deletion shares the descriptor-relative removal boundary
/// with historical cleanup, without enrolling content in any recycle facility.
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

/// A migration validates the content-hashed snapshot that will be removed,
/// while permanent project deletion needs only its already accepted ownership.
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

/// No cleanup history is kept. Absence is reported only while the original
/// filesystem is available; a disconnected disk is never treated as cleaned.
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
