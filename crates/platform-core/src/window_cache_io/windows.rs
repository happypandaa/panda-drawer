//! Private, recoverable preference transactions. Windows has no rename swap or
//! directory fsync: flush the immutable journal, then move only into absent names.
use super::{invalid, private_directory};
use crate::{
    app_windows::{read_snapshot, same_snapshot, Snapshot, MAX_CACHE_BYTES},
    file_identity::PersistedIdentity,
    ApiError,
};
use platform_native::windows::files::{self as native, FileIdentity, PinnedDirectory};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::windows::fs::OpenOptionsExt,
    path::{Component, Path, PathBuf},
};

const CURRENT: &str = "window-size.json";
const JOURNAL: &str = ".window-size-transaction.json";
const MAX_JOURNAL_BYTES: u64 = 64 * 1024;
const SHARE_READ: u32 = 1;
const SHARE_DELETE: u32 = 4;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    identity: PersistedIdentity,
    bytes: Vec<u8>,
}
impl Saved {
    fn from_snapshot(value: &Snapshot) -> Self {
        Self {
            identity: value.identity.clone(),
            bytes: value.bytes.clone(),
        }
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            identity: self.identity.clone(),
            bytes: self.bytes.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", deny_unknown_fields)]
enum Action {
    Publish {
        previous: Option<Saved>,
        next: Saved,
    },
    Retire {
        source: String,
        expected: PersistedIdentity,
        bytes: Option<Vec<u8>>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    version: u32,
    id: String,
    owner: PersistedIdentity,
    action: Action,
}
impl Transaction {
    fn candidate(&self, owner: &Path) -> PathBuf {
        owner.join(format!(".window-size-write-{}.json", self.id))
    }
    fn captured(&self, owner: &Path) -> PathBuf {
        owner.join(format!(".window-size-retiring-{}.json", self.id))
    }
    fn validate(&self, owner: &Scope) -> Result<(), ApiError> {
        let identity_valid = |identity: &PersistedIdentity| {
            identity.windows_file_id.is_some() && identity.volume_uuid.is_none()
        };
        let snapshot_valid = |saved: &Saved| {
            saved.bytes.len() as u64 <= MAX_CACHE_BYTES && identity_valid(&saved.identity)
        };
        let valid = match &self.action {
            Action::Publish { previous, next } => {
                snapshot_valid(next) && previous.as_ref().is_none_or(snapshot_valid)
            }
            Action::Retire {
                source,
                expected,
                bytes,
            } => {
                cache_name(OsStr::new(source)).is_ok()
                    && !source.eq_ignore_ascii_case(JOURNAL)
                    && identity_valid(expected)
                    && bytes
                        .as_ref()
                        .is_none_or(|bytes| bytes.len() as u64 <= MAX_CACHE_BYTES)
            }
        };
        if self.version != 1
            || !uuid::Uuid::parse_str(&self.id)
                .is_ok_and(|id| !id.is_nil() && id.to_string() == self.id)
            || !identity_valid(&self.owner)
            || !self.owner.same_runtime_object(&owner.identity)
            || !valid
        {
            return Err(recovery_required(&owner.path));
        }
        Ok(())
    }
}

struct Scope {
    path: PathBuf,
    identity: PersistedIdentity,
    _pin: PinnedDirectory,
}
impl Scope {
    fn open(owner: &Path) -> Result<Self, ApiError> {
        private_directory(owner)?;
        let pin = native::pin_directory(owner)?;
        native::verify_private(&pin.file)?;
        let identity = PersistedIdentity::from_file(&pin.file)
            .map_err(|_| invalid("窗口尺寸目录身份无法核验。"))?;
        Ok(Self {
            path: owner.into(),
            identity,
            _pin: pin,
        })
    }
}

fn recovery_required(owner: &Path) -> ApiError {
    ApiError::new(
        "WINDOW_CACHE_RECOVERY_REQUIRED",
        "窗口尺寸事务现场尚未核实，已保留所有文件并停止写入。",
    )
    .outcome("unknown")
    .detail(
        "conflictPath",
        owner.join(JOURNAL).to_string_lossy().into_owned(),
    )
}

fn changed(path: &Path) -> ApiError {
    ApiError::new(
        "WINDOW_CACHE_CHANGED",
        "窗口尺寸文件已发生外部变化，事务现场已保留。",
    )
    .outcome("unknown")
    .detail("conflictPath", path.to_string_lossy().into_owned())
}

fn cache_name(name: &OsStr) -> Result<&str, ApiError> {
    let value = name
        .to_str()
        .ok_or_else(|| invalid("窗口尺寸文件名无效。"))?;
    if !(value == CURRENT || value.starts_with(".window-size-"))
        || !matches!(
            Path::new(value).components().next(),
            Some(Component::Normal(_))
        )
        || Path::new(value).components().count() != 1
        || value.contains(['/', '\\', ':'])
        || value.ends_with(['.', ' '])
    {
        return Err(invalid("窗口尺寸文件名无效。"));
    }
    Ok(value)
}

// Only transaction-owned files are locked this way. Existing private read and
// snapshot policies remain unchanged. The journal denies concurrent readers and
// writers; deletion is shared so the bound journal can be removed before close.
fn locked_ordinary(path: &Path, share: u32) -> Result<File, ApiError> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(share)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if platform_native::filesystem::is_reparse(&metadata) || !metadata.is_file() {
        return Err(invalid("窗口尺寸事务材料必须是普通文件。"));
    }
    Ok(file)
}

fn locked_private(path: &Path, share: u32) -> Result<File, ApiError> {
    let file = locked_ordinary(path, share)?;
    if native::identity(&file)?.links != 1 {
        return Err(invalid("窗口尺寸事务记录必须是独立普通文件。"));
    }
    native::verify_private(&file)?;
    Ok(file)
}

struct Journal {
    path: PathBuf,
    identity: FileIdentity,
    transaction: Transaction,
    _file: File,
}
impl Journal {
    fn open(owner: &Scope) -> Result<Option<Self>, ApiError> {
        let path = owner.path.join(JOURNAL);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
            Ok(_) => (),
        }
        let mut file = locked_private(&path, SHARE_DELETE)?;
        if file.metadata()?.len() > MAX_JOURNAL_BYTES {
            return Err(recovery_required(&owner.path));
        }
        let identity = native::identity(&file)?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_JOURNAL_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(recovery_required(&owner.path));
        }
        let transaction: Transaction =
            serde_json::from_slice(&bytes).map_err(|_| recovery_required(&owner.path))?;
        transaction.validate(owner)?;
        Ok(Some(Self {
            path,
            identity,
            transaction,
            _file: file,
        }))
    }
    fn finish(self) -> Result<(), ApiError> {
        native::remove_file(&self.path, &self.identity)
            .map_err(|error| ApiError::from(error).outcome("unknown"))?;
        // Closing this read handle completes deletion of the bound journal.
        drop(self);
        Ok(())
    }
}

