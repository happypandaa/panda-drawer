//! Two private, bounded runtime logs. Only redacted diagnostics reach the file.
use crate::ApiError;
use serde_json::{json, Value};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, Mutex},
};

pub(crate) const LOG_LIMIT: usize = 256 * 1024;
const LINE_LIMIT: usize = 16 * 1024;
type Result<T> = std::result::Result<T, ApiError>;

fn failure() -> ApiError {
    ApiError::new(
        "NODE_RUNTIME_LOG_IO",
        "运行日志无法写入，请检查本应用控制目录的权限和可用磁盘空间。",
    )
}

#[derive(Clone)]
pub(crate) struct RuntimeLog(Arc<Mutex<LogState>>);
struct LogState {
    file: File,
    error: Option<ApiError>,
}

impl RuntimeLog {
    /// Called only at actual launch, after preflight. Focusing an existing app
    /// and read-only preflight never rotate its logs.
    pub(crate) fn start(control: &Path) -> Result<Self> {
        let file = private_current(control).map_err(|_| failure())?;
        Ok(Self(Arc::new(Mutex::new(LogState { file, error: None }))))
    }
    #[cfg(test)]
    pub(crate) fn fixture_replace_file(&self, file: File) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).file = file;
    }
    pub(crate) fn error(&self) -> Option<ApiError> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .error
            .clone()
    }
    pub(crate) fn line(&self, bytes: &[u8]) {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if state.error.is_some() {
            return;
        }
        if append_bounded(&mut state.file, bytes).is_err() {
            state.error = Some(failure());
        }
    }
    pub(crate) fn event(&self, name: &str, mut fields: Value) {
        fields["platformEvent"] = json!(name);
        fields["time"] = json!(time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "unavailable".into()));
        if let Ok(mut bytes) = serde_json::to_vec(&fields) {
            bytes.push(b'\n');
            self.line(&bytes);
        }
    }
}

fn append_bounded(file: &mut File, bytes: &[u8]) -> std::io::Result<()> {
    // Every caller supplies one bounded sanitized line, including platform events.
    if bytes.len() > LOG_LIMIT {
        return Err(std::io::Error::other("oversized log line"));
    }
    let metadata = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o7777 != 0o600
        {
            return Err(std::io::Error::other("unsafe runtime log inode"));
        }
    }
    let len = metadata.len();
    if len.saturating_add(bytes.len() as u64) > LOG_LIMIT as u64 {
        // Leave 64 KiB of headroom so repeated short diagnostic lines do
        // not rewrite the entire retained tail for every incoming read block.
        let keep = (LOG_LIMIT * 3 / 4)
            .saturating_sub(bytes.len())
            .min(len as usize);
        file.seek(SeekFrom::Start(len - keep as u64))?;
        let mut tail = vec![0; keep];
        file.read_exact(&mut tail)?;
        // Discard the partial oldest line after truncation, retaining whole lines.
        let start = tail
            .iter()
            .position(|b| *b == b'\n')
            .map_or(tail.len(), |i| i + 1);
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        file.write_all(&tail[start..])?;
    } else {
        file.seek(SeekFrom::End(0))?;
    }
    file.write_all(bytes)
}

