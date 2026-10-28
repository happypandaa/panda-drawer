//! WKWebView downloads need an explicit handler. Download to an owned private
//! staging directory, then ask where to save with a non-blocking native dialog.
use platform_core::{
    io_lifetime::TrackedIo, native_project_io::NativeProjectIoGuard, projects::check_no_symlinks,
    ApiError, RuntimeHandle,
};
use std::{
    collections::HashMap,
    fs::{self, File},
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
#[cfg(test)]
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
};
use tauri::{webview::DownloadEvent, AppHandle, Url, Webview};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
}
fn identity(metadata: &fs::Metadata) -> Identity {
    Identity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}
fn unique_name() -> String {
    format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    )
}
fn invalid(message: &str) -> ApiError {
    ApiError::new("DOWNLOAD_FAILED", message)
}

struct PendingDownload {
    directory: PathBuf,
    directory_identity: Identity,
    file: PathBuf,
    suggested_name: String,
    writer: Option<NativeProjectIoGuard>,
    engine_accepted: AtomicBool,
    engine_finished: AtomicBool,
}
impl PendingDownload {
    fn create_owned(writer: NativeProjectIoGuard, suggested_name: &str) -> Result<Self, ApiError> {
        let result = writer
            .stage_parent()
            .and_then(|parent| Self::create_at(&parent, suggested_name));
        match result {
            Ok(mut stage) => {
                stage.writer = Some(writer);
                Ok(stage)
            }
            Err(error) => {
                let _ = writer.finish();
                Err(error)
            }
        }
    }
    #[cfg(test)]
    fn create(data_root: &Path, suggested_name: &str) -> Result<Self, ApiError> {
        Self::create_at(&data_root.join("native-downloads"), suggested_name)
    }
    fn create_at(parent: &Path, suggested_name: &str) -> Result<Self, ApiError> {
        check_no_symlinks(parent, true)?;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(parent)
            .or_else(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    Ok(())
                } else {
                    Err(error)
                }
            })?;
        check_no_symlinks(parent, false)?;
        let directory = parent.join(unique_name());
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let directory_identity = identity(&fs::symlink_metadata(&directory)?);
        let suggested_name = Path::new(suggested_name)
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| {
                !value.is_empty() && value.len() <= 240 && !value.chars().any(char::is_control)
            })
            .unwrap_or("download.bin")
            .to_owned();
        Ok(Self {
            file: directory.join("payload"),
            directory,
            directory_identity,
            suggested_name,
            writer: None,
            engine_accepted: AtomicBool::new(false),
            engine_finished: AtomicBool::new(false),
        })
    }
    fn validate_directory(&self) -> Result<(), ApiError> {
        check_no_symlinks(&self.directory, false)?;
        let metadata = fs::symlink_metadata(&self.directory)?;
        if !metadata.is_dir() || identity(&metadata) != self.directory_identity {
            return Err(invalid("Temporary download directory changed"));
        }
        Ok(())
    }
    fn open_file(&self) -> Result<File, ApiError> {
        if self.engine_accepted.load(Ordering::Acquire)
            && !self.engine_finished.load(Ordering::Acquire)
        {
            return Err(invalid("Browser download has not confirmed Finished"));
        }
        if let Some(writer) = &self.writer {
            writer.validate()?;
        }
        self.validate_directory()?;
        check_no_symlinks(&self.file, false)?;
        let metadata = fs::symlink_metadata(&self.file)?;
        if !metadata.is_file() || metadata.len() > MAX_DOWNLOAD_BYTES {
            return Err(invalid(
                "Download exceeds the internal 256 MiB save limit or is not a regular file",
            ));
        }
        let file = File::open(&self.file)?;
        let opened = file.metadata()?;
        if identity(&metadata) != identity(&opened)
            || !opened.is_file()
            || opened.len() > MAX_DOWNLOAD_BYTES
        {
            return Err(invalid("Temporary download changed before saving"));
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        Ok(file)
    }
    fn cleanup(&self) -> Result<(), ApiError> {
        self.validate_directory()?;
        if fs::symlink_metadata(&self.file)
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        {
            fs::remove_file(&self.file)?;
        }
        // Do not recursively remove unrelated files or replaced directories.
        fs::remove_dir(&self.directory)?;
        File::open(self.directory.parent().unwrap())?.sync_all()?;
        Ok(())
    }
}