fn write_journal(owner: &Scope, transaction: &Transaction) -> Result<Journal, ApiError> {
    transaction.validate(owner)?;
    let path = owner
        .path
        .join(format!(".window-size-journal-{}.json", transaction.id));
    let bytes = serde_json::to_vec(transaction)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(invalid("窗口尺寸事务记录超出上限。"));
    }
    let mut file = native::create_private_file(&path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    // An incomplete preparation never becomes the active journal. An existing
    // transaction is never overwritten or adopted by this invocation.
    native::rename_file(&path, &owner.path.join(JOURNAL), false)
        .map_err(|error| ApiError::from(error).outcome("unknown"))?;
    let journal = Journal::open(owner)?.ok_or_else(|| recovery_required(&owner.path))?;
    if journal.transaction.id != transaction.id {
        return Err(recovery_required(&owner.path));
    }
    Ok(journal)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    JournalReady,
    PreviousCaptured,
    Published,
    RetireCaptured,
}

fn move_absent(from: &Path, to: &Path) -> Result<(), ApiError> {
    native::rename_file(from, to, false).map_err(|error| ApiError::from(error).outcome("unknown"))
}

fn remove_expected(
    path: &Path,
    expected: &PersistedIdentity,
    bytes: Option<&[u8]>,
) -> Result<(), ApiError> {
    // Hold the content read handle without write sharing through the identity
    // and optional byte checks and the DELETE-handle operation. Like the Mac
    // retirement path, do not add leaf DACL or single-link admission here.
    let mut file = locked_ordinary(path, SHARE_READ | SHARE_DELETE)?;
    let observed =
        PersistedIdentity::from_file(&file).map_err(|_| invalid("窗口尺寸移除身份无法核验。"))?;
    if !expected.same_runtime_object(&observed) {
        return Err(changed(path));
    }
    if let Some(expected_bytes) = bytes {
        let mut actual = Vec::new();
        (&mut file)
            .take(MAX_CACHE_BYTES + 1)
            .read_to_end(&mut actual)?;
        if actual != expected_bytes {
            return Err(changed(path));
        }
    }
    native::remove_file_allowing_hard_links(path, &native::identity(&file)?)
        .map_err(|error| ApiError::from(error).outcome("unknown"))?;
    drop(file);
    Ok(())
}

