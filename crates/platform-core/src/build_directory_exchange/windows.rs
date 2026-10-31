//! Recoverable no-overwrite directory exchange. Windows has no rename swap or
//! directory fsync. Flush the immutable task-local journal before any moves;
//! each successful rename still requires the exact final namespace observation.
use crate::{builds::ensure_dir, projects::check_no_symlinks, ApiError};
use platform_native::windows::files::{self as native, FileIdentity, PinnedDirectory};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{OsStr, OsString},
    fs, io,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Component, Path, PathBuf},
};

#[path = "windows_journal.rs"]
mod journal;
use journal::Journal;

const JOURNAL: &str = ".artifact-directory-exchange.json";
type Result<T> = std::result::Result<T, ApiError>;

fn unconfirmed(staged: &Path, cause: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        "BUILD_ARTIFACT_EXCHANGE_UNCONFIRMED",
        "产物目录交换尚未核实，已保留事务和所有目录，未授权回滚或清理。",
    )
    .outcome("unknown")
    .detail("artifactExchangePending", true)
    .detail(
        "exchangeJournal",
        journal_path(staged).to_string_lossy().into_owned(),
    )
    .detail("cause", cause.to_string())
}

fn journal_path(staged: &Path) -> PathBuf {
    staged.parent().unwrap_or(staged).join(JOURNAL)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryId {
    volume_serial: u64,
    file_id: [u8; 16],
}
impl DirectoryId {
    fn from_native(identity: &FileIdentity) -> io::Result<Self> {
        if !identity.directory {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "Expected an ordinary directory",
            ));
        }
        Ok(Self {
            volume_serial: identity.volume_serial,
            file_id: identity.file_id,
        })
    }
    fn native(self) -> FileIdentity {
        FileIdentity {
            volume_serial: self.volume_serial,
            file_id: self.file_id,
            links: 0,
            directory: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Name(Vec<u16>);
impl Name {
    fn from_os(value: &OsStr) -> io::Result<Self> {
        let name = Self(value.encode_wide().collect());
        name.checked()?;
        Ok(name)
    }
    fn checked(&self) -> io::Result<OsString> {
        let name = OsString::from_wide(&self.0);
        if self.0.contains(&0)
            || self
                .0
                .iter()
                .any(|unit| *unit == u16::from(b'/') || *unit == u16::from(b'\\'))
            || !matches!(
                Path::new(&name).components().next(),
                Some(Component::Normal(_))
            )
            || Path::new(&name).components().count() != 1
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Exchange journal name is not one leaf",
            ));
        }
        Ok(name)
    }
}

struct Parent {
    path: PathBuf,
    identity: DirectoryId,
    _pins: [PinnedDirectory; 2],
}
impl Parent {
    fn open(path: &Path) -> io::Result<Self> {
        let original = native::pin_directory(path)?;
        let (path, canonical) = native::directory_scope(&original.file)?;
        Ok(Self {
            path,
            identity: DirectoryId::from_native(&native::identity(&original.file)?)?,
            _pins: [original, canonical],
        })
    }
}

