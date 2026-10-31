use super::invalid;
use platform_core::{native_project_io::NativeProjectIoGuard, ApiError};
use platform_native::windows::{files as native, removal};
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);
type Result<T> = std::result::Result<T, ApiError>;
fn name() -> String {
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
pub(super) struct PendingDownload {
    pub(super) file: PathBuf,
    pub(super) suggested_name: String,
    pub(super) writer: Option<NativeProjectIoGuard>,
    pub(super) engine_accepted: AtomicBool,
    pub(super) engine_finished: AtomicBool,
    directory: PathBuf,
    directory_identity: native::FileIdentity,
    directory_pin: Option<native::PinnedDirectory>,
}
impl PendingDownload {
    pub(super) fn create_owned(writer: NativeProjectIoGuard, suggested_name: &str) -> Result<Self> {
        // The writer and original fixed owner subtree are recorded before the
        // engine can receive any pathname or create its first payload byte.
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
    pub(super) fn create_at(parent: &Path, suggested_name: &str) -> Result<Self> {
        let _parent = native::pin_directory(
            parent
                .parent()
                .ok_or_else(|| invalid("Download parent is missing"))?,
        )?;
        match native::create_private_directory(parent) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        let _scope = native::pin_directory(parent)?;
        let directory = parent.join(name());
        native::create_private_directory(&directory)?;
        let pin = native::pin_directory(&directory)?;
        let directory_identity = native::identity(&pin.file)?;
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
            directory_pin: Some(pin),
            suggested_name,
            writer: None,
            engine_accepted: AtomicBool::new(false),
            engine_finished: AtomicBool::new(false),
        })
    }
    fn validate_directory(&self) -> Result<()> {
        let actual = native::open_no_reparse(&self.directory)?;
        if !actual.identity.directory || !self.directory_identity.same_object(&actual.identity) {
            return Err(invalid("Temporary download directory changed"));
        }
        Ok(())
    }
    pub(super) fn open_file(&self) -> Result<File> {
        if self.engine_accepted.load(Ordering::Acquire)
            && !self.engine_finished.load(Ordering::Acquire)
        {
            return Err(invalid("Browser download has not confirmed Finished"));
        }
        if let Some(writer) = &self.writer {
            writer.validate()?;
        }
        self.validate_directory()?;
        let actual = native::open_no_reparse(&self.file)?;
        if actual.identity.directory || actual.file.metadata()?.len() > MAX_DOWNLOAD_BYTES {
            return Err(invalid(
                "Download exceeds the internal 256 MiB save limit or is not a regular file",
            ));
        }
        let file =
            native::open_owned_file_private_allowing_hard_links(&self.file, &actual.identity)?;
        if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_DOWNLOAD_BYTES {
            return Err(invalid("Temporary download changed before saving"));
        }
        Ok(file)
    }
    pub(super) fn cleanup(&mut self) -> Result<()> {
        self.validate_directory()?;
        match native::open_no_reparse(&self.file) {
            Ok(actual) if !actual.identity.directory => {
                let identity = actual.identity;
                drop(actual.file);
                remove(&self.file, &identity)?;
            }
            Ok(_) => {
                return Err(invalid(
                    "Temporary download has an unknown payload type; preserving it",
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        // No recursive deletion: unknown child names keep the directory and
        // writer marker. Release only our exact directory pin before removal.
        drop(self.directory_pin.take());
        remove(&self.directory, &self.directory_identity)
    }
}
fn remove(path: &Path, identity: &native::FileIdentity) -> Result<()> {
    let _parent = native::pin_directory(
        path.parent()
            .ok_or_else(|| invalid("Download parent is missing"))?,
    )?;
    let busy = |error: &io::Error| matches!(error.raw_os_error(), Some(5 | 32 | 33));
    let mut waits = [20, 40, 80, 160, 320, 640, 1000].into_iter();
    loop {
        match removal::remove(path, identity) {
            Ok(()) => break,
            Err(error) if busy(&error) => {
                if let Some(ms) = waits.next() {
                    std::thread::sleep(Duration::from_millis(ms));
                } else {
                    return Err(error.into());
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    for ms in [0, 20, 40, 80, 160, 320, 640, 1000] {
        if ms > 0 {
            std::thread::sleep(Duration::from_millis(ms));
        }
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) if busy(&error) => (),
            Err(error) => return Err(error.into()),
            Ok(_) => match removal::inspect(path) {
                Ok(entry) if !identity.same_object(&entry.identity) => {
                    return Err(invalid("Temporary download was replaced during cleanup"))
                }
                Ok(_) => (),
                Err(error) if busy(&error) || error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            },
        }
    }
    Err(invalid("Temporary download cleanup remains pending")
        .outcome("unknown")
        .detail("remainingPath", path.to_string_lossy().into_owned()))
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
