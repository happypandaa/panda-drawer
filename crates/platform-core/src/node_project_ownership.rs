//! Current project-exclusive material ownership. This is a live index, not history.
//! It is created before work starts and removed with the project on deletion.
use crate::{
    builds::atomic_json, file_identity::PersistedIdentity, projects::check_no_symlinks, ApiError,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, ApiError>;
static OWNERSHIP_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
#[derive(Clone)]
pub(crate) struct NodeProjectOwnership {
    data: PathBuf,
    workspace: PathBuf,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ownership {
    version: u32,
    app_id: String,
    artifacts: BTreeMap<String, Option<PersistedIdentity>>,
    tasks: BTreeSet<String>,
    generated_source: Option<GeneratedSource>,
    retained_materials: Vec<GeneratedSource>,
    source_restores: Vec<SourceRestoreOwnership>,
}
const SOURCE_RESTORE_DIRECTORIES: [&str; 2] = [
    "local-projects-node-restores",
    "local-projects-node-restore-preparing",
];
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceRestoreOwnership {
    source_root: PathBuf,
    source_identity: PersistedIdentity,
    directories: BTreeMap<String, Option<PersistedIdentity>>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GeneratedSource {
    path: PathBuf,
    identity: PersistedIdentity,
}
fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new("NODE_PROJECT_OWNERSHIP_INVALID", message)
}
fn app_id(value: &str) -> Result<()> {
    if Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value) {
        Ok(())
    } else {
        Err(invalid("项目归属标识无效。"))
    }
}
fn identifier(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|rest| {
        rest.len() == 32
            && rest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}
impl NodeProjectOwnership {
    pub(crate) fn new(data: PathBuf, workspace: PathBuf) -> Self {
        Self { data, workspace }
    }
    fn root(&self) -> PathBuf {
        self.data.join("node-project-ownership")
    }
    pub(crate) fn directory(&self, id: &str) -> Result<PathBuf> {
        app_id(id)?;
        Ok(self.root().join(id))
    }
    fn path(&self, id: &str) -> Result<PathBuf> {
        Ok(self.directory(id)?.join("ownership.json"))
    }
    fn read(&self, id: &str) -> Result<Ownership> {
        let path = self.path(id)?;
        check_no_symlinks(&path, false)?;
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.len() > 1024 * 1024 {
            return Err(invalid("项目归属索引不是有效普通文件。"));
        }
        let bytes = fs::read(path)?;
        if bytes.len() > 1024 * 1024 {
            return Err(invalid("项目归属索引过大。"));
        }
        let record: Ownership = serde_json::from_slice(&bytes)?;
        if record.version != 1 || record.app_id != id {
            return Err(invalid("项目归属索引与实例不符。"));
        }
        for key in record.artifacts.keys() {
            self.resolve(id, key)?;
        }
        if let Some(source) = &record.generated_source {
            validate_generated_path(&source.path)?;
        }
        for material in &record.retained_materials {
            validate_generated_path(&material.path)?;
        }
        for restore in &record.source_restores {
            validate_generated_path(&restore.source_root)?;
            if restore.directories.len() != SOURCE_RESTORE_DIRECTORIES.len()
                || restore
                    .directories
                    .keys()
                    .any(|name| !SOURCE_RESTORE_DIRECTORIES.contains(&name.as_str()))
            {
                return Err(invalid("源码恢复材料种类无效。"));
            }
        }
        if record.tasks.iter().any(|task| !identifier(task, "task_")) {
            return Err(invalid("项目任务归属无效。"));
        }
        Ok(record)
    }
    fn update(
        &self,
        id: &str,
        create: bool,
        change: impl FnOnce(&mut Ownership) -> Result<()>,
    ) -> Result<()> {
        app_id(id)?;
        let _lock = OWNERSHIP_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let path = self.path(id)?;
        check_no_symlinks(&path, true)?;
        let mut record = match fs::symlink_metadata(&path) {
            Ok(_) => self.read(id)?,
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => Ownership {
                version: 1,
                app_id: id.into(),
                artifacts: BTreeMap::new(),
                tasks: BTreeSet::new(),
                generated_source: None,
                retained_materials: vec![],
                source_restores: vec![],
            },
            Err(error) => return Err(error.into()),
        };
        change(&mut record)?;
        for directory in [self.root(), self.directory(id)?] {
            check_no_symlinks(&directory, true)?;
            if !directory.exists() {
                fs::create_dir(&directory)?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
            }
        }
        atomic_json(&path, &record)
    }
    pub(crate) fn initialize(&self, id: &str) -> Result<()> {
        self.update(id, true, |_| Ok(()))
    }
    /// Called while initialization holds the empty, explicitly selected root.
    /// Existing author directories never enter this field.
    pub(crate) fn record_generated_source(&self, id: &str, path: &Path) -> Result<()> {
        validate_generated_path(path)?;
        check_no_symlinks(path, false)?;
        if !path.is_dir() {
            return Err(invalid("生成项目根目录无效。"));
        }
        let identity = PersistedIdentity::capture(path).map_err(|e| invalid(e.to_string()))?;
        self.update(id, false, |record| {
            if let Some(previous) = &record.generated_source {
                if previous.path != path || !previous.identity.matches_captured(&identity) {
                    return Err(invalid("生成项目根目录归属已存在。"));
                }
            } else {
                record.generated_source = Some(GeneratedSource {
                    path: path.into(),
                    identity,
                });
            }
            Ok(())
        })
    }
    pub(crate) fn has_generated_source(&self, id: &str, path: &Path) -> Result<bool> {
        Ok(self.read(id)?.generated_source.is_some_and(|source| {
            source.path == path && source.identity.matches_path(path).unwrap_or(false)
        }))
    }
    pub(crate) fn generated_source(&self, id: &str) -> Result<Option<PathBuf>> {
        Ok(self.read(id)?.generated_source.map(|source| source.path))
    }
    /// Only platform restoration payloads below an already claimed source root
    /// enter this scope. The author repository and its ordinary Git history do not.
    pub(crate) fn record_source_restore(&self, id: &str, source_root: &Path) -> Result<()> {
        validate_generated_path(source_root)?;
        let source_identity = crate::node_apps::development::require_claimed_source_owner(
            &self.data,
            id,
            source_root,
        )?;
        let mut directories = BTreeMap::new();
        for name in SOURCE_RESTORE_DIRECTORIES {
            directories.insert(
                name.into(),
                directory_identity_or_absent(&source_root.join(".git").join(name))?,
            );
        }
        self.update(id, false, |record| {
            if !source_identity.matches_path(source_root).unwrap_or(false) {
                return Err(invalid("源码恢复材料的项目根目录已变化。"));
            }
            if let Some(previous) = record
                .source_restores
                .iter_mut()
                .find(|restore| restore.source_root == source_root)
            {
                if !previous.source_identity.matches_captured(&source_identity) {
                    return Err(invalid("源码恢复材料的项目归属已变化。"));
                }
                for (name, identity) in directories {
                    let path = source_root.join(".git").join(&name);
                    if let Some(Some(previous)) = previous.directories.get(&name) {
                        if path.exists() && !previous.matches_path(&path).unwrap_or(false) {
                            return Err(invalid("源码恢复材料目录身份已变化。"));
                        }
                    }
                    if previous.directories.get(&name).is_some_and(Option::is_none)
                        && identity.is_some()
                    {
                        return Err(invalid("预留的源码恢复目录已由其他写入者创建，未接管。"));
                    }
                }
            } else {
                if directories.values().any(Option::is_some) {
                    return Err(invalid("源码目录已存在未登记的同名恢复材料，未接管。"));
                }
                record.source_restores.push(SourceRestoreOwnership {
                    source_root: source_root.into(),
                    source_identity,
                    directories,
                });
            }
            Ok(())
        })
    }
    pub(crate) fn capture_source_restore_directory(
        &self,
        id: &str,
        source_root: &Path,
        name: &str,
        identity: &PersistedIdentity,
    ) -> Result<()> {
        if !SOURCE_RESTORE_DIRECTORIES.contains(&name) {
            return Err(invalid("源码恢复材料种类无效。"));
        }
        let source_identity = crate::node_apps::development::require_claimed_source_owner(
            &self.data,
            id,
            source_root,
        )?;
        let path = source_root.join(".git").join(name);
        check_no_symlinks(&path, false)?;
        if !path.is_dir() || !identity.matches_path(&path).unwrap_or(false) {
            return Err(invalid("新建源码恢复目录身份已变化。"));
        }
        self.update(id, false, |record| {
            let restore = record
                .source_restores
                .iter_mut()
                .find(|restore| {
                    restore.source_root == source_root
                        && restore.source_identity.matches_captured(&source_identity)
                })
                .ok_or_else(|| invalid("源码恢复材料未预先登记。"))?;
            let reserved = restore
                .directories
                .get_mut(name)
                .ok_or_else(|| invalid("源码恢复目录未预留。"))?;
            if reserved
                .as_ref()
                .is_some_and(|previous| !previous.matches_captured(identity))
            {
                return Err(invalid("源码恢复目录已有不同归属。"));
            }
            *reserved = Some(identity.clone());
            Ok(())
        })
    }
    pub(crate) fn external_owned_paths(&self, id: &str) -> Result<Vec<PathBuf>> {
        let record = self.read(id)?;
        let mut paths: Vec<_> = record
            .retained_materials
            .into_iter()
            .map(|material| material.path)
            .collect();
        if let Some(source) = record.generated_source {
            if !paths.contains(&source.path) {
                paths.push(source.path);
            }
        }
        for restore in record.source_restores {
            for name in SOURCE_RESTORE_DIRECTORIES {
                paths.push(restore.source_root.join(".git").join(name));
            }
        }
        deduplicate_paths(&mut paths);
        Ok(paths)
    }
    /// The public migration has independently copied and verified the complete
    /// workspace and drained its writers. Rebind only recorded current material;
    /// keep this project's old copy in its own deletion scope.
    pub(crate) fn rebind_after_migration(data: &Path, old: &Path, new: &Path) -> Result<()> {
        let old_owner = Self::new(data.into(), old.into());
        let new_owner = Self::new(data.into(), new.into());
        let root = new_owner.root();
        check_no_symlinks(&root, true)?;
        if !root.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if Uuid::parse_str(&id).is_err() {
                continue;
            }
            let admitted = new_owner.read(&id)?;
            let mut copied_source_roots = vec![];
            for key in admitted
                .artifacts
                .keys()
                .filter(|key| key.starts_with("claim:"))
            {
                let claim = old_owner.resolve(&id, key)?.join("owner.json");
                check_no_symlinks(&claim, true)?;
                if !claim.exists() {
                    continue;
                }
                let metadata = fs::symlink_metadata(&claim)?;
                if !metadata.is_file() || metadata.len() > 1024 * 1024 {
                    return Err(invalid("源码归属记录无效。"));
                }
                let value: serde_json::Value = serde_json::from_slice(&fs::read(&claim)?)?;
                let source = value["sourceRoot"]
                    .as_str()
                    .map(PathBuf::from)
                    .ok_or_else(|| invalid("源码归属根目录缺失。"))?;
                let Ok(relative) = source.strip_prefix(old) else {
                    continue;
                };
                crate::node_apps::development::require_claimed_source_owner(data, &id, &source)?;
                let target = new.join(relative);
                crate::node_apps::development::claim_source_after_verified_migration(
                    data, new, &id, &target,
                )?;
                let already_owned =
                    admitted.artifacts.keys().any(|key| {
                        new_owner
                            .resolve(&id, key)
                            .is_ok_and(|path| target.starts_with(path))
                    }) || admitted.generated_source.as_ref().is_some_and(|generated| {
                        generated
                            .path
                            .strip_prefix(old)
                            .is_ok_and(|relative| target.starts_with(new.join(relative)))
                    }) || admitted
                        .retained_materials
                        .iter()
                        .any(|material| target.starts_with(&material.path));
                if !already_owned {
                    copied_source_roots.push(GeneratedSource {
                        path: target.clone(),
                        identity: PersistedIdentity::capture(&target)
                            .map_err(|error| invalid(error.to_string()))?,
                    });
                }
            }
            new_owner.update(&id, false, |record| {
                for (key, identity) in &mut record.artifacts {
                    let before = old_owner.resolve(&id, key)?;
                    let after = new_owner.resolve(&id, key)?;
                    if before == after {
                        continue;
                    }
                    rebind_material(&before, &after, identity, &mut record.retained_materials)?;
                }
                if let Some(source) = &mut record.generated_source {
                    if let Ok(relative) = source.path.strip_prefix(old) {
                        let before = source.path.clone();
                        let after = new.join(relative);
                        let mut identity = Some(source.identity.clone());
                        rebind_material(
                            &before,
                            &after,
                            &mut identity,
                            &mut record.retained_materials,
                        )?;
                        source.path = after;
                        source.identity =
                            identity.ok_or_else(|| invalid("生成项目根目录迁移归属缺失。"))?;
                    }
                }
                let mut migrated_restores = vec![];
                for restore in &record.source_restores {
                    let Ok(relative) = restore.source_root.strip_prefix(old) else {
                        continue;
                    };
                    let target_root = new.join(relative);
                    if record
                        .source_restores
                        .iter()
                        .any(|item| item.source_root == target_root)
                    {
                        continue;
                    }
                    if !restore
                        .source_identity
                        .matches_path(&restore.source_root)
                        .unwrap_or(false)
                    {
                        return Err(invalid("迁移前源码恢复材料的项目根目录已变化。"));
                    }
                    let target_identity = PersistedIdentity::capture(&target_root)
                        .map_err(|error| invalid(error.to_string()))?;
                    let mut copied = restore.clone();
                    copied.source_root = target_root;
                    copied.source_identity = target_identity;
                    for (name, identity) in &mut copied.directories {
                        rebind_material(
                            &restore.source_root.join(".git").join(name),
                            &copied.source_root.join(".git").join(name),
                            identity,
                            &mut vec![],
                        )?;
                    }
                    migrated_restores.push(copied);
                }
                record.source_restores.extend(migrated_restores);
                for copied in copied_source_roots {
                    if !record
                        .retained_materials
                        .iter()
                        .any(|material| material.path == copied.path)
                    {
                        record.retained_materials.push(copied);
                    }
                }
                Ok(())
            })?;
        }
        Ok(())
    }
    fn record(&self, id: &str, key: String) -> Result<()> {
        let path = self.resolve(id, &key)?;
        check_no_symlinks(&path, true)?;
        let identity = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {
                Some(PersistedIdentity::capture(&path).map_err(|e| invalid(e.to_string()))?)
            }
            Ok(_) => return Err(invalid("项目材料位置不是普通目录。")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        self.update(id, false, |record| {
            if let Some(Some(previous)) = record.artifacts.get(&key) {
                if path.exists()
                    && !previous
                        .matches_path(&path)
                        .map_err(|e| invalid(e.to_string()))?
                {
                    return Err(invalid("项目材料目录身份已变化。"));
                }
            }
            if identity.is_some() || !record.artifacts.contains_key(&key) {
                record.artifacts.insert(key, identity);
            }
            Ok(())
        })
    }
    pub(crate) fn record_task(&self, id: &str, task: &str) -> Result<()> {
        if !identifier(task, "task_") {
            return Err(invalid("任务标识无效。"));
        }
        self.record(id, format!("task:{task}"))?;
        self.update(id, false, |record| {
            record.tasks.insert(task.into());
            Ok(())
        })
    }
    pub(crate) fn record_source(&self, id: &str, source: &str) -> Result<()> {
        self.record(id, format!("source:{source}"))
    }
    pub(crate) fn record_installation(&self, id: &str, candidate: &str) -> Result<()> {
        self.record(id, format!("installation:{candidate}"))
    }
    pub(crate) fn record_build(&self, id: &str, task: &str) -> Result<()> {
        self.record(id, format!("build:{task}"))
    }
    pub(crate) fn record_onboarding(&self, id: &str, task: &str) -> Result<()> {
        self.record(id, format!("onboarding:{task}"))
    }
    pub(crate) fn forget_source_claim(&self, id: &str, hash: &str) -> Result<()> {
        let key = format!("claim:{hash}");
        self.resolve(id, &key)?;
        self.update(id, false, |record| {
            record.artifacts.remove(&key);
            Ok(())
        })
    }
    pub(crate) fn record_material_operation(&self, id: &str, path: &Path) -> Result<()> {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("材料目录名称无效。"))?;
        if path.parent() == Some(self.data.join("node-development-builds").as_path()) {
            self.record_build(id, name)
        } else if path.parent() == Some(self.data.join("node-apps/.installations").as_path()) {
            self.record_installation(id, name)
        } else {
            Err(invalid("材料目录不属于本项目固定管理位置。"))
        }
    }
    pub(crate) fn record_source_claim(&self, id: &str, hash: &str) -> Result<()> {
        self.record(id, format!("claim:{hash}"))
    }
    pub(crate) fn record_publication(&self, id: &str) -> Result<()> {
        self.record(id, "publication".into())
    }
    pub(crate) fn reserve_published_source_identity(
        &self,
        id: &str,
        directory: &str,
        expected: &PersistedIdentity,
    ) -> Result<bool> {
        let key = format!("author:{directory}");
        let path = self.resolve(id, &key)?;
        check_no_symlinks(&path, true)?;
        if path.exists() && !expected.matches_path(&path).unwrap_or(false) {
            return Err(invalid("发布目标已被其他目录占用，未接管。"));
        }
        let mut newly_reserved = false;
        self.update(id, false, |record| {
            match record.artifacts.get(&key) {
                Some(Some(previous)) if !previous.matches_captured(expected) => {
                    return Err(invalid("源码发布目标已属于另一份材料。"))
                }
                Some(Some(_)) => (),
                _ => {
                    record.artifacts.insert(key, Some(expected.clone()));
                    newly_reserved = true;
                }
            }
            Ok(())
        })?;
        Ok(newly_reserved)
    }
    pub(crate) fn forget_unpublished_source_identity(
        &self,
        id: &str,
        directory: &str,
        expected: &PersistedIdentity,
    ) -> Result<()> {
        let key = format!("author:{directory}");
        let path = self.resolve(id, &key)?;
        check_no_symlinks(&path, true)?;
        if expected.matches_path(&path).unwrap_or(false) {
            return Ok(());
        }
        self.update(id, false, |record| {
            if record
                .artifacts
                .get(&key)
                .and_then(Option::as_ref)
                .is_some_and(|identity| identity.matches_captured(expected))
            {
                record.artifacts.remove(&key);
            }
            Ok(())
        })
    }
    fn resolve(&self, id: &str, key: &str) -> Result<PathBuf> {
        app_id(id)?;
        if key == "publication" {
            return Ok(self
                .workspace
                .join(".platform/source-publications")
                .join(id));
        }
        let (kind, value) = key
            .split_once(':')
            .ok_or_else(|| invalid("项目材料种类无效。"))?;
        match kind {
            "claim"
                if value.len() == 64
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) =>
            {
                Ok(self.data.join("node-source-owners").join(value))
            }
            "onboarding" if identifier(value, "task_") => {
                Ok(self.data.join("onboarding/imports").join(value))
            }
            "task" if identifier(value, "task_") => {
                Ok(self.workspace.join(".platform/tasks").join(value))
            }
            "source" if identifier(value, "src_") => {
                Ok(self.workspace.join(".platform/sources").join(value))
            }
            "installation" if identifier(value, "install_") => {
                Ok(self.data.join("node-apps/.installations").join(value))
            }
            "build" if identifier(value, "task_") => {
                Ok(self.data.join("node-development-builds").join(value))
            }
            "author"
                if !value.is_empty()
                    && value.len() <= 240
                    && Path::new(value).components().count() == 1
                    && value != "."
                    && value != ".."
                    && !value.contains(['/', '\\', '\0']) =>
            {
                Ok(self.workspace.join("project-sources").join(value))
            }
            _ => Err(invalid("项目材料名称不在固定管理范围内。")),
        }
    }
    pub(crate) fn owned_paths(&self, id: &str) -> Result<Vec<PathBuf>> {
        let record = self.read(id)?;
        let mut paths = Vec::new();
        for (key, identity) in &record.artifacts {
            let path = self.resolve(id, key)?;
            check_no_symlinks(&path, true)?;
            match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if !metadata.is_dir()
                        || !match identity {
                            Some(identity) => identity.matches_path(&path).unwrap_or(false),
                            None => fs::read_dir(&path)?.next().is_none(),
                        }
                    {
                        return Err(invalid(format!(
                            "项目材料目录归属尚未核实：{}",
                            path.display()
                        )));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if let Some(identity) = identity {
                        identity
                            .absent_on_expected_volume(&path)
                            .map_err(|error| invalid(error.to_string()))?;
                    }
                }
                Err(error) => return Err(error.into()),
            }
            paths.push(path);
        }
        for source in record
            .generated_source
            .into_iter()
            .chain(record.retained_materials)
        {
            check_no_symlinks(&source.path, true)?;
            match fs::symlink_metadata(&source.path) {
                Ok(metadata)
                    if metadata.is_dir()
                        && source.identity.matches_path(&source.path).unwrap_or(false) => {}
                Ok(_) => return Err(invalid("项目受管材料目录身份已变化。")),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    source
                        .identity
                        .absent_on_expected_volume(&source.path)
                        .map_err(|error| invalid(error.to_string()))?;
                }
                Err(error) => return Err(error.into()),
            }
            if !paths.contains(&source.path) {
                paths.push(source.path);
            }
        }
        for restore in record.source_restores {
            for (name, identity) in restore.directories {
                let path = restore.source_root.join(".git").join(name);
                if paths.iter().any(|ancestor| path.starts_with(ancestor)) {
                    continue;
                }
                check_no_symlinks(&restore.source_root, true)?;
                if restore.source_root.exists()
                    && !restore
                        .source_identity
                        .matches_path(&restore.source_root)
                        .unwrap_or(false)
                {
                    return Err(invalid("源码恢复材料的项目根目录已变化。"));
                }
                validate_owned_directory(&path, identity.as_ref())?;
                paths.push(path);
            }
        }
        deduplicate_paths(&mut paths);
        // This directory is last: the deletion journal has captured all ownership.
        paths.push(self.directory(id)?);
        Ok(paths)
    }
    pub(crate) fn task_ids(&self, id: &str) -> Result<Vec<String>> {
        Ok(self.read(id)?.tasks.into_iter().collect())
    }
    pub(crate) fn source_roots(&self, id: &str) -> Result<Vec<PathBuf>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Claim {
            version: u32,
            app_id: String,
            source_root: PathBuf,
            identity: PersistedIdentity,
        }
        let record = self.read(id)?;
        let mut roots = vec![];
        for key in record
            .artifacts
            .keys()
            .filter(|key| key.starts_with("claim:"))
        {
            let directory = self.resolve(id, key)?;
            let path = directory.join("owner.json");
            check_no_symlinks(&path, true)?;
            if !path.try_exists()? {
                continue;
            }
            let claim: Claim = serde_json::from_slice(&fs::read(&path)?)?;
            if claim.version != 1 || claim.app_id != id || !claim.source_root.is_absolute() {
                return Err(invalid("源码占有记录身份不符。"));
            }
            check_no_symlinks(&claim.source_root, true)?;
            match fs::symlink_metadata(&claim.source_root) {
                Ok(_) => {
                    let current = crate::node_apps::development::require_claimed_source_owner(
                        &self.data,
                        id,
                        &claim.source_root,
                    )?;
                    if !claim.identity.matches_captured(&current) {
                        return Err(invalid("源码目录身份已变化。"));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    claim
                        .identity
                        .absent_on_expected_volume(&claim.source_root)
                        .map_err(|error| invalid(error.to_string()))?;
                }
                Err(error) => return Err(error.into()),
            }
            roots.push(claim.source_root);
        }
        if let Some(source) = record.generated_source {
            roots.push(source.path);
        }
        roots.extend(
            record
                .source_restores
                .into_iter()
                .map(|restore| restore.source_root),
        );
        roots.sort();
        roots.dedup();
        Ok(roots)
    }
    pub(crate) fn all_owned_scopes(&self) -> Result<Vec<(PathBuf, Vec<PathBuf>)>> {
        let mut scopes = vec![];
        check_no_symlinks(&self.root(), true)?;
        if !self.root().exists() {
            return Ok(scopes);
        }
        for entry in fs::read_dir(self.root())? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if id == ".pending-deletions" {
                continue;
            }
            app_id(&id)?;
            scopes.push((self.path(&id)?, self.owned_paths(&id)?));
        }
        scopes.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(scopes)
    }
    /// Global tool admission uses exact owned roots, without inspecting any
    /// project's contents or requiring unrelated materials to be healthy.
    pub(crate) fn require_global_tool_unowned(&self, candidate: &Path) -> Result<()> {
        fn contains(candidate: &Path, root: &Path, identity: Option<&PersistedIdentity>) -> bool {
            candidate.starts_with(root)
                || identity.is_some_and(|identity| {
                    candidate
                        .ancestors()
                        .any(|ancestor| identity.matches_path(ancestor).unwrap_or(false))
                })
        }
        check_no_symlinks(&self.root(), true)?;
        if !self.root().try_exists()? {
            return Ok(());
        }
        for entry in fs::read_dir(self.root())? {
            let id = entry?.file_name().to_string_lossy().into_owned();
            if id == ".pending-deletions" {
                continue;
            }
            let record = self.read(&id)?;
            for (key, identity) in &record.artifacts {
                if contains(candidate, &self.resolve(&id, key)?, identity.as_ref()) {
                    return Err(crate::project_tool_boundary::project_owned());
                }
            }
            for source in record
                .generated_source
                .iter()
                .chain(record.retained_materials.iter())
            {
                if contains(candidate, &source.path, Some(&source.identity)) {
                    return Err(crate::project_tool_boundary::project_owned());
                }
            }
            for restore in &record.source_restores {
                for (name, identity) in &restore.directories {
                    let path = restore.source_root.join(".git").join(name);
                    if contains(candidate, &path, identity.as_ref()) {
                        return Err(crate::project_tool_boundary::project_owned());
                    }
                }
            }
        }
        Ok(())
    }
}
fn rebind_material(
    before: &Path,
    after: &Path,
    identity: &mut Option<PersistedIdentity>,
    retained: &mut Vec<GeneratedSource>,
) -> Result<()> {
    check_no_symlinks(before, true)?;
    check_no_symlinks(after, true)?;
    if !after.exists() {
        if before.exists() {
            return Err(invalid("项目材料未包含在已核验的工作区迁移副本。"));
        }
        return Ok(());
    }
    let observed = PersistedIdentity::capture(after).map_err(|error| invalid(error.to_string()))?;
    if identity
        .as_ref()
        .is_some_and(|identity| identity.matches_captured(&observed))
    {
        return Ok(());
    }
    let previous =
        PersistedIdentity::capture(before).map_err(|error| invalid(error.to_string()))?;
    if !match identity {
        Some(identity) => identity.matches_captured(&previous),
        None => fs::read_dir(before)?.next().is_none(),
    } {
        return Err(invalid("迁移前项目材料目录身份已变化。"));
    }
    if let Some(existing) = retained.iter().find(|material| material.path == before) {
        if !existing.identity.matches_captured(&previous) {
            return Err(invalid("保留的项目迁移副本目录身份已变化。"));
        }
    } else {
        retained.push(GeneratedSource {
            path: before.into(),
            identity: previous,
        });
    }
    *identity = Some(observed);
    Ok(())
}
fn directory_identity_or_absent(path: &Path) -> Result<Option<PersistedIdentity>> {
    check_no_symlinks(path, true)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(Some(
            PersistedIdentity::capture(path).map_err(|error| invalid(error.to_string()))?,
        )),
        Ok(_) => Err(invalid("源码恢复材料位置不是普通目录。")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn validate_owned_directory(path: &Path, identity: Option<&PersistedIdentity>) -> Result<()> {
    check_no_symlinks(path, true)?;
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.is_dir()
                && match identity {
                    Some(identity) => identity.matches_path(path).unwrap_or(false),
                    None => fs::read_dir(path)?.next().is_none(),
                } =>
        {
            Ok(())
        }
        Ok(_) => Err(invalid("源码恢复材料目录归属尚未核实。")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(identity) = identity {
                identity
                    .absent_on_expected_volume(path)
                    .map_err(|error| invalid(error.to_string()))?;
            }
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}
fn deduplicate_paths(paths: &mut Vec<PathBuf>) {
    let candidates = std::mem::take(paths);
    for path in &candidates {
        if !paths.contains(path)
            && !candidates
                .iter()
                .any(|ancestor| ancestor != path && path.starts_with(ancestor))
        {
            paths.push(path.clone());
        }
    }
}
fn validate_generated_path(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.parent().is_none()
        || path.components().any(|part| {
            !matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(invalid("生成项目根目录必须是明确的绝对目录。"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, NodeProjectOwnership, String) {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let data = base.join("disposable-data");
        fs::create_dir(&data).unwrap();
        let ownership = NodeProjectOwnership::new(data, base.join("disposable-workspace"));
        let id = Uuid::new_v4().to_string();
        ownership.initialize(&id).unwrap();
        (temporary, ownership, id)
    }

    #[test]
    fn ownership_reserves_material_before_creation_and_keeps_owner_last() {
        let (_temporary, ownership, id) = fixture();
        let source = format!("src_{}", Uuid::new_v4().simple());
        let path = ownership.workspace.join(".platform/sources").join(&source);
        ownership.record_source(&id, &source).unwrap();
        assert!(!ownership.workspace.exists());
        // An interruption between mkdir and identity capture leaves only an
        // empty, pre-reserved directory. It can still be removed.
        fs::create_dir_all(&path).unwrap();
        let paths = ownership.owned_paths(&id).unwrap();
        assert_eq!(paths, vec![path.clone(), ownership.directory(&id).unwrap()]);
        ownership.record_source(&id, &source).unwrap();
        fs::write(path.join("payload"), "project owned content").unwrap();
        assert_eq!(ownership.owned_paths(&id).unwrap()[0], path);
    }

    #[test]
    fn replaced_material_cannot_be_claimed_by_a_retry() {
        let (_temporary, ownership, id) = fixture();
        let task = format!("task_{}", Uuid::new_v4().simple());
        let path = ownership.data.join("node-development-builds").join(&task);
        fs::create_dir_all(&path).unwrap();
        ownership.record_build(&id, &task).unwrap();
        fs::rename(&path, path.with_extension("previous")).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(ownership.record_build(&id, &task).is_err());
        assert!(ownership.owned_paths(&id).is_err());
    }

    #[test]
    fn late_callbacks_never_recreate_a_deleted_owner() {
        let (_temporary, ownership, id) = fixture();
        fs::remove_dir_all(ownership.directory(&id).unwrap()).unwrap();
        assert!(ownership
            .record_task(&id, &format!("task_{}", Uuid::new_v4().simple()))
            .is_err());
        assert!(!ownership.directory(&id).unwrap().exists());
    }

    #[test]
    fn generated_roots_are_explicit_and_identity_bound() {
        let (temporary, ownership, id) = fixture();
        let external = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-generated-project");
        fs::create_dir(&external).unwrap();
        assert!(!ownership.owned_paths(&id).unwrap().contains(&external));
        ownership.record_generated_source(&id, &external).unwrap();
        fs::write(external.join(".git"), "uncommitted project material").unwrap();
        assert!(ownership.has_generated_source(&id, &external).unwrap());
        assert!(ownership.owned_paths(&id).unwrap().contains(&external));
        fs::rename(&external, external.with_extension("previous")).unwrap();
        fs::create_dir(&external).unwrap();
        assert!(!ownership.has_generated_source(&id, &external).unwrap());
        assert!(ownership.record_generated_source(&id, &external).is_err());
        assert!(ownership.owned_paths(&id).is_err());
    }
    #[test]
    fn missing_known_generated_material_on_another_volume_is_not_deleted() {
        let (temporary, owner, id) = fixture();
        let root = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-offline-source");
        fs::create_dir(&root).unwrap();
        owner.record_generated_source(&id, &root).unwrap();
        fs::remove_dir(&root).unwrap();
        owner.owned_paths(&id).unwrap();
        owner
            .update(&id, false, |record| {
                let expected = &mut record.generated_source.as_mut().unwrap().identity;
                if expected.volume_uuid.is_some() {
                    expected.volume_uuid = Some(Uuid::new_v4().to_string());
                } else {
                    expected.device ^= 1;
                }
                Ok(())
            })
            .unwrap();
        assert!(owner.owned_paths(&id).is_err());
    }
    fn claimed_source(ownership: &NodeProjectOwnership, id: &str, source: &Path) {
        fs::create_dir_all(source.join(".git")).unwrap();
        crate::node_apps::development::claim_source_after_verified_migration(
            &ownership.data,
            &ownership.workspace,
            id,
            source,
        )
        .unwrap();
    }
    fn create_restore_directories(ownership: &NodeProjectOwnership, id: &str, source: &Path) {
        ownership.record_source_restore(id, source).unwrap();
        for name in SOURCE_RESTORE_DIRECTORIES {
            let path = source.join(".git").join(name);
            fs::create_dir(&path).unwrap();
            ownership
                .capture_source_restore_directory(
                    id,
                    source,
                    name,
                    &PersistedIdentity::capture(&path).unwrap(),
                )
                .unwrap();
            fs::write(path.join("recovery-payload"), "project recovery material").unwrap();
        }
    }
    #[test]
    fn restore_ownership_requires_the_claim_and_covers_only_fixed_git_material() {
        let (temporary, ownership, id) = fixture();
        let source = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-external-author");
        fs::create_dir_all(source.join(".git")).unwrap();
        fs::write(source.join(".git/HEAD"), "keep ordinary Git").unwrap();
        assert!(ownership.record_source_restore(&id, &source).is_err());
        claimed_source(&ownership, &id, &source);
        create_restore_directories(&ownership, &id, &source);
        assert!(ownership
            .record_source_restore(&Uuid::new_v4().to_string(), &source)
            .is_err());
        assert!(ownership
            .capture_source_restore_directory(
                &id,
                &source,
                "objects",
                &PersistedIdentity::capture(&source.join(".git")).unwrap()
            )
            .is_err());
        let paths = ownership.owned_paths(&id).unwrap();
        assert!(!paths.contains(&source));
        assert!(!paths.contains(&source.join(".git")));
        for name in SOURCE_RESTORE_DIRECTORIES {
            assert!(paths.contains(&source.join(".git").join(name)));
        }
        assert_eq!(
            fs::read_to_string(source.join(".git/HEAD")).unwrap(),
            "keep ordinary Git"
        );
    }
    #[test]
    fn restoration_never_adopts_preexisting_or_raced_author_directories() {
        for preexisting in [true, false] {
            let (temporary, ownership, id) = fixture();
            let source = temporary
                .path()
                .canonicalize()
                .unwrap()
                .join("disposable-raced-author");
            claimed_source(&ownership, &id, &source);
            if !preexisting {
                ownership.record_source_restore(&id, &source).unwrap();
            }
            let foreign = source.join(".git").join(SOURCE_RESTORE_DIRECTORIES[0]);
            fs::create_dir(&foreign).unwrap();
            fs::write(foreign.join("author-content"), "must not adopt").unwrap();
            assert!(ownership.record_source_restore(&id, &source).is_err());
            if preexisting {
                assert!(ownership.read(&id).unwrap().source_restores.is_empty());
            } else {
                assert!(ownership.owned_paths(&id).is_err());
            }
            assert_eq!(
                fs::read_to_string(foreign.join("author-content")).unwrap(),
                "must not adopt"
            );
        }
    }
    #[test]
    fn a_wholly_owned_source_covers_its_nested_restore_material_once() {
        let (_temporary, ownership, id) = fixture();
        let source = ownership
            .workspace
            .join("project-sources/disposable-owned-author");
        claimed_source(&ownership, &id, &source);
        ownership
            .reserve_published_source_identity(
                &id,
                "disposable-owned-author",
                &PersistedIdentity::capture(&source).unwrap(),
            )
            .unwrap();
        create_restore_directories(&ownership, &id, &source);
        let paths = ownership.owned_paths(&id).unwrap();
        assert!(paths.contains(&source));
        assert!(!paths
            .iter()
            .any(|path| path != &source && path.starts_with(&source)));
    }
}