struct Pair {
    staged_parent: Parent,
    current_parent: Parent,
    staged_name: Name,
    current_name: Name,
}
impl Pair {
    fn open(staged: &Path, current: &Path) -> io::Result<Self> {
        let invalid = || {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Exchange path has no parent or leaf",
            )
        };
        let staged_parent = Parent::open(staged.parent().ok_or_else(invalid)?)?;
        let current_parent = Parent::open(current.parent().ok_or_else(invalid)?)?;
        Ok(Self {
            staged_parent,
            current_parent,
            staged_name: Name::from_os(staged.file_name().ok_or_else(invalid)?)?,
            current_name: Name::from_os(current.file_name().ok_or_else(invalid)?)?,
        })
    }
    fn staged(&self) -> PathBuf {
        self.staged_parent
            .path
            .join(OsString::from_wide(&self.staged_name.0))
    }
    fn current(&self) -> PathBuf {
        self.current_parent
            .path
            .join(OsString::from_wide(&self.current_name.0))
    }
    fn held(&self, transaction: &Transaction) -> PathBuf {
        self.staged_parent.path.join(&transaction.held)
    }
    fn journal(&self) -> PathBuf {
        self.staged_parent.path.join(JOURNAL)
    }
    fn verify(&self) -> io::Result<()> {
        for parent in [&self.staged_parent, &self.current_parent] {
            let observed = native::open_no_reparse(&parent.path)?;
            if DirectoryId::from_native(&observed.identity)? != parent.identity {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Exchange parent identity changed",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    version: u32,
    id: String,
    staged_parent: DirectoryId,
    current_parent: DirectoryId,
    staged_name: Name,
    current_name: Name,
    next: DirectoryId,
    previous: Option<DirectoryId>,
    held: String,
}
impl Transaction {
    fn validate(&self, pair: &Pair) -> io::Result<()> {
        let uuid = uuid::Uuid::parse_str(&self.id).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "Invalid exchange journal ID")
        })?;
        self.staged_name.checked()?;
        self.current_name.checked()?;
        if self.version != 1
            || uuid.is_nil()
            || uuid.to_string() != self.id
            || self.held != format!(".artifact-directory-held-{}", self.id)
            || self.staged_parent != pair.staged_parent.identity
            || self.current_parent != pair.current_parent.identity
            || self.staged_name != pair.staged_name
            || self.current_name != pair.current_name
            || self.next.volume_serial != self.staged_parent.volume_serial
            || self.current_parent.volume_serial != self.staged_parent.volume_serial
            || self.previous.is_some_and(|previous| {
                previous.volume_serial != self.current_parent.volume_serial || previous == self.next
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Exchange journal is not bound to this directory pair",
            ));
        }
        pair.verify()
    }
}

fn directory(path: &Path) -> io::Result<Option<DirectoryId>> {
    match native::open_no_reparse(path) {
        Ok(opened) => DirectoryId::from_native(&opened.identity).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    JournalReady,
    PreviousHeld,
    NextPublished,
    Exchanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    HoldPrevious,
    PublishNext,
    RestorePreviousName,
    Complete,
}

fn step(pair: &Pair, transaction: &Transaction) -> io::Result<Step> {
    pair.verify()?;
    let staged = directory(&pair.staged())?;
    let current = directory(&pair.current())?;
    let held = directory(&pair.held(transaction))?;
    let next = Some(transaction.next);
    let previous = transaction.previous;
    let result = if let Some(previous_id) = previous {
        match (staged, current, held) {
            (s, c, None) if s == next && c == previous => Some(Step::HoldPrevious),
            (s, None, h) if s == next && h == previous => Some(Step::PublishNext),
            (None, c, h) if c == next && h == previous => Some(Step::RestorePreviousName),
            (Some(s), c, None) if s == previous_id && c == next => Some(Step::Complete),
            _ => None,
        }
    } else {
        match (staged, current, held) {
            (s, None, None) if s == next => Some(Step::PublishNext),
            (None, c, None) if c == next => Some(Step::Complete),
            _ => None,
        }
    };
    result.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Exchange names no longer match any exact journal state",
        )
    })
}

fn apply(
    pair: &Pair,
    journal: &Journal,
    observe: &mut impl FnMut(Point, &Transaction) -> Result<()>,
) -> Result<()> {
    loop {
        journal
            .verify(pair)
            .map_err(|error| unconfirmed(&pair.staged(), error))?;
        let transaction = &journal.transaction;
        let selected =
            step(pair, transaction).map_err(|error| unconfirmed(&pair.staged(), error))?;
        let (from, to, expected, point) = match selected {
            Step::HoldPrevious => (
                pair.current(),
                pair.held(transaction),
                transaction.previous.unwrap(),
                Point::PreviousHeld,
            ),
            Step::PublishNext => (
                pair.staged(),
                pair.current(),
                transaction.next,
                Point::NextPublished,
            ),
            Step::RestorePreviousName => (
                pair.held(transaction),
                pair.staged(),
                transaction.previous.unwrap(),
                Point::Exchanged,
            ),
            Step::Complete => return Ok(()),
        };
        native::rename_directory_noreplace_expected(&from, &to, &expected.native())
            .map_err(|error| unconfirmed(&pair.staged(), error))?;
        observe(point, transaction).map_err(|error| unconfirmed(&pair.staged(), error))?;
    }
}

