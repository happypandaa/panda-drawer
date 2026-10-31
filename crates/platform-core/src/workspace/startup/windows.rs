//! Observe the fixed settings and migration tree before initialization writes.
//! Valid journals use the shared recovery state machine. Unselected unknown
//! names remain untouched, as in the original startup enumeration.
use crate::{
    workspace::{self, MigrationJournal},
    ApiError,
};
use platform_native::{
    tool_files::Facts,
    windows::files::{self as native, PinnedDirectory},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, ApiError>;
const CAPABILITY: &str = "workspace-startup-recovery";
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Observation {
    Missing,
    Empty,
    Present,
}

fn unknown(path: &Path, cause: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        "WORKSPACE_STARTUP_INVENTORY_UNCONFIRMED",
        "Windows 工作区迁移现场尚未核实，现有材料已保留。",
    )
    .detail("platform", "windows")
    .detail("capability", CAPABILITY)
    .detail("inventory", "unconfirmed")
    .detail("path", path.to_string_lossy().into_owned())
    .detail("cause", cause.to_string())
    .outcome("unknown")
}
fn inconsistent(path: &Path, reference: Value) -> ApiError {
    ApiError::new(
        "WORKSPACE_MIGRATION_FAILED",
        "工作区设置仍引用迁移，但对应迁移记录缺失或无法核对；现有设置和工作区均已保留。",
    )
    .detail("platform", "windows")
    .detail("capability", CAPABILITY)
    .detail("inventory", "inconsistent")
    .detail("path", path.to_string_lossy().into_owned())
    .detail("migrationId", reference)
    .outcome("unknown")
}

struct SettingsSnapshot {
    path: PathBuf,
    opened: Option<(File, Facts)>,
    value: Value,
}
impl SettingsSnapshot {
    fn read(root: &Path, required: bool) -> Result<Self> {
        let path = root.join("settings.json");
        let mut file = match native::open_read_file_allowing_hard_links(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => {
                return Ok(Self {
                    path,
                    opened: None,
                    value: Value::Null,
                });
            }
            Err(error) => return Err(unknown(&path, error)),
        };
        // Preserve the existing settings contract, including admitted hard
        // links. No ACL admission is imposed on system parents or journals.
        native::verify_private(&file).map_err(|error| unknown(&path, error))?;
        let before = Facts::from_file(&file).map_err(|error| unknown(&path, error))?;
        if before.len() > MAX_SETTINGS_BYTES {
            return Err(unknown(&path, "工作区设置超过原有大小限制。"));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_SETTINGS_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| unknown(&path, error))?;
        if bytes.len() as u64 != before.len() || bytes.len() as u64 > MAX_SETTINGS_BYTES {
            return Err(unknown(&path, "读取设置时内容大小发生变化。"));
        }
        let value = serde_json::from_slice(&bytes).map_err(|error| unknown(&path, error))?;
        let snapshot = Self {
            path,
            opened: Some((file, before)),
            value,
        };
        snapshot.verify()?;
        Ok(snapshot)
    }
    fn verify(&self) -> Result<()> {
        match &self.opened {
            None => match fs::symlink_metadata(&self.path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(unknown(&self.path, error)),
                Ok(_) => Err(unknown(&self.path, "设置在检查期间出现。")),
            },
            Some((file, before)) => {
                let after = Facts::from_file(file).map_err(|error| unknown(&self.path, error))?;
                let current =
                    Facts::capture(&self.path).map_err(|error| unknown(&self.path, error))?;
                if !before.unchanged(&after) || !before.unchanged(&current) {
                    return Err(unknown(&self.path, "设置在检查期间变化。"));
                }
                Ok(())
            }
        }
    }
    fn verify_selection(&self, journals: &BTreeMap<String, MigrationJournal>) -> Result<()> {
        let Some(reference) = self
            .value
            .get("migrationId")
            .filter(|value| !value.is_null())
        else {
            return Ok(());
        };
        let id = reference
            .as_str()
            .filter(|id| crate::projects::valid_id(id))
            .ok_or_else(|| inconsistent(&self.path, reference.clone()))?;
        let journal = journals
            .get(id)
            .ok_or_else(|| inconsistent(&self.path, reference.clone()))?;
        if !journal.published
            || self.value["workspaceRoot"].as_str().map(Path::new)
                != Some(journal.destination_path.as_path())
        {
            return Err(inconsistent(&self.path, reference.clone()));
        }
        // Actual target/selection-marker admission remains with the original
        // RuntimePaths validation; this early checkpoint only reads fixed state.
        Ok(())
    }
}