/// Streaming exact-value redaction precedes line splitting. Pending raw bytes
/// never exceed the longest configured secret plus one input chunk. Very long
/// lines are split at 16 KiB so an application without newlines stays bounded.
pub(crate) struct DiagnosticLines {
    secrets: Vec<Vec<u8>>,
    pending: Vec<u8>,
    line: Vec<u8>,
    batch: Vec<u8>,
    log: RuntimeLog,
}
impl DiagnosticLines {
    pub(crate) fn new(log: RuntimeLog, secrets: Vec<String>) -> Self {
        let mut secrets: Vec<_> = secrets
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(String::into_bytes)
            .collect();
        secrets.sort_by_key(|b| std::cmp::Reverse(b.len()));
        secrets.dedup();
        Self {
            secrets,
            pending: Vec::new(),
            line: Vec::new(),
            batch: Vec::new(),
            log,
        }
    }
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        self.drain(false);
        self.flush_batch();
    }
    pub(crate) fn finish(&mut self) {
        self.drain(true);
        if !self.line.is_empty() {
            self.flush(self.line.len());
        }
        self.flush_batch();
    }
    fn drain(&mut self, finish: bool) {
        let mut read = 0;
        while read < self.pending.len() {
            let remaining = &self.pending[read..];
            // A longer secret may share a prefix with a shorter one. Wait until
            // it can be ruled out before emitting or replacing either value.
            if !finish
                && self
                    .secrets
                    .iter()
                    .any(|s| remaining.len() < s.len() && s.starts_with(remaining))
            {
                break;
            }
            let matched = self
                .secrets
                .iter()
                .find(|s| remaining.starts_with(s))
                .map(Vec::len);
            if let Some(count) = matched {
                self.emit(b"***");
                read += count;
            } else {
                let byte = self.pending[read];
                self.emit(&[byte]);
                read += 1;
            }
        }
        self.pending.drain(..read);
    }
    fn emit(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.line.push(*byte);
            if *byte == b'\n' {
                self.flush(self.line.len());
            } else if self.line.len() >= LINE_LIMIT {
                // Keep an unfinished trailing UTF-8 character for the next
                // segment. Invalid bytes elsewhere are rendered as U+FFFD.
                let mut end = self.line.len();
                for offset in 1..=4.min(self.line.len()) {
                    let index = self.line.len() - offset;
                    let byte = self.line[index];
                    if byte < 0x80 {
                        break;
                    }
                    if byte >= 0xc0 {
                        let expected = match byte {
                            0xc2..=0xdf => 2,
                            0xe0..=0xef => 3,
                            0xf0..=0xf4 => 4,
                            _ => 0,
                        };
                        if expected > offset
                            && self.line[index + 1..]
                                .iter()
                                .all(|b| (0x80..=0xbf).contains(b))
                        {
                            end = index;
                        }
                        break;
                    }
                }
                if end > 0 {
                    self.flush(end);
                }
            }
        }
    }
    fn flush(&mut self, count: usize) {
        let mut text = String::from_utf8_lossy(&self.line[..count]).into_owned();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        if self.batch.len() + text.len() > 64 * 1024 {
            self.flush_batch();
        }
        self.batch.extend_from_slice(text.as_bytes());
        self.line.drain(..count);
    }
    fn flush_batch(&mut self) {
        if !self.batch.is_empty() {
            self.log.line(&self.batch);
            self.batch.clear();
        }
    }
}

