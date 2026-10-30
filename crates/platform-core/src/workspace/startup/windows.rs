//! Read-only admission for the fixed migration recovery tree. Other workspace,
//! application and platform material is neither inventoried nor adopted here.
use crate::ApiError;
use platform_native::{
    tool_files::Facts,
    windows::files::{self as native, PinnedDirectory},
};
use serde_json::Value;
use std::{
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
fn retained(path: &Path) -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 工作区迁移的启动恢复尚未完成，现有迁移材料已保留。",
    )
    .detail("platform", "windows")
    .detail("capability", CAPABILITY)
    .detail("inventory", "retained")
    .detail("path", path.to_string_lossy().into_owned())
    .outcome("not_started")
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
}
impl SettingsSnapshot {
    fn read(root: &Path, required: bool) -> Result<Self> {
        let path = root.join("settings.json");
        let mut file = match native::open_read_file_allowing_hard_links(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => {
                return Ok(Self { path, opened: None });
            }
            Err(error) => return Err(unknown(&path, error)),
        };
        // This is the existing RuntimePaths settings-file contract, not a
        // private-ACL requirement for empty migration roots or system parents.
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
        let selected: Value =
            serde_json::from_slice(&bytes).map_err(|error| unknown(&path, error))?;
        if selected
            .get("migrationId")
            .is_some_and(|value| !value.is_null())
        {
            // The selected journal also carries sourceCleanup/cleanupTaskId.
            // An empty physical tree never erases that reference or proves the
            // old source's automatic finalization already completed.
            return Err(inconsistent(&path, selected["migrationId"].clone()));
        }
        let snapshot = Self {
            path,
            opened: Some((file, before)),
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
}

fn verify_directory(path: &Path, pinned: &PinnedDirectory) -> Result<()> {
    let expected = native::identity(&pinned.file).map_err(|error| unknown(path, error))?;
    let current = native::open_no_reparse(path).map_err(|error| unknown(path, error))?;
    if !expected.directory || !expected.same_object(&current.identity) {
        return Err(unknown(path, "目录身份在检查期间变化。"));
    }
    Ok(())
}
fn require_empty(path: &Path) -> Result<()> {
    match fs::read_dir(path)
        .map_err(|error| unknown(path, error))?
        .next()
    {
        None => Ok(()),
        Some(Ok(_)) => Err(retained(path)),
        Some(Err(error)) => Err(unknown(path, error)),
    }
}
fn inspect_with(
    data_root: &Path,
    settings_required: bool,
    mut observe: impl FnMut(Observation, &Path) -> Result<()>,
) -> Result<Observation> {
    let selected = native::pin_directory(data_root).map_err(|error| unknown(data_root, error))?;
    let (root, data) =
        native::directory_scope(&selected.file).map_err(|error| unknown(data_root, error))?;
    let path = root.join("migrations");
    let migration = match native::pin_directory(&path) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            observe(Observation::Missing, &path)?;
            let settings = SettingsSnapshot::read(&root, settings_required)?;
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    settings.verify()?;
                    verify_directory(&root, &data)?;
                    return Ok(Observation::Missing);
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
    require_empty(&path)?;
    observe(Observation::Empty, &path)?;
    let settings = SettingsSnapshot::read(&root, settings_required)?;
    require_empty(&path)?;
    let after = migration
        .file
        .metadata()
        .map_err(|error| unknown(&path, error))?
        .last_write_time();
    settings.verify()?;
    verify_directory(&path, &migration)?;
    verify_directory(&root, &data)?;
    if before != after {
        return Err(unknown(&path, "迁移目录在检查期间变化。"));
    }
    Ok(Observation::Empty)
}

pub(super) fn require_before_initialization(data_root: &Path) -> Result<()> {
    inspect_with(data_root, false, |_, _| Ok(())).map(|_| ())
}
pub(super) fn recover_startup(data_root: &Path) -> Result<()> {
    // Runtime startup still requires the real settings file; only the earlier
    // first-installation checkpoint may observe it genuinely absent.
    inspect_with(data_root, true, |_, _| Ok(())).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-workspace-startup-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        (temporary, root)
    }
    fn settings(root: &Path, migration: Value) -> Vec<u8> {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schemaVersion":1,"identifier":"disposable-inventory-fixture",
            "environment":"development","workspaceRoot":root.join("retained-workspace"),
            "lastPort":null,"migrationId":migration
        }))
        .unwrap();
        let mut file = native::create_private_file(&root.join("settings.json")).unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        bytes
    }
    #[test]
    fn missing_or_inherited_empty_migration_roots_preserve_other_application_material() {
        let (_temporary, root) = fixture();
        fs::create_dir(root.join("node-project-ownership")).unwrap();
        fs::write(
            root.join("node-project-ownership/retained"),
            b"other application evidence",
        )
        .unwrap();
        assert_eq!(
            inspect_with(&root, false, |_, _| Ok(())).unwrap(),
            Observation::Missing
        );
        assert!(!root.join("migrations").exists());
        assert!(!root.join("settings.json").exists());
        let bytes = settings(&root, Value::Null);
        recover_startup(&root).unwrap();
        fs::create_dir(root.join("migrations")).unwrap();
        assert_eq!(
            inspect_with(&root, true, |_, _| Ok(())).unwrap(),
            Observation::Empty
        );
        assert_eq!(fs::read(root.join("settings.json")).unwrap(), bytes);
        assert_eq!(
            fs::read(root.join("node-project-ownership/retained")).unwrap(),
            b"other application evidence"
        );
        assert_eq!(fs::read_dir(root.join("migrations")).unwrap().count(), 0);
    }
    #[test]
    fn private_hard_linked_settings_keep_the_existing_paths_admission_contract() {
        let (_temporary, root) = fixture();
        let bytes = settings(&root, Value::Null);
        let original = root.join("settings.json");
        let retained_link = root.join("retained-settings-link.json");
        fs::hard_link(&original, &retained_link).unwrap();
        let before = Facts::capture(&original).unwrap();
        assert_eq!(before.links(), 2);
        let opened = native::open_read_file_allowing_hard_links(&original).unwrap();
        native::verify_private(&opened).unwrap();
        for empty_root in [false, true] {
            if empty_root {
                fs::create_dir(root.join("migrations")).unwrap();
            }
            require_before_initialization(&root).unwrap();
            recover_startup(&root).unwrap();
            assert_eq!(fs::read(&original).unwrap(), bytes);
            assert_eq!(fs::read(&retained_link).unwrap(), bytes);
            assert!(before.unchanged(&Facts::capture(&original).unwrap()));
            assert!(before.same_object(&Facts::capture(&retained_link).unwrap()));
        }
    }
    #[test]
    fn selected_missing_journals_never_imply_old_source_cleanup_has_finished() {
        for empty_root in [false, true] {
            let (_temporary, root) = fixture();
            let bytes = settings(&root, Value::String("task_disposable_migration".into()));
            if empty_root {
                fs::create_dir(root.join("migrations")).unwrap();
            }
            for result in [require_before_initialization(&root), recover_startup(&root)] {
                let error = result.unwrap_err();
                assert_eq!(error.code, "WORKSPACE_MIGRATION_FAILED");
                assert_eq!(error.details["inventory"], "inconsistent");
                assert_eq!(error.details["operationOutcome"], "unknown");
                assert_eq!(error.details["migrationId"], "task_disposable_migration");
            }
            assert_eq!(fs::read(root.join("settings.json")).unwrap(), bytes);
        }
    }
    #[test]
    fn pending_completed_unknown_and_empty_migration_containers_are_retained() {
        for name in ["task_pending", "task_completed", "unknown", "task_empty"] {
            let (_temporary, root) = fixture();
            let directory = root.join("migrations").join(name);
            fs::create_dir_all(&directory).unwrap();
            let record = serde_json::to_vec(&serde_json::json!({
                "phase":name,"committed":true,"epochsApplied":false,
                "sourceCleanup":{"phase":"awaitingReopen"},"cleanupTaskId":"task_retained"
            }))
            .unwrap();
            if name != "task_empty" {
                fs::write(directory.join("journal.json"), &record).unwrap();
            }
            let error = require_before_initialization(&root).unwrap_err();
            assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
            assert_eq!(error.details["capability"], CAPABILITY);
            assert_eq!(error.details["operationOutcome"], "not_started");
            assert_eq!(fs::read_dir(root.join("migrations")).unwrap().count(), 1);
            if name != "task_empty" {
                assert_eq!(fs::read(directory.join("journal.json")).unwrap(), record);
            } else {
                assert_eq!(fs::read_dir(directory).unwrap().count(), 0);
            }
        }
    }
    #[test]
    fn newly_appearing_migration_material_cannot_be_reported_absent_or_empty() {
        for observation in [Observation::Missing, Observation::Empty] {
            let (_temporary, root) = fixture();
            if observation == Observation::Empty {
                fs::create_dir(root.join("migrations")).unwrap();
            }
            let error = inspect_with(&root, false, |actual, path| {
                assert_eq!(actual, observation);
                if observation == Observation::Missing {
                    fs::create_dir(path).unwrap();
                }
                fs::write(path.join("appeared"), b"retained concurrent evidence").unwrap();
                Ok(())
            })
            .unwrap_err();
            assert!(matches!(
                error.code.as_str(),
                "WORKSPACE_STARTUP_INVENTORY_UNCONFIRMED" | "PLATFORM_FEATURE_UNAVAILABLE"
            ));
            assert_eq!(
                fs::read(root.join("migrations/appeared")).unwrap(),
                b"retained concurrent evidence"
            );
        }
    }
    #[test]
    fn migration_junctions_and_ordinary_files_are_never_treated_as_missing() {
        let (_temporary, root) = fixture();
        let target = root.join("disposable-external-target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("retained"), b"retained junction bytes").unwrap();
        let link = root.join("migrations");
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(status.status.success());
        assert_eq!(
            require_before_initialization(&root).unwrap_err().code,
            "WORKSPACE_STARTUP_INVENTORY_UNCONFIRMED"
        );
        assert_eq!(
            fs::read(target.join("retained")).unwrap(),
            b"retained junction bytes"
        );
        fs::remove_dir(&link).unwrap();
        fs::write(&link, b"retained unexpected migration file").unwrap();
        assert!(require_before_initialization(&root).is_err());
        assert_eq!(
            fs::read(link).unwrap(),
            b"retained unexpected migration file"
        );
    }
    #[test]
    fn required_settings_and_non_notfound_read_failures_do_not_become_empty_success() {
        let (_temporary, root) = fixture();
        assert!(recover_startup(&root).is_err());
        assert!(!root.join("settings.json").exists());
        let mut file = native::create_private_file(&root.join("settings.json")).unwrap();
        file.write_all(b"not-json retained settings").unwrap();
        drop(file);
        assert!(require_before_initialization(&root).is_err());
        assert_eq!(
            fs::read(root.join("settings.json")).unwrap(),
            b"not-json retained settings"
        );
    }
    #[test]
    fn raw_verbatim_mixed_roots_observe_the_actual_settings_reference() {
        let (_temporary, root) = fixture();
        let actual = root.join("nested/data 中文");
        fs::create_dir_all(&actual).unwrap();
        let bytes = settings(
            &actual,
            Value::String("task_retained_source_cleanup".into()),
        );
        let mut raw = root.as_os_str().to_os_string();
        raw.push(r"\nested/data 中文");
        let error = require_before_initialization(&PathBuf::from(raw)).unwrap_err();
        assert_eq!(error.code, "WORKSPACE_MIGRATION_FAILED");
        assert_eq!(fs::read(actual.join("settings.json")).unwrap(), bytes);
    }
}
