//! WKWebView downloads need an explicit handler. Download to an owned private
//! staging directory, then ask where to save with a non-blocking native dialog.
use platform_core::{projects::check_no_symlinks, ApiError, RuntimeHandle};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
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
}
impl PendingDownload {
    fn create(data_root: &Path, suggested_name: &str) -> Result<Self, ApiError> {
        let parent = data_root.join("runtime/native-downloads");
        check_no_symlinks(&parent, true)?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&parent)?;
        check_no_symlinks(&parent, false)?;
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
        Ok(file)
    }
    fn cleanup(&self) {
        if self.validate_directory().is_err() {
            return;
        }
        if fs::symlink_metadata(&self.file)
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        {
            let _ = fs::remove_file(&self.file);
        }
        // Do not recursively remove unrelated files or replaced directories.
        let _ = fs::remove_dir(&self.directory);
    }
}

impl Drop for PendingDownload {
    fn drop(&mut self) {
        // Closing a WebView may drop its callback before Finished is delivered.
        self.cleanup();
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

fn publish_copy(
    stage: &PendingDownload,
    destination: &Path,
    validate: impl Fn(&Path) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    publish_file(stage.open_file()?, destination, validate)
}

/// Publish a completed attachment or browser download after native save consent.
/// The caller keeps any project operation lease alive until this returns.
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
) -> impl Fn(Webview, DownloadEvent<'_>) -> bool + Send + Sync + 'static {
    let pending = Arc::new(Mutex::new(HashMap::<String, PendingDownload>::new()));
    move |webview, event| match event {
        DownloadEvent::Requested { url, destination } => {
            let info = handle.info();
            let permitted = Url::parse(&info.base_url)
                .ok()
                .zip(webview.url().ok())
                .is_some_and(|(base, current)| permitted_url(&base, &id, &current, &url));
            if !permitted
                || handle
                    .store()
                    .project(&id)
                    .and_then(|project| project.validate_epoch(None))
                    .is_err()
            {
                return false;
            }
            let Ok(mut downloads) = pending.lock() else {
                return false;
            };
            let key = url.to_string();
            // Finished.path is unavailable on macOS; URL is our completion key.
            if downloads.contains_key(&key) {
                return false;
            }
            let suggested = destination
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("download.bin");
            match PendingDownload::create(&info.data_root, suggested) {
                Ok(stage) => {
                    *destination = stage.file.clone();
                    downloads.insert(key, stage);
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
            if !success {
                stage.cleanup();
                notify_error(&app, &invalid("Browser download did not complete"));
                return false;
            }
            if let Err(error) = stage.open_file() {
                stage.cleanup();
                notify_error(&app, &error);
                return false;
            }
            let app = app.clone();
            let handle = handle.clone();
            app.dialog()
                .file()
                .set_parent(&webview.window())
                .set_title("保存项目下载")
                .set_file_name(&stage.suggested_name)
                .save_file(move |selected| {
                    // File copying must not block the UI thread or the dialog completion.
                    std::thread::spawn(move || {
                        if let Some(selected) = selected {
                            let result = selected
                                .into_path()
                                .map_err(|error| invalid(&error.to_string()))
                                .and_then(|destination| {
                                    publish_copy(&stage, &destination, |path| {
                                        handle.validate_download_destination(path)
                                    })
                                });
                            if let Err(error) = result {
                                notify_error(&app, &error);
                            }
                        }
                        stage.cleanup();
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
        stage.cleanup();
        assert_eq!(fs::read(&unknown).unwrap(), b"preserve");
        assert!(stage.directory.exists());
        fs::remove_file(&stage.file).unwrap();
        File::create(&stage.file)
            .unwrap()
            .set_len(MAX_DOWNLOAD_BYTES + 1)
            .unwrap();
        assert!(stage.open_file().is_err());
        stage.cleanup();
        assert!(!stage.directory.exists());
        let stage = PendingDownload::create(&root, "test").unwrap();
        let held = stage.directory.with_extension("held");
        fs::rename(&stage.directory, &held).unwrap();
        fs::create_dir(&stage.directory).unwrap();
        fs::write(stage.directory.join("unknown"), b"preserve").unwrap();
        assert!(stage.open_file().is_err());
        stage.cleanup();
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
        stage.cleanup();
        fs::remove_dir_all(root).unwrap();
    }
}