impl Drop for PendingDownload {
    fn drop(&mut self) {
        if self.engine_accepted.load(Ordering::Acquire)
            && !self.engine_finished.load(Ordering::Acquire)
        {
            return;
        }
        if self.cleanup().is_ok() {
            if let Some(writer) = self.writer.take() {
                if let Err(error) = writer.finish() {
                    eprintln!("Native download writer receipt: {}", error.code);
                }
            }
        }
    }
}

// Until Finished, WebKit owns the destination and may still write it. Dropping
// the callback alone is not an engine-cancellation acknowledgement. Retain both
// evidence and the busy lease in that exceptional case, rather than deleting
// the engine's file and falsely reporting a drained service.
struct EngineDownload {
    resource: Option<ProjectDownload>,
    ticket: Option<super::engine_downloads::EngineTicket>,
}
type ProjectDownload = TrackedIo<TrackedIo<PendingDownload>>;
fn pending_stage(value: &ProjectDownload) -> &PendingDownload {
    value.resource().resource()
}
impl EngineDownload {
    fn finished(mut self) -> ProjectDownload {
        pending_stage(self.resource.as_ref().expect("owned download"))
            .engine_finished
            .store(true, Ordering::Release);
        self.ticket.take().expect("owned engine ticket").finished();
        self.resource.take().expect("owned download")
    }
}
impl Drop for EngineDownload {
    fn drop(&mut self) {
        if let Some(resource) = self.resource.take() {
            eprintln!("Native download callback ended without Finished; preserving its stage and blocking drain");
            std::mem::forget(resource);
        }
    }
}

fn permitted_url(base: &Url, id: &str, current: &Url, requested: &Url) -> bool {
    let prefix = format!("/p/{id}/");
    let same_origin = |url: &Url| {
        url.origin() == base.origin() && url.username().is_empty() && url.password().is_none()
    };
    if !same_origin(current) || !current.path().starts_with(&prefix) {
        return false;
    }
    if requested.scheme() == "blob" {
        return requested
            .as_str()
            .strip_prefix("blob:")
            .and_then(|inner| Url::parse(inner).ok())
            .is_some_and(|inner| {
                same_origin(&inner)
                    && !inner.path().is_empty()
                    && inner.path() != "/"
                    && inner.query().is_none()
                    && inner.fragment().is_none()
            });
    }
    same_origin(requested)
        && requested.path().starts_with(&prefix)
        && !requested.path().contains("%2f")
        && !requested.path().contains("%2F")
        && !requested.path().contains("%5c")
        && !requested.path().contains("%5C")
}