#[cfg(unix)]
fn private_current(control: &Path) -> std::io::Result<File> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
        path::Component,
    };
    fn c(value: &[u8]) -> std::io::Result<CString> {
        CString::new(value).map_err(|_| std::io::Error::other("invalid log path"))
    }
    fn opened(fd: i32) -> std::io::Result<File> {
        if fd < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }
    let mut directory = opened(unsafe {
        libc::open(
            c(b"/")?.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    })?;
    if !control.is_absolute() {
        return Err(std::io::Error::other("relative log path"));
    }
    for part in control.components() {
        match part {
            Component::RootDir => (),
            Component::Normal(name) => {
                directory = opened(unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        c(name.as_bytes())?.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                })?;
            }
            _ => return Err(std::io::Error::other("noncanonical log path")),
        }
    }
    let logs = c(b"logs")?;
    if unsafe { libc::mkdirat(directory.as_raw_fd(), logs.as_ptr(), 0o700) } != 0
        && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(std::io::Error::last_os_error());
    }
    let directory = opened(unsafe {
        libc::openat(
            directory.as_raw_fd(),
            logs.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    })?;
    let metadata = directory.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o7777 != 0o700 {
        return Err(std::io::Error::other("unsafe log directory"));
    }
    let current = c(b"current.log")?;
    let previous = c(b"previous.log")?;
    // Open every existing target without following links or blocking on FIFOs,
    // and inspect the inode before rename/removal. The directory FD pins scope.
    let check = |name: &CString| -> std::io::Result<bool> {
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            return Ok(false);
        }
        let file = opened(fd)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o7777 != 0o600
            || metadata.len() > LOG_LIMIT as u64
        {
            return Err(std::io::Error::other("unsafe log file"));
        }
        Ok(true)
    };
    let has_current = check(&current)?;
    let has_previous = check(&previous)?;
    if has_current {
        if has_previous
            && unsafe { libc::unlinkat(directory.as_raw_fd(), previous.as_ptr(), 0) } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        if unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                current.as_ptr(),
                directory.as_raw_fd(),
                previous.as_ptr(),
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    opened(unsafe {
        libc::openat(
            directory.as_raw_fd(),
            current.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    })
}
#[cfg(not(unix))]
fn private_current(_control: &Path) -> std::io::Result<File> {
    Err(std::io::Error::other("private runtime logs unsupported"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{symlink, PermissionsExt},
    };
    fn root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("disposable-node-runtime-logs-")
            .tempdir()
            .unwrap()
    }
    fn text(root: &Path) -> String {
        fs::read_to_string(root.join("logs/current.log")).unwrap()
    }
    #[test]
    fn redacts_cross_chunk_overlapping_and_partial_secret_values_before_writing() {
        let root = root();
        let path = fs::canonicalize(root.path()).unwrap();
        let log = RuntimeLog::start(&path).unwrap();
        let mut lines = DiagnosticLines::new(
            log,
            vec!["abc".into(), "abcdef".into(), "私人密钥".into(), "".into()],
        );
        for byte in "before abcdef 私人密钥 abc\nlast abcdef".as_bytes() {
            lines.push(&[*byte]);
        }
        lines.finish();
        assert_eq!(text(&path), "before *** *** ***\nlast ***\n");
    }
    #[test]
    fn long_lines_preserve_multibyte_boundaries_and_replace_invalid_encoding() {
        let root = root();
        let path = fs::canonicalize(root.path()).unwrap();
        let mut lines =
            DiagnosticLines::new(RuntimeLog::start(&path).unwrap(), vec!["私人密钥".into()]);
        let mut bytes = vec![b'x'; LINE_LIMIT - 1];
        bytes.extend_from_slice("中文😀私人密钥".as_bytes());
        bytes.extend_from_slice(&[0xff, b'\n']);
        for chunk in bytes.chunks(7) {
            lines.push(chunk);
        }
        lines.finish();
        let output = text(&path);
        assert!(output.contains("中文😀***�\n"));
        assert!(!output.contains("私人密钥"));
        assert!(!output.contains("�中"));
    }
    #[test]
    fn rolling_logs_are_bounded_private_and_rotate_only_two_files() {
        let root = root();
        let path = fs::canonicalize(root.path()).unwrap();
        let log = RuntimeLog::start(&path).unwrap();
        let mut lines = DiagnosticLines::new(log.clone(), vec!["artificial-key".into()]);
        for _ in 0..300 {
            lines.push(&vec![b'x'; 4096]);
        }
        lines.push(b"artificial-key\nnewest\n");
        lines.finish();
        log.event("exited", json!({"exitCode":0,"signal":null}));
        let old = text(&path);
        assert!(old.len() <= LOG_LIMIT);
        assert!(old.contains("***\nnewest\n"));
        assert!(!old.contains("artificial-key"));
        assert_eq!(
            fs::metadata(path.join("logs"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(path.join("logs/current.log"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(lines);
        drop(log);
        let second = RuntimeLog::start(&path).unwrap();
        second.line(b"second\n");
        assert_eq!(
            fs::read_to_string(path.join("logs/previous.log")).unwrap(),
            old
        );
        drop(second);
        let _third = RuntimeLog::start(&path).unwrap();
        assert_eq!(
            fs::read_to_string(path.join("logs/previous.log")).unwrap(),
            "second\n"
        );
        assert_eq!(fs::read_dir(path.join("logs")).unwrap().count(), 2);
    }
    #[test]
    fn many_short_lines_are_batched_and_remain_bounded() {
        let root = root();
        let path = fs::canonicalize(root.path()).unwrap();
        let mut lines = DiagnosticLines::new(RuntimeLog::start(&path).unwrap(), vec![]);
        let chunk = b"x\n".repeat(2048);
        for _ in 0..256 {
            lines.push(&chunk);
        }
        lines.push(b"latest-short-line\n");
        lines.finish();
        let output = text(&path);
        assert!(output.len() <= LOG_LIMIT);
        assert!(output.ends_with("latest-short-line\n"));
        assert!(output
            .lines()
            .all(|line| line == "x" || line == "latest-short-line"));
    }
    #[test]
    fn refuses_links_special_files_and_wrong_permissions() {
        for kind in [
            "directory-link",
            "file-link",
            "hardlink",
            "fifo",
            "permissions",
        ] {
            let root = root();
            let path = fs::canonicalize(root.path()).unwrap();
            let external = path.join("author-file");
            fs::write(&external, b"keep author source").unwrap();
            if kind == "directory-link" {
                symlink(&path, path.join("logs")).unwrap();
            } else {
                fs::create_dir(path.join("logs")).unwrap();
                fs::set_permissions(path.join("logs"), fs::Permissions::from_mode(0o700)).unwrap();
                let target = path.join("logs/current.log");
                match kind {
                    "file-link" => symlink(&external, target).unwrap(),
                    "hardlink" => fs::hard_link(&external, target).unwrap(),
                    "fifo" => {
                        use std::{ffi::CString, os::unix::ffi::OsStrExt};
                        assert_eq!(
                            unsafe {
                                libc::mkfifo(
                                    CString::new(target.as_os_str().as_bytes())
                                        .unwrap()
                                        .as_ptr(),
                                    0o600,
                                )
                            },
                            0
                        );
                    }
                    _ => {
                        fs::write(&target, b"wrong mode").unwrap();
                        fs::set_permissions(target, fs::Permissions::from_mode(0o644)).unwrap();
                    }
                }
            }
            assert!(RuntimeLog::start(&path).is_err(), "{kind}");
            assert_eq!(fs::read(external).unwrap(), b"keep author source");
        }
    }
}
