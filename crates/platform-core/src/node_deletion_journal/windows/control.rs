//! A compare-and-swap task record without overwrite rename. Every active
//! immutable intent was completely flushed before it acquired its fixed name.
use super::*;
use platform_native::windows::{files as native, removal};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::windows::fs::OpenOptionsExt,
};

pub(super) const CHANGE: &str = ".record-change.json";
// Two <=1MiB UTF-8 JSON records, each at most doubled by JSON string escaping,
// plus bounded IDs/field names. This does not lower the original record budget.
const MAX_CHANGE: u64 = MAX_RECORD_BYTES * 4 + 64 * 1024;
const OPEN_REPARSE: u32 = 0x0020_0000;
const SHARE_READ: u32 = 1;
const SHARE_DELETE: u32 = 4;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    identity: PersistedIdentity,
    bytes: String,
}

pub(super) struct Written(Saved);
impl Written {
    pub(super) fn bytes(&self) -> &[u8] {
        self.0.bytes.as_bytes()
    }
}

pub(super) fn is_published(directory: &Path, written: &Written) -> Result<bool> {
    let _scope = directory_file(directory)?;
    let (file, bytes) = read_record(&directory.join(RECORD_NAME))?;
    written.0.matches(&file, &bytes)
}
impl Saved {
    fn matches(&self, file: &File, bytes: &[u8]) -> Result<bool> {
        Ok(self.identity.matches_open_runtime(file).map_err(fs_error)?
            && self.bytes.as_bytes() == bytes)
    }
    fn valid(&self) -> bool {
        self.bytes.len() as u64 <= MAX_RECORD_BYTES
            && self.identity.windows_file_id.is_some()
            && self.identity.volume_uuid.is_none()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    version: u32,
    id: String,
    owner: PersistedIdentity,
    previous: Saved,
    next: Saved,
}
impl Change {
    fn candidate(&self, directory: &Path) -> PathBuf {
        directory.join(format!(".write-{}", self.id))
    }
    fn captured(&self, directory: &Path) -> PathBuf {
        directory.join(format!(".record-old-{}", self.id))
    }
    fn validate(&self, directory: &Directory) -> Result<()> {
        canonical_uuid(&self.id)?;
        if self.version != 1
            || !self
                .owner
                .matches_open_runtime(directory)
                .map_err(fs_error)?
            || !self.previous.valid()
            || !self.next.valid()
            || self.previous.identity.matches_captured(&self.next.identity)
        {
            return Err(error("删除任务变更 intent 与当前私有目录不符。").outcome("unknown"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Point {
    IntentPublished,
    PreviousCaptured,
    NextPublished,
    PreviousRemoved,
}

fn unknown(issue: impl std::fmt::Display) -> ApiError {
    error(format!(
        "删除任务控制记录变更尚未核实，原文件及 intent 已保留：{issue}"
    ))
    .outcome("unknown")
    .detail("recordChangePending", true)
}

fn bytes(file: &File, limit: u64) -> Result<Vec<u8>> {
    if file.metadata()?.len() > limit {
        return Err(error("删除任务文件过大。"));
    }
    let mut reader = file;
    reader.seek(SeekFrom::Start(0))?;
    let mut result = Vec::new();
    reader.take(limit + 1).read_to_end(&mut result)?;
    if result.len() as u64 > limit {
        return Err(error("删除任务文件过大。"));
    }
    Ok(result)
}

fn locked(path: &Path, limit: u64, exclusive: bool) -> Result<(File, Vec<u8>)> {
    ordinary(path, false)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_DELETE | if exclusive { 0 } else { SHARE_READ })
        .custom_flags(OPEN_REPARSE)
        .open(path)?;
    let metadata = file.metadata()?;
    let identity = native::identity(&file)?;
    if !metadata.is_file()
        || platform_native::filesystem::is_reparse(&metadata)
        || identity.directory
        || identity.links != 1
    {
        return Err(error("删除任务文件类型或链接归属无效。"));
    }
    native::verify_private(&file)?;
    let content = bytes(&file, limit)?;
    let current = native::open_no_reparse(path)?;
    if !identity.same_object(&current.identity)
        || current.identity.links != 1
        || content.len() as u64 != metadata.len()
    {
        return Err(error("读取删除任务时文件身份发生变化。"));
    }
    Ok((file, content))
}

pub(super) fn read_record(path: &Path) -> Result<(File, Vec<u8>)> {
    locked(path, MAX_RECORD_BYTES, false)
}

fn optional(path: &Path) -> Result<Option<(File, Vec<u8>)>> {
    if absent(path)? {
        return Ok(None);
    }
    read_record(path).map(Some)
}

struct Active {
    change: Change,
    file: File,
    identity: native::FileIdentity,
    bytes: Vec<u8>,
}
impl Active {
    fn open(path: &Path, directory: &Directory) -> Result<Option<Self>> {
        if absent(path)? {
            return Ok(None);
        }
        let (file, bytes) = locked(path, MAX_CHANGE, true)?;
        let change: Change = serde_json::from_slice(&bytes).map_err(fs_error)?;
        change.validate(directory)?;
        let identity = native::identity(&file)?;
        Ok(Some(Self {
            change,
            file,
            identity,
            bytes,
        }))
    }
    fn verify(&self, path: &Path, directory: &Directory) -> Result<()> {
        self.change.validate(directory)?;
        let now = native::open_no_reparse(path)?;
        if !now.identity.same_object(&self.identity)
            || now.identity.links != 1
            || bytes(&self.file, MAX_CHANGE)? != self.bytes
        {
            return Err(unknown("active intent 身份或字节变化。"));
        }
        native::verify_private(&self.file)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Prepared,
    Captured,
    Published,
    OldRemoved,
}

fn step(directory: &Path, change: &Change) -> Result<Step> {
    let current = optional(&directory.join(RECORD_NAME))?;
    let candidate = optional(&change.candidate(directory))?;
    let captured = optional(&change.captured(directory))?;
    let is = |actual: &Option<(File, Vec<u8>)>, saved: &Saved| -> Result<bool> {
        actual
            .as_ref()
            .map_or(Ok(false), |(file, bytes)| saved.matches(file, bytes))
    };
    if is(&current, &change.previous)? && is(&candidate, &change.next)? && captured.is_none() {
        Ok(Step::Prepared)
    } else if current.is_none() && is(&candidate, &change.next)? && is(&captured, &change.previous)?
    {
        Ok(Step::Captured)
    } else if is(&current, &change.next)? && candidate.is_none() && is(&captured, &change.previous)?
    {
        Ok(Step::Published)
    } else if is(&current, &change.next)? && candidate.is_none() && captured.is_none() {
        Ok(Step::OldRemoved)
    } else {
        Err(unknown("task.json/candidate/captured 不是许可的精确状态。"))
    }
}

fn retire(path: &Path, saved: &Saved) -> Result<()> {
    let (file, bytes) = read_record(path)?;
    if !saved.matches(&file, &bytes)? {
        return Err(unknown("旧控制记录被替换。"));
    }
    removal::remove(path, &native::identity(&file)?)?;
    drop(file);
    if !absent_with_identity(path, Some(&saved.identity))? {
        return Err(unknown("控制记录删除请求尚未产生名字缺失。"));
    }
    Ok(())
}

fn apply(
    directory: &Path,
    scope: &Directory,
    active: Active,
    mut observe: impl FnMut(Point) -> Result<()>,
) -> Result<()> {
    let path = directory.join(CHANGE);
    active.verify(&path, scope)?;
    loop {
        match step(directory, &active.change)? {
            Step::Prepared => {
                native::rename_file_noreplace_expected(
                    &directory.join(RECORD_NAME),
                    &active.change.captured(directory),
                    &native_identity(&active.change.previous.identity, false)?,
                )?;
                observe(Point::PreviousCaptured)?;
            }
            Step::Captured => {
                native::rename_file_noreplace_expected(
                    &active.change.candidate(directory),
                    &directory.join(RECORD_NAME),
                    &native_identity(&active.change.next.identity, false)?,
                )?;
                observe(Point::NextPublished)?;
            }
            Step::Published => {
                retire(&active.change.captured(directory), &active.change.previous)?;
                observe(Point::PreviousRemoved)?;
            }
            Step::OldRemoved => break,
        }
        active.verify(&path, scope)?;
    }
    active.verify(&path, scope)?;
    removal::remove(&path, &active.identity)?;
    drop(active);
    if !absent(&path)? {
        return Err(unknown("变更 intent 尚未确认退役。"));
    }
    Ok(())
}

pub(super) fn resume(directory: &Path) -> Result<()> {
    let scope = directory_file(directory)?;
    if let Some(active) = Active::open(&directory.join(CHANGE), &scope).map_err(unknown)? {
        apply(directory, &scope, active, |_| Ok(())).map_err(unknown)?;
    }
    Ok(())
}

/// Uninstall inventory observes a precise pending update without replaying it.
/// Both record versions authorize the same captured directory objects; actual
/// target identities are independently checked by the inventory caller.
pub(super) fn inventory_record(directory: &Path) -> Result<Vec<u8>> {
    let scope = directory_file(directory)?;
    if let Some(active) = Active::open(&directory.join(CHANGE), &scope).map_err(unknown)? {
        step(directory, &active.change)?;
        return Ok(active.change.next.bytes.as_bytes().to_vec());
    }
    Ok(read_record(&directory.join(RECORD_NAME))?.1)
}

/// A failed mutation can retain its own already-flushed next revision while
/// another name is inaccessible. This is only an in-memory retry receipt: every
/// later mutation still has to resume and verify the complete active intent.
#[cfg(test)]
pub(super) fn authored_next(directory: &Path, expected: &[u8]) -> Result<bool> {
    let scope = directory_file(directory)?;
    let Some(active) = Active::open(&directory.join(CHANGE), &scope).map_err(unknown)? else {
        return Ok(read_record(&directory.join(RECORD_NAME))?.1 == expected);
    };
    if active.change.next.bytes.as_bytes() != expected {
        return Ok(false);
    }
    for path in [
        directory.join(RECORD_NAME),
        active.change.candidate(directory),
    ] {
        if let Ok(Some((file, bytes))) = optional(&path) {
            if active.change.next.matches(&file, &bytes)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) fn atomic_record(directory: &Path, expected: Option<&[u8]>, next: &[u8]) -> Result<()> {
    atomic_with(directory, expected, next, |_| Ok(()))
}

pub(super) fn atomic_with(
    directory: &Path,
    expected: Option<&[u8]>,
    next: &[u8],
    mut observe: impl FnMut(Point) -> Result<()>,
) -> Result<()> {
    atomic_with_receipt(directory, expected, next, &mut None, &mut observe)
}

pub(super) fn atomic_with_receipt(
    directory: &Path,
    expected: Option<&[u8]>,
    next: &[u8],
    written: &mut Option<Written>,
    mut observe: impl FnMut(Point) -> Result<()>,
) -> Result<()> {
    if next.len() as u64 > MAX_RECORD_BYTES {
        return Err(error("删除任务记录过大。"));
    }
    let scope = directory_file(directory)?;
    resume(directory)?;
    let previous = expected
        .map(|expected| {
            let (file, bytes) = read_record(&directory.join(RECORD_NAME))?;
            if bytes != expected {
                return Err(error("删除任务已被其他写入更改。"));
            }
            Ok(Saved {
                identity: PersistedIdentity::from_file(&file).map_err(fs_error)?,
                bytes: String::from_utf8(bytes).map_err(fs_error)?,
            })
        })
        .transpose()?;
    let id = Uuid::new_v4().to_string();
    let candidate = directory.join(format!(".write-{id}"));
    let mut file = native::create_private_file_for_publication(&candidate)?;
    file.write_all(next)?;
    file.sync_all()?;
    let identity = PersistedIdentity::from_file(&file).map_err(fs_error)?;
    let next_saved = Saved {
        identity: identity.clone(),
        bytes: String::from_utf8(next.to_vec()).map_err(fs_error)?,
    };
    *written = Some(Written(next_saved.clone()));
    if previous.is_none() {
        native::rename_private_file_handle(&file, &directory.join(RECORD_NAME), false)?;
        drop(file);
        let (published, actual) = read_record(&directory.join(RECORD_NAME)).map_err(unknown)?;
        if actual != next
            || !identity
                .matches_open_runtime(&published)
                .map_err(fs_error)?
        {
            return Err(unknown("首次 task.json 发布后身份变化。"));
        }
        return Ok(());
    }
    drop(file);
    let change = Change {
        version: 1,
        id,
        owner: PersistedIdentity::from_file(&scope).map_err(fs_error)?,
        previous: previous.unwrap(),
        next: next_saved,
    };
    change.validate(&scope)?;
    let encoded = serde_json::to_vec(&change)?;
    if encoded.len() as u64 > MAX_CHANGE {
        return Err(error("删除任务控制 intent 超出原记录派生预算。"));
    }
    let preparing = directory.join(format!(".write-{}", Uuid::new_v4()));
    let mut intent = native::create_private_file_for_publication(&preparing)?;
    intent.write_all(&encoded)?;
    intent.sync_all()?;
    let intent_id = native::identity(&intent)?;
    native::rename_private_file_handle(&intent, &directory.join(CHANGE), false)?;
    drop(intent);
    let active = Active::open(&directory.join(CHANGE), &scope)
        .map_err(unknown)?
        .ok_or_else(|| unknown("刚发布的 intent 已缺失。"))?;
    if !intent_id.same_object(&active.identity) || active.bytes != encoded {
        return Err(unknown("发布的 intent 已被替换。"));
    }
    observe(Point::IntentPublished).map_err(unknown)?;
    apply(directory, &scope, active, observe).map_err(unknown)
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
