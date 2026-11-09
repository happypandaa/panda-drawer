//! Private Node records and HANDLE-bound, resumable intent retirement.
use super::*;
use platform_native::windows::{files as native, removal};
use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::windows::ffi::{OsStrExt, OsStringExt},
    sync::Mutex as StdMutex,
    time::{Duration, Instant},
};

const SOURCE_RECORD_BYTES: usize = 256 * 1024;
// Compact JSON expands each source byte to at most three digits and a comma;
// keep the existing source budget plus room for all fixed identity fields.
const RECEIPT_BYTES: usize = SOURCE_RECORD_BYTES * 4 + 64 * 1024;

pub(super) fn ordinary_path(path: &Path, directory: bool) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(fail("应用路径必须为规范的绝对路径。"));
    }
    let opened = native::open_no_reparse(path)?;
    if opened.identity.directory != directory {
        return Err(fail("应用文件类型不符。"));
    }
    if !directory && opened.identity.links != 1 {
        return Err(fail("应用记录不能是硬链接。"));
    }
    Ok(())
}
pub(super) fn read_small(path: &Path) -> Result<Vec<u8>> {
    ordinary_path(path, false)?;
    let file = native::open_read_file(path)?;
    if file.metadata()?.len() > 256 * 1024 {
        return Err(fail("应用记录必须为大小受限且不共享硬链接的普通文件。"));
    }
    let mut bytes = Vec::new();
    file.take(256 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 256 * 1024 {
        return Err(fail("应用记录超出大小限制。"));
    }
    Ok(bytes)
}
#[derive(Clone, Copy)]
pub(super) enum IntentCleanupPoint {
    BeforeCapture,
    Captured,
}
pub(super) struct RetainedFile(StdMutex<Option<File>>);
impl RetainedFile {
    pub(super) fn metadata(&self) -> std::io::Result<fs::Metadata> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .ok_or_else(|| std::io::Error::other("Intent handle has retired"))?
            .metadata()
    }
}
struct Retirement {
    capture: PathBuf,
    gate: PathBuf,
    active: PathBuf,
    journal_path: PathBuf,
    receipt: CleanupReceipt,
    journal_bytes: Vec<u8>,
    journal: Option<File>,
    _parent: Option<native::PinnedDirectory>,
    complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CleanupIdentity {
    volume_serial: u64,
    file_id: [u8; 16],
    links: u32,
    directory: bool,
}
impl CleanupIdentity {
    fn of(identity: &native::FileIdentity) -> Self {
        Self {
            volume_serial: identity.volume_serial,
            file_id: identity.file_id,
            links: identity.links,
            directory: identity.directory,
        }
    }
    fn native(&self) -> native::FileIdentity {
        native::FileIdentity {
            volume_serial: self.volume_serial,
            file_id: self.file_id,
            links: self.links,
            directory: self.directory,
        }
    }
    fn matches(&self, identity: &native::FileIdentity) -> bool {
        self.directory == identity.directory && self.native().same_object(identity)
    }
}

/// Publication is permitted only from remove_if_owned, after its caller has
/// confirmed no application was created or all runtime resources really closed.
/// This immutable receipt carries that fact across a dropped Inspection/host.
/// It is outside the gate so even a pending gate deletion remains retryable.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CleanupReceipt {
    version: u32,
    resources_closed: bool,
    parent: CleanupIdentity,
    gate: CleanupIdentity,
    source: CleanupIdentity,
    journal: CleanupIdentity,
    active_leaf: Vec<u16>,
    source_bytes: Vec<u8>,
}
pub(super) struct StartIntentOwnership {
    pub(super) file: RetainedFile,
    pub(super) contents: Vec<u8>,
    retirement: StdMutex<Option<Retirement>>,
}
fn changed() -> ApiError {
    fail("启动标记归属或内容发生变化，已保留记录，不能解除启动保护。")
}
fn same_file(file: &File, path: &Path, bytes: &[u8]) -> Result<native::FileIdentity> {
    same_file_reading(file, path, bytes, read_small)
}
fn same_file_reading(
    file: &File,
    path: &Path,
    bytes: &[u8],
    read: impl FnOnce(&Path) -> Result<Vec<u8>>,
) -> Result<native::FileIdentity> {
    let owned = native::identity(file)?;
    let selected = native::open_no_reparse(path)?;
    if owned.directory
        || owned.links != 1
        || selected.identity.directory
        || selected.identity.links != 1
        || !owned.same_object(&selected.identity)
        || read(path)? != bytes
    {
        return Err(changed());
    }
    let after = native::open_no_reparse(path)?;
    if after.identity.directory
        || after.identity.links != 1
        || !owned.same_object(&after.identity)
        || native::identity(file)?.links != 1
    {
        return Err(changed());
    }
    Ok(owned)
}