pub(super) struct Inventory {
    _data: PinnedDirectory,
    _migration: Option<PinnedDirectory>,
    _records: Vec<PinnedDirectory>,
}
fn verify_directory(path: &Path, pinned: &PinnedDirectory) -> Result<()> {
    let expected = native::identity(&pinned.file).map_err(|error| unknown(path, error))?;
    let current = native::open_no_reparse(path).map_err(|error| unknown(path, error))?;
    if !expected.directory || !expected.same_object(&current.identity) {
        return Err(unknown(path, "目录身份在检查期间变化。"));
    }
    Ok(())
}
fn is_empty(path: &Path) -> Result<bool> {
    match fs::read_dir(path)
        .map_err(|error| unknown(path, error))?
        .next()
    {
        None => Ok(true),
        Some(Ok(_)) => Ok(false),
        Some(Err(error)) => Err(unknown(path, error)),
    }
}

fn inspect_with(
    data_root: &Path,
    settings_required: bool,
    mut observe: impl FnMut(Observation, &Path) -> Result<()>,
) -> Result<(Observation, Inventory)> {
    let selected = native::pin_directory(data_root).map_err(|error| unknown(data_root, error))?;
    let (root, data) =
        native::directory_scope(&selected.file).map_err(|error| unknown(data_root, error))?;
    let path = root.join("migrations");
    let migration = match native::pin_directory(&path) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            observe(Observation::Missing, &path)?;
            let settings = SettingsSnapshot::read(&root, settings_required)?;
            settings.verify_selection(&BTreeMap::new())?;
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    settings.verify()?;
                    verify_directory(&root, &data)?;
                    return Ok((
                        Observation::Missing,
                        Inventory {
                            _data: data,
                            _migration: None,
                            _records: vec![],
                        },
                    ));
                }
                Err(error) => return Err(unknown(&path, error)),
                Ok(_) => return Err(unknown(&path, "迁移目录在检查期间出现。")),
            }
        }
        Err(error) => return Err(unknown(&path, error)),
    };
    let before = migration
        .file
        .metadata()
        .map_err(|error| unknown(&path, error))?
        .last_write_time();
    let observation = if is_empty(&path)? {
        Observation::Empty
    } else {
        Observation::Present
    };
    observe(observation, &path)?;
    let settings = SettingsSnapshot::read(&root, settings_required)?;
    let mut records = Vec::new();
    let mut journals = BTreeMap::new();
    for entry in fs::read_dir(&path)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        let journal_path = entry.path().join("journal.json");
        // Preserve the original enumeration boundary. Unknown names and empty
        // unrelated containers are retained, never mistaken for valid journals.
        if !crate::projects::valid_id(&id) || !journal_path.is_file() {
            continue;
        }
        let record =
            native::pin_directory(&entry.path()).map_err(|error| unknown(&entry.path(), error))?;
        let journal: MigrationJournal = workspace::read_json(&journal_path)
            .map_err(|error| workspace::ioerr(error).outcome("unknown"))?;
        workspace::validate_journal(&journal, &id)
            .map_err(|error| workspace::ioerr(error).outcome("unknown"))?;
        let task = entry.path().join("task");
        if task.is_dir() {
            records.push(native::pin_directory(&task).map_err(|error| unknown(&task, error))?);
        }
        verify_directory(&entry.path(), &record)?;
        records.push(record);
        journals.insert(id, journal);
    }
    settings.verify_selection(&journals)?;
    if observation == Observation::Empty {
        let after = migration
            .file
            .metadata()
            .map_err(|error| unknown(&path, error))?
            .last_write_time();
        if !is_empty(&path)? || before != after {
            return Err(unknown(&path, "迁移目录在空目录检查期间变化。"));
        }
    }
    settings.verify()?;
    verify_directory(&path, &migration)?;
    verify_directory(&root, &data)?;
    Ok((
        observation,
        Inventory {
            _data: data,
            _migration: Some(migration),
            _records: records,
        },
    ))
}

pub(super) fn require_before_initialization(data_root: &Path) -> Result<()> {
    inspect_with(data_root, false, |_, _| Ok(())).map(|_| ())
}
pub(super) fn pin_recovery(data_root: &Path) -> Result<Inventory> {
    inspect_with(data_root, true, |_, _| Ok(())).map(|(_, inventory)| inventory)
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
