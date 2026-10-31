//! Source-only exchange. The fixed-width control block is part of the original
//! task journal, so the guard's existing phase-growth reservation stays exact.
use super::*;
use platform_native::windows::files as native;
use std::ops::Deref;

const CAPTURE: &str = ".source-exchange-held";

pub(super) fn validate_path(path: &str) -> Result<(), ApiError> {
    // The same native component rules as Windows source acquisition. These
    // lengths are UTF-16 units, never the attachment API's UTF-8 byte allowance.
    for part in path.split('/') {
        let base = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        let device = ["COM", "LPT"].iter().any(|prefix| {
            base.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });
        if part
            .chars()
            .any(|value| value < ' ' || "<>:\"\\|?*".contains(value))
            || part.ends_with([' ', '.'])
            || part.encode_utf16().count() > 255
            || matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || device
        {
            return Err(restore_error(
                "PROJECT_SOURCE_RESTORE_PATH_UNSUPPORTED",
                "目标版本包含 Windows 无法使用的文件名；请修改源码后重新取回。",
            )
            .detail("path", path));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeIdentity {
    volume_serial: u64,
    file_id: [u8; 16],
}
impl NativeIdentity {
    fn capture(file: &File) -> Result<Self, ApiError> {
        let identity = native::identity(file)?;
        Ok(Self {
            volume_serial: identity.volume_serial,
            file_id: identity.file_id,
        })
    }
    fn matches(&self, file: &File) -> Result<bool, ApiError> {
        Ok(self == &Self::capture(file)?)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Stamp {
    pub(super) descriptor: RestoreDescriptor,
    identity: NativeIdentity,
    read_only: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingMove {
    // All allowed values have one JSON byte; entry always has five ASCII bytes.
    kind: u8,
    entry: String,
}
impl PendingMove {
    fn idle() -> Self {
        Self {
            kind: 0,
            entry: "00000".into(),
        }
    }
    fn active(index: usize, direction: RestoreAction) -> Self {
        Self {
            kind: if direction == RestoreAction::Undo {
                2
            } else {
                1
            },
            entry: format!("{index:05}"),
        }
    }
    fn index(&self, journal: &RestoreJournal) -> Result<Option<usize>, ApiError> {
        if self.entry.len() != 5 || !self.entry.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(restore_changed());
        }
        if self.kind == 0 {
            return if self.entry == "00000" {
                Ok(None)
            } else {
                Err(restore_changed())
            };
        }
        if !matches!(
            (self.kind, journal.phase.as_str()),
            (1, "applying") | (2, "undoing")
        ) {
            return Err(restore_changed());
        }
        let index = self.entry.parse::<usize>().map_err(|_| restore_changed())?;
        let entry = journal.entries.get(index).ok_or_else(restore_changed)?;
        if entry.before.is_none() || entry.after.is_none() {
            return Err(restore_changed());
        }
        Ok(Some(index))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RestoreJournal {
    pub(super) schema_version: u32,
    pub(super) operation: String,
    pub(super) project_id: String,
    pub(super) task_id: String,
    pub(super) phase: String,
    pub(super) plan_digest: String,
    pub(super) plan: RestorePlan,
    pub(super) entries: Vec<RestoreEntry>,
    pub(super) git: ExecutableInfo,
    operation_identity: NativeIdentity,
    pending_move: PendingMove,
}

pub(super) struct Directory(File);
impl Deref for Directory {
    type Target = File;
    fn deref(&self) -> &File {
        &self.0
    }
}
impl Directory {
    pub(super) fn sync_all(&self) -> std::io::Result<()> {
        native::sync_directory(&self.0)
    }
}
pub(super) fn open_directory(path: &Path) -> Result<Directory, ApiError> {
    Ok(Directory(native::directory_file(path)?))
}
pub(super) fn create_file(path: &Path, _: bool) -> Result<File, ApiError> {
    Ok(native::create_private_file(path)?)
}
pub(super) fn create_staging(path: &Path) -> Result<(), ApiError> {
    Ok(native::create_private_directory(path)?)
}
pub(super) fn executable(_: bool) -> bool {
    false
}
pub(super) fn valid_stamp(value: &Stamp) -> bool {
    !value.descriptor.executable
}
pub(super) fn same_volume(left: &Path, right: &Path) -> Result<bool, ApiError> {
    Ok(native::open_no_reparse(left)?.identity.volume_serial
        == native::open_no_reparse(right)?.identity.volume_serial)
}
pub(super) fn ordinary_single_file(path: &Path, metadata: &fs::Metadata) -> Result<bool, ApiError> {
    Ok(metadata.is_file()
        && !platform_native::filesystem::is_reparse(metadata)
        && native::open_no_reparse(path)?.identity.links == 1)
}
pub(super) fn stamp(path: &Path) -> Result<Option<Stamp>, ApiError> {
    let opened = match native::open_no_reparse(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if opened.identity.directory || opened.identity.links != 1 {
        return Err(restore_changed());
    }
    let before = platform_native::directory::stamp(&opened.file)?;
    let bytes = read_file(path, MAX_BLOB)?;
    let named = native::open_no_reparse(path)?;
    if before != platform_native::directory::stamp(&named.file)?
        || !opened.identity.same_object(&named.identity)
    {
        return Err(restore_changed());
    }
    Ok(Some(Stamp {
        descriptor: RestoreDescriptor {
            sha256: sha256(&bytes),
            size: bytes.len() as u64,
            executable: false,
        },
        identity: NativeIdentity::capture(&opened.file)?,
        read_only: opened.file.metadata()?.permissions().readonly(),
    }))
}
pub(super) fn binding_metadata(project: &ProjectHandle, branch: &str) -> Result<String, ApiError> {
    let mut values = BTreeMap::new();
    for path in [
        "HEAD",
        "config",
        "index",
        "packed-refs",
        "shallow",
        "info/attributes",
        "info/exclude",
        branch,
    ] {
        let absolute = project.root.join(".git").join(path);
        let observed = match native::open_no_reparse(&absolute) {
            Ok(file) if !file.identity.directory && file.identity.links == 1 => Some(format!(
                "{:?}",
                platform_native::directory::stamp(&file.file)?
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            _ => return Err(restore_changed()),
        };
        values.insert(path, observed);
    }
    Ok(sha256(&serde_json::to_vec(&values)?))
}
pub(super) fn new_journal(
    root: &Path,
    task: &str,
    plan_digest: String,
    plan: RestorePlan,
    entries: Vec<RestoreEntry>,
    git: ExecutableInfo,
) -> Result<RestoreJournal, ApiError> {
    let root = native::pin_directory(root)?;
    Ok(RestoreJournal {
        schema_version: 1,
        operation: "sourceRestore".into(),
        project_id: plan.project_id.clone(),
        task_id: task.into(),
        phase: "prepared".into(),
        plan_digest,
        plan,
        entries,
        git,
        operation_identity: NativeIdentity::capture(&root.file)?,
        pending_move: PendingMove::idle(),
    })
}
fn absent(path: &Path) -> Result<bool, ApiError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Ok(_) => Ok(false),
        Err(error) => Err(error.into()),
    }
}
pub(super) fn verify_journal_root(root: &Path, journal: &RestoreJournal) -> Result<(), ApiError> {
    let directory = native::pin_directory(root)?;
    if !journal.operation_identity.matches(&directory.file)? {
        return Err(restore_changed());
    }
    if journal.pending_move.index(journal)?.is_none() && !absent(&root.join(CAPTURE))? {
        return Err(restore_changed());
    }
    Ok(())
}
pub(super) fn require_terminal_ready(
    root: &Path,
    journal: &RestoreJournal,
) -> Result<(), ApiError> {
    verify_journal_root(root, journal)?;
    if journal.pending_move.kind != 0 || !absent(&root.join(CAPTURE))? {
        return Err(restore_changed());
    }
    Ok(())
}
pub(super) fn ensure_live_parent(root: &Path, path: &str) -> Result<(), ApiError> {
    valid_relative(path)?;
    let parent = Path::new(path).parent().ok_or_else(unsafe_path)?;
    let mut current = root.to_owned();
    for part in parent.components() {
        current.push(part.as_os_str());
        match native::open_no_reparse(&current) {
            Ok(directory) if directory.identity.directory => (),
            Ok(_) => return Err(restore_changed()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                native::create_private_directory(&current)?;
                open_directory(current.parent().ok_or_else(unsafe_path)?)?.sync_all()?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
fn expected_identity(value: &Stamp) -> native::FileIdentity {
    native::FileIdentity {
        volume_serial: value.identity.volume_serial,
        file_id: value.identity.file_id,
        links: 1,
        directory: false,
    }
}
fn rename_file(from: &Path, to: &Path, expected: &Stamp) -> Result<(), ApiError> {
    native::rename_file_noreplace_expected(from, to, &expected_identity(expected))?;
    Ok(())
}
pub(super) fn move_prepared(
    from: &Path,
    to: &Path,
    journal: &RestoreJournal,
) -> Result<(), ApiError> {
    let expected = native::open_no_reparse(from)?.identity;
    if !expected.directory
        || !journal
            .operation_identity
            .matches(&native::open_no_reparse(from)?.file)?
    {
        return Err(restore_changed());
    }
    native::rename_directory_noreplace_expected(from, to, &expected)?;
    Ok(())
}
pub(super) fn move_preserving(from: &Path, to: &Path, exchange: bool) -> Result<(), ApiError> {
    if exchange {
        return Err(restore_changed());
    } // Exchange requires its bound journal.
    let expected = native::open_no_reparse(from)?.identity;
    if expected.directory {
        native::rename_directory_noreplace_expected(from, to, &expected)?;
    } else {
        native::rename_file_noreplace_expected(from, to, &expected)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ready,
    OldHeld,
    NewVisible,
    Exchanged,
}
fn state(
    project: &ProjectHandle,
    root: &Path,
    journal: &RestoreJournal,
) -> Result<State, ApiError> {
    let _root = native::pin_directory(root)?;
    verify_journal_root(root, journal)?;
    if identity(&project.root)? != journal.plan.binding.root_identity {
        return Err(restore_changed());
    }
    let index = journal
        .pending_move
        .index(journal)?
        .ok_or_else(restore_changed)?;
    let entry = &journal.entries[index];
    let (old, next) = if journal.pending_move.kind == 1 {
        (&entry.before, &entry.after)
    } else {
        (&entry.after, &entry.before)
    };
    let live = stamp(&checked_relative(&project.root, &entry.path, true)?)?;
    let slot = stamp(&super::slot(root, index))?;
    let captured = stamp(&root.join(CAPTURE))?;
    if &live == old && &slot == next && captured.is_none() {
        Ok(State::Ready)
    } else if live.is_none() && &slot == next && &captured == old {
        Ok(State::OldHeld)
    } else if &live == next && slot.is_none() && &captured == old {
        Ok(State::NewVisible)
    } else if &live == next && &slot == old && captured.is_none() {
        Ok(State::Exchanged)
    } else {
        Err(restore_changed())
    }
}
pub(super) fn classify_journal_entry(
    project: &ProjectHandle,
    root: &Path,
    index: usize,
    entry: &RestoreEntry,
    journal: &RestoreJournal,
) -> Result<&'static str, ApiError> {
    verify_journal_root(root, journal)?;
    if journal.pending_move.index(journal)? != Some(index) {
        return classify_entry(project, root, index, entry);
    }
    let original = matches!(
        state(project, root, journal)?,
        State::Ready | State::OldHeld
    );
    Ok(if original == (journal.pending_move.kind == 1) {
        "before"
    } else {
        "after"
    })
}
pub(super) fn verify_mixed_source_at(
    project: &ProjectHandle,
    root: &Path,
    journal: &RestoreJournal,
    changes: &[RecoveryChange],
) -> Result<(), ApiError> {
    verify_journal_root(root, journal)?;
    if changes.iter().any(|change| change.state == "unknown") {
        return Err(restore_changed());
    }
    let scan = scan_source(
        &project.root,
        &project.manifest,
        &included_roots(&project.manifest),
    )?;
    if !scan.issues.is_empty() || !scan.unclassified.is_empty() || !scan.markers.is_empty() {
        return Err(restore_changed());
    }
    let mut actual = scan
        .files
        .iter()
        .map(|(path, file)| (path.clone(), descriptor(file)))
        .collect::<BTreeMap<_, _>>();
    if let Some(index) = journal.pending_move.index(journal)? {
        if state(project, root, journal)? == State::OldHeld {
            let entry = &journal.entries[index];
            if actual.contains_key(&entry.path)
                || protected(&entry.path, &project.manifest).is_some()
                || !included(&entry.path, &included_roots(&project.manifest))
            {
                return Err(restore_changed());
            }
            let old = if journal.pending_move.kind == 1 {
                &entry.before
            } else {
                &entry.after
            };
            actual.insert(
                entry.path.clone(),
                old.as_ref().ok_or_else(restore_changed)?.descriptor.clone(),
            );
        }
    }
    let mut expected = journal.plan.before.clone();
    for (entry, change) in journal.entries.iter().zip(changes) {
        if change.state == "after" {
            if let Some(after) = &entry.after {
                expected.insert(entry.path.clone(), after.descriptor.clone());
            } else {
                expected.remove(&entry.path);
            }
        }
    }
    if actual != expected {
        return Err(restore_changed());
    }
    Ok(())
}
fn publish(root: &Path, journal: &RestoreJournal) -> Result<(), ApiError> {
    write_journal(root, journal)?;
    let named = load_journal(root, &journal.project_id, &journal.task_id)?;
    if serde_json::to_value(&named)? != serde_json::to_value(journal)? {
        return Err(restore_changed());
    }
    Ok(())
}
fn clear_pending(root: &Path, journal: &mut RestoreJournal) -> Result<(), ApiError> {
    let original = journal.pending_move.clone();
    journal.pending_move = PendingMove::idle();
    if let Err(error) = publish(root, journal) {
        journal.pending_move = original;
        return Err(error);
    }
    Ok(())
}
pub(super) fn stabilize_pending(
    project: &ProjectHandle,
    root: &Path,
    journal: &mut RestoreJournal,
) -> Result<(), ApiError> {
    let _root = native::pin_directory(root)?;
    let _project = native::pin_directory(&project.root)?;
    verify_journal_root(root, journal)?;
    let Some(index) = journal.pending_move.index(journal)? else {
        return Ok(());
    };
    // Preserve the original transaction's all-source refusal before the first
    // stabilizing move, including changes outside this pending entry.
    let changes = classify_all(project, root, journal)?;
    verify_mixed_source_at(project, root, journal, &changes)?;
    let entry = &journal.entries[index];
    let old = if journal.pending_move.kind == 1 {
        entry.before.as_ref()
    } else {
        entry.after.as_ref()
    }
    .ok_or_else(restore_changed)?;
    match state(project, root, journal)? {
        State::OldHeld => rename_file(
            &root.join(CAPTURE),
            &checked_relative(&project.root, &entry.path, true)?,
            old,
        )?,
        State::NewVisible => rename_file(&root.join(CAPTURE), &super::slot(root, index), old)?,
        State::Ready | State::Exchanged => (),
    }
    if !matches!(
        state(project, root, journal)?,
        State::Ready | State::Exchanged
    ) {
        return Err(restore_changed());
    }
    clear_pending(root, journal)
}
pub(super) fn move_entry(
    project: &ProjectHandle,
    root: &Path,
    journal: &mut RestoreJournal,
    index: usize,
    direction: RestoreAction,
    live: &Path,
    held: &Path,
) -> Result<(), ApiError> {
    move_entry_with(
        Move {
            project,
            root,
            index,
            direction,
            live,
            held,
        },
        journal,
        |_| Ok(()),
    )
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Point {
    Prepared,
    OldHeld,
    NewVisible,
    Exchanged,
    Cleared,
}
#[cfg(test)]
pub(super) fn fixture_interrupt(
    project: &ProjectHandle,
    root: &Path,
    journal: &mut RestoreJournal,
    index: usize,
    direction: RestoreAction,
    wanted: Point,
) -> Result<(), ApiError> {
    journal.phase = if direction == RestoreAction::Undo {
        "undoing"
    } else {
        "applying"
    }
    .into();
    write_journal(root, journal)?;
    let live = checked_relative(&project.root, &journal.entries[index].path, true)?;
    let held = super::slot(root, index);
    move_entry_with(
        Move {
            project,
            root,
            index,
            direction,
            live: &live,
            held: &held,
        },
        journal,
        |point| {
            if point == wanted {
                Err(ApiError::new(
                    "DISPOSABLE_SOURCE_NATIVE_INTERRUPTION",
                    format!("{point:?}"),
                ))
            } else {
                Ok(())
            }
        },
    )
}
struct Move<'a> {
    project: &'a ProjectHandle,
    root: &'a Path,
    index: usize,
    direction: RestoreAction,
    live: &'a Path,
    held: &'a Path,
}
fn move_entry_with(
    request: Move<'_>,
    journal: &mut RestoreJournal,
    mut observe: impl FnMut(Point) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    let Move {
        project,
        root,
        index,
        direction,
        live,
        held,
    } = request;
    let _root = native::pin_directory(root)?;
    let _project = native::pin_directory(&project.root)?;
    require_terminal_ready(root, journal)?;
    if identity(&project.root)? != journal.plan.binding.root_identity {
        return Err(restore_changed());
    }
    let entry = journal
        .entries
        .get(index)
        .ok_or_else(restore_changed)?
        .clone();
    if live != checked_relative(&project.root, &entry.path, true)?
        || held != super::slot(root, index)
    {
        return Err(restore_changed());
    }
    match (entry.before.as_ref(), entry.after.as_ref(), direction) {
        (Some(before), Some(after), _) => {
            let (old, next) = if direction == RestoreAction::Continue {
                (before, after)
            } else {
                (after, before)
            };
            if stamp(live)?.as_ref() != Some(old) || stamp(held)?.as_ref() != Some(next) {
                return Err(restore_changed());
            }
            journal.pending_move = PendingMove::active(index, direction);
            publish(root, journal)?;
            observe(Point::Prepared)?;
            loop {
                let point = match state(project, root, journal)? {
                    State::Ready => {
                        rename_file(live, &root.join(CAPTURE), old)?;
                        Point::OldHeld
                    }
                    State::OldHeld => {
                        rename_file(held, live, next)?;
                        Point::NewVisible
                    }
                    State::NewVisible => {
                        rename_file(&root.join(CAPTURE), held, old)?;
                        Point::Exchanged
                    }
                    State::Exchanged => break,
                };
                observe(point)?;
            }
            clear_pending(root, journal)?;
            observe(Point::Cleared)?;
        }
        (None, Some(after), RestoreAction::Continue) | (Some(after), None, RestoreAction::Undo) => {
            rename_file(held, live, after)?
        }
        (Some(before), None, RestoreAction::Continue)
        | (None, Some(before), RestoreAction::Undo) => rename_file(live, held, before)?,
        _ => return Err(restore_changed()),
    }
    Ok(())
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