fn journal_exists(staged: &Path) -> io::Result<bool> {
    // Pin the actual task parent even for a missing-name observation. No
    // current parent is created or required by this read-only checkpoint.
    let parent = Parent::open(staged.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Staged directory has no parent",
        )
    })?)?;
    match fs::symlink_metadata(parent.path.join(JOURNAL)) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub(super) fn pending(staged: &Path, current: &Path) -> Result<bool> {
    if !journal_exists(staged).map_err(|error| unconfirmed(staged, error))? {
        return Ok(false);
    }
    let pair = Pair::open(staged, current).map_err(|error| unconfirmed(staged, error))?;
    let journal = Journal::open(&pair)
        .map_err(|error| unconfirmed(staged, error))?
        .ok_or_else(|| unconfirmed(staged, "Exchange journal disappeared during observation"))?;
    journal
        .verify(&pair)
        .and_then(|()| step(&pair, &journal.transaction).map(|_| ()))
        .map_err(|error| unconfirmed(staged, error))?;
    Ok(true)
}

pub(super) fn resume_pending(staged: &Path, current: &Path) -> Result<bool> {
    resume_with(staged, current, &mut |_, _| Ok(()))
}
fn resume_with(
    staged: &Path,
    current: &Path,
    observe: &mut impl FnMut(Point, &Transaction) -> Result<()>,
) -> Result<bool> {
    if !journal_exists(staged).map_err(|error| unconfirmed(staged, error))? {
        return Ok(false);
    }
    let pair = Pair::open(staged, current).map_err(|error| unconfirmed(staged, error))?;
    let journal = Journal::open(&pair)
        .map_err(|error| unconfirmed(staged, error))?
        .ok_or_else(|| unconfirmed(staged, "Exchange journal disappeared before recovery"))?;
    apply(&pair, &journal, observe)?;
    journal
        .finish(&pair)
        .map_err(|error| unconfirmed(staged, error))?;
    Ok(true)
}

pub(super) fn replace_directory(staged: &Path, current: &Path) -> Result<()> {
    replace_with(staged, current, &mut |_, _| Ok(()))
}
fn replace_with(
    staged: &Path,
    current: &Path,
    observe: &mut impl FnMut(Point, &Transaction) -> Result<()>,
) -> Result<()> {
    // Pending recovery must precede the ordinary source existence check: the
    // second move deliberately leaves the staged name absent.
    if resume_with(staged, current, observe)? {
        return Ok(());
    }
    check_no_symlinks(staged, false)?;
    check_no_symlinks(current, true)?;
    if !current.exists() {
        ensure_dir(current.parent().unwrap())?;
    }
    let pair = Pair::open(staged, current)?;
    let next = directory(&pair.staged())?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Staged artifact directory is missing",
        )
    })?;
    let previous = directory(&pair.current())?;
    if previous == Some(next) {
        return Ok(());
    }
    if pair.staged_parent.identity.volume_serial != pair.current_parent.identity.volume_serial {
        return Err(ApiError::new(
            "BUILD_ARTIFACT_EXCHANGE_CROSS_VOLUME",
            "Artifact directory exchange requires one filesystem",
        )
        .outcome("not_started"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let transaction = Transaction {
        version: 1,
        held: format!(".artifact-directory-held-{id}"),
        id,
        staged_parent: pair.staged_parent.identity,
        current_parent: pair.current_parent.identity,
        staged_name: pair.staged_name.clone(),
        current_name: pair.current_name.clone(),
        next,
        previous,
    };
    transaction.validate(&pair)?;
    if directory(&pair.held(&transaction))?.is_some() {
        return Err(unconfirmed(staged, "Predeclared held name is occupied"));
    }
    let journal =
        Journal::publish(&pair, transaction).map_err(|error| unconfirmed(staged, error))?;
    observe(Point::JournalReady, &journal.transaction)
        .map_err(|error| unconfirmed(staged, error))?;
    apply(&pair, &journal, observe)?;
    journal
        .finish(&pair)
        .map_err(|error| unconfirmed(staged, error))
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
