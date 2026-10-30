//! Host-owned application window preferences. These confer no browser permissions.
use crate::{projects::check_no_symlinks, ApiError};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

pub const MIN_WIDTH: f64 = 640.0;
pub const MIN_HEIGHT: f64 = 480.0;
const MAX_CACHE_BYTES: u64 = 4096;
const MAX_ENTRIES: usize = 4096;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppWindowConfig {
    pub width: u32,
    pub height: u32,
    #[serde(default = "remember_size")]
    pub remember_size: bool,
}
fn remember_size() -> bool {
    true
}
impl Default for AppWindowConfig {
    fn default() -> Self {
        Self {
            width: 1080,
            height: 780,
            remember_size: true,
        }
    }
}
impl AppWindowConfig {
    pub fn valid(&self) -> bool {
        self.width > 0 && self.height > 0 && self.width <= 32768 && self.height <= 32768
    }

    pub fn from_value(value: Option<&serde_json::Value>) -> Result<Self, ApiError> {
        let Some(value) = value else {
            return Ok(Self::default());
        };
        let config: Self = serde_json::from_value(value.clone())
            .map_err(|_| ApiError::new("INVALID_APP_WINDOW", "应用窗口尺寸声明无效。"))?;
        if !config.valid() {
            return Err(ApiError::new(
                "INVALID_APP_WINDOW",
                "应用窗口尺寸声明无效。",
            ));
        }
        Ok(config)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LogicalWindowSize {
    pub width: f64,
    pub height: f64,
}
impl LogicalWindowSize {
    pub fn valid(self) -> bool {
        self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.0
            && self.height > 0.0
            && self.width <= 32768.0
            && self.height <= 32768.0
    }

    pub fn from_physical(width: u32, height: u32, scale: f64) -> Option<Self> {
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let size = Self {
            width: f64::from(width) / scale,
            height: f64::from(height) / scale,
        };
        size.valid().then_some(size)
    }
}

/// Keep normal content dimensions inside the monitor work area, leaving room
/// for native decorations. The existing application minimum remains in force.
pub fn fit_window_size(
    requested: LogicalWindowSize,
    screen: Option<LogicalWindowSize>,
) -> LogicalWindowSize {
    let available = screen
        .filter(|size| size.valid())
        .unwrap_or(LogicalWindowSize {
            width: 1112.0,
            height: 844.0,
        });
    let maximum_width = (available.width - 32.0).max(MIN_WIDTH);
    let maximum_height = (available.height - 64.0).max(MIN_HEIGHT);
    let requested = if requested.valid() {
        requested
    } else {
        LogicalWindowSize {
            width: 1080.0,
            height: 780.0,
        }
    };
    LogicalWindowSize {
        width: requested.width.clamp(MIN_WIDTH, maximum_width),
        height: requested.height.clamp(MIN_HEIGHT, maximum_height),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
enum ProjectKind {
    Web,
    Node,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WindowIdentity {
    kind: ProjectKind,
    id: String,
}
impl WindowIdentity {
    /// Project IDs are immutable across public workspace migration. The root is
    /// accepted at this call boundary but no longer part of preference identity.
    pub fn project(_workspace: &Path, id: &str) -> Self {
        Self {
            kind: ProjectKind::Web,
            id: id.into(),
        }
    }
    pub fn node(id: &str) -> Self {
        Self {
            kind: ProjectKind::Node,
            id: id.into(),
        }
    }
    fn checked(kind: &str, id: &str) -> Result<Self, ApiError> {
        let identity = match kind {
            "web" => Self::project(Path::new("/"), id),
            "node" => Self::node(id),
            _ => return Err(invalid("窗口尺寸项目类型无效。")),
        };
        identity.validate()?;
        Ok(identity)
    }
    fn validate(&self) -> Result<(), ApiError> {
        let valid = match self.kind {
            ProjectKind::Web => self.id.starts_with("prj_") && crate::projects::valid_id(&self.id),
            ProjectKind::Node => uuid::Uuid::parse_str(&self.id)
                .is_ok_and(|id| !id.is_nil() && id.to_string() == self.id),
        };
        if valid {
            Ok(())
        } else {
            Err(invalid("窗口尺寸项目标识无效。"))
        }
    }
    pub fn matches_project(&self, kind: &str, id: &str) -> bool {
        self.id == id
            && matches!(
                (self.kind, kind),
                (ProjectKind::Web, "web") | (ProjectKind::Node, "node")
            )
    }
}

/// A single window generation. Revocation remains attached to old callback
/// clones even after the project's cache and temporary tombstone are removed.
#[derive(Clone)]
pub struct WindowSizePermit {
    identity: WindowIdentity,
    live: Arc<AtomicBool>,
}
impl WindowSizePermit {
    pub fn revoke(&self) {
        self.live.store(false, Ordering::Release);
    }
    pub fn is_revoked(&self) -> bool {
        !self.live.load(Ordering::Acquire)
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cache {
    schema_version: u32,
    kind: ProjectKind,
    project_id: String,
    size: LogicalWindowSize,
}
#[derive(Clone)]
struct Snapshot {
    bytes: Vec<u8>,
    identity: crate::file_identity::PersistedIdentity,
}
struct Entry {
    saved: Option<LogicalWindowSize>,
    previous: Option<Snapshot>,
    writable: bool,
    dirty: bool,
    permit: Option<Arc<AtomicBool>>,
}
struct Inner {
    entries: BTreeMap<WindowIdentity, Entry>,
    revoked: BTreeSet<WindowIdentity>,
    warning: Option<String>,
}
pub struct WindowSizeStore {
    data_root: PathBuf,
    data_identity: Option<crate::file_identity::PersistedIdentity>,
    inner: Mutex<Inner>,
}
fn invalid(message: &str) -> ApiError {
    ApiError::new("INVALID_WINDOW_CACHE", message)
}
impl WindowSizeStore {
    /// Only current per-project preference files are read, lazily. No global
    /// cache, old preference history or legacy project path is adopted.
    pub fn new(data_root: &Path) -> Self {
        Self {
            data_root: data_root.into(),
            data_identity: crate::file_identity::PersistedIdentity::capture(data_root).ok(),
            inner: Mutex::new(Inner {
                entries: BTreeMap::new(),
                revoked: BTreeSet::new(),
                warning: None,
            }),
        }
    }
    pub fn warning(&self) -> Option<String> {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .warning
            .clone()
    }
    fn owner(&self, identity: &WindowIdentity) -> Result<PathBuf, ApiError> {
        identity.validate()?;
        check_no_symlinks(&self.data_root, false)?;
        if !self
            .data_identity
            .as_ref()
            .is_some_and(|expected| expected.matches_path(&self.data_root).unwrap_or(false))
        {
            return Err(invalid("窗口尺寸数据根身份已变化。"));
        }
        Ok(self
            .data_root
            .join(match identity.kind {
                ProjectKind::Web => "web-project-ownership",
                ProjectKind::Node => "node-project-ownership",
            })
            .join(&identity.id))
    }
    fn writable_owner(&self, identity: &WindowIdentity) -> Result<PathBuf, ApiError> {
        let owner = self.owner(identity)?;
        private_directory(&owner)?;
        // Never recreate an owner from a late window callback. Its current
        // ownership index must still exist before any preference publication.
        open_private(&owner.join("ownership.json"))?;
        Ok(owner)
    }
    fn load(&self, identity: &WindowIdentity) -> Result<Entry, ApiError> {
        let owner = self.writable_owner(identity)?;
        let previous = read_snapshot(&owner.join("window-size.json"))?;
        let saved = previous
            .as_ref()
            .map(|snapshot| {
                let cache: Cache = serde_json::from_slice(&snapshot.bytes)
                    .map_err(|_| invalid("本项目窗口尺寸缓存无效，使用默认尺寸。"))?;
                if cache.schema_version != 1
                    || cache.kind != identity.kind
                    || cache.project_id != identity.id
                    || !cache.size.valid()
                {
                    return Err(invalid("本项目窗口尺寸缓存身份或数值无效，使用默认尺寸。"));
                }
                Ok(cache.size)
            })
            .transpose()?;
        Ok(Entry {
            saved,
            previous,
            writable: true,
            dirty: false,
            permit: None,
        })
    }
    fn ensure_entry(&self, inner: &mut Inner, identity: &WindowIdentity) {
        if inner.entries.contains_key(identity) || inner.revoked.contains(identity) {
            return;
        }
        if inner.entries.len() >= MAX_ENTRIES {
            inner.warning = Some("窗口尺寸内存缓存已达上限，本次使用默认尺寸。".into());
            return;
        }
        let entry = match self.load(identity) {
            Ok(entry) => entry,
            Err(error) => {
                inner.warning = Some(error.message);
                // In particular, a late initial-size callback for an absent
                // owner must not recreate even an empty in-memory ID record.
                return;
            }
        };
        inner.entries.insert(identity.clone(), entry);
    }
    pub fn initial_size(
        &self,
        identity: &WindowIdentity,
        config: AppWindowConfig,
        screen: Option<LogicalWindowSize>,
    ) -> LogicalWindowSize {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if config.remember_size {
            self.ensure_entry(&mut inner, identity);
        }
        let saved = if config.remember_size && !inner.revoked.contains(identity) {
            inner.entries.get(identity).and_then(|entry| entry.saved)
        } else {
            None
        };
        fit_window_size(
            saved.unwrap_or(LogicalWindowSize {
                width: f64::from(config.width),
                height: f64::from(config.height),
            }),
            screen,
        )
    }
    pub fn permit(&self, identity: &WindowIdentity) -> Result<WindowSizePermit, ApiError> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if inner.revoked.contains(identity) {
            return Err(invalid("项目窗口尺寸写入已撤销。"));
        }
        self.writable_owner(identity)?;
        self.ensure_entry(&mut inner, identity);
        let entry = inner
            .entries
            .get_mut(identity)
            .filter(|entry| entry.writable)
            .ok_or_else(|| invalid("本项目窗口尺寸缓存不可写。"))?;
        if let Some(previous) = entry.permit.take() {
            previous.store(false, Ordering::Release);
        }
        let live = Arc::new(AtomicBool::new(true));
        entry.permit = Some(live.clone());
        Ok(WindowSizePermit {
            identity: identity.clone(),
            live,
        })
    }
    pub fn remember(
        &self,
        permit: &WindowSizePermit,
        config: AppWindowConfig,
        size: LogicalWindowSize,
    ) {
        if !config.remember_size
            || !size.valid()
            || size.width < MIN_WIDTH
            || size.height < MIN_HEIGHT
            || permit.is_revoked()
        {
            return;
        }
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if inner.revoked.contains(&permit.identity) || permit.is_revoked() {
            return;
        }
        let Some(entry) = inner.entries.get_mut(&permit.identity) else {
            return;
        };
        if !entry.writable
            || !entry
                .permit
                .as_ref()
                .is_some_and(|live| Arc::ptr_eq(live, &permit.live))
            || entry.saved == Some(size)
        {
            return;
        }
        entry.saved = Some(size);
        entry.dirty = true;
    }
    pub fn flush(&self) -> Result<(), ApiError> {
        self.flush_with(|| {})
    }
    fn flush_with(&self, mut before_publish: impl FnMut()) -> Result<(), ApiError> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut first = None;
        for (identity, entry) in &mut inner.entries {
            if !entry.writable
                || !entry.dirty
                || entry
                    .permit
                    .as_ref()
                    .is_none_or(|live| !live.load(Ordering::Acquire))
            {
                continue;
            }
            let result = (|| {
                let owner = self.writable_owner(identity)?;
                let bytes = serde_json::to_vec(&Cache {
                    schema_version: 1,
                    kind: identity.kind,
                    project_id: identity.id.clone(),
                    size: entry
                        .saved
                        .ok_or_else(|| invalid("窗口尺寸缓存缺少数值。"))?,
                })?;
                publish_cache(&owner, entry.previous.as_ref(), &bytes, &mut before_publish)
            })();
            match result {
                Ok(snapshot) => {
                    entry.previous = Some(snapshot);
                    entry.dirty = false;
                }
                Err(error) => {
                    entry.writable = false;
                    if first.is_none() {
                        first = Some(error);
                    }
                }
            }
        }
        if let Some(error) = first {
            inner.warning = Some(error.message.clone());
            Err(error)
        } else {
            Ok(())
        }
    }
    /// Synchronous revocation serializes with remember/flush. The core invokes
    /// this before removing the owner, including CLI and deletion recovery.
    pub fn forget_project(&self, kind: &str, id: &str) -> Result<(), ApiError> {
        let identity = WindowIdentity::checked(kind, id)?;
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.revoked.insert(identity.clone());
        if let Some(entry) = inner.entries.remove(&identity) {
            if let Some(live) = entry.permit {
                live.store(false, Ordering::Release);
            }
        }
        let owner = self.owner(&identity)?;
        match fs::symlink_metadata(&owner) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
            Ok(_) => (),
        }
        private_directory(&owner)?;
        let targets = fs::read_dir(&owner)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()?;
        for path in targets {
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if name == "window-size.json" || name.starts_with(".window-size-") {
                let file = open_private(&path)?;
                let expected = crate::file_identity::PersistedIdentity::from_file(&file)
                    .map_err(|_| invalid("窗口尺寸缓存身份无法核验。"))?;
                retire_file(&owner, path.file_name().unwrap(), &expected, None)?;
            }
        }
        Ok(())
    }
    /// Called only after the complete owner directory is absent. Old callback
    /// clones stay revoked through their own permits, without an ID history.
    pub fn finish_forget(&self, kind: &str, id: &str) -> Result<(), ApiError> {
        let identity = WindowIdentity::checked(kind, id)?;
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let owner = self.owner(&identity)?;
        match fs::symlink_metadata(owner) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Some(entry) = inner.entries.remove(&identity) {
                    if let Some(live) = entry.permit {
                        live.store(false, Ordering::Release);
                    }
                }
                inner.revoked.remove(&identity);
                Ok(())
            }
            Err(error) => Err(error.into()),
            Ok(_) => Err(invalid("项目归属目录尚未清完，窗口尺寸写入仍保持撤销。")),
        }
    }
}

use crate::window_cache_io::{open_private, private_directory};
fn read_snapshot(path: &Path) -> Result<Option<Snapshot>, ApiError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => (),
    }
    let file = open_private(path)?;
    if file.metadata()?.len() > MAX_CACHE_BYTES {
        return Err(invalid("本项目窗口尺寸缓存超出上限。"));
    }
    let identity = crate::file_identity::PersistedIdentity::from_file(&file)
        .map_err(|_| invalid("窗口尺寸缓存身份无法核验。"))?;
    let mut bytes = Vec::new();
    file.take(MAX_CACHE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return Err(invalid("本项目窗口尺寸缓存超出上限。"));
    }
    Ok(Some(Snapshot { bytes, identity }))
}
fn same_snapshot(left: Option<&Snapshot>, right: Option<&Snapshot>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.bytes == right.bytes && left.identity.same_runtime_object(&right.identity)
        }
        _ => false,
    }
}
#[cfg(target_os = "macos")]
fn publish_cache(
    owner: &Path,
    previous: Option<&Snapshot>,
    bytes: &[u8],
    before_publish: &mut impl FnMut(),
) -> Result<Snapshot, ApiError> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::OpenOptionsExt,
        },
    };
    let path = owner.join("window-size.json");
    if !same_snapshot(read_snapshot(&path)?.as_ref(), previous) {
        return Err(ApiError::new(
            "WINDOW_CACHE_CHANGED",
            "本项目窗口尺寸缓存已被外部修改，已停止写入。",
        ));
    }
    private_directory(owner)?;
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(owner)?;
    let name = format!(".window-size-write-{}.json", uuid::Uuid::new_v4());
    let temporary = owner.join(&name);
    let from = CString::new(name).unwrap();
    let to = CString::new("window-size.json").unwrap();
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            from.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut pending = unsafe { fs::File::from_raw_fd(descriptor) };
    pending.write_all(bytes)?;
    pending.sync_all()?;
    let identity = crate::file_identity::PersistedIdentity::from_file(&pending)
        .map_err(|_| invalid("窗口尺寸暂存身份无法核验。"))?;
    before_publish();
    if unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            from.as_ptr(),
            directory.as_raw_fd(),
            to.as_ptr(),
            if previous.is_some() {
                libc::RENAME_SWAP
            } else {
                libc::RENAME_EXCL
            },
        )
    } != 0
    {
        return Err(ApiError::new(
            "WINDOW_CACHE_PUBLISH_FAILED",
            "窗口尺寸缓存未发布，暂存仍在本项目控制目录。",
        )
        .detail("conflictPath", temporary.to_string_lossy().into_owned()));
    }
    directory.sync_all()?;
    let current = read_snapshot(&path)?.ok_or_else(|| invalid("窗口尺寸缓存发布结果缺失。"))?;
    if current.bytes != bytes || !identity.same_runtime_object(&current.identity) {
        return Err(ApiError::new(
            "WINDOW_CACHE_CHANGED",
            "窗口尺寸缓存发布时发生外部变化，现场保留在本项目控制目录。",
        ));
    }
    if let Some(previous) = previous {
        let displaced = read_snapshot(&temporary)?;
        if !same_snapshot(displaced.as_ref(), Some(previous)) {
            return Err(ApiError::new(
                "WINDOW_CACHE_CHANGED",
                "外部窗口尺寸文件已换出保留，本项目停止继续写入。",
            )
            .detail("conflictPath", temporary.to_string_lossy().into_owned()));
        }
        retire_file(
            owner,
            temporary.file_name().unwrap(),
            &previous.identity,
            Some(&previous.bytes),
        )?;
    }
    Ok(current)
}
#[cfg(not(target_os = "macos"))]
fn publish_cache(
    _: &Path,
    _: Option<&Snapshot>,
    _: &[u8],
    _: &mut impl FnMut(),
) -> Result<Snapshot, ApiError> {
    Err(ApiError::new(
        "WINDOW_CACHE_UNSUPPORTED",
        "当前系统尚未启用原子窗口尺寸缓存发布。",
    ))
}