fn read_receipt_bytes(path: &Path) -> Result<Vec<u8>> {
    let file = native::open_read_file(path)?;
    if file.metadata()?.len() > RECEIPT_BYTES as u64 {
        return Err(changed());
    }
    let mut bytes = Vec::new();
    file.take((RECEIPT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > RECEIPT_BYTES {
        return Err(changed());
    }
    Ok(bytes)
}

fn present(path: &Path) -> Result<Option<native::FileIdentity>> {
    match native::open_no_reparse(path) {
        Ok(opened) => Ok(Some(opened.identity)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) if matches!(error.raw_os_error(), Some(5 | 32 | 33)) => {
            // A prior attempt may already have requested standard deletion.
            // Access denied is never absence: wait for actual NotFound only.
            require_absent(path)?;
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

fn receipt_gate(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let uuid = name
        .strip_prefix(START_INTENT_CLEANUP_PREFIX)?
        .strip_suffix(".json")?;
    let id = Uuid::parse_str(uuid).ok()?;
    if id.to_string() != uuid {
        return None;
    }
    Some(path.with_file_name(format!("{START_INTENT_CLEANUP_PREFIX}{id}")))
}

impl Retirement {
    fn prepare(path: &Path, source: &File, source_bytes: &[u8]) -> Result<Self> {
        let identity = same_file(source, path, source_bytes)?;
        let parent = path.parent().ok_or_else(changed)?;
        let pin = native::pin_directory(parent)?;
        let gate = parent.join(format!("{START_INTENT_CLEANUP_PREFIX}{}", Uuid::new_v4()));
        native::create_private_directory(&gate)?;
        let gate_identity = native::open_no_reparse(&gate)?.identity;
        let journal_path = gate.with_file_name(format!(
            "{}.json",
            gate.file_name()
                .ok_or_else(changed)?
                .to_str()
                .ok_or_else(changed)?
        ));
        // The journal owns its actual creation ID before that ID is serialized.
        // A partial write stays unknown evidence and never authorizes capture.
        let mut journal = native::create_private_file_for_publication(&journal_path)?;
        let receipt = CleanupReceipt {
            version: 1,
            resources_closed: true,
            parent: CleanupIdentity::of(&native::identity(&pin.file)?),
            gate: CleanupIdentity::of(&gate_identity),
            source: CleanupIdentity::of(&identity),
            journal: CleanupIdentity::of(&native::identity(&journal)?),
            active_leaf: path
                .file_name()
                .ok_or_else(changed)?
                .encode_wide()
                .collect(),
            source_bytes: source_bytes.to_vec(),
        };
        let journal_bytes = serde_json::to_vec(&receipt)?;
        journal.write_all(&journal_bytes)?;
        journal.sync_all()?;
        platform_native::filesystem::sync_directory(parent)?;
        Ok(Self {
            capture: gate.join("captured-intent"),
            gate,
            active: path.to_path_buf(),
            journal_path,
            receipt,
            journal_bytes,
            journal: Some(journal),
            _parent: Some(pin),
            complete: false,
        })
    }

    fn load(path: &Path, gate: PathBuf) -> Result<Self> {
        let parent = path.parent().ok_or_else(changed)?;
        let pin = native::pin_directory(parent)?;
        let journal = native::open_read_file(path)?;
        native::verify_private(&journal)?;
        let journal_bytes = read_receipt_bytes(path)?;
        let receipt: CleanupReceipt =
            serde_json::from_slice(&journal_bytes).map_err(|_| changed())?;
        let leaf = OsString::from_wide(&receipt.active_leaf);
        let mut components = Path::new(&leaf).components();
        if receipt.version != 1
            || !receipt.resources_closed
            || receipt.source_bytes.len() > SOURCE_RECORD_BYTES
            // This receipt version only retires the current run marker. Other
            // names/streams are unknown material, never deletion authority.
            || receipt.active_leaf != "active.json".encode_utf16().collect::<Vec<_>>()
            || !receipt.parent.directory
            || !receipt.gate.directory
            || receipt.source.directory
            || receipt.source.links != 1
            || receipt.journal.directory
            || receipt.journal.links != 1
            || !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
            || receipt.active_leaf.contains(&0)
            || serde_json::to_vec(&receipt)? != journal_bytes
            || !receipt.parent.matches(&native::identity(&pin.file)?)
            || !receipt.journal.matches(&native::identity(&journal)?)
        {
            return Err(changed());
        }
        same_file_reading(&journal, path, &journal_bytes, read_receipt_bytes)?;
        Ok(Self {
            capture: gate.join("captured-intent"),
            gate,
            active: parent.join(leaf),
            journal_path: path.to_path_buf(),
            receipt,
            journal_bytes,
            journal: Some(journal),
            _parent: Some(pin),
            complete: false,
        })
    }

    fn confirm_journal(&self) -> Result<()> {
        let parent = self._parent.as_ref().ok_or_else(changed)?;
        if !self
            .receipt
            .parent
            .matches(&native::identity(&parent.file)?)
        {
            return Err(changed());
        }
        let journal = self.journal.as_ref().ok_or_else(changed)?;
        native::verify_private(journal)?;
        let identity = same_file_reading(
            journal,
            &self.journal_path,
            &self.journal_bytes,
            read_receipt_bytes,
        )?;
        if !self.receipt.journal.matches(&identity) {
            return Err(changed());
        }
        Ok(())
    }

    fn resume(
        &mut self,
        original: &mut Option<File>,
        mut observe: impl FnMut(IntentCleanupPoint),
    ) -> Result<()> {
        if self.complete {
            return Ok(());
        }
        if self.journal.is_none() {
            // Only this owner's successful last deletion request can leave its
            // journal handle absent. Never claim completion from an OS5 result.
            require_absent(&self.active)?;
            require_absent(&self.capture)?;
            require_absent(&self.gate)?;
            require_absent(&self.journal_path)?;
            self._parent.take();
            self.complete = true;
            return Ok(());
        }
        self.confirm_journal()?;
        let gate = present(&self.gate)?;
        if let Some(gate) = &gate {
            if !self.receipt.gate.matches(gate) {
                return Err(changed());
            }
        }
        let gate_pin = if gate.is_some() {
            let pin = native::pin_directory(&self.gate)?;
            if !self.receipt.gate.matches(&native::identity(&pin.file)?) {
                return Err(changed());
            }
            Some(pin)
        } else {
            None
        };
        let active = present(&self.active)?;
        let captured = if gate.is_some() {
            present(&self.capture)?
        } else {
            None
        };
        if let Some(active) = active {
            if gate.is_none() || captured.is_some() || !self.receipt.source.matches(&active) {
                return Err(changed());
            }
            let file = native::open_read_file(&self.active)?;
            same_file(&file, &self.active, &self.receipt.source_bytes)?;
            self.confirm_journal()?;
            native::rename_file_noreplace_expected(
                &self.active,
                &self.capture,
                &self.receipt.source.native(),
            )
            .map_err(|_| changed())?;
            drop(file);
            observe(IntentCleanupPoint::Captured);
        }
        // A new public marker is a conflict, even if our capture is still valid.
        require_absent(&self.active)?;
        if let Some(captured) = present(&self.capture)? {
            if !self.receipt.source.matches(&captured) {
                return Err(changed());
            }
            let file = native::open_read_file(&self.capture)?;
            same_file(&file, &self.capture, &self.receipt.source_bytes)?;
            self.confirm_journal()?;
            removal::remove(&self.capture, &self.receipt.source.native())?;
            drop(file);
            // An original read HANDLE also keeps standard deletion pending.
            original.take();
        }
        require_absent(&self.capture)?;
        require_absent(&self.active)?;
        self.confirm_journal()?;
        // Keep the capture directory fixed during capture/removal, then release
        // our own directory pin before asking for its real deletion.
        drop(gate_pin);
        if let Some(gate) = present(&self.gate)? {
            if !self.receipt.gate.matches(&gate) {
                return Err(changed());
            }
            // Unknown children are never traversed or adopted by this receipt.
            removal::remove(&self.gate, &self.receipt.gate.native())?;
        }
        require_absent(&self.gate)?;
        self.confirm_journal()?;
        removal::remove(&self.journal_path, &self.receipt.journal.native())?;
        self.journal.take();
        require_absent(&self.journal_path)?;
        self._parent.take();
        self.complete = true;
        Ok(())
    }
}

pub(super) fn resume_cleanup_receipts(slot: &Path, boot: std::time::SystemTime) -> Result<()> {
    match fs::symlink_metadata(slot) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(_) => ordinary_path(slot, true)?,
    }
    for entry in fs::read_dir(slot)? {
        let path = entry?.path();
        let Some(gate) = receipt_gate(&path) else {
            continue;
        };
        let result = (|| -> Result<()> {
            if present(&path)?.is_none() {
                return Ok(());
            }
            let mut retired = Retirement::load(&path, gate)?;
            retired.resume(&mut None, |_| ())
        })();
        if let Err(error) = result {
            // Preserve Mac's existing pre-boot admission exception. Boot time
            // never grants this adapter authority to delete invalid material;
            // leave it unchanged for the original cleanup scanner to classify.
            if fs::symlink_metadata(&path)?
                .created()
                .is_ok_and(|created| created < boot)
            {
                continue;
            }
            return Err(error);
        }
    }
    Ok(())
}
impl StartIntentOwnership {
    pub(super) fn require_current(&self, path: &Path) -> Result<()> {
        let guard = self.file.0.lock().unwrap_or_else(|p| p.into_inner());
        same_file(guard.as_ref().ok_or_else(changed)?, path, &self.contents).map(|_| ())
    }
    pub(super) fn open_existing(path: &Path) -> Result<Self> {
        ordinary_path(path, false)?;
        let file = native::open_read_file(path)?;
        native::verify_current_user_owner(&file)?;
        let contents = read_small(path)?;
        same_file(&file, path, &contents)?;
        Ok(Self {
            file: RetainedFile(StdMutex::new(Some(file))),
            contents,
            retirement: StdMutex::new(None),
        })
    }
    pub(super) fn create(path: &Path, value: &impl Serialize) -> Result<Self> {
        ordinary_path(path.parent().ok_or_else(|| fail("记录目录无效。"))?, true)?;
        let contents = serde_json::to_vec_pretty(value)?;
        let mut file = native::create_private_file_for_publication(path)?;
        file.write_all(&contents)?;
        file.sync_all()?;
        platform_native::filesystem::sync_directory(path.parent().unwrap())?;
        Ok(Self {
            file: RetainedFile(StdMutex::new(Some(file))),
            contents,
            retirement: StdMutex::new(None),
        })
    }
    pub(super) fn publish(&mut self, path: &Path, value: &impl Serialize) -> Result<()> {
        let file = self
            .file
            .0
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
            .ok_or_else(changed)?;
        same_file(file, path, &self.contents)?;
        let contents = serde_json::to_vec_pretty(value)?;
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        file.write_all(&contents)?;
        file.sync_all()?;
        self.contents = contents;
        same_file(file, path, &self.contents).map(|_| ())
    }
    pub(super) fn remove_if_owned(&self, path: &Path) -> Result<()> {
        self.remove_if_owned_with(path, |_| ())
    }
    pub(super) fn remove_if_owned_with(
        &self,
        path: &Path,
        mut observe: impl FnMut(IntentCleanupPoint),
    ) -> Result<()> {
        let mut state = self.retirement.lock().unwrap_or_else(|p| p.into_inner());
        let mut file = self.file.0.lock().unwrap_or_else(|p| p.into_inner());
        if state.is_none() {
            let held = file.as_ref().ok_or_else(changed)?;
            *state = Some(Retirement::prepare(path, held, &self.contents)?);
            observe(IntentCleanupPoint::BeforeCapture);
        }
        let retired = state.as_mut().unwrap();
        if retired.active != path || retired.receipt.source_bytes != self.contents {
            return Err(changed());
        }
        retired.resume(&mut file, observe)
    }
}
fn require_absent(path: &Path) -> Result<()> {
    let start = Instant::now();
    let delays = [20, 40, 80, 160, 320, 640, 1000];
    let mut next = 0;
    loop {
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) if !matches!(e.raw_os_error(), Some(5 | 32 | 33)) => return Err(e.into()),
            _ => (),
        }
        if next == delays.len() || start.elapsed() > Duration::from_secs(3) {
            return Err(changed().detail("remainingPath", path.to_string_lossy().into_owned()));
        }
        std::thread::sleep(Duration::from_millis(delays[next]));
        next += 1;
    }
}
pub(super) fn move_node_record_exclusive(from: &Path, to: &Path) -> Result<()> {
    native::rename_file(from, to, false).map_err(Into::into)
}

#[cfg(test)]
#[path = "windows_files_tests.rs"]
mod tests;
