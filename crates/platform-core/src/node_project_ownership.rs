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
    initialization_source: Option<GeneratedSource>,
    retained_materials: Vec<GeneratedSource>,
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
        if let Some(source) = &record.initialization_source {
            validate_generated_path(&source.path)?;
        }
        for material in &record.retained_materials {
            validate_generated_path(&material.path)?;
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
                initialization_source: None,
                retained_materials: vec![],
            },
            Err(error) => return Err(error.into()),
        };
        change(&mut record)?;
        for directory in [self.root(), self.directory(id)?] {
            crate::web_project_ownership_io::prepare_directory(&directory)?;
        }
        atomic_json(&path, &record)
    }
    pub(crate) fn initialize(&self, id: &str) -> Result<()> {
        self.update(id, true, |_| Ok(()))
    }
    /// Permanent deletion authority for platform-downloaded project folders.
    /// A user-selected folder never enters this field, even when initialized here.
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
    #[cfg(test)]
    pub(crate) fn has_generated_source(&self, id: &str, path: &Path) -> Result<bool> {
        Ok(self.read(id)?.generated_source.is_some_and(|source| {
            source.path == path && source.identity.matches_path(path).unwrap_or(false)
        }))
    }
    /// A pending initialization keeps its UUID and exact root identity without
    /// granting permission to delete the user's selected project folder.
    pub(crate) fn record_initialization_source(&self, id: &str, path: &Path) -> Result<()> {
        validate_generated_path(path)?;
        check_no_symlinks(path, false)?;
        if !path.is_dir() {
            return Err(invalid("初始化项目根目录无效。"));
        }
        let identity = PersistedIdentity::capture(path).map_err(|e| invalid(e.to_string()))?;
        self.update(id, false, |record| {
            if record.generated_source.is_some() {
                return Err(invalid("平台所有项目不能认作用户目录初始化。"));
            }
            if let Some(previous) = &record.initialization_source {
                if previous.path != path || !previous.identity.matches_captured(&identity) {
                    return Err(invalid("初始化项目根目录已有不同记录。"));
                }
            } else {
                record.initialization_source = Some(GeneratedSource {
                    path: path.into(),
                    identity,
                });
            }
            Ok(())
        })
    }
    pub(crate) fn has_initialization_source(&self, id: &str, path: &Path) -> Result<bool> {
        Ok(self.read(id)?.initialization_source.is_some_and(|source| {
            source.path == path && source.identity.matches_path(path).unwrap_or(false)
        }))
    }
    pub(crate) fn finish_initialization_source(&self, id: &str, path: &Path) -> Result<()> {
        if self.read(id)?.initialization_source.is_none() {
            return Ok(());
        }
        self.update(id, false, |record| {
            if let Some(source) = &record.initialization_source {
                if source.path != path || !source.identity.matches_path(path).unwrap_or(false) {
                    return Err(invalid("初始化项目根目录身份已变化。"));
                }
                record.initialization_source = None;
            }
            Ok(())
        })
    }
    pub(crate) fn generated_source(&self, id: &str) -> Result<Option<PathBuf>> {
        Ok(self.read(id)?.generated_source.map(|source| source.path))
    }

    pub(crate) fn owns_source_directory(&self, id: &str, source: &Path) -> Result<bool> {
        // A source question must not reinterpret every task/author record using
        // this manager's workspace spelling. During a verified workspace switch
        // the source binding and the ownership index commit in separate steps.
        let record = self.read(id)?;
        for material in record
            .generated_source
            .iter()
            .chain(record.retained_materials.iter())
        {
            if material.path == source {
                verify_source_root(source, Some(&material.identity))?;
                return Ok(true);
            }
        }
        for (key, identity) in &record.artifacts {
            if key.starts_with("author:") && self.resolve(id, key)? == source {
                verify_source_root(source, identity.as_ref())?;
                return Ok(true);
            }
        }
        Ok(false)
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
                    let current = crate::node_apps::source_ownership::require_claimed_source_owner(
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
        if let Some(source) = record.initialization_source {
            if !source.identity.matches_path(&source.path).unwrap_or(false) {
                return Err(invalid("初始化项目目录身份已变化。"));
            }
            roots.push(source.path);
        }
        roots.sort();
        roots.dedup();
        Ok(roots)
    }
    /// Includes unregistered initialized folders: a source claim never grants
    /// deletion ownership, and workspace cleanup must preserve those originals.
    pub(crate) fn user_source_roots(&self) -> Result<Vec<PathBuf>> {
        let mut roots = Vec::new();
        for (ledger, owned) in self.all_owned_scopes()? {
            let id = ledger
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .ok_or_else(|| invalid("项目归属目录标识无效。"))?;
            for source in self.source_roots(id)? {
                if !owned.iter().any(|root| source.starts_with(root)) {
                    roots.push(source);
                }
            }
        }
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
fn verify_source_root(path: &Path, expected: Option<&PersistedIdentity>) -> Result<()> {
    check_no_symlinks(path, true)?;
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.is_dir()
                && match expected {
                    Some(expected) => expected.matches_path(path).unwrap_or(false),
                    None => fs::read_dir(path)?.next().is_none(),
                } =>
        {
            Ok(())
        }
        Ok(_) => Err(invalid("项目源码目录归属已变化。")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(expected) = expected {
                expected
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
    if !platform_native::filesystem::is_explicit_non_root_absolute_path(path) {
        return Err(invalid("生成项目根目录必须是明确的绝对目录。"));
    }
    Ok(())
}

#[cfg(all(test, windows))]
#[path = "node_project_ownership_windows_tests.rs"]
mod windows_tests;

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
        let path = ownership.data.join("onboarding/imports").join(&task);
        fs::create_dir_all(&path).unwrap();
        ownership.record_onboarding(&id, &task).unwrap();
        fs::rename(&path, path.with_extension("previous")).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(ownership.record_onboarding(&id, &task).is_err());
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
    #[test]
    fn selected_folder_initialization_never_grants_deletion_ownership() {
        let (temporary, ownership, id) = fixture();
        let source = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-user-starter");
        fs::create_dir(&source).unwrap();
        ownership
            .record_initialization_source(&id, &source)
            .unwrap();
        assert!(ownership.has_initialization_source(&id, &source).unwrap());
        assert!(!ownership.has_generated_source(&id, &source).unwrap());
        assert!(!ownership.owned_paths(&id).unwrap().contains(&source));
        assert!(!ownership
            .external_owned_paths(&id)
            .unwrap()
            .contains(&source));
        fs::create_dir(source.join("data")).unwrap();
        fs::write(source.join("data/retained.txt"), b"user data").unwrap();
        ownership
            .finish_initialization_source(&id, &source)
            .unwrap();
        assert!(!ownership.has_initialization_source(&id, &source).unwrap());
        assert!(!ownership.owned_paths(&id).unwrap().contains(&source));
        assert_eq!(
            fs::read(source.join("data/retained.txt")).unwrap(),
            b"user data"
        );
    }
}