#[cfg(test)]
fn destination_identity(path: &Path) -> Result<Option<Identity>, ApiError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            Ok(Some(identity(&metadata)))
        }
        Ok(_) => Err(invalid("Save destination must be a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
fn publish_copy(
    stage: &PendingDownload,
    destination: &Path,
    validate: impl Fn(&Path) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    publish_file(stage.open_file()?, destination, |path| {
        if let Some(writer) = &stage.writer {
            writer.validate()?;
        }
        validate(path)
    })
}

/// Publish a completed attachment or browser download after native save consent.
/// The caller keeps any project operation lease alive until this returns.
#[cfg(test)]
pub(crate) fn publish_file(
    mut source: File,
    destination: &Path,
    validate: impl Fn(&Path) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    let source_before = source.metadata()?;
    if !source_before.is_file() || source_before.len() > MAX_DOWNLOAD_BYTES {
        return Err(invalid(
            "Download exceeds the internal 256 MiB save limit or is not a regular file",
        ));
    }
    validate(destination)?;
    let expected = destination_identity(destination)?;
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("Save destination needs a parent directory"))?;
    let parent_identity = identity(&fs::metadata(parent)?);
    let temporary = parent.join(format!(".local-projects-download-{}", unique_name()));
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let size = io::copy(&mut (&mut source).take(MAX_DOWNLOAD_BYTES + 1), &mut output)?;
        if size > MAX_DOWNLOAD_BYTES {
            return Err(invalid("Download exceeds the internal 256 MiB save limit"));
        }
        let source_after = source.metadata()?;
        if size != source_before.len()
            || source_before.len() != source_after.len()
            || source_before.modified().ok() != source_after.modified().ok()
        {
            return Err(invalid("Temporary download changed while copying"));
        }
        output.flush()?;
        output.sync_all()?;
        validate(destination)?;
        if identity(&fs::metadata(parent)?) != parent_identity
            || destination_identity(destination)? != expected
        {
            return Err(invalid(
                "Save destination changed while copying; choose it again",
            ));
        }
        if expected.is_some() {
            fs::rename(&temporary, destination)?;
        } else {
            // Atomic no-replace publication when the user selected a new filename.
            fs::hard_link(&temporary, destination)?;
            fs::remove_file(&temporary)?;
        }
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn notify_error(app: &AppHandle, error: &ApiError) {
    app.dialog()
        .message(format!("下载未保存：{}", error.message))
        .title("保存下载")
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

pub fn handler(
    app: AppHandle,
    handle: RuntimeHandle,
    id: String,
    epoch: String,
    engines: Arc<super::engine_downloads::EngineDownloads>,
) -> impl Fn(Webview, DownloadEvent<'_>) -> bool + Send + Sync + 'static {
    let pending = Arc::new(Mutex::new(HashMap::<String, EngineDownload>::new()));
    let window_admission = engines.bind_handler(&id);
    move |webview, event| match event {
        DownloadEvent::Requested { url, destination } => {
            let Ok(((io_lease, writer), project_lease, mut ticket)) =
                engines.admit_project(&id, || {
                    window_admission.check()?;
                    let lease = handle.io_lifetime().admit()?;
                    let writer =
                        handle.native_project_io(&id, &epoch, "native-webview-download")?;
                    Ok((lease, writer))
                })
            else {
                return false;
            };
            let info = handle.info();
            let permitted = Url::parse(&info.base_url)
                .ok()
                .zip(webview.url().ok())
                .is_some_and(|(base, current)| permitted_url(&base, &id, &current, &url));
            if !permitted {
                let _ = writer.finish();
                return false;
            }
            let Ok(mut downloads) = pending.lock() else {
                let _ = writer.finish();
                return false;
            };
            let key = url.to_string();
            // Finished.path is unavailable on macOS; URL is our completion key.
            if downloads.contains_key(&key) {
                let _ = writer.finish();
                return false;
            }
            let suggested = destination
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("download.bin");
            match PendingDownload::create_owned(writer, suggested) {
                Ok(stage) => {
                    *destination = stage.file.clone();
                    ticket.started();
                    stage.engine_accepted.store(true, Ordering::Release);
                    downloads.insert(
                        key,
                        EngineDownload {
                            resource: Some(project_lease.hold(io_lease.hold(stage))),
                            ticket: Some(ticket),
                        },
                    );
                    true
                }
                Err(error) => {
                    notify_error(&app, &error);
                    false
                }
            }
        }
        DownloadEvent::Finished { url, success, .. } => {
            let stage = pending
                .lock()
                .ok()
                .and_then(|mut downloads| downloads.remove(url.as_str()));
            let Some(stage) = stage else {
                return false;
            };
            let stage = stage.finished();
            if !engines.project_open(&id) {
                return false;
            }
            if !success {
                notify_error(&app, &invalid("Browser download did not complete"));
                return false;
            }
            if let Err(error) = pending_stage(&stage).open_file() {
                notify_error(&app, &error);
                return false;
            }
            let suggested_name = pending_stage(&stage).suggested_name.clone();
            let Ok(deferred) = engines.defer_project(&id, stage) else {
                return false;
            };
            let app = app.clone();
            let handle = handle.clone();
            app.dialog()
                .file()
                .set_parent(&webview.window())
                .set_title("保存项目下载")
                .set_file_name(&suggested_name)
                .save_file(move |selected| {
                    let Some(selected) = selected else {
                        return;
                    };
                    // Claim and stop share admission's lock. An unselected stage
                    // is revoked on stop; a successful claim keeps the original
                    // lease through copying and private-stage destruction.
                    let Ok(stage) = deferred.claim() else {
                        return;
                    };
                    tauri::async_runtime::spawn_blocking(move || {
                        let result = selected
                            .into_path()
                            .map_err(|error| invalid(&error.to_string()))
                            .and_then(|destination| {
                                let pending = pending_stage(&stage);
                                let writer = pending.writer.as_ref().ok_or_else(|| {
                                    invalid("Project writer admission is missing")
                                })?;
                                handle.publish_native_file(
                                    writer,
                                    pending.open_file()?,
                                    &destination,
                                )
                            });
                        if let Err(error) = result {
                            notify_error(&app, &error);
                        }
                        drop(stage);
                    });
                });
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temporary() -> PathBuf {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("m5-download-{}", unique_name()));
        fs::create_dir(&root).unwrap();
        root
    }
    #[test]
    fn only_project_and_exact_origin_blob_downloads_are_allowed() {
        let base = Url::parse("http://127.0.0.1:4321").unwrap();
        let current = base.join("/p/prj_test/").unwrap();
        for value in [
            "http://127.0.0.1:4321/p/prj_test/api/v1/file?path=a.txt",
            "blob:http://127.0.0.1:4321/abcd",
        ] {
            assert!(
                permitted_url(&base, "prj_test", &current, &Url::parse(value).unwrap()),
                "{value}"
            );
        }
        for value in [
            "https://example.com/file",
            "http://127.0.0.1:4322/p/prj_test/file",
            "http://127.0.0.1:4321/p/prj_other/file",
            "http://127.0.0.1:4321/p/prj_test/../prj_other/file",
            "http://127.0.0.1:4321/p/prj_test/a%2fb",
            "blob:https://example.com/id",
            "blob:http://127.0.0.1:43211/id",
            "blob:http://user@127.0.0.1:4321/id",
            "data:text/plain,hello",
        ] {
            assert!(
                !permitted_url(&base, "prj_test", &current, &Url::parse(value).unwrap()),
                "{value}"
            );
        }
        assert!(!permitted_url(
            &base,
            "prj_test",
            &base.join("/p/other/").unwrap(),
            &base.join("/p/prj_test/file").unwrap()
        ));
    }
    #[test]
    fn staging_rejects_symlinks_replacement_and_oversized_files_without_deleting_unknown_files() {
        use std::os::unix::fs::symlink;
        let root = temporary();
        let stage = PendingDownload::create(&root, "example.txt").unwrap();
        let unknown = root.join("unknown");
        fs::write(&unknown, b"preserve").unwrap();
        symlink(&unknown, &stage.file).unwrap();
        assert!(stage.open_file().is_err());
        let _ = stage.cleanup();
        assert_eq!(fs::read(&unknown).unwrap(), b"preserve");
        assert!(stage.directory.exists());
        fs::remove_file(&stage.file).unwrap();
        File::create(&stage.file)
            .unwrap()
            .set_len(MAX_DOWNLOAD_BYTES + 1)
            .unwrap();
        assert!(stage.open_file().is_err());
        let _ = stage.cleanup();
        assert!(!stage.directory.exists());
        let stage = PendingDownload::create(&root, "test").unwrap();
        let held = stage.directory.with_extension("held");
        fs::rename(&stage.directory, &held).unwrap();
        fs::create_dir(&stage.directory).unwrap();
        fs::write(stage.directory.join("unknown"), b"preserve").unwrap();
        assert!(stage.open_file().is_err());
        let _ = stage.cleanup();
        assert!(stage.directory.join("unknown").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn publication_copies_only_after_validation_and_preserves_source_on_failure() {
        let root = temporary();
        let stage = PendingDownload::create(&root, "example.txt").unwrap();
        fs::write(&stage.file, b"payload").unwrap();
        let output = root.join("saved.txt");
        assert!(publish_copy(&stage, &output, |_| Err(invalid("protected"))).is_err());
        assert!(!output.exists());
        assert!(stage.file.exists());
        publish_copy(&stage, &output, |_| Ok(())).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"payload");
        fs::write(&stage.file, b"updated").unwrap();
        publish_copy(&stage, &output, |_| Ok(())).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"updated");
        let _ = stage.cleanup();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn accepted_stage_requires_a_real_finished_receipt_before_reading_or_cleanup() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let stage = PendingDownload::create(&root, "disposable.txt").unwrap();
        let directory = stage.directory.clone();
        fs::write(&stage.file, b"unfinished engine bytes").unwrap();
        stage.engine_accepted.store(true, Ordering::Release);
        assert!(stage.open_file().is_err());
        drop(stage);
        assert!(
            directory.join("payload").exists(),
            "Drop must not fabricate Finished"
        );
        let stage = PendingDownload::create(&root, "disposable-finished.txt").unwrap();
        let directory = stage.directory.clone();
        fs::write(&stage.file, b"confirmed engine bytes").unwrap();
        stage.engine_accepted.store(true, Ordering::Release);
        stage.engine_finished.store(true, Ordering::Release);
        drop(stage.open_file().unwrap());
        drop(stage);
        assert!(!directory.exists());
    }
    #[tokio::test]
    async fn stopped_save_dialog_cleans_private_stage_and_cannot_publish_later() {
        let temporary = tempfile::TempDir::new().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let stage = PendingDownload::create(&root, "artificial.txt").unwrap();
        fs::write(&stage.file, b"artificial local content").unwrap();
        let directory = stage.directory.clone();
        let registry = platform_core::io_lifetime::IoLifetime::default();
        let deferred = registry
            .defer(registry.admit().unwrap().hold(stage))
            .unwrap();
        registry.close();
        registry.drain().await;
        assert!(!directory.exists());
        assert!(deferred.claim().is_err());
        assert!(!root.join("selected-after-stop.txt").exists());
    }
    #[tokio::test]
    async fn selected_save_owns_stage_until_worker_finishes_cleanup() {
        let temporary = tempfile::TempDir::new().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let stage = PendingDownload::create(&root, "artificial.txt").unwrap();
        fs::write(&stage.file, b"artificial local content").unwrap();
        let directory = stage.directory.clone();
        let destination = root.join("accepted.txt");
        let registry = platform_core::io_lifetime::IoLifetime::default();
        let selected = registry
            .defer(registry.admit().unwrap().hold(stage))
            .unwrap()
            .claim()
            .unwrap();
        registry.seal();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), registry.drain())
                .await
                .is_err()
        );
        let output = destination.clone();
        tokio::task::spawn_blocking(move || publish_copy(selected.resource(), &output, |_| Ok(())))
            .await
            .unwrap()
            .unwrap();
        registry.drain().await;
        assert!(!directory.exists());
        assert_eq!(fs::read(destination).unwrap(), b"artificial local content");
    }
    #[tokio::test]
    async fn finished_engine_releases_only_its_ticket_while_the_actual_save_stage_stays_busy() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let engines = Arc::new(super::super::engine_downloads::EngineDownloads::default());
        let io = platform_core::io_lifetime::IoLifetime::default();
        let (lease, project_lease, mut ticket) = engines
            .admit_project("prj_disposable_download", || io.admit())
            .unwrap();
        let stage = PendingDownload::create(&root, "artificial.txt").unwrap();
        let directory = stage.directory.clone();
        fs::write(&stage.file, b"artificial engine content").unwrap();
        ticket.started();
        let engine = EngineDownload {
            resource: Some(project_lease.hold(lease.hold(stage))),
            ticket: Some(ticket),
        };
        engines.close();
        io.close();
        io.seal();
        let stage = engine.finished();
        engines.wait_for_finished().await;
        assert!(directory.exists());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), io.drain())
                .await
                .is_err()
        );
        drop(stage);
        io.drain().await;
        assert!(!directory.exists());
    }
    #[tokio::test]
    async fn callback_drop_keeps_the_real_stage_and_io_busy_as_well_as_the_engine_barrier() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let engines = Arc::new(super::super::engine_downloads::EngineDownloads::default());
        let io = platform_core::io_lifetime::IoLifetime::default();
        let (lease, project_lease, mut ticket) = engines
            .admit_project("prj_disposable_download", || io.admit())
            .unwrap();
        let stage = PendingDownload::create(&root, "artificial.txt").unwrap();
        let file = stage.file.clone();
        fs::write(&file, b"artificial unfinished engine content").unwrap();
        ticket.started();
        let engine = EngineDownload {
            resource: Some(project_lease.hold(lease.hold(stage))),
            ticket: Some(ticket),
        };
        engines.close();
        io.close();
        io.seal();
        drop(engine);
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(20),
            engines.wait_for_finished()
        )
        .await
        .is_err());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), io.drain())
                .await
                .is_err()
        );
        assert_eq!(
            fs::read(file).unwrap(),
            b"artificial unfinished engine content"
        );
    }
}