fn apply(
    owner: &Scope,
    transaction: &Transaction,
    observe: &mut impl FnMut(Point) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    transaction.validate(owner)?;
    let captured = transaction.captured(&owner.path);
    match &transaction.action {
        Action::Publish { previous, next } => {
            let candidate = transaction.candidate(&owner.path);
            let active = owner.path.join(CURRENT);
            let previous = previous.as_ref().map(Saved::snapshot);
            let next = next.snapshot();
            let current = read_snapshot(&active)?;
            if same_snapshot(current.as_ref(), Some(&next)) {
                if read_snapshot(&candidate)?.is_some() {
                    return Err(changed(&candidate));
                }
            } else {
                if !same_snapshot(read_snapshot(&candidate)?.as_ref(), Some(&next)) {
                    return Err(changed(&candidate));
                }
                let displaced = read_snapshot(&captured)?;
                if same_snapshot(current.as_ref(), previous.as_ref()) && displaced.is_none() {
                    if previous.is_some() {
                        move_absent(&active, &captured)?;
                        observe(Point::PreviousCaptured)?;
                        if !same_snapshot(read_snapshot(&captured)?.as_ref(), previous.as_ref()) {
                            return Err(changed(&captured));
                        }
                    }
                } else if !(current.is_none()
                    && previous.is_some()
                    && same_snapshot(displaced.as_ref(), previous.as_ref()))
                {
                    return Err(changed(&active));
                }
                move_absent(&candidate, &active)?;
                observe(Point::Published)?;
                if !same_snapshot(read_snapshot(&active)?.as_ref(), Some(&next)) {
                    return Err(changed(&active));
                }
            }
            if let Some(displaced) = read_snapshot(&captured)? {
                if !same_snapshot(Some(&displaced), previous.as_ref()) {
                    return Err(changed(&captured));
                }
                remove_expected(&captured, &displaced.identity, Some(&displaced.bytes))?;
            }
            Ok(())
        }
        Action::Retire {
            source,
            expected,
            bytes,
        } => {
            let source = owner.path.join(source);
            match fs::symlink_metadata(&captured) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    match fs::symlink_metadata(&source) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                        Err(error) => return Err(error.into()),
                        Ok(_) => (),
                    }
                    move_absent(&source, &captured)?;
                    observe(Point::RetireCaptured)?;
                }
                Err(error) => return Err(error.into()),
                Ok(_) => (),
            }
            remove_expected(&captured, expected, bytes.as_deref())
        }
    }
}

pub(crate) fn recover(owner: &Path) -> Result<(), ApiError> {
    let owner = Scope::open(owner)?;
    let Some(journal) = Journal::open(&owner)? else {
        return Ok(());
    };
    apply(&owner, &journal.transaction, &mut |_| Ok(()))?;
    journal.finish()
}

pub(crate) fn publish_cache(
    owner: &Path,
    previous: Option<&Snapshot>,
    bytes: &[u8],
    before_publish: &mut impl FnMut(),
) -> Result<Snapshot, ApiError> {
    publish_with(owner, previous, bytes, before_publish, &mut |_| Ok(()))
}

