use super::{require_manager, AppState};
use platform_core::build_profile::{APP_VERSION, LOCAL_UPDATE_TEST_PROFILE, PRODUCTION_PROFILE};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
#[cfg(feature = "local-update-test")]
use transport::Source;
use transport::{ErrorKind, Ready, Release, Transport};

#[cfg(target_os = "macos")]
pub(super) mod macos;
mod transport;

#[cfg(feature = "local-update-test")]
const TEST_PORT: u16 = 18427;
const STATE_FILE: &str = "preferences.json";
const RESULT_FILE: &str = "installation-result.json";
const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const CANNOT_CHECK: &str = "检查更新失败";
const CANNOT_DOWNLOAD: &str = "下载没有完成";
const CANNOT_INSTALL: &str = "更新没有完成";
const UNAVAILABLE: &str = "当前版本不提供自动更新";
const BUSY: &str = "正在处理更新，请稍后重试";
const CHANGED: &str = "更新状态已改变，请重新确认";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Phase {
    #[default]
    Idle,
    Checking,
    Available,
    Downloading,
    Ready,
    Installing,
    Failed,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Snapshot {
    enabled: bool,
    phase: Phase,
    current_version: &'static str,
    version: Option<String>,
    notes: Option<String>,
    downloaded_bytes: Option<u64>,
    total_bytes: Option<u64>,
    last_checked_at: Option<u64>,
    error: Option<String>,
    installed_version: Option<String>,
    installed_notes: Option<String>,
    ready_version: Option<String>,
    ready_notes: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Preferences {
    skipped: Option<String>,
    route: Option<Route>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Route {
    region: String,
    fallback: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallationResult {
    previous: String,
    version: String,
    notes: String,
    outcome: InstallOutcome,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum InstallOutcome {
    Installed,
    PreviousPreserved,
    Unknown,
}

struct Data {
    snapshot: Snapshot,
    candidate: Option<Release>,
    ready: Option<Ready>,
    preferences: Preferences,
}

pub(super) struct PlatformUpdate {
    root: PathBuf,
    operation: tokio::sync::Mutex<()>,
    data: Mutex<Data>,
    hold_exit: AtomicBool,
    #[cfg(target_os = "macos")]
    _install_lock: Option<fs::File>,
}

pub(super) fn enabled() -> bool {
    if option_env!("PANDA_DRAWER_UPDATE_PUBLIC_KEY").is_none_or(str::is_empty) {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        (LOCAL_UPDATE_TEST_PROFILE || PRODUCTION_PROFILE) && macos::installed_bundle().is_ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn milliseconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(super) fn update_root(data_root: &Path) -> Result<PathBuf, String> {
    let root = data_root.join("platform-updates");
    if !root.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .map_err(|_| UNAVAILABLE)?;
        }
        #[cfg(not(unix))]
        fs::create_dir(&root).map_err(|_| UNAVAILABLE)?;
    }
    let meta = fs::symlink_metadata(&root).map_err(|_| UNAVAILABLE)?;
    if !meta.is_dir() || platform_native::filesystem::is_reparse(&meta) {
        return Err(UNAVAILABLE.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o777 != 0o700 || meta.uid() != unsafe { libc::geteuid() } {
            return Err(UNAVAILABLE.into());
        }
    }
    Ok(root)
}

fn read_json<T: serde::de::DeserializeOwned>(root: &Path, name: &str) -> io::Result<T> {
    let file = platform_native::filesystem::open_read(
        &root.join(name),
        platform_native::filesystem::ReadOptions::NoLinksNonblocking,
    )?;
    if !file.metadata()?.is_file() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn write_json<T: Serialize>(root: &Path, name: &str, data: &T) -> io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer(&mut file, data).map_err(io::Error::other)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(root.join(name)).map_err(|error| error.error)?;
    Ok(())
}

fn error_text(error: &transport::UpdateError, otherwise: &str) -> String {
    match error.kind {
        ErrorKind::Integrity => "更新文件校验不通过，已丢弃".into(),
        ErrorKind::Space => "磁盘空间不足，无法下载更新".into(),
        _ => otherwise.into(),
    }
}

fn transport_for(preferences: &Preferences) -> Result<(Transport, usize), String> {
    let key = option_env!("PANDA_DRAWER_UPDATE_PUBLIC_KEY")
        .filter(|key| !key.is_empty())
        .ok_or(UNAVAILABLE)?;
    let target = if cfg!(target_os = "macos") {
        "darwin-aarch64"
    } else {
        "windows-x86_64"
    };
    #[cfg(feature = "local-update-test")]
    let sources = vec![Source::loopback(TEST_PORT)];
    #[cfg(not(feature = "local-update-test"))]
    let sources = {
        let route = preferences.route.as_ref();
        transport::configured_sources(
            route.map_or("global", |value| value.region.as_str()),
            route.is_none_or(|value| value.fallback),
        )
        .map_err(|_| UNAVAILABLE)?
    };
    #[cfg(feature = "local-update-test")]
    let _ = preferences;
    let transport = Transport::new(key, sources, target).map_err(|_| UNAVAILABLE)?;
    let count = transport.source_count();
    Ok((transport, count))
}

fn supersedes(candidate: &Release, ready: &Ready) -> bool {
    match (
        semver::Version::parse(&candidate.version),
        semver::Version::parse(&ready.release.version),
    ) {
        (Ok(candidate), Ok(ready)) => candidate.cmp_precedence(&ready).is_gt(),
        _ => false,
    }
}

impl PlatformUpdate {
    pub(super) fn new(data_root: &Path) -> Result<Arc<Self>, String> {
        let root = enabled().then(|| update_root(data_root).ok()).flatten();
        let active = root.is_some();
        let root = root.unwrap_or_default();
        #[cfg(target_os = "macos")]
        let install_lock = if active {
            Some(macos::start_lock(&root).map_err(|_| BUSY)?)
        } else {
            None
        };
        let preferences = if active {
            read_json(&root, STATE_FILE).unwrap_or_default()
        } else {
            Preferences::default()
        };
        Ok(Arc::new(Self {
            root,
            operation: tokio::sync::Mutex::new(()),
            data: Mutex::new(Data {
                snapshot: Snapshot {
                    enabled: active,
                    current_version: APP_VERSION,
                    ..Snapshot::default()
                },
                candidate: None,
                ready: None,
                preferences,
            }),
            hold_exit: AtomicBool::new(false),
            #[cfg(target_os = "macos")]
            _install_lock: install_lock,
        }))
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        self.data.lock().unwrap().snapshot.clone()
    }
    pub(super) fn decorate_platform_info(&self, platform: &mut serde_json::Value) {
        let snapshot = self.snapshot();
        platform["updates"]["enabled"] = serde_json::json!(snapshot.enabled);
        if snapshot.enabled {
            platform["updates"]["available"] = serde_json::json!(snapshot.version.is_some());
            platform["updates"]["state"] = serde_json::json!(snapshot.phase);
            platform["updates"]["version"] = serde_json::json!(snapshot.version);
        }
    }
    pub(super) fn holds_exit(&self) -> bool {
        self.hold_exit.load(Ordering::Acquire)
    }

    fn show(&self, app: &AppHandle, change: impl FnOnce(&mut Data)) {
        let snapshot = {
            let mut data = self.data.lock().unwrap();
            change(&mut data);
            data.snapshot.clone()
        };
        let _ = app.emit_to("manager", "platform-update-state", snapshot);
    }

    fn set_ready(data: &mut Data, ready: Option<Ready>) {
        data.snapshot.ready_version = ready.as_ref().map(|ready| ready.release.version.clone());
        data.snapshot.ready_notes = ready.as_ref().map(|ready| ready.release.notes.clone());
        data.ready = ready;
    }

    fn phase(data: &mut Data, phase: Phase) {
        data.snapshot.phase = phase;
        data.snapshot.error = None;
        data.snapshot.downloaded_bytes = None;
        data.snapshot.total_bytes = None;
    }

    fn settled(data: &mut Data) {
        if let Some(candidate) = data.candidate.as_ref() {
            data.snapshot.version = Some(candidate.version.clone());
            data.snapshot.notes = Some(candidate.notes.clone());
            Self::phase(data, Phase::Available);
        } else if let Some(ready) = data.ready.as_ref() {
            data.snapshot.version = Some(ready.release.version.clone());
            data.snapshot.notes = Some(ready.release.notes.clone());
            Self::phase(data, Phase::Ready);
        } else {
            data.snapshot.version = None;
            data.snapshot.notes = None;
            Self::phase(data, Phase::Idle);
        }
    }

    fn failure(&self, app: &AppHandle, message: String) {
        self.show(app, |data| {
            Self::phase(data, Phase::Failed);
            data.snapshot.error = Some(message);
        });
    }

    pub(super) fn start(self: Arc<Self>, app: AppHandle) {
        if !self.snapshot().enabled {
            return;
        }
        tauri::async_runtime::spawn(async move {
            {
                let _lock = self.operation.lock().await;
                let preferences = self.data.lock().unwrap().preferences.clone();
                let skipped = preferences.skipped.clone();
                let root = self.root.clone();
                if let Ok(Ok(ready)) = tokio::task::spawn_blocking(move || {
                    let (transport, _) = transport_for(&preferences)?;
                    transport
                        .recover(&root, APP_VERSION)
                        .map_err(|_| UNAVAILABLE.to_owned())
                })
                .await
                {
                    self.show(&app, |data| {
                        Self::set_ready(
                            data,
                            ready
                                .filter(|ready| skipped.as_deref() != Some(&ready.release.version)),
                        );
                        Self::settled(data);
                    });
                }
            }
            tokio::time::sleep(if LOCAL_UPDATE_TEST_PROFILE {
                Duration::from_secs(2)
            } else {
                FIRST_CHECK
            })
            .await;
            loop {
                let Some(state) = app.try_state::<AppState>() else {
                    break;
                };
                if state.shutting_down.load(Ordering::Acquire) {
                    break;
                }
                let due = self.snapshot().last_checked_at.is_none_or(|time| {
                    milliseconds().saturating_sub(time) >= CHECK_INTERVAL.as_millis() as u64
                });
                if due {
                    let failed = self.check(&app, false).await.is_err();
                    tokio::time::sleep(if failed {
                        Duration::from_secs(300)
                    } else {
                        Duration::from_secs(30)
                    })
                    .await;
                } else {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            }
        });
    }

    pub(super) fn startup_completed(&self, app: &AppHandle) {
        if !self.snapshot().enabled {
            return;
        }
        let Ok(result) = read_json::<InstallationResult>(&self.root, RESULT_FILE) else {
            return;
        };
        if fs::remove_file(self.root.join(RESULT_FILE)).is_err() {
            return;
        }
        if result.outcome == InstallOutcome::Installed
            && result.version == APP_VERSION
            && result.previous != APP_VERSION
        {
            self.show(app, |data| {
                data.snapshot.installed_version = Some(result.version);
                data.snapshot.installed_notes = Some(result.notes);
            });
        } else {
            let message = if result.outcome == InstallOutcome::PreviousPreserved
                && result.previous == APP_VERSION
            {
                "更新没有完成，已保留当前版本"
            } else {
                "无法确认更新结果"
            };
            self.failure(app, message.into());
        }
    }

    async fn check(&self, app: &AppHandle, manual: bool) -> Result<(), String> {
        let _lock = self.operation.try_lock().map_err(|_| BUSY)?;
        self.ensure_open(app)?;
        let preferences = self.data.lock().unwrap().preferences.clone();
        let (transport, count) = transport_for(&preferences)?;
        self.show(app, |data| Self::phase(data, Phase::Checking));
        let mut result = transport.check_source(0, APP_VERSION).await;
        if count > 1
            && result
                .as_ref()
                .is_err_and(|error| error.kind == ErrorKind::Network)
        {
            result = transport.check_source(1, APP_VERSION).await;
        }
        match result {
            Ok(candidate) => {
                self.show(app, |data| {
                    data.snapshot.last_checked_at = Some(milliseconds());
                    data.candidate = candidate.filter(|version| {
                        (manual || data.preferences.skipped.as_deref() != Some(&version.version))
                            && data
                                .ready
                                .as_ref()
                                .is_none_or(|ready| supersedes(version, ready))
                    });
                    Self::settled(data);
                });
                Ok(())
            }
            Err(error) => {
                let message = error_text(&error, CANNOT_CHECK);
                if manual {
                    self.failure(app, message.clone());
                } else {
                    eprintln!("Platform update check deferred: {message}");
                    self.show(app, Self::settled);
                }
                Err(message)
            }
        }
    }

    async fn download(&self, app: &AppHandle, version: String) -> Result<(), String> {
        let _lock = self.operation.try_lock().map_err(|_| BUSY)?;
        self.ensure_open(app)?;
        let (mut release, preferences) = {
            let data = self.data.lock().unwrap();
            let current = data
                .candidate
                .as_ref()
                .filter(|c| c.version == version)
                .ok_or(CHANGED)?;
            (current.clone(), data.preferences.clone())
        };
        let (transport, count) = transport_for(&preferences)?;
        let mut used_fallback = release.source != 0;
        self.show(app, |data| Self::phase(data, Phase::Downloading));
        for attempt in 0..3 {
            let result = transport
                .download(&release, &self.root, |bytes, total| {
                    self.show(app, |data| {
                        data.snapshot.downloaded_bytes = Some(bytes);
                        data.snapshot.total_bytes = total;
                    });
                })
                .await;
            match result {
                Ok(ready) => {
                    self.show(app, |data| {
                        Self::set_ready(data, Some(ready));
                        data.candidate = None;
                        Self::settled(data);
                    });
                    return Ok(());
                }
                Err(error) if error.kind == ErrorKind::Network && attempt < 2 => {
                    self.ensure_open(app).map_err(|error| {
                        self.show(app, Self::settled);
                        error
                    })?;
                    if count > 1 && !used_fallback {
                        used_fallback = true;
                        match transport.check_source(1, APP_VERSION).await {
                            Ok(Some(next)) if next.version == version => release = next,
                            Ok(Some(next)) => {
                                self.show(app, |data| {
                                    data.candidate = Some(next).filter(|candidate| {
                                        data.ready
                                            .as_ref()
                                            .is_none_or(|ready| supersedes(candidate, ready))
                                    });
                                    Self::settled(data);
                                });
                                return Ok(());
                            }
                            Ok(None) => {
                                self.failure(app, CANNOT_DOWNLOAD.into());
                                return Err(CANNOT_DOWNLOAD.into());
                            }
                            Err(error) if error.kind == ErrorKind::Network => {}
                            Err(error) => {
                                let message = error_text(&error, CANNOT_DOWNLOAD);
                                self.failure(app, message.clone());
                                return Err(message);
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
                }
                Err(error) => {
                    let message = error_text(&error, CANNOT_DOWNLOAD);
                    self.failure(app, message.clone());
                    return Err(message);
                }
            }
        }
        unreachable!("download loop always returns on the last attempt")
    }

    fn ensure_open(&self, app: &AppHandle) -> Result<(), String> {
        let state = self.snapshot();
        if !state.enabled {
            return Err(UNAVAILABLE.into());
        }
        if state.phase == Phase::Installing
            || app
                .try_state::<AppState>()
                .is_none_or(|state| state.shutting_down.load(Ordering::Acquire))
        {
            return Err(BUSY.into());
        }
        Ok(())
    }
}

fn manager(window: &WebviewWindow) -> Result<(AppHandle, Arc<PlatformUpdate>), String> {
    require_manager(window).map_err(|error| error.message)?;
    let app = window.app_handle().clone();
    let state = app.try_state::<AppState>().ok_or(UNAVAILABLE)?;
    Ok((app.clone(), state.platform_update.clone()))
}

#[tauri::command]
pub(super) fn platform_update_state(window: WebviewWindow) -> Result<Snapshot, String> {
    Ok(manager(&window)?.1.snapshot())
}

#[tauri::command]
pub(super) async fn platform_update_check(window: WebviewWindow) -> Result<(), String> {
    let (app, update) = manager(&window)?;
    tauri::async_runtime::spawn(async move { update.check(&app, true).await })
        .await
        .map_err(|_| CANNOT_CHECK)?
}

#[tauri::command]
pub(super) async fn platform_update_download(
    window: WebviewWindow,
    version: String,
) -> Result<(), String> {
    let (app, update) = manager(&window)?;
    tauri::async_runtime::spawn(async move { update.download(&app, version).await })
        .await
        .map_err(|_| CANNOT_DOWNLOAD)?
}

#[tauri::command]
pub(super) async fn platform_update_set_route(
    window: WebviewWindow,
    route: String,
    mode: String,
) -> Result<(), String> {
    let (app, update) = manager(&window)?;
    update.ensure_open(&app)?;
    if LOCAL_UPDATE_TEST_PROFILE {
        return Ok(());
    }
    let (region, fallback) = match (route.as_str(), mode.as_str()) {
        ("cn" | "global", "automatic") => (route, true),
        (_, "cn" | "global") => (mode, false),
        _ => return Err(UNAVAILABLE.into()),
    };
    let _lock = update.operation.lock().await;
    update.ensure_open(&app)?;
    let mut preferences = update.data.lock().unwrap().preferences.clone();
    let config = Route { region, fallback };
    if preferences.route.as_ref() == Some(&config) {
        return Ok(());
    }
    preferences.route = Some(config);
    write_json(&update.root, STATE_FILE, &preferences).map_err(|_| UNAVAILABLE)?;
    update.show(&app, |data| {
        data.preferences = preferences;
        data.snapshot.last_checked_at = None;
        data.candidate = None;
        PlatformUpdate::settled(data);
    });
    Ok(())
}

#[tauri::command]
pub(super) fn platform_update_skip(window: WebviewWindow, version: String) -> Result<(), String> {
    let (app, update) = manager(&window)?;
    let _lock = update.operation.try_lock().map_err(|_| BUSY)?;
    update.ensure_open(&app)?;
    let mut preferences = {
        let data = update.data.lock().unwrap();
        if !data
            .candidate
            .as_ref()
            .is_some_and(|candidate| candidate.version == version)
            && !data
                .ready
                .as_ref()
                .is_some_and(|ready| ready.release.version == version)
        {
            return Err(CHANGED.into());
        }
        data.preferences.clone()
    };
    preferences.skipped = Some(version.clone());
    write_json(&update.root, STATE_FILE, &preferences).map_err(|_| UNAVAILABLE)?;
    update.show(&app, |data| {
        data.preferences = preferences;
        if data
            .candidate
            .as_ref()
            .is_some_and(|candidate| candidate.version == version)
        {
            data.candidate = None;
        }
        if data
            .ready
            .as_ref()
            .is_some_and(|ready| ready.release.version == version)
        {
            PlatformUpdate::set_ready(data, None);
        }
        PlatformUpdate::settled(data);
    });
    Ok(())
}

#[derive(Serialize)]
pub(super) struct InstallPlan {
    items: Vec<String>,
    blocked: bool,
}

async fn inspect_install(app: &AppHandle) -> Result<InstallPlan, String> {
    let state = app.try_state::<AppState>().ok_or(UNAVAILABLE)?;
    let mut items: Vec<String> = app
        .webview_windows()
        .into_iter()
        .filter(|(label, _)| label.starts_with("app-"))
        .filter_map(|(_, window)| window.title().ok())
        .collect();
    for name in state
        .handle
        .update_running_app_names()
        .await
        .map_err(|_| CANNOT_INSTALL)?
    {
        if !items.contains(&name) {
            items.push(name);
        }
    }
    let blocked = state.local_data_cleanup.active() || state.handle.update_has_active_work();
    if blocked {
        items.push("平台正在处理任务".into());
    }
    Ok(InstallPlan { items, blocked })
}

#[tauri::command]
pub(super) async fn platform_update_install_plan(
    window: WebviewWindow,
    version: String,
) -> Result<InstallPlan, String> {
    let (app, update) = manager(&window)?;
    update.ensure_open(&app)?;
    if update.snapshot().ready_version.as_deref() != Some(&version) {
        return Err(CHANGED.into());
    }
    inspect_install(&app).await
}

#[tauri::command]
pub(super) async fn platform_update_install(
    window: WebviewWindow,
    version: String,
    confirmed: bool,
) -> Result<(), String> {
    let (app, update) = manager(&window)?;
    let _lock = update.operation.try_lock().map_err(|_| BUSY)?;
    update.ensure_open(&app)?;
    let check = inspect_install(&app).await?;
    if check.blocked || !confirmed && !check.items.is_empty() {
        return Err("请确认安装前需要关闭的应用与任务".into());
    }
    let (ready, preferences) = {
        let data = update.data.lock().unwrap();
        let ready = data
            .ready
            .as_ref()
            .filter(|ready| ready.release.version == version)
            .ok_or(CHANGED)?;
        (ready.clone(), data.preferences.clone())
    };
    let (transport, _) = transport_for(&preferences)?;
    let root = update.root.clone();
    let owner = app.clone();
    update.show(&app, |data| {
        data.snapshot.version = Some(ready.release.version.clone());
        data.snapshot.notes = Some(ready.release.notes.clone());
        PlatformUpdate::phase(data, Phase::Installing);
    });
    #[cfg(target_os = "macos")]
    {
        let worker = update.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(message) =
                macos::install(owner.clone(), root, preferences, transport, ready).await
            {
                worker.failure(&owner, message);
            }
        });
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (ready, transport, owner, root);
        update.failure(&app, UNAVAILABLE.into());
        Err(UNAVAILABLE.into())
    }
}
