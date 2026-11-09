//! Crash-surviving admission for project subprocess writers. No PID from this
//! file is ever signalled: a same-boot orphan has no recoverable Child owner.
use crate::{file_identity::PersistedIdentity, projects::check_no_symlinks, ApiError};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, ApiError>;
const MAX_MARKER: u64 = 4096;
const MAX_WRITERS: usize = 4096;
const MESSAGE: &str = "尚不能确认项目写入进程已结束，暂不能继续。若平台曾异常退出，请重新启动电脑后重试；仅重启平台不能证明旧进程已结束。";
fn blocked() -> ApiError {
    ApiError::new("PROJECT_WRITER_UNCONFIRMED", MESSAGE).outcome("unknown")
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Kind {
    Node,
    Web,
    Platform,
}
#[derive(Clone)]
pub(crate) struct WriterOwner {
    data: PathBuf,
    kind: Kind,
    id: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Marker {
    version: u32,
    kind: Kind,
    owner_id: String,
    token: String,
    boot_unix_micros: u64,
    boot_session_uuid: String,
    component: String,
}
/// There is deliberately no deleting Drop. The process owner must explicitly
/// provide its live group/reap/I/O closure receipt before calling finish().
pub(crate) struct WriterGuard {
    path: PathBuf,
    identity: PersistedIdentity,
    bytes: Vec<u8>,
    closed: bool,
}

impl WriterOwner {
    pub(crate) fn node(data: &Path, id: &str) -> Result<Self> {
        Self::new(data, Kind::Node, id)
    }
    pub(crate) fn web(data: &Path, id: &str) -> Result<Self> {
        Self::new(data, Kind::Web, id)
    }
    pub(crate) fn platform(data: &Path) -> Result<Self> {
        Self::new(data, Kind::Platform, "platform")
    }
    fn new(data: &Path, kind: Kind, id: &str) -> Result<Self> {
        let valid = match kind {
            Kind::Node => Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id),
            Kind::Web => uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id),
            Kind::Platform => id == "platform",
        };
        if !valid
            || !data.is_absolute()
            || data.components().any(|part| {
                !matches!(
                    part,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
        {
            return Err(blocked());
        }
        Ok(Self {
            data: data.into(),
            kind,
            id: id.into(),
        })
    }
    fn root(&self) -> PathBuf {
        match self.kind {
            Kind::Node => self.data.join("source-import-ownership").join(&self.id),
            Kind::Web => self.data.join("app-ownership").join(&self.id),
            Kind::Platform => self.data.join("platform-writers"),
        }
    }
    fn writers(&self) -> PathBuf {
        self.root().join("writers")
    }
    pub(crate) fn begin(&self, component: &str) -> Result<WriterGuard> {
        if component.is_empty() || component.len() > 256 || component.chars().any(char::is_control)
        {
            return Err(blocked());
        }
        // Ownership is established by the admitting project operation. A
        // process must never recreate an owner after deletion or adopt a path.
        if self.kind == Kind::Platform {
            check_no_symlinks(&self.data, false).map_err(|_| blocked())?;
            create_private_directory(&self.root())?;
        }
        check_no_symlinks(&self.root(), false).map_err(|_| blocked())?;
        private_directory(&self.root())?;
        if self.kind != Kind::Platform {
            let owner = open_private(&self.root().join("ownership.json"))?;
            if owner.metadata()?.len() == 0 {
                return Err(blocked());
            }
        }
        let directory = self.writers();
        create_private_directory(&directory)?;
        let token = Uuid::new_v4().to_string();
        let marker = Marker {
            version: 1,
            kind: self.kind,
            owner_id: self.id.clone(),
            token: token.clone(),
            boot_unix_micros: boot()?,
            boot_session_uuid: boot_session()?,
            component: component.into(),
        };
        let bytes = serde_json::to_vec(&marker).map_err(|_| blocked())?;
        let path = directory.join(format!("{token}.json"));
        // A crash or disk-full while writing this unpublished name cannot have
        // spawned a child. Recovery may safely remove only these preparations.
        let preparing = directory.join(format!(".preparing-{}", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&preparing)
            .map_err(|_| blocked())?;
        file.write_all(&bytes).map_err(|_| blocked())?;
        file.sync_all().map_err(|_| blocked())?;
        rename_exclusive(&preparing, &path)?;
        File::open(&directory)?.sync_all().map_err(|_| blocked())?;
        let identity = PersistedIdentity::from_file(&file).map_err(|_| blocked())?;
        Ok(WriterGuard {
            path,
            identity,
            bytes,
            closed: false,
        })
    }
    #[cfg(test)]
    pub(crate) fn fixture_mark_unconfirmed(&self) -> Result<PathBuf> {
        Ok(self.begin("disposable-unconfirmed-writer")?.path)
    }
}
impl WriterGuard {
    pub(crate) fn finish(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        let bytes = read_marker(&self.path)?;
        if bytes != self.bytes
            || !self
                .identity
                .matches_path(&self.path)
                .map_err(|_| blocked())?
        {
            return Err(blocked());
        }
        retire(&self.path, &self.identity, &self.bytes, false)?;
        self.closed = true;
        Ok(())
    }
}
fn boot() -> Result<u64> {
    let value = crate::node_process_identity::boot_time()
        .map_err(|_| blocked())?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| blocked())?
        .as_micros();
    let value = u64::try_from(value).map_err(|_| blocked())?;
    if value == 0 {
        return Err(blocked());
    }
    Ok(value)
}
fn boot_session() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let name = std::ffi::CString::new("kern.bootsessionuuid").map_err(|_| blocked())?;
        let mut bytes = [0u8; 128];
        let mut len = bytes.len();
        if unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                bytes.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        } != 0
            || len == 0
            || len > bytes.len()
        {
            return Err(blocked());
        }
        let value = std::str::from_utf8(bytes[..len].strip_suffix(&[0]).unwrap_or(&bytes[..len]))
            .map_err(|_| blocked())?;
        let uuid = Uuid::parse_str(value).map_err(|_| blocked())?;
        if uuid.is_nil() {
            return Err(blocked());
        }
        Ok(uuid.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(blocked())
    }
}
fn create_private_directory(path: &Path) -> Result<()> {
    check_no_symlinks(path, true).map_err(|_| blocked())?;
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => File::open(path.parent().ok_or_else(blocked)?)?.sync_all()?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err(blocked()),
    }
    private_directory(path)
}
fn valid_marker_name(path: &Path, token: &str) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    name == format!("{token}.json")
        || name
            .strip_prefix(".retiring-")
            .and_then(|s| s.strip_suffix(".json"))
            .is_some_and(|s| Uuid::parse_str(s).is_ok())
}
fn rename_exclusive(from: &Path, to: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from = std::ffi::CString::new(from.as_os_str().as_bytes()).map_err(|_| blocked())?;
    let to = std::ffi::CString::new(to.as_os_str().as_bytes()).map_err(|_| blocked())?;
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(not(target_os = "macos"))]
    let result = {
        let _ = (from, to);
        -1
    };
    if result == 0 {
        Ok(())
    } else {
        Err(blocked())
    }
}
fn retire(path: &Path, identity: &PersistedIdentity, bytes: &[u8], preparing: bool) -> Result<()> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    let parent = path.parent().ok_or_else(blocked)?;
    private_directory(parent)?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(parent)
        .map_err(|_| blocked())?;
    let name =
        CString::new(path.file_name().ok_or_else(blocked)?.as_bytes()).map_err(|_| blocked())?;
    let staged = CString::new(if preparing {
        format!(".preparing-{}", Uuid::new_v4())
    } else {
        format!(".retiring-{}.json", Uuid::new_v4())
    })
    .map_err(|_| blocked())?;
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            name.as_ptr(),
            directory.as_raw_fd(),
            staged.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(not(target_os = "macos"))]
    let result = -1;
    if result != 0 {
        return Err(blocked());
    }
    let validation = (|| -> Result<()> {
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                staged.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(blocked());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let current = PersistedIdentity::from_file(&file).map_err(|_| blocked())?;
        if !identity.same_runtime_object(&current) {
            return Err(blocked());
        }
        let mut current_bytes = Vec::new();
        file.take(MAX_MARKER + 1)
            .read_to_end(&mut current_bytes)
            .map_err(|_| blocked())?;
        if current_bytes != bytes {
            return Err(blocked());
        }
        Ok(())
    })();
    if validation.is_err() {
        #[cfg(target_os = "macos")]
        unsafe {
            libc::renameatx_np(
                directory.as_raw_fd(),
                staged.as_ptr(),
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::RENAME_EXCL,
            );
        }
        return Err(blocked());
    }
    if unsafe { libc::unlinkat(directory.as_raw_fd(), staged.as_ptr(), 0) } != 0 {
        return Err(blocked());
    }
    directory.sync_all().map_err(|_| blocked())
}
fn private_directory(path: &Path) -> Result<()> {
    check_no_symlinks(path, false).map_err(|_| blocked())?;
    let metadata = fs::symlink_metadata(path).map_err(|_| blocked())?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(blocked());
    }
    Ok(())
}
fn open_private(path: &Path) -> Result<File> {
    check_no_symlinks(path, false).map_err(|_| blocked())?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| blocked())?;
    let metadata = file.metadata().map_err(|_| blocked())?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o7777 != 0o600
    {
        return Err(blocked());
    }
    Ok(file)
}
fn read_marker(path: &Path) -> Result<Vec<u8>> {
    let file = open_private(path)?;
    if file.metadata()?.len() > MAX_MARKER {
        return Err(blocked());
    }
    let mut bytes = Vec::new();
    file.take(MAX_MARKER + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| blocked())?;
    if bytes.len() as u64 > MAX_MARKER {
        return Err(blocked());
    }
    Ok(bytes)
}
/// Read-only on an absent directory. A verified earlier OS boot alone permits
/// retiring orphan guards; an interrupted task or restarted platform does not.
pub(crate) fn require_owner_clear(owner: &WriterOwner) -> Result<()> {
    require_owner_clear_at(owner, &boot_session()?)
}
fn require_owner_clear_at(owner: &WriterOwner, current_boot: &str) -> Result<()> {
    let directory = owner.writers();
    match fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(blocked()),
        Ok(_) => (),
    }
    private_directory(&directory)?;
    let mut markers = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|_| blocked())? {
        if markers.len() >= MAX_WRITERS {
            return Err(blocked());
        }
        let path = entry.map_err(|_| blocked())?.path();
        let bytes = read_marker(&path)?;
        let preparing = path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix(".preparing-"))
            .is_some_and(|s| Uuid::parse_str(s).is_ok());
        if preparing {
            markers.push((
                path.clone(),
                PersistedIdentity::capture(&path).map_err(|_| blocked())?,
                bytes,
                true,
            ));
            continue;
        }
        let marker: Marker = serde_json::from_slice(&bytes).map_err(|_| blocked())?;
        if marker.version != 1
            || marker.kind != owner.kind
            || marker.owner_id != owner.id
            || marker.boot_unix_micros == 0
            || Uuid::parse_str(&marker.token).map_or(true, |id| id.is_nil())
            || !valid_marker_name(&path, &marker.token)
            || Uuid::parse_str(&marker.boot_session_uuid).map_or(true, |id| id.is_nil())
            || marker.component.is_empty()
            || marker.component.len() > 256
            || marker.component.chars().any(char::is_control)
        {
            return Err(blocked());
        }
        if current_boot == marker.boot_session_uuid {
            return Err(blocked());
        }
        markers.push((
            path.clone(),
            PersistedIdentity::capture(&path).map_err(|_| blocked())?,
            bytes,
            false,
        ));
    }
    // Validate the complete set before changing any marker. No PID/PGID appears
    // in the schema, and this path never signals or executes another program.
    for (path, identity, bytes, preparing) in markers {
        if read_marker(&path)? != bytes || !identity.matches_path(&path).map_err(|_| blocked())? {
            return Err(blocked());
        }
        retire(&path, &identity, &bytes, preparing)?;
    }
    File::open(directory)?.sync_all().map_err(|_| blocked())?;
    Ok(())
}
pub(crate) fn require_all_clear(data: &Path) -> Result<()> {
    let current_boot = boot_session()?;
    require_owner_clear_at(&WriterOwner::platform(data)?, &current_boot)?;
    for (name, kind) in [
        ("source-import-ownership", Kind::Node),
        ("app-ownership", Kind::Web),
    ] {
        let root = data.join(name);
        match fs::symlink_metadata(&root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(blocked()),
            Ok(_) => (),
        }
        private_directory(&root)?;
        for entry in fs::read_dir(root).map_err(|_| blocked())? {
            let entry = entry.map_err(|_| blocked())?;
            let id = entry.file_name().into_string().map_err(|_| blocked())?;
            if id == ".pending-deletions" {
                continue;
            }
            let owner = WriterOwner::new(data, kind, &id)?;
            require_owner_clear_at(&owner, &current_boot)?;
        }
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    fn fixture() -> (tempfile::TempDir, WriterOwner) {
        let temporary = tempfile::Builder::new()
            .prefix("disposable-durable-writers-")
            .tempdir()
            .unwrap();
        let data = temporary.path().canonicalize().unwrap();
        let owner = WriterOwner::web(&data, "dadf7112-7109-5efa-9571-56d2e3bfbb89").unwrap();
        fs::create_dir_all(owner.root()).unwrap();
        for path in [data.join("app-ownership"), owner.root()] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(owner.root().join("ownership.json"), b"{}").unwrap();
        fs::set_permissions(
            owner.root().join("ownership.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        (temporary, owner)
    }
    #[test]
    fn marker_survives_dropped_guard_and_only_verified_new_boot_retires_it() {
        let (_temp, owner) = fixture();
        let path = owner.fixture_mark_unconfirmed().unwrap();
        assert!(require_owner_clear(&owner).is_err());
        assert!(require_all_clear(&owner.data).is_err());
        let record: Marker = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(require_owner_clear_at(&owner, &record.boot_session_uuid).is_err());
        assert!(path.exists());
        require_owner_clear_at(&owner, &Uuid::new_v4().to_string()).unwrap();
        assert!(!path.exists());
        require_owner_clear(&owner).unwrap();
    }
    #[test]
    fn unknown_schema_damage_links_and_nonprivate_markers_fail_closed() {
        for kind in ["schema", "damage", "link", "permissions"] {
            let (_temp, owner) = fixture();
            let guard = owner.begin("disposable-test").unwrap();
            let path = guard.path.clone();
            drop(guard);
            match kind {
                "schema" => {
                    let mut value: serde_json::Value =
                        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    value["version"] = serde_json::json!(2);
                    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
                }
                "damage" => fs::write(&path, b"unfinished marker").unwrap(),
                "link" => {
                    let other = owner.data.join("author-file");
                    fs::write(&other, b"keep").unwrap();
                    fs::remove_file(&path).unwrap();
                    symlink(other, &path).unwrap();
                }
                _ => fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap(),
            }
            assert!(
                require_owner_clear_at(&owner, &Uuid::new_v4().to_string()).is_err(),
                "{kind}"
            );
            assert!(path.symlink_metadata().is_ok());
        }
    }
    #[test]
    fn explicit_closure_removes_only_its_original_private_marker() {
        let (_temp, owner) = fixture();
        let mut guard = owner.begin("disposable-test").unwrap();
        assert!(require_owner_clear(&owner).is_err());
        guard.finish().unwrap();
        guard.finish().unwrap();
        require_owner_clear(&owner).unwrap();
        assert_eq!(fs::metadata(owner.writers()).unwrap().mode() & 0o777, 0o700);
        let mut guard = owner.begin("disposable-test").unwrap();
        fs::write(&guard.path, b"replacement").unwrap();
        assert!(guard.finish().is_err());
        assert!(guard.path.exists());
    }
    #[test]
    fn checking_absent_scope_never_creates_owner_or_writer_directories() {
        let (_temp, owner) = fixture();
        let directory = owner.writers();
        require_owner_clear(&owner).unwrap();
        assert!(!directory.exists());
    }
    #[test]
    fn incomplete_preparation_is_safe_to_clean_without_accepting_a_published_damage() {
        let (_temp, owner) = fixture();
        let mut guard = owner.begin("disposable-test").unwrap();
        guard.finish().unwrap();
        let preparing = owner
            .writers()
            .join(format!(".preparing-{}", Uuid::new_v4()));
        fs::write(&preparing, b"{partial preparation").unwrap();
        fs::set_permissions(&preparing, fs::Permissions::from_mode(0o600)).unwrap();
        require_owner_clear(&owner).unwrap();
        assert!(!preparing.exists());
        let final_path = owner.writers().join(format!("{}.json", Uuid::new_v4()));
        fs::write(&final_path, b"{partial published marker").unwrap();
        fs::set_permissions(&final_path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(require_owner_clear(&owner).is_err());
        assert!(final_path.exists());
    }
    #[tokio::test]
    async fn registry_guard_outlives_waiter_until_real_output_readers_join() {
        use crate::owned_process::{IoPlan, OwnedProcessRegistry, ProcessOptions};
        let (_temp, owner) = fixture();
        let registry = OwnedProcessRegistry::default().with_writer_owner(owner.clone());
        let (release, wait) = tokio::sync::oneshot::channel();
        let mut command = tokio::process::Command::new("/usr/bin/true");
        command.env_clear();
        let mut run = registry
            .start(
                command,
                ProcessOptions::new("disposable-writer", std::time::Duration::from_secs(5)),
                (),
                move |pipes| {
                    let mut stdout = pipes.stdout;
                    let mut stderr = pipes.stderr;
                    drop(pipes.stdin);
                    IoPlan::new(async move {
                        let mut output = Vec::new();
                        tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut output).await?;
                        Ok(())
                    })
                    .drain("delayed-stderr", async move {
                        let mut output = Vec::new();
                        tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut output).await?;
                        let _ = wait.await;
                        Ok(())
                    })
                },
            )
            .unwrap();
        assert!(require_owner_clear(&owner).is_err());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(40), &mut run)
                .await
                .is_err()
        );
        assert!(require_owner_clear(&owner).is_err());
        release.send(()).unwrap();
        (&mut run).await.unwrap();
        assert!(run.resource_closed().await);
        require_owner_clear(&owner).unwrap();
    }
    #[tokio::test]
    async fn unconfirmed_reader_failure_never_retires_its_guard_on_drop() {
        use crate::owned_process::{IoPlan, OwnedProcessRegistry, ProcessOptions};
        let (_temp, owner) = fixture();
        let registry = OwnedProcessRegistry::default().with_writer_owner(owner.clone());
        let run = registry
            .start(
                tokio::process::Command::new("/usr/bin/true"),
                ProcessOptions::new("disposable-writer", std::time::Duration::from_secs(5)),
                (),
                move |pipes| {
                    let mut stdout = pipes.stdout;
                    drop(pipes.stdin);
                    drop(pipes.stderr);
                    IoPlan::new(async move {
                        let mut output = Vec::new();
                        tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut output).await?;
                        Ok(())
                    })
                    .drain("unconfirmed-output", async move {
                        panic!("disposable writer reader failure");
                        #[allow(unreachable_code)]
                        Ok(())
                    })
                },
            )
            .unwrap();
        assert!(run.await.is_err());
        assert!(require_owner_clear(&owner).is_err());
        assert!(!registry.shutdown().await.is_empty());
        drop(registry);
        assert!(require_owner_clear(&owner).is_err());
    }
    #[tokio::test]
    async fn host_sigkill_orphan_writer_keeps_same_boot_scope_blocked() {
        use crate::owned_process::{IoPlan, OwnedProcessRegistry, ProcessOptions};
        use std::os::unix::process::ExitStatusExt;
        const HELPER: &str = "PANDA_DISPOSABLE_DURABLE_WRITER_HOST";
        if let Some(data) = std::env::var_os(HELPER) {
            let data = PathBuf::from(data);
            let owner = WriterOwner::web(&data, "dadf7112-7109-5efa-9571-56d2e3bfbb89").unwrap();
            let registry = OwnedProcessRegistry::default().with_writer_owner(owner);
            let mut command = tokio::process::Command::new("/usr/bin/python3");
            command.arg("-c").arg("import sys,time;\np=sys.argv[1];end=time.monotonic()+2.5\nwith open(p,'ab',buffering=0) as f:\n while time.monotonic()<end:\n  f.write(b'owned orphan writer tick\\n');time.sleep(.025)\nopen(sys.argv[2],'w').write('writer file closed')").arg(data.join("orphan-output")).arg(data.join("writer-done")).env_clear();
            let _run = registry
                .start(
                    command,
                    ProcessOptions::new(
                        "disposable-orphan-writer",
                        std::time::Duration::from_secs(20),
                    ),
                    (),
                    move |pipes| {
                        let mut stdout = pipes.stdout;
                        let mut stderr = pipes.stderr;
                        drop(pipes.stdin);
                        IoPlan::new(async move {
                            let mut output = Vec::new();
                            tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut output).await?;
                            Ok(())
                        })
                        .drain("stderr", async move {
                            let mut output = Vec::new();
                            tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut output).await?;
                            Ok(())
                        })
                    },
                )
                .unwrap();
            fs::write(
                data.join("host-ready"),
                b"guard published and child spawned",
            )
            .unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            panic!("the disposable host must have been killed by its parent fixture");
        }
        let (_temp, owner) = fixture();
        let mut host = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "durable_writer::unix::tests::host_sigkill_orphan_writer_keeps_same_boot_scope_blocked",
                "--nocapture",
            ])
            .env_clear()
            .env(HELPER, &owner.data)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !owner.data.join("host-ready").exists()
                || !owner.data.join("orphan-output").exists()
            {
                assert!(
                    host.try_wait().unwrap().is_none(),
                    "disposable child host exited before its writer was ready"
                );
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        host.kill().unwrap();
        assert_eq!(host.wait().unwrap().signal(), Some(9));
        let before = fs::metadata(owner.data.join("orphan-output"))
            .unwrap()
            .len();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            fs::metadata(owner.data.join("orphan-output"))
                .unwrap()
                .len()
                > before,
            "the real orphan keeps writing after its actual host SIGKILL"
        );
        assert_eq!(
            require_owner_clear(&owner).unwrap_err().code,
            "PROJECT_WRITER_UNCONFIRMED"
        );
        assert!(require_all_clear(&owner.data).is_err());
        // The bounded artificial writer exits itself. Never kill a stale PID.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !owner.data.join("writer-done").exists() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            require_owner_clear(&owner).is_err(),
            "platform restart/task interruption cannot manufacture a resource closure receipt"
        );
        require_owner_clear_at(&owner, &Uuid::new_v4().to_string()).unwrap();
        assert!(owner.data.join("orphan-output").exists());
    }
}
