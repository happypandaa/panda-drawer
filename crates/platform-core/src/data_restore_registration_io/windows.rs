//! Exact, recoverable file exchange under the original restore write gate.
//! Pure inspection never moves a name. Before visibility stabilization restores
//! the old registration; after visibility it only completes the committed swap.
use super::*;
use platform_native::windows::files::{self as native, FileIdentity as NativeId, PinnedDirectory};
use std::{
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::OpenOptionsExt,
    },
    time::Duration,
};

const JOURNAL: &str = ".registration-exchange.json";
const MAX_INNER: u64 = 64 * 1024;
pub(super) const MAX_EXCHANGE_JOURNAL: u64 = MAX_INNER;

fn pending(path: &Path, cause: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        "PROJECT_DATA_RESTORE_REGISTRATION_EXCHANGE_PENDING",
        "注册代次交换尚未核实，原文件和事务凭据已保留。",
    )
    .outcome("unknown")
    .detail("pending", true)
    .detail("businessBlocked", true)
    .detail("retainedPath", path.to_string_lossy().into_owned())
    .detail("cause", cause.to_string())
}
#[derive(Clone)]
pub(super) struct Binding {
    pub(super) live: PathBuf,
    pub(super) operation: PathBuf,
    pub(super) operation_identity: FileIdentity,
    pub(super) direction: DataRestoreDirection,
    pub(super) old: FileProof,
    pub(super) next: FileProof,
}
impl Binding {
    fn slot(&self) -> PathBuf {
        self.operation.join(match self.direction {
            DataRestoreDirection::Continue => APPLY_SLOT,
            DataRestoreDirection::Undo => UNDO_SLOT,
        })
    }
}
struct Scope {
    binding: Binding,
    live_parent: PinnedDirectory,
    operation: PinnedDirectory,
    live_parent_identity: FileIdentity,
}
impl Scope {
    fn open(binding: &Binding) -> Result<Self, ApiError> {
        let live_parent = native::pin_directory(binding.live.parent().ok_or_else(invalid)?)?;
        let operation = native::pin_directory(&binding.operation)?;
        if !binding
            .operation_identity
            .matches_open_runtime(&operation.file)
            .map_err(|_| invalid())?
            || !same_volume(&binding.old.identity, &binding.next.identity)
            || binding
                .old
                .identity
                .matches_captured(&binding.next.identity)
            || native::identity(&live_parent.file)?.volume_serial != binding.old.identity.device
            || native::identity(&operation.file)?.volume_serial != binding.old.identity.device
        {
            return Err(invalid());
        }
        let live_parent_identity =
            FileIdentity::from_file(&live_parent.file).map_err(|_| invalid())?;
        Ok(Self {
            binding: binding.clone(),
            live_parent,
            operation,
            live_parent_identity,
        })
    }
    fn verify(&self) -> Result<(), ApiError> {
        for (path, identity, file) in [
            (
                self.binding.live.parent().ok_or_else(invalid)?,
                &self.live_parent_identity,
                &self.live_parent.file,
            ),
            (
                self.binding.operation.as_path(),
                &self.binding.operation_identity,
                &self.operation.file,
            ),
        ] {
            if !identity.matches_open_runtime(file).map_err(|_| invalid())?
                || !identity.matches_path(path).map_err(|_| invalid())?
            {
                return Err(changed());
            }
        }
        Ok(())
    }
    fn journal(&self) -> PathBuf {
        self.binding.operation.join(JOURNAL)
    }
    fn held(&self, record: &Record) -> PathBuf {
        self.binding.operation.join(&record.held)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    version: u32,
    token: String,
    record_identity: FileIdentity,
    operation_identity: FileIdentity,
    live_parent_identity: FileIdentity,
    live_name: Vec<u16>,
    slot_name: String,
    held: String,
    direction: DataRestoreDirection,
    old: FileProof,
    next: FileProof,
}
impl Record {
    fn validate(&self, scope: &Scope) -> Result<(), ApiError> {
        let token = Uuid::parse_str(&self.token).map_err(|_| invalid())?;
        let expected_slot = scope.binding.slot();
        let live_name = std::ffi::OsString::from_wide(&self.live_name);
        let components = Path::new(&live_name).components().collect::<Vec<_>>();
        if self.version != 1
            || token.is_nil()
            || token.to_string() != self.token
            || self.live_name.contains(&0)
            || self
                .live_name
                .iter()
                .any(|unit| *unit == u16::from(b'/') || *unit == u16::from(b'\\'))
            || !matches!(components.as_slice(), [Component::Normal(_)])
            || scope.binding.live.file_name() != Some(live_name.as_os_str())
            || expected_slot.file_name().and_then(|name| name.to_str())
                != Some(self.slot_name.as_str())
            || self.held != format!(".registration-held-{}", self.token)
            || self.direction != scope.binding.direction
            || !self
                .operation_identity
                .matches_captured(&scope.binding.operation_identity)
            || !self
                .live_parent_identity
                .matches_captured(&scope.live_parent_identity)
            || !self.old.matches(&scope.binding.old)
            || !self.next.matches(&scope.binding.next)
        {
            return Err(invalid());
        }
        scope.verify()
    }
}
struct Journal {
    record: Record,
    identity: NativeId,
    bytes: Vec<u8>,
    file: File,
}
fn journal_bytes(mut file: &File) -> Result<Vec<u8>, ApiError> {
    if file.metadata()?.len() > MAX_INNER {
        return Err(invalid());
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(MAX_INNER + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INNER {
        return Err(invalid());
    }
    Ok(bytes)
}
impl Journal {
    fn open(scope: &Scope) -> Result<Option<Self>, ApiError> {
        let path = scope.journal();
        let file = match OpenOptions::new()
            .read(true)
            .share_mode(4)
            .custom_flags(0x0020_0000)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        let identity = native::identity(&file)?;
        if !metadata.is_file()
            || platform_native::filesystem::is_reparse(&metadata)
            || identity.links != 1
        {
            return Err(invalid());
        }
        // This independently created control journal is private. Existing
        // registrations, data, roots and ancestors gain no DACL policy.
        native::verify_private(&file)?;
        let bytes = journal_bytes(&file)?;
        let record: Record = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        record.validate(scope)?;
        if !record
            .record_identity
            .matches_open_runtime(&file)
            .map_err(|_| invalid())?
        {
            return Err(changed());
        }
        Ok(Some(Self {
            record,
            identity,
            bytes,
            file,
        }))
    }
    fn publish(scope: &Scope) -> Result<Self, ApiError> {
        let token = Uuid::new_v4();
        let preparing = scope
            .binding
            .operation
            .join(format!(".registration-preparing-{token}.json"));
        let mut file = native::create_private_file_for_publication(&preparing)?;
        let record = Record {
            version: 1,
            token: token.to_string(),
            record_identity: FileIdentity::from_file(&file).map_err(|_| invalid())?,
            operation_identity: scope.binding.operation_identity.clone(),
            live_parent_identity: scope.live_parent_identity.clone(),
            live_name: scope
                .binding
                .live
                .file_name()
                .ok_or_else(invalid)?
                .encode_wide()
                .collect(),
            slot_name: scope
                .binding
                .slot()
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(invalid)?
                .into(),
            held: format!(".registration-held-{token}"),
            direction: scope.binding.direction,
            old: scope.binding.old.clone(),
            next: scope.binding.next.clone(),
        };
        record.validate(scope)?;
        let bytes = serde_json::to_vec(&record)?;
        if bytes.len() as u64 > MAX_INNER {
            return Err(invalid());
        }
        file.write_all(&bytes)?;
        file.sync_all()?;
        native::rename_private_file_handle(&file, &scope.journal(), false)?;
        drop(file);
        let journal = Self::open(scope)?.ok_or_else(changed)?;
        if journal.bytes != bytes {
            return Err(changed());
        }
        Ok(journal)
    }
    fn verify(&self, scope: &Scope) -> Result<(), ApiError> {
        self.record.validate(scope)?;
        let named = native::open_no_reparse(&scope.journal())?;
        if !self.identity.same_object(&named.identity) || journal_bytes(&self.file)? != self.bytes {
            return Err(changed());
        }
        Ok(())
    }
    fn retire(self, scope: &Scope) -> Result<(), ApiError> {
        self.verify(scope)?;
        let identity = self.identity.clone();
        native::remove_file(&scope.journal(), &identity)?;
        drop(self);
        for delay in [0, 20, 40, 80, 160, 320, 640, 1000] {
            if delay != 0 {
                std::thread::sleep(Duration::from_millis(delay));
            }
            match fs::symlink_metadata(scope.journal()) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) if matches!(error.raw_os_error(), Some(5 | 32 | 33)) => (),
                Err(error) => return Err(error.into()),
                Ok(_) => {
                    if let Ok(actual) = native::open_no_reparse(&scope.journal()) {
                        if !identity.same_object(&actual.identity) {
                            return Err(changed());
                        }
                    }
                }
            }
        }
        Err(pending(
            &scope.journal(),
            "Exchange journal retirement is unconfirmed",
        ))
    }
}

pub(super) fn pending_direction(
    operation: &Path,
) -> Result<Option<DataRestoreDirection>, ApiError> {
    let path = operation.join(JOURNAL);
    let file = match native::open_read_file(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    native::verify_private(&file)?;
    let record: Record = serde_json::from_slice(&journal_bytes(&file)?).map_err(|_| invalid())?;
    if !record
        .record_identity
        .matches_open_runtime(&file)
        .map_err(|_| invalid())?
    {
        return Err(changed());
    }
    Ok(Some(record.direction))
}

/// Return only exact current-format names validated against the immutable
/// original pair. Callers add these names to their original metadata set and
/// charge both count and bytes to the existing restore budgets.
pub(super) fn metadata(
    binding_for: &impl Fn(DataRestoreDirection) -> Binding,
) -> Result<Vec<(String, u64)>, ApiError> {
    let initial = binding_for(DataRestoreDirection::Continue);
    let mut result = Vec::new();
    let direction = pending_direction(&initial.operation)?;
    let mut held = None;
    if let Some(direction) = direction {
        let scope = Scope::open(&binding_for(direction))?;
        let journal = Journal::open(&scope)?.ok_or_else(changed)?;
        journal.verify(&scope)?;
        let observed = state(&scope, &journal.record)?;
        result.push((JOURNAL.into(), journal.bytes.len() as u64));
        if matches!(observed, State::OldHeld | State::NewVisible) {
            held = Some(journal.record.held.clone());
            result.push((journal.record.held.clone(), journal.record.old.bytes));
        }
    }
    let _operation = native::pin_directory(&initial.operation)?;
    for entry in fs::read_dir(&initial.operation)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with(".registration-held-") {
            if held.as_deref() != Some(name.as_str()) {
                return Err(changed());
            }
        } else if let Some(token) = name
            .strip_prefix(".registration-preparing-")
            .and_then(|name| name.strip_suffix(".json"))
        {
            let token = Uuid::parse_str(token).map_err(|_| invalid())?;
            if token.is_nil() || name != format!(".registration-preparing-{token}.json") {
                return Err(invalid());
            }
            let file = native::open_read_file(&entry.path())?;
            native::verify_private(&file)?;
            let bytes = journal_bytes(&file)?;
            let record: Record = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            let scope = Scope::open(&binding_for(record.direction))?;
            record.validate(&scope)?;
            if record.token != token.to_string()
                || !record
                    .record_identity
                    .matches_open_runtime(&file)
                    .map_err(|_| invalid())?
            {
                return Err(changed());
            }
            result.push((name, bytes.len() as u64));
        }
    }
    result.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State {
    Ready,
    OldHeld,
    NewVisible,
    Exchanged,
}
impl State {
    pub(super) fn committed(self) -> bool {
        matches!(self, Self::NewVisible | Self::Exchanged)
    }
}
fn optional_proof(path: &Path) -> Result<Option<FileProof>, ApiError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        Ok(_) => Ok(Some(read_proof(path, MAX_RECORD, false)?.0)),
    }
}
fn matches(expected: &FileProof, actual: &Option<FileProof>) -> bool {
    actual
        .as_ref()
        .is_some_and(|actual| expected.matches(actual))
}
fn state(scope: &Scope, record: &Record) -> Result<State, ApiError> {
    scope.verify()?;
    let live = optional_proof(&scope.binding.live)?;
    let slot = optional_proof(&scope.binding.slot())?;
    let held = optional_proof(&scope.held(record))?;
    if matches(&record.old, &live) && matches(&record.next, &slot) && held.is_none() {
        Ok(State::Ready)
    } else if live.is_none() && matches(&record.next, &slot) && matches(&record.old, &held) {
        Ok(State::OldHeld)
    } else if matches(&record.next, &live) && slot.is_none() && matches(&record.old, &held) {
        Ok(State::NewVisible)
    } else if matches(&record.next, &live) && matches(&record.old, &slot) && held.is_none() {
        Ok(State::Exchanged)
    } else {
        Err(changed())
    }
}
fn rename(from: &Path, to: &Path, proof: &FileProof) -> Result<(), ApiError> {
    let expected = NativeId {
        volume_serial: proof.identity.device,
        file_id: proof.identity.windows_file_id.ok_or_else(invalid)?,
        links: 1,
        directory: false,
    };
    native::rename_file_noreplace_expected(from, to, &expected)?;
    Ok(())
}
pub(super) fn inspect_pending(binding: &Binding) -> Result<Option<State>, ApiError> {
    let scope = Scope::open(binding).map_err(|error| pending(&binding.operation, error))?;
    let result = (|| {
        let Some(journal) = Journal::open(&scope)? else {
            return Ok(None);
        };
        journal.verify(&scope)?;
        state(&scope, &journal.record).map(Some)
    })();
    result.map_err(|error: ApiError| pending(&scope.journal(), error))
}
/// Caller holds the original freeze/write permit. Pre-commit states restore
/// Before, preserving both user choices; a visible new epoch only moves forward.
pub(super) fn stabilize_pending(binding: &Binding) -> Result<bool, ApiError> {
    let scope = Scope::open(binding)?;
    let result = (|| {
        let Some(journal) = Journal::open(&scope)? else {
            return Ok(false);
        };
        journal.verify(&scope)?;
        match state(&scope, &journal.record)? {
            State::Ready | State::Exchanged => (),
            State::OldHeld => rename(
                &scope.held(&journal.record),
                &scope.binding.live,
                &journal.record.old,
            )?,
            State::NewVisible => rename(
                &scope.held(&journal.record),
                &scope.binding.slot(),
                &journal.record.old,
            )?,
        }
        if !matches!(
            state(&scope, &journal.record)?,
            State::Ready | State::Exchanged
        ) {
            return Err(changed());
        }
        journal.retire(&scope)?;
        Ok(true)
    })();
    result.map_err(|error: ApiError| pending(&scope.journal(), error))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    JournalReady,
    OldHeld,
    NewVisible,
    Exchanged,
    JournalRetired,
}
pub(super) fn exchange(binding: &Binding) -> Result<(), ApiError> {
    exchange_with(binding, &mut |_, _| Ok(()))
}
fn exchange_with(
    binding: &Binding,
    observe: &mut impl FnMut(Point, &Path) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    let scope = Scope::open(binding)?;
    let result = (|| {
        if Journal::open(&scope)?.is_some() {
            return Err(changed());
        }
        if !matches(&binding.old, &optional_proof(&binding.live)?)
            || !matches(&binding.next, &optional_proof(&binding.slot())?)
        {
            return Err(changed());
        }
        let journal = Journal::publish(&scope)?;
        let held = scope.held(&journal.record);
        observe(Point::JournalReady, &held)?;
        loop {
            journal.verify(&scope)?;
            let (from, to, proof, point) = match state(&scope, &journal.record)? {
                State::Ready => (
                    binding.live.clone(),
                    scope.held(&journal.record),
                    &journal.record.old,
                    Point::OldHeld,
                ),
                State::OldHeld => (
                    binding.slot(),
                    binding.live.clone(),
                    &journal.record.next,
                    Point::NewVisible,
                ),
                State::NewVisible => (
                    scope.held(&journal.record),
                    binding.slot(),
                    &journal.record.old,
                    Point::Exchanged,
                ),
                State::Exchanged => break,
            };
            rename(&from, &to, proof)?;
            observe(point, &held)?;
        }
        journal.retire(&scope)?;
        observe(Point::JournalRetired, &held)?;
        Ok(())
    })();
    result.map_err(|error: ApiError| pending(&scope.journal(), error))
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
