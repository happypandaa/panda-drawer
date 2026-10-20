//! Host-owned application window preferences. These confer no browser permissions.
use crate::{projects::check_no_symlinks, ApiError};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub const MIN_WIDTH: f64 = 640.0;
pub const MIN_HEIGHT: f64 = 480.0;
const MAX_CACHE_BYTES: u64 = 1024 * 1024;
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WindowIdentity(String);
impl WindowIdentity {
    pub fn project(workspace: &Path, id: &str) -> Self {
        Self(
            serde_json::to_string(&("project", workspace, id))
                .expect("Serializable window identity"),
        )
    }
    pub fn node(id: &str) -> Self {
        Self(serde_json::to_string(&("node", id)).expect("Serializable window identity"))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cache {
    schema_version: u32,
    sizes: BTreeMap<String, LogicalWindowSize>,
}
struct Inner {
    sizes: BTreeMap<String, LogicalWindowSize>,
    previous: Option<Vec<u8>>,
    writable: bool,
    dirty: bool,
    warning: Option<String>,
}
pub struct WindowSizeStore {
    path: PathBuf,
    inner: Mutex<Inner>,
}

impl WindowSizeStore {
    /// A malformed cache is optional UI state: preserve it and continue without
    /// writing over it. This never initializes or redirects a workspace.
    pub fn new(data_root: &Path) -> Self {
        let path = data_root.join("app-window-sizes.json");
        let mut inner = Inner {
            sizes: BTreeMap::new(),
            previous: None,
            writable: true,
            dirty: false,
            warning: None,
        };
        match read_cache(&path) {
            Ok(Some(bytes)) => {
                match serde_json::from_slice::<Cache>(&bytes) {
                    Ok(cache)
                        if cache.schema_version == 1
                            && cache.sizes.len() <= MAX_ENTRIES
                            && cache
                                .sizes
                                .iter()
                                .all(|(key, size)| key.len() <= 8192 && size.valid()) =>
                    {
                        inner.sizes = cache.sizes
                    }
                    _ => {
                        inner.writable = false;
                        inner.warning =
                            Some("窗口尺寸缓存无效，已保留原文件并使用应用默认尺寸。".into());
                    }
                }
                inner.previous = Some(bytes);
            }
            Ok(None) => {}
            Err(error) => {
                inner.writable = false;
                inner.warning = Some(format!("窗口尺寸缓存无法读取，已保留现场：{}", error.code));
            }
        }
        Self {
            path,
            inner: Mutex::new(inner),
        }
    }

    pub fn warning(&self) -> Option<String> {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .warning
            .clone()
    }

    pub fn initial_size(
        &self,
        identity: &WindowIdentity,
        config: AppWindowConfig,
        screen: Option<LogicalWindowSize>,
    ) -> LogicalWindowSize {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let saved = config
            .remember_size
            .then(|| inner.sizes.get(&identity.0))
            .flatten()
            .copied();
        fit_window_size(
            saved.unwrap_or(LogicalWindowSize {
                width: f64::from(config.width),
                height: f64::from(config.height),
            }),
            screen,
        )
    }

    pub fn remember(
        &self,
        identity: &WindowIdentity,
        config: AppWindowConfig,
        size: LogicalWindowSize,
    ) {
        if !config.remember_size
            || !size.valid()
            || size.width < MIN_WIDTH
            || size.height < MIN_HEIGHT
        {
            return;
        }
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if inner.sizes.get(&identity.0) == Some(&size)
            || (!inner.sizes.contains_key(&identity.0) && inner.sizes.len() >= MAX_ENTRIES)
        {
            return;
        }
        inner.sizes.insert(identity.0.clone(), size);
        inner.dirty = true;
    }

    pub fn flush(&self) -> Result<(), ApiError> {
        self.flush_with(|| {})
    }

    // The seam runs after the optimistic byte check but before the atomic
    // publication, so tests exercise the actual external-replacement race.
    fn flush_with(&self, before_publish: impl FnOnce()) -> Result<(), ApiError> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if !inner.writable || !inner.dirty {
            return Ok(());
        }
        if read_cache(&self.path)? != inner.previous {
            inner.writable = false;
            return Err(ApiError::new(
                "WINDOW_CACHE_CHANGED",
                "窗口尺寸缓存已被外部修改，已保留现场。",
            ));
        }
        let cache = Cache {
            schema_version: 1,
            sizes: inner.sizes.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&cache)?;
        if bytes.len() as u64 > MAX_CACHE_BYTES {
            return Err(ApiError::new(
                "WINDOW_CACHE_FULL",
                "窗口尺寸缓存超过本机记录上限。",
            ));
        }
        if let Err(error) = publish_cache(
            &self.path,
            inner.previous.as_deref(),
            &bytes,
            before_publish,
        ) {
            inner.writable = false;
            inner.warning = Some(error.message.clone());
            return Err(error);
        }
        inner.previous = Some(bytes);
        inner.dirty = false;
        Ok(())
    }
}

/// RENAME_EXCL protects first publication. RENAME_SWAP preserves the object
/// actually present at the commit point, including an external replacement
/// made after the optimistic check. Never delete the exchanged object: even a
/// later unlink-by-name check would introduce another replacement race.
#[cfg(target_os = "macos")]
fn publish_cache(
    path: &Path,
    previous: Option<&[u8]>,
    bytes: &[u8],
    before_publish: impl FnOnce(),
) -> Result<(), ApiError> {
    use std::{
        ffi::CString,
        io::Write,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::OpenOptionsExt},
        },
    };
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::new("INVALID_WINDOW_CACHE", "窗口尺寸缓存目录无效。"))?;
    check_no_symlinks(parent, false)?;
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(parent)?;
    let temporary_name = format!(
        ".app-window-sizes-preserved-{}.json",
        uuid::Uuid::new_v4().simple()
    );
    let temporary = parent.join(&temporary_name);
    let from = CString::new(temporary_name)
        .map_err(|_| ApiError::new("INVALID_WINDOW_CACHE", "窗口尺寸缓存暂存名称无效。"))?;
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            from.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut pending = unsafe { fs::File::from_raw_fd(descriptor) };
    pending.write_all(bytes)?;
    pending.sync_all()?;
    let to = CString::new(
        path.file_name()
            .ok_or_else(|| ApiError::new("INVALID_WINDOW_CACHE", "窗口尺寸缓存名称无效。"))?
            .as_bytes(),
    )
    .map_err(|_| ApiError::new("INVALID_WINDOW_CACHE", "窗口尺寸缓存名称无效。"))?;
    before_publish();
    let flags = if previous.is_some() {
        libc::RENAME_SWAP
    } else {
        libc::RENAME_EXCL
    };
    if unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            from.as_ptr(),
            directory.as_raw_fd(),
            to.as_ptr(),
            flags,
        )
    } != 0
    {
        let cause = std::io::Error::last_os_error();
        return Err(ApiError::new(
            "WINDOW_CACHE_PUBLISH_FAILED",
            "窗口尺寸缓存未发布；当前文件和本次暂存均已保留。",
        )
        .detail("preservedPath", temporary.to_string_lossy().into_owned())
        .detail("reason", cause.to_string()));
    }
    directory.sync_all().map_err(|error| {
        ApiError::new(
            "WINDOW_CACHE_SYNC_FAILED",
            "窗口尺寸缓存发布后的目录同步失败，现场已保留。",
        )
        .detail("preservedPath", temporary.to_string_lossy().into_owned())
        .detail("reason", error.to_string())
    })?;
    if let Some(expected) = previous {
        // The displaced name is intentionally retained on both success and
        // conflict. It is local preference history, never a project backup.
        if read_cache(&temporary).ok().flatten().as_deref() != Some(expected) {
            return Err(ApiError::new(
                "WINDOW_CACHE_CHANGED",
                "发布时发现外部窗口尺寸变更，原文件已换出保留；已停止后续写入。",
            )
            .detail("preservedPath", temporary.to_string_lossy().into_owned()));
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn publish_cache(_: &Path, _: Option<&[u8]>, _: &[u8], _: impl FnOnce()) -> Result<(), ApiError> {
    Err(ApiError::new(
        "WINDOW_CACHE_UNSUPPORTED",
        "当前系统没有启用原子窗口尺寸缓存发布。",
    ))
}

fn read_cache(path: &Path) -> Result<Option<Vec<u8>>, ApiError> {
    check_no_symlinks(path, true)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.len() > MAX_CACHE_BYTES {
        return Err(ApiError::new(
            "INVALID_WINDOW_CACHE",
            "窗口尺寸缓存文件无效。",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(ApiError::new(
                "INVALID_WINDOW_CACHE",
                "窗口尺寸缓存不能是硬链接。",
            ));
        }
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_CACHE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return Err(ApiError::new(
            "INVALID_WINDOW_CACHE",
            "窗口尺寸缓存文件超过读取上限。",
        ));
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn screen() -> Option<LogicalWindowSize> {
        Some(LogicalWindowSize {
            width: 1920.0,
            height: 1080.0,
        })
    }

    #[test]
    fn author_dimensions_are_optional_strict_and_have_a_remember_opt_out() {
        assert_eq!(
            AppWindowConfig::from_value(None).unwrap(),
            AppWindowConfig::default()
        );
        let config = AppWindowConfig::from_value(Some(&json!({"width":900,"height":650}))).unwrap();
        assert!(config.remember_size);
        assert!(
            !AppWindowConfig::from_value(Some(
                &json!({"width":900,"height":650,"rememberSize":false})
            ))
            .unwrap()
            .remember_size
        );
        for invalid in [
            json!(null),
            json!({"width":900}),
            json!({"width":0,"height":650}),
            json!({"width":900.5,"height":650}),
            json!({"width":900,"height":650,"unknown":true}),
        ] {
            assert!(AppWindowConfig::from_value(Some(&invalid)).is_err());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn saved_dimensions_override_initial_configuration_and_survive_reopening() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let store = WindowSizeStore::new(&directory);
        let identity = WindowIdentity::node("instance-a");
        let config = AppWindowConfig {
            width: 900,
            height: 650,
            remember_size: true,
        };
        assert_eq!(
            store.initial_size(&identity, config, screen()),
            LogicalWindowSize {
                width: 900.0,
                height: 650.0
            }
        );
        store.remember(
            &identity,
            config,
            LogicalWindowSize {
                width: 1100.0,
                height: 720.0,
            },
        );
        store.flush().unwrap();
        let reopened = WindowSizeStore::new(&directory);
        assert_eq!(
            reopened.initial_size(&identity, config, screen()),
            LogicalWindowSize {
                width: 1100.0,
                height: 720.0
            }
        );
        assert_eq!(
            reopened.initial_size(
                &identity,
                AppWindowConfig {
                    remember_size: false,
                    ..config
                },
                screen()
            ),
            LogicalWindowSize {
                width: 900.0,
                height: 650.0
            }
        );
        let saved = fs::read(directory.join("app-window-sizes.json")).unwrap();
        reopened.remember(
            &identity,
            AppWindowConfig {
                remember_size: false,
                ..config
            },
            LogicalWindowSize {
                width: 800.0,
                height: 500.0,
            },
        );
        reopened.flush().unwrap();
        assert_eq!(
            fs::read(directory.join("app-window-sizes.json")).unwrap(),
            saved
        );
    }

    #[test]
    fn workspace_type_and_registration_identity_never_share_size_records() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let store = WindowSizeStore::new(&directory);
        let config = AppWindowConfig::default();
        let identity = WindowIdentity::project(Path::new("/disposable/workspace-a"), "same-id");
        store.remember(
            &identity,
            config,
            LogicalWindowSize {
                width: 900.0,
                height: 600.0,
            },
        );
        for other in [
            WindowIdentity::project(Path::new("/disposable/workspace-b"), "same-id"),
            WindowIdentity::node("same-id"),
            WindowIdentity::project(Path::new("/disposable/workspace-a"), "other-id"),
        ] {
            assert_eq!(
                store.initial_size(&other, config, screen()),
                LogicalWindowSize {
                    width: 1080.0,
                    height: 780.0
                }
            );
        }
    }

    #[test]
    fn bad_or_external_cache_changes_are_preserved_without_blocking_initial_size() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let file = directory.join("app-window-sizes.json");
        fs::write(&file, b"precious-invalid-cache").unwrap();
        let store = WindowSizeStore::new(&directory);
        let identity = WindowIdentity::node("instance-a");
        let config = AppWindowConfig::default();
        assert!(store.warning().is_some());
        assert_eq!(
            store.initial_size(&identity, config, screen()),
            LogicalWindowSize {
                width: 1080.0,
                height: 780.0
            }
        );
        store.remember(
            &identity,
            config,
            LogicalWindowSize {
                width: 900.0,
                height: 600.0,
            },
        );
        store.flush().unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"precious-invalid-cache");
        fs::remove_file(&file).unwrap();
        let store = WindowSizeStore::new(&directory);
        store.remember(
            &identity,
            config,
            LogicalWindowSize {
                width: 900.0,
                height: 600.0,
            },
        );
        fs::write(&file, b"external-change").unwrap();
        assert!(store.flush().is_err());
        assert_eq!(fs::read(&file).unwrap(), b"external-change");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn first_publication_cannot_overwrite_an_external_file_created_after_comparison() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let path = directory.join("app-window-sizes.json");
        let store = WindowSizeStore::new(&directory);
        store.remember(
            &WindowIdentity::node("disposable"),
            AppWindowConfig::default(),
            LogicalWindowSize {
                width: 900.0,
                height: 600.0,
            },
        );
        let error = store
            .flush_with(|| fs::write(&path, b"external file created at publication").unwrap())
            .unwrap_err();
        assert_eq!(error.code, "WINDOW_CACHE_PUBLISH_FAILED");
        assert_eq!(
            fs::read(&path).unwrap(),
            b"external file created at publication"
        );
        assert!(Path::new(error.details["preservedPath"].as_str().unwrap()).is_file());
        store.flush().unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            b"external file created at publication"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn exchange_retains_the_external_replacement_present_at_publication() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let path = directory.join("app-window-sizes.json");
        let identity = WindowIdentity::node("disposable");
        let config = AppWindowConfig::default();
        let original = WindowSizeStore::new(&directory);
        original.remember(
            &identity,
            config,
            LogicalWindowSize {
                width: 900.0,
                height: 600.0,
            },
        );
        original.flush().unwrap();
        let store = WindowSizeStore::new(&directory);
        store.remember(
            &identity,
            config,
            LogicalWindowSize {
                width: 1000.0,
                height: 700.0,
            },
        );
        let external = directory.join("external-writer");
        fs::write(&external, b"replacement from another writer").unwrap();
        let error = store
            .flush_with(|| fs::rename(&external, &path).unwrap())
            .unwrap_err();
        assert_eq!(error.code, "WINDOW_CACHE_CHANGED");
        let preserved = PathBuf::from(error.details["preservedPath"].as_str().unwrap());
        assert_eq!(
            fs::read(&preserved).unwrap(),
            b"replacement from another writer"
        );
        let current = fs::read(&path).unwrap();
        let published: Cache = serde_json::from_slice(&current).unwrap();
        assert_eq!(
            published.sizes[&identity.0],
            LogicalWindowSize {
                width: 1000.0,
                height: 700.0
            }
        );
        store.remember(
            &identity,
            config,
            LogicalWindowSize {
                width: 1200.0,
                height: 750.0,
            },
        );
        store.flush().unwrap();
        assert_eq!(fs::read(&path).unwrap(), current);
        assert_eq!(
            fs::read(&preserved).unwrap(),
            b"replacement from another writer"
        );
    }

    #[test]
    fn logical_sizes_ignore_minimization_and_fit_changed_dpi_and_work_area() {
        assert_eq!(
            LogicalWindowSize::from_physical(1800, 1200, 2.0),
            Some(LogicalWindowSize {
                width: 900.0,
                height: 600.0
            })
        );
        assert!(LogicalWindowSize::from_physical(0, 0, 2.0).is_none());
        assert!(LogicalWindowSize::from_physical(1800, 1200, f64::NAN).is_none());
        assert_eq!(
            fit_window_size(
                LogicalWindowSize {
                    width: 2500.0,
                    height: 1800.0
                },
                Some(LogicalWindowSize {
                    width: 1280.0,
                    height: 800.0
                })
            ),
            LogicalWindowSize {
                width: 1248.0,
                height: 736.0
            }
        );
        assert_eq!(
            fit_window_size(
                LogicalWindowSize {
                    width: 100.0,
                    height: 100.0
                },
                screen()
            ),
            LogicalWindowSize {
                width: 640.0,
                height: 480.0
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn cache_links_are_preserved_and_never_used_or_replaced() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let original = directory.join("preserve-original");
        fs::write(&original, b"precious evidence").unwrap();
        let cache = directory.join("app-window-sizes.json");
        for hard in [false, true] {
            if hard {
                fs::hard_link(&original, &cache).unwrap();
            } else {
                symlink(&original, &cache).unwrap();
            }
            let store = WindowSizeStore::new(&directory);
            assert!(store.warning().is_some());
            store.remember(
                &WindowIdentity::node("disposable"),
                AppWindowConfig::default(),
                LogicalWindowSize {
                    width: 900.0,
                    height: 600.0,
                },
            );
            store.flush().unwrap();
            assert_eq!(fs::read(&original).unwrap(), b"precious evidence");
            assert_eq!(fs::read(&cache).unwrap(), b"precious evidence");
            fs::remove_file(&cache).unwrap();
        }
    }
}