#[cfg(target_os = "macos")]
fn retire_file(
    owner: &Path,
    name: &std::ffi::OsStr,
    expected: &crate::file_identity::PersistedIdentity,
    bytes: Option<&[u8]>,
) -> Result<(), ApiError> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::OpenOptionsExt},
        },
    };
    private_directory(owner)?;
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(owner)?;
    let from = CString::new(name.as_bytes()).map_err(|_| invalid("窗口尺寸文件名无效。"))?;
    let to = CString::new(format!(
        ".window-size-retiring-{}.json",
        uuid::Uuid::new_v4()
    ))
    .unwrap();
    if unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            from.as_ptr(),
            directory.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let result = (|| {
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                to.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if descriptor < 0 {
            return Err(invalid("窗口尺寸移除身份无法核验。"));
        }
        let file = unsafe { fs::File::from_raw_fd(descriptor) };
        let observed = crate::file_identity::PersistedIdentity::from_file(&file)
            .map_err(|_| invalid("窗口尺寸移除身份无法核验。"))?;
        if !expected.same_runtime_object(&observed) {
            return Err(invalid("窗口尺寸文件身份已变化，未删除。"));
        }
        if let Some(expected_bytes) = bytes {
            let mut current = Vec::new();
            file.take(MAX_CACHE_BYTES + 1).read_to_end(&mut current)?;
            if current != expected_bytes {
                return Err(invalid("窗口尺寸文件内容已变化，未删除。"));
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        unsafe {
            libc::renameatx_np(
                directory.as_raw_fd(),
                to.as_ptr(),
                directory.as_raw_fd(),
                from.as_ptr(),
                libc::RENAME_EXCL,
            );
        }
        return Err(error);
    }
    if unsafe { libc::unlinkat(directory.as_raw_fd(), to.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    directory.sync_all()?;
    Ok(())
}
#[cfg(not(target_os = "macos"))]
fn retire_file(
    _: &Path,
    _: &std::ffi::OsStr,
    _: &crate::file_identity::PersistedIdentity,
    _: Option<&[u8]>,
) -> Result<(), ApiError> {
    Err(ApiError::new(
        "WINDOW_CACHE_UNSUPPORTED",
        "当前系统尚未启用窗口尺寸缓存清理。",
    ))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    fn config() -> AppWindowConfig {
        AppWindowConfig::default()
    }
    fn size(width: f64, height: f64) -> LogicalWindowSize {
        LogicalWindowSize { width, height }
    }
    fn screen() -> Option<LogicalWindowSize> {
        Some(size(2400.0, 1800.0))
    }
    fn fixture() -> (tempfile::TempDir, PathBuf, WindowSizeStore) {
        let temporary = tempfile::Builder::new()
            .prefix("disposable-project-window-size-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let store = WindowSizeStore::new(&root);
        (temporary, root, store)
    }
    fn owner(root: &Path, identity: &WindowIdentity) -> PathBuf {
        let store = WindowSizeStore::new(root);
        let owner = store.owner(identity).unwrap();
        fs::create_dir_all(&owner).unwrap();
        for path in [owner.parent().unwrap(), &owner] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(owner.join("ownership.json"), b"{}").unwrap();
        fs::set_permissions(
            owner.join("ownership.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        owner
    }
    fn web(id: &str) -> WindowIdentity {
        WindowIdentity::project(Path::new("/disposable/workspace"), id)
    }
    fn node() -> WindowIdentity {
        WindowIdentity::node("10000000-0000-4000-8000-000000000001")
    }
    #[test]
    fn declaration_and_monitor_fitting_preserve_existing_normal_size_rules() {
        assert_eq!(AppWindowConfig::from_value(None).unwrap(), config());
        assert!(!AppWindowConfig {
            width: 0,
            ..config()
        }
        .valid());
        assert!(AppWindowConfig::from_value(Some(
            &serde_json::json!({"width":1000,"height":700,"unknown":true})
        ))
        .is_err());
        assert_eq!(
            LogicalWindowSize::from_physical(1800, 1200, 2.0),
            Some(size(900.0, 600.0))
        );
        assert!(LogicalWindowSize::from_physical(1, 1, f64::NAN).is_none());
        assert_eq!(
            fit_window_size(size(3000.0, 3000.0), Some(size(1000.0, 800.0))),
            size(968.0, 736.0)
        );
        assert_eq!(
            fit_window_size(size(1.0, 1.0), screen()),
            size(MIN_WIDTH, MIN_HEIGHT)
        );
    }
    #[test]
    fn per_project_files_remain_isolated_and_web_identity_survives_workspace_migration() {
        let (_temp, root, store) = fixture();
        let a = web("prj_window_a");
        let b = web("prj_window_b");
        let n = node();
        for id in [&a, &b, &n] {
            owner(&root, id);
        }
        for (id, value) in [
            (&a, size(900.0, 600.0)),
            (&b, size(1200.0, 850.0)),
            (&n, size(800.0, 700.0)),
        ] {
            let permit = store.permit(id).unwrap();
            store.remember(&permit, config(), value);
        }
        store.flush().unwrap();
        let reopened = WindowSizeStore::new(&root);
        assert_eq!(
            reopened.initial_size(&a, config(), screen()),
            size(900.0, 600.0)
        );
        assert_eq!(
            reopened.initial_size(&b, config(), screen()),
            size(1200.0, 850.0)
        );
        assert_eq!(
            reopened.initial_size(&n, config(), screen()),
            size(800.0, 700.0)
        );
        let migrated =
            WindowIdentity::project(Path::new("/disposable/new-workspace"), "prj_window_a");
        assert_eq!(migrated, a);
        assert_eq!(
            reopened.initial_size(&migrated, config(), screen()),
            size(900.0, 600.0)
        );
        assert!(!root.join("app-window-sizes.json").exists());
    }
    #[test]
    fn consecutive_saves_keep_only_current_preference_without_history() {
        let (_temp, root, store) = fixture();
        let id = node();
        let owner = owner(&root, &id);
        let permit = store.permit(&id).unwrap();
        for step in 0..30 {
            store.remember(&permit, config(), size(900.0 + f64::from(step), 650.0));
            store.flush().unwrap();
            assert_eq!(fs::read_dir(&owner).unwrap().count(), 2);
        }
        assert_eq!(
            fs::metadata(owner.join("window-size.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            WindowSizeStore::new(&root).initial_size(&id, config(), screen()),
            size(929.0, 650.0)
        );
        assert!(fs::read_dir(root).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("preserved")));
    }
    #[test]
    fn new_window_generation_revokes_old_callbacks_without_disturbing_other_projects() {
        let (_temp, root, store) = fixture();
        let a = web("prj_window_a");
        let b = web("prj_window_b");
        owner(&root, &a);
        owner(&root, &b);
        let old = store.permit(&a).unwrap();
        let b_permit = store.permit(&b).unwrap();
        let new = store.permit(&a).unwrap();
        assert!(old.is_revoked());
        store.remember(&new, config(), size(1000.0, 700.0));
        store.remember(&old, config(), size(800.0, 600.0));
        store.remember(&b_permit, config(), size(1100.0, 800.0));
        store.flush().unwrap();
        assert_eq!(
            store.initial_size(&a, config(), screen()),
            size(1000.0, 700.0)
        );
        assert_eq!(
            store.initial_size(&b, config(), screen()),
            size(1100.0, 800.0)
        );
    }
    #[test]
    fn forget_clears_disk_and_memory_and_late_callbacks_cannot_recreate_owner() {
        let (_temp, root, store) = fixture();
        let a = web("prj_window_a");
        let b = web("prj_window_b");
        let a_owner = owner(&root, &a);
        let b_owner = owner(&root, &b);
        let a_permit = store.permit(&a).unwrap();
        let late = a_permit.clone();
        let b_permit = store.permit(&b).unwrap();
        store.remember(&a_permit, config(), size(900.0, 650.0));
        store.remember(&b_permit, config(), size(1000.0, 750.0));
        store.flush().unwrap();
        store.remember(&a_permit, config(), size(950.0, 650.0));
        store.forget_project("web", "prj_window_a").unwrap();
        assert!(late.is_revoked());
        assert!(!a_owner.join("window-size.json").exists());
        assert!(store.permit(&a).is_err());
        store.remember(&late, config(), size(1200.0, 800.0));
        store.flush().unwrap();
        assert!(!a_owner.join("window-size.json").exists());
        assert!(store.finish_forget("web", "prj_window_a").is_err());
        fs::remove_dir_all(&a_owner).unwrap();
        store.finish_forget("web", "prj_window_a").unwrap();
        assert!(!store.inner.lock().unwrap().entries.contains_key(&a));
        assert!(store.inner.lock().unwrap().revoked.is_empty());
        store.remember(&late, config(), size(1200.0, 800.0));
        assert!(store.permit(&a).is_err());
        store.flush().unwrap();
        assert!(!a_owner.exists());
        assert_eq!(
            store.initial_size(&a, config(), screen()),
            size(1080.0, 780.0)
        );
        assert!(!store.inner.lock().unwrap().entries.contains_key(&a));
        assert!(b_owner.join("window-size.json").exists());
        assert_eq!(
            store.initial_size(&b, config(), screen()),
            size(1000.0, 750.0)
        );
    }
    #[test]
    fn invalid_single_project_cache_cannot_disable_another_project_and_global_legacy_is_ignored() {
        let (_temp, root, store) = fixture();
        let a = web("prj_window_a");
        let b = web("prj_window_b");
        let a_owner = owner(&root, &a);
        let b_owner = owner(&root, &b);
        fs::write(a_owner.join("window-size.json"), b"bad cache").unwrap();
        fs::set_permissions(
            a_owner.join("window-size.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        fs::write(root.join("app-window-sizes.json"), b"legacy is not read").unwrap();
        fs::write(
            root.join(".app-window-sizes-preserved-old.json"),
            b"legacy is not scanned",
        )
        .unwrap();
        assert_eq!(
            store.initial_size(&a, config(), screen()),
            size(1080.0, 780.0)
        );
        assert!(store.permit(&a).is_err());
        let permit = store.permit(&b).unwrap();
        store.remember(&permit, config(), size(1100.0, 800.0));
        store.flush().unwrap();
        assert!(b_owner.join("window-size.json").exists());
        assert_eq!(
            fs::read(a_owner.join("window-size.json")).unwrap(),
            b"bad cache"
        );
        assert_eq!(
            fs::read(root.join("app-window-sizes.json")).unwrap(),
            b"legacy is not read"
        );
    }
    #[test]
    fn remember_disabled_and_non_normal_sizes_do_not_create_preferences() {
        let (_temp, root, store) = fixture();
        let id = node();
        let owner = owner(&root, &id);
        let permit = store.permit(&id).unwrap();
        for value in [size(1.0, 1.0), size(f64::NAN, 600.0), size(0.0, 600.0)] {
            store.remember(&permit, config(), value);
        }
        store.remember(
            &permit,
            AppWindowConfig {
                remember_size: false,
                ..config()
            },
            size(900.0, 650.0),
        );
        store.flush().unwrap();
        assert!(!owner.join("window-size.json").exists());
    }
    #[test]
    fn cache_links_and_external_replacements_are_preserved_within_project_scope() {
        let (_temp, root, store) = fixture();
        let id = node();
        let owner = owner(&root, &id);
        let external = root.join("author-original");
        fs::write(&external, b"keep author file").unwrap();
        symlink(&external, owner.join("window-size.json")).unwrap();
        assert!(store.permit(&id).is_err());
        store.flush().unwrap();
        assert_eq!(fs::read(&external).unwrap(), b"keep author file");
    }
    #[test]
    fn atomic_publish_conflict_is_project_local_and_does_not_disable_other_projects() {
        let (_temp, root, store) = fixture();
        let a = web("prj_window_a");
        let b = web("prj_window_b");
        let a_owner = owner(&root, &a);
        let b_owner = owner(&root, &b);
        let a_permit = store.permit(&a).unwrap();
        let b_permit = store.permit(&b).unwrap();
        store.remember(&a_permit, config(), size(900.0, 650.0));
        store.flush().unwrap();
        store.remember(&a_permit, config(), size(950.0, 650.0));
        store.remember(&b_permit, config(), size(1100.0, 800.0));
        let error = store
            .flush_with(|| {
                let replacement = a_owner.join("external-replacement");
                fs::write(&replacement, b"preserve external cache replacement").unwrap();
                fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();
                fs::rename(replacement, a_owner.join("window-size.json")).unwrap();
            })
            .unwrap_err();
        let conflict = PathBuf::from(error.details["conflictPath"].as_str().unwrap());
        assert_eq!(conflict.parent(), Some(a_owner.as_path()));
        assert_eq!(
            fs::read(conflict).unwrap(),
            b"preserve external cache replacement"
        );
        assert!(b_owner.join("window-size.json").exists());
        store.forget_project("web", "prj_window_a").unwrap();
        assert_eq!(fs::read_dir(&a_owner).unwrap().count(), 1);
        assert!(b_owner.join("window-size.json").exists());
    }
}