fn publish_with(
    owner: &Path,
    previous: Option<&Snapshot>,
    bytes: &[u8],
    before_publish: &mut impl FnMut(),
    observe: &mut impl FnMut(Point) -> Result<(), ApiError>,
) -> Result<Snapshot, ApiError> {
    recover(owner)?;
    let owner = Scope::open(owner)?;
    let active = owner.path.join(CURRENT);
    if !same_snapshot(read_snapshot(&active)?.as_ref(), previous) {
        return Err(changed(&active));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let candidate = owner.path.join(format!(".window-size-write-{id}.json"));
    let mut file = native::create_private_file(&candidate)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    let next = Snapshot {
        identity: PersistedIdentity::from_file(&file)
            .map_err(|_| invalid("窗口尺寸暂存身份无法核验。"))?,
        bytes: bytes.into(),
    };
    drop(file);
    let transaction = Transaction {
        version: 1,
        id,
        owner: owner.identity.clone(),
        action: Action::Publish {
            previous: previous.map(Saved::from_snapshot),
            next: Saved::from_snapshot(&next),
        },
    };
    let journal = write_journal(&owner, &transaction)?;
    observe(Point::JournalReady)?;
    before_publish();
    apply(&owner, &transaction, observe)?;
    journal.finish()?;
    let current = read_snapshot(&active)?.ok_or_else(|| changed(&active))?;
    if !same_snapshot(Some(&current), Some(&next)) {
        return Err(changed(&active));
    }
    Ok(current)
}

pub(crate) fn retire_file(
    owner: &Path,
    name: &OsStr,
    expected: &PersistedIdentity,
    bytes: Option<&[u8]>,
) -> Result<(), ApiError> {
    retire_with(owner, name, expected, bytes, &mut |_| Ok(()))
}

fn retire_with(
    owner: &Path,
    name: &OsStr,
    expected: &PersistedIdentity,
    bytes: Option<&[u8]>,
    observe: &mut impl FnMut(Point) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    recover(owner)?;
    let owner = Scope::open(owner)?;
    let name = cache_name(name)?;
    // A new retirement retains the original missing-source failure. Only a
    // previously published journal can establish an idempotent cleanup retry.
    fs::symlink_metadata(owner.path.join(name))?;
    let transaction = Transaction {
        version: 1,
        id: uuid::Uuid::new_v4().to_string(),
        owner: owner.identity.clone(),
        action: Action::Retire {
            source: name.into(),
            expected: expected.clone(),
            bytes: bytes.map(<[u8]>::to_vec),
        },
    };
    let journal = write_journal(&owner, &transaction)?;
    observe(Point::JournalReady)?;
    apply(&owner, &transaction, observe)?;
    journal.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-window-transaction-中文 space-")
            .tempdir()
            .unwrap();
        let owner = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("private-owner");
        native::create_private_directory(&owner).unwrap();
        (temporary, owner)
    }

    fn write_private(path: &Path, bytes: &[u8]) {
        let mut file = native::create_private_file(path).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }

    fn allow_leaf_inheritance(path: &Path) {
        use std::os::windows::process::CommandExt;
        let system_root = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        let output = std::process::Command::new(system_root.join("System32").join("icacls.exe"))
            .arg(path)
            .arg("/inheritance:e")
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Could not change disposable leaf inheritance: {output:?}"
        );
    }

    fn interrupted() -> ApiError {
        ApiError::new(
            "TEST_INTERRUPTED",
            "Disposable transaction boundary interruption",
        )
        .outcome("unknown")
    }

    fn names(owner: &Path) -> Vec<PathBuf> {
        let mut files = fs::read_dir(owner)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        files.sort();
        files
    }

    #[test]
    fn real_first_and_repeated_publications_leave_only_the_private_current_file() {
        let (_temporary, owner) = fixture();
        let mut previous = None;
        for index in 0..12 {
            let bytes = format!("{{\"disposable\":{index}}}");
            let saved =
                publish_cache(&owner, previous.as_ref(), bytes.as_bytes(), &mut || {}).unwrap();
            assert_eq!(saved.bytes, bytes.as_bytes());
            assert_eq!(names(&owner), [owner.join(CURRENT)]);
            let file = super::super::open_private(&owner.join(CURRENT)).unwrap();
            native::verify_private(&file).unwrap();
            if let Some(old) = &previous {
                assert!(!old.identity.same_runtime_object(&saved.identity));
            }
            previous = Some(saved);
        }
        retire_file(
            &owner,
            OsStr::new(CURRENT),
            &previous.unwrap().identity,
            None,
        )
        .unwrap();
        assert!(names(&owner).is_empty());
    }

    #[test]
    fn every_known_two_move_boundary_recovers_without_replaying_over_external_files() {
        for update in [false, true] {
            for point in [
                Point::JournalReady,
                Point::PreviousCaptured,
                Point::Published,
            ] {
                if !update && point == Point::PreviousCaptured {
                    continue;
                }
                let (_temporary, owner) = fixture();
                let previous = update.then(|| {
                    publish_cache(&owner, None, b"old disposable preference", &mut || {}).unwrap()
                });
                let error = publish_with(
                    &owner,
                    previous.as_ref(),
                    b"new disposable preference",
                    &mut || {},
                    &mut |observed| {
                        if observed == point {
                            Err(interrupted())
                        } else {
                            Ok(())
                        }
                    },
                )
                .unwrap_err();
                assert_eq!(error.code, "TEST_INTERRUPTED");
                assert!(owner.join(JOURNAL).exists());
                recover(&owner).unwrap();
                recover(&owner).unwrap();
                assert_eq!(
                    fs::read(owner.join(CURRENT)).unwrap(),
                    b"new disposable preference"
                );
                assert_eq!(names(&owner), [owner.join(CURRENT)]);
            }
        }
    }

    #[test]
    fn an_actual_killed_writer_recovers_after_either_rename_boundary() {
        use std::{
            os::windows::process::CommandExt,
            process::Stdio,
            time::{Duration, Instant},
        };
        for point in ["captured", "published"] {
            let (temporary, owner) = fixture();
            publish_cache(&owner, None, b"before killed writer", &mut || {}).unwrap();
            let ready = temporary.path().join("writer-ready");
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "window_cache_io::windows::tests::interrupted_writer_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("PANDA_WINDOW_CACHE_FIXTURE_OWNER", &owner)
                .env("PANDA_WINDOW_CACHE_FIXTURE_READY", &ready)
                .env("PANDA_WINDOW_CACHE_FIXTURE_POINT", point)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x0800_0000)
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !ready.exists() {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "Writer fixture exited before its rename boundary"
                );
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("Writer fixture did not reach its rename boundary");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            child.kill().unwrap();
            assert!(!child.wait().unwrap().success());
            assert!(owner.join(JOURNAL).exists());
            recover(&owner).unwrap();
            assert_eq!(
                fs::read(owner.join(CURRENT)).unwrap(),
                b"after killed writer"
            );
            assert_eq!(names(&owner), [owner.join(CURRENT)]);
        }
    }

    #[test]
    #[ignore = "Exact child fixture invoked by the real killed-writer test"]
    fn interrupted_writer_fixture() {
        let Some(owner) = std::env::var_os("PANDA_WINDOW_CACHE_FIXTURE_OWNER") else {
            return;
        };
        let owner = PathBuf::from(owner);
        let ready = std::env::var_os("PANDA_WINDOW_CACHE_FIXTURE_READY").unwrap();
        let point = std::env::var("PANDA_WINDOW_CACHE_FIXTURE_POINT").unwrap();
        let previous = read_snapshot(&owner.join(CURRENT)).unwrap().unwrap();
        publish_with(
            &owner,
            Some(&previous),
            b"after killed writer",
            &mut || {},
            &mut |observed| {
                if (point == "captured" && observed == Point::PreviousCaptured)
                    || (point == "published" && observed == Point::Published)
                {
                    let mut marker = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&ready)
                        .unwrap();
                    marker.write_all(b"ready").unwrap();
                    marker.sync_all().unwrap();
                    std::thread::sleep(std::time::Duration::from_secs(30));
                }
                Ok(())
            },
        )
        .unwrap();
    }

    #[test]
    fn a_foreign_replacement_before_capture_is_preserved_with_the_candidate_and_journal() {
        let (_temporary, owner) = fixture();
        let previous = publish_cache(&owner, None, b"original", &mut || {}).unwrap();
        let external_old = owner.join("external-retained-original.json");
        let error = publish_cache(&owner, Some(&previous), b"candidate", &mut || {
            native::rename_file(&owner.join(CURRENT), &external_old, false).unwrap();
            write_private(&owner.join(CURRENT), b"foreign replacement");
        })
        .unwrap_err();
        assert_eq!(error.code, "WINDOW_CACHE_CHANGED");
        let before = names(&owner);
        assert!(recover(&owner).is_err());
        assert_eq!(names(&owner), before);
        assert_eq!(
            fs::read(owner.join(CURRENT)).unwrap(),
            b"foreign replacement"
        );
        assert_eq!(fs::read(external_old).unwrap(), b"original");
        assert!(before.iter().any(|path| path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".window-size-write-")
            && fs::read(path).unwrap() == b"candidate"));
    }

    #[test]
    fn a_foreign_file_between_the_two_moves_is_never_overwritten_or_removed() {
        let (_temporary, owner) = fixture();
        let previous = publish_cache(&owner, None, b"old preference", &mut || {}).unwrap();
        let error = publish_with(
            &owner,
            Some(&previous),
            b"new preference",
            &mut || {},
            &mut |point| {
                if point == Point::PreviousCaptured {
                    write_private(&owner.join(CURRENT), b"external between moves");
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.details["operationOutcome"], "unknown");
        let before = names(&owner);
        assert!(recover(&owner).is_err());
        assert_eq!(names(&owner), before);
        assert_eq!(
            fs::read(owner.join(CURRENT)).unwrap(),
            b"external between moves"
        );
        assert!(before.iter().any(|path| path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".window-size-retiring-")
            && fs::read(path).unwrap() == b"old preference"));
        assert!(before.iter().any(|path| path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".window-size-write-")
            && fs::read(path).unwrap() == b"new preference"));
    }

    #[test]
    fn retirement_recovers_after_capture_but_preserves_identity_or_byte_conflicts() {
        let (_temporary, owner) = fixture();
        let saved = publish_cache(&owner, None, b"retire this", &mut || {}).unwrap();
        assert_eq!(
            retire_with(
                &owner,
                OsStr::new(CURRENT),
                &saved.identity,
                Some(&saved.bytes),
                &mut |point| {
                    if point == Point::RetireCaptured {
                        Err(interrupted())
                    } else {
                        Ok(())
                    }
                }
            )
            .unwrap_err()
            .code,
            "TEST_INTERRUPTED"
        );
        assert!(!owner.join(CURRENT).exists());
        recover(&owner).unwrap();
        assert!(names(&owner).is_empty());

        for identity_conflict in [false, true] {
            let (_temporary, owner) = fixture();
            let saved = publish_cache(&owner, None, b"preserve this", &mut || {}).unwrap();
            let mut expected = saved.identity.clone();
            if identity_conflict {
                expected.windows_file_id.as_mut().unwrap()[15] ^= 1;
            }
            let bytes = if identity_conflict {
                saved.bytes.as_slice()
            } else {
                b"different expected bytes"
            };
            let error =
                retire_file(&owner, OsStr::new(CURRENT), &expected, Some(bytes)).unwrap_err();
            assert_eq!(error.code, "WINDOW_CACHE_CHANGED");
            let retained = PathBuf::from(error.details["conflictPath"].as_str().unwrap());
            assert_eq!(fs::read(&retained).unwrap(), b"preserve this");
            let before = names(&owner);
            assert!(recover(&owner).is_err());
            assert_eq!(names(&owner), before);
            assert_eq!(fs::read(retained).unwrap(), b"preserve this");
        }
    }

    #[test]
    fn retirement_preserves_missing_source_and_optional_byte_check_policies() {
        let (_temporary, owner) = fixture();
        let saved = publish_cache(&owner, None, b"disposable", &mut || {}).unwrap();
        retire_file(&owner, OsStr::new(CURRENT), &saved.identity, None).unwrap();
        assert_eq!(
            retire_file(&owner, OsStr::new(CURRENT), &saved.identity, None)
                .unwrap_err()
                .code,
            "FILESYSTEM_ERROR"
        );
        assert!(names(&owner).is_empty());

        // This is a normal Windows filename longer than 200 UTF-8 bytes. An
        // omitted byte comparison retains the original retirement policy and
        // must not acquire read_snapshot's separate 4096-byte limit.
        let name = format!(".window-size-{}.json", "临".repeat(80));
        let path = owner.join(&name);
        write_private(&path, &vec![b'x'; MAX_CACHE_BYTES as usize + 1]);
        let expected = PersistedIdentity::capture(&path).unwrap();
        retire_file(&owner, OsStr::new(&name), &expected, None).unwrap();
        assert!(names(&owner).is_empty());
    }

    #[test]
    fn retirement_removes_only_the_bound_name_and_preserves_the_other_hard_link() {
        let (_temporary, owner) = fixture();
        let saved = publish_cache(&owner, None, b"retained linked bytes", &mut || {}).unwrap();
        let retained = owner.parent().unwrap().join("disposable-other-link.json");
        fs::hard_link(owner.join(CURRENT), &retained).unwrap();
        assert!(super::super::open_private(&owner.join(CURRENT)).is_err());
        retire_file(
            &owner,
            OsStr::new(CURRENT),
            &saved.identity,
            Some(&saved.bytes),
        )
        .unwrap();
        assert!(names(&owner).is_empty());
        assert_eq!(fs::read(&retained).unwrap(), b"retained linked bytes");
        assert!(saved.identity.matches_path(&retained).unwrap());
        assert_eq!(
            native::open_no_reparse(&retained).unwrap().identity.links,
            1
        );
    }

    #[test]
    fn retirement_does_not_add_a_leaf_dacl_rule_to_owner_and_identity_checks() {
        let (_temporary, owner) = fixture();
        let saved =
            publish_cache(&owner, None, b"same object after DACL change", &mut || {}).unwrap();
        let path = owner.join(CURRENT);
        allow_leaf_inheritance(&path);
        private_directory(&owner).unwrap();
        let opened = native::open_no_reparse(&path).unwrap();
        assert!(native::verify_private(&opened.file).is_err());
        assert!(saved.identity.matches_path(&path).unwrap());
        drop(opened);
        retire_file(
            &owner,
            OsStr::new(CURRENT),
            &saved.identity,
            Some(&saved.bytes),
        )
        .unwrap();
        assert!(names(&owner).is_empty());
    }

    #[test]
    fn recovery_still_requires_an_independent_private_journal() {
        let (_temporary, owner) = fixture();
        assert!(
            publish_with(&owner, None, b"pending", &mut || {}, &mut |point| {
                if point == Point::JournalReady {
                    Err(interrupted())
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        let journal = owner.join(JOURNAL);
        let link = owner.parent().unwrap().join("disposable-journal-link.json");
        fs::hard_link(&journal, &link).unwrap();
        let before = names(&owner);
        assert!(recover(&owner).is_err());
        assert_eq!(names(&owner), before);
        fs::remove_file(&link).unwrap();
        allow_leaf_inheritance(&journal);
        assert!(native::verify_private(&native::open_no_reparse(&journal).unwrap().file).is_err());
        assert!(recover(&owner).is_err());
        assert_eq!(names(&owner), before);
        assert!(!owner.join(CURRENT).exists());
    }

    #[test]
    fn a_live_journal_excludes_concurrent_recovery_and_corrupt_material_stays_untouched() {
        let (_temporary, owner) = fixture();
        publish_with(&owner, None, b"current", &mut || {}, &mut |point| {
            if point == Point::JournalReady {
                assert!(recover(&owner).is_err());
                assert!(publish_cache(&owner, None, b"concurrent", &mut || {}).is_err());
            }
            Ok(())
        })
        .unwrap();
        let previous = read_snapshot(&owner.join(CURRENT)).unwrap().unwrap();
        assert!(publish_with(
            &owner,
            Some(&previous),
            b"pending",
            &mut || {},
            &mut |point| {
                if point == Point::PreviousCaptured {
                    Err(interrupted())
                } else {
                    Ok(())
                }
            }
        )
        .is_err());
        fs::write(owner.join(JOURNAL), b"incomplete foreign journal").unwrap();
        let before = names(&owner);
        assert_eq!(
            recover(&owner).unwrap_err().code,
            "WINDOW_CACHE_RECOVERY_REQUIRED"
        );
        assert_eq!(names(&owner), before);
        assert_eq!(
            fs::read(owner.join(JOURNAL)).unwrap(),
            b"incomplete foreign journal"
        );
    }
}
