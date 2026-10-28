//! Current, project-exclusive web material. Only new admissions create an index.
use crate::{
    builds::atomic_json,
    file_identity::PersistedIdentity,
    projects::{check_no_symlinks, valid_id},
    ApiError,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

type Result<T> = std::result::Result<T, ApiError>;
static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
#[derive(Clone)]
pub(crate) struct WebProjectOwnership {
    data: PathBuf,
    workspace: PathBuf,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OwnedPath {
    path: PathBuf,
    identity: PersistedIdentity,
    directory: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ownership {
    version: u32,
    project_id: String,
    workspace_root: PathBuf,
    project_root: Option<OwnedPath>,
    tasks: BTreeMap<String, Option<PersistedIdentity>>,
    retained: Vec<OwnedPath>,
    export_stages: BTreeMap<String, ExportStage>,
    file_deletions: BTreeMap<PathBuf, crate::web_file_deletion::FileDeletionPlan>,
    removed_materials: BTreeMap<PathBuf, PersistedIdentity>,
    exports: BTreeMap<String, ExportTarget>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExportTarget {
    path: PathBuf,
    task_id: String,
    parent_identity: PersistedIdentity,
    claim_identity: Option<PersistedIdentity>,
    identity: Option<PersistedIdentity>,
    stage_path: Option<PathBuf>,
    published: bool,
    removed: bool,
    release_intent: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExportStage {
    path: PathBuf,
    parent_identity: PersistedIdentity,
    identity: Option<PersistedIdentity>,
}
fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new("WEB_PROJECT_OWNERSHIP_INVALID", message)
}
fn project_id(id: &str) -> Result<()> {
    if valid_id(id) && id.starts_with("prj_") {
        Ok(())
    } else {
        Err(invalid("网页项目归属标识无效。"))
    }
}
fn task_id(id: &str) -> Result<()> {
    if valid_id(id) && id.starts_with("task_") {
        Ok(())
    } else {
        Err(invalid("网页任务标识无效。"))
    }
}
fn absolute(path: &Path) -> Result<()> {
    if path.is_absolute()
        && path.parent().is_some()
        && !path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        Ok(())
    } else {
        Err(invalid("受管路径必须是明确绝对路径。"))
    }
}
fn capture(path: &Path) -> Result<Option<OwnedPath>> {
    check_no_symlinks(path, true)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() || metadata.is_file() => Ok(Some(OwnedPath {
            path: path.into(),
            identity: PersistedIdentity::capture(path)
                .map_err(|error| invalid(error.to_string()))?,
            directory: metadata.is_dir(),
        })),
        Ok(_) => Err(invalid("受管材料不是普通文件或目录。")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
impl OwnedPath {
    fn verify(&self) -> Result<()> {
        absolute(&self.path)?;
        check_no_symlinks(&self.path, true)?;
        if let Some(current) = capture(&self.path)? {
            if current.directory != self.directory
                || !self.identity.matches_captured(&current.identity)
            {
                return Err(invalid("受管材料身份已变化。"));
            }
        } else {
            self.identity
                .absent_on_expected_volume(&self.path)
                .map_err(|error| invalid(error.to_string()))?;
        }
        Ok(())
    }
}
impl WebProjectOwnership {
    pub(crate) fn new(data: PathBuf, workspace: PathBuf) -> Self {
        Self { data, workspace }
    }
    fn root(&self) -> PathBuf {
        self.data.join("web-project-ownership")
    }
    pub(crate) fn directory(&self, id: &str) -> Result<PathBuf> {
        project_id(id)?;
        Ok(self.root().join(id))
    }
    pub(crate) fn path(&self, id: &str) -> Result<PathBuf> {
        Ok(self.directory(id)?.join("ownership.json"))
    }
    fn read(&self, id: &str) -> Result<Ownership> {
        let path = self.path(id)?;
        check_no_symlinks(&path, false)?;
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
            return Err(invalid("网页项目归属索引无效。"));
        }
        let record: Ownership = serde_json::from_slice(&fs::read(path)?)?;
        if record.version != 1 || record.project_id != id {
            return Err(invalid("网页归属索引与项目不符。"));
        }
        for task in record.tasks.keys() {
            task_id(task)?;
        }
        for path in record.project_root.iter().chain(record.retained.iter()) {
            absolute(&path.path)?;
        }
        for (task, stage) in &record.export_stages {
            validate_export_stage(task, &stage.path)?;
        }
        for (key, export) in &record.exports {
            absolute(&export.path)?;
            task_id(&export.task_id)?;
            if crate::web_export_claims::key_from(
                &export.parent_identity,
                export
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| invalid("导出文件名无效。"))?,
            )? != *key
            {
                return Err(invalid("导出目标归属键不符。"));
            }
        }
        for (path, plan) in &record.file_deletions {
            plan.validate()?;
            if path != &plan.original || !self.authorizes_file(&record, path, &plan.expected) {
                return Err(invalid("文件删除计划不属于项目精确材料。"));
            }
        }
        Ok(record)
    }
    fn update(
        &self,
        id: &str,
        create: bool,
        change: impl FnOnce(&mut Ownership) -> Result<()>,
    ) -> Result<()> {
        project_id(id)?;
        let _guard = LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut record = match self.read(id) {
            Ok(record) => record,
            Err(error)
                if create && !self.path(id)?.exists() && error.code == "FILESYSTEM_ERROR" =>
            {
                Ownership {
                    version: 1,
                    project_id: id.into(),
                    workspace_root: self.workspace.clone(),
                    project_root: None,
                    tasks: BTreeMap::new(),
                    retained: vec![],
                    export_stages: BTreeMap::new(),
                    file_deletions: BTreeMap::new(),
                    removed_materials: BTreeMap::new(),
                    exports: BTreeMap::new(),
                }
            }
            Err(error) => return Err(error),
        };
        if record.workspace_root != self.workspace {
            return Err(invalid("网页项目工作区归属已变化。"));
        }
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
        atomic_json(&self.path(id)?, &record)
    }
    pub(crate) fn initialize(&self, id: &str) -> Result<()> {
        self.update(id, true, |_| Ok(()))
    }
    pub(crate) fn discard_unpublished(&self, id: &str) -> Result<()> {
        if !self.path(id)?.exists() {
            return Ok(());
        }
        let record = self.read(id)?;
        if record.project_root.is_some()
            || !record.tasks.is_empty()
            || !record.retained.is_empty()
            || !record.export_stages.is_empty()
            || fixed_paths(&self.workspace, id)
                .iter()
                .any(|path| path.exists())
        {
            return Err(invalid("项目已受理材料，不能按空登记丢弃。"));
        }
        let directory = self.directory(id)?;
        let identity =
            PersistedIdentity::capture(&directory).map_err(|error| invalid(error.to_string()))?;
        crate::history_cleanup::remove_owned_directory(&directory, &identity).map(|_| ())
    }
    pub(crate) fn record_task(&self, id: &str, task: &str) -> Result<()> {
        task_id(task)?;
        self.update(id, false, |record| {
            record.tasks.entry(task.into()).or_insert(None);
            Ok(())
        })
    }
    pub(crate) fn task_ids(&self, id: &str) -> Result<Vec<String>> {
        Ok(self.read(id)?.tasks.into_keys().collect())
    }
    pub(crate) fn task_directory_identity(
        &self,
        id: &str,
        task: &str,
    ) -> Result<Option<PersistedIdentity>> {
        task_id(task)?;
        self.read(id)?
            .tasks
            .get(task)
            .cloned()
            .ok_or_else(|| invalid("任务未预先登记归属。"))
    }
    pub(crate) fn capture_task_directory(
        &self,
        id: &str,
        task: &str,
        identity: &PersistedIdentity,
    ) -> Result<()> {
        task_id(task)?;
        let path = self.workspace.join(".platform/tasks").join(task);
        if !identity.matches_path(&path).unwrap_or(false) {
            return Err(invalid("新建任务目录身份变化。"));
        }
        self.update(id, false, |record| {
            let reserved = record
                .tasks
                .get_mut(task)
                .ok_or_else(|| invalid("任务未预先登记归属。"))?;
            if reserved
                .as_ref()
                .is_some_and(|previous| !previous.matches_captured(identity))
            {
                return Err(invalid("任务目录已有不同归属。"));
            }
            *reserved = Some(identity.clone());
            Ok(())
        })
    }
    pub(crate) fn rebind_recreated_task_directory(
        &self,
        id: &str,
        task: &str,
        previous: Option<&PersistedIdentity>,
        identity: &PersistedIdentity,
    ) -> Result<()> {
        task_id(task)?;
        let path = self.workspace.join(".platform/tasks").join(task);
        if !identity.matches_path(&path).unwrap_or(false) {
            return Err(invalid("重建任务目录身份变化。"));
        }
        self.update(id, false, |record| {
            let reserved = record
                .tasks
                .get_mut(task)
                .ok_or_else(|| invalid("任务未预先登记归属。"))?;
            if !match (reserved.as_ref(), previous) {
                (Some(a), Some(b)) => a.matches_captured(b),
                (None, None) => true,
                _ => false,
            } {
                return Err(invalid("任务目录原归属已变化。"));
            }
            *reserved = Some(identity.clone());
            Ok(())
        })
    }
    fn verify_task_path(&self, task: &str, identity: Option<&PersistedIdentity>) -> Result<()> {
        let path = self.workspace.join(".platform/tasks").join(task);
        check_no_symlinks(&path, true)?;
        if path.exists()
            && (!path.is_dir()
                || !match identity {
                    Some(identity) => identity.matches_path(&path).unwrap_or(false),
                    None => fs::read_dir(&path)?.next().is_none(),
                })
        {
            return Err(invalid("任务目录归属或身份已变化。"));
        }
        if !path.exists() {
            if let Some(identity) = identity {
                identity
                    .absent_on_expected_volume(&path)
                    .map_err(|error| invalid(error.to_string()))?;
            }
        }
        Ok(())
    }
    pub(crate) fn verify_task_directory(&self, id: &str, task: &str) -> Result<()> {
        let identity = self.task_directory_identity(id, task)?;
        self.verify_task_path(task, identity.as_ref())
    }
    pub(crate) fn reserve_export_stage(&self, id: &str, task: &str, path: &Path) -> Result<()> {
        validate_export_stage(task, path)?;
        check_no_symlinks(path, true)?;
        if path.exists() {
            return Err(invalid("导出临时目标已存在，未接管。"));
        }
        let parent_identity = PersistedIdentity::capture(path.parent().unwrap())
            .map_err(|error| invalid(error.to_string()))?;
        self.update(id, false, |record| {
            if !record.tasks.contains_key(task) {
                return Err(invalid("导出任务尚未登记归属。"));
            }
            if record.export_stages.contains_key(task) {
                return Err(invalid("导出任务已有临时材料。"));
            }
            record.export_stages.insert(
                task.into(),
                ExportStage {
                    path: path.into(),
                    parent_identity,
                    identity: None,
                },
            );
            Ok(())
        })
    }
    pub(crate) fn capture_export_stage(
        &self,
        id: &str,
        task: &str,
        path: &Path,
        identity: &PersistedIdentity,
    ) -> Result<()> {
        validate_export_stage(task, path)?;
        let current = capture(path)?.ok_or_else(|| invalid("导出临时文件不存在。"))?;
        if current.directory || !identity.matches_captured(&current.identity) {
            return Err(invalid("导出临时文件身份变化。"));
        }
        self.update(id, false, |record| {
            let stage = record
                .export_stages
                .get_mut(task)
                .filter(|stage| stage.path == path)
                .ok_or_else(|| invalid("导出临时材料未预先登记。"))?;
            if !stage
                .parent_identity
                .matches_path(path.parent().unwrap())
                .unwrap_or(false)
                || stage
                    .identity
                    .as_ref()
                    .is_some_and(|previous| !previous.matches_captured(identity))
            {
                return Err(invalid("导出临时材料归属变化。"));
            }
            stage.identity = Some(identity.clone());
            Ok(())
        })
    }
    pub(crate) fn forget_export_stage(&self, id: &str, task: &str) -> Result<()> {
        self.update(id, false, |record| {
            record.export_stages.remove(task);
            Ok(())
        })
    }
    pub(crate) fn record_project_root(&self, id: &str, path: &Path) -> Result<()> {
        absolute(path)?;
        if path.parent() != Some(self.workspace.join("projects").as_path()) {
            return Err(invalid("网页项目目录不在当前工作区。"));
        }
        let current = capture(path)?.ok_or_else(|| invalid("网页项目目录尚未发布。"))?;
        if !current.directory {
            return Err(invalid("网页项目根不是目录。"));
        }
        self.update(id, false, |record| {
            if record.project_root.as_ref().is_some_and(|previous| {
                previous.path != path || !previous.identity.matches_captured(&current.identity)
            }) {
                return Err(invalid("网页项目目录归属已变化。"));
            }
            record.project_root = Some(current);
            Ok(())
        })
    }
    pub(crate) fn material_paths(&self, id: &str) -> Result<Vec<PathBuf>> {
        let record = self.read(id)?;
        if record.workspace_root != self.workspace {
            return Err(invalid("网页项目工作区归属已变化。"));
        }
        let mut paths = fixed_paths(&self.workspace, id);
        if let Some(root) = &record.project_root {
            root.verify()?;
            paths.push(root.path.clone());
        }
        for (task, identity) in &record.tasks {
            self.verify_task_path(task, identity.as_ref())?;
            paths.push(self.workspace.join(".platform/tasks").join(task));
            paths.push(self.workspace.join(".platform/operations").join(task));
        }
        for retained in &record.retained {
            if record.removed_materials.contains_key(&retained.path) {
                continue;
            }
            if !record.file_deletions.contains_key(&retained.path) {
                retained.verify()?;
            }
            paths.push(retained.path.clone());
        }
        for stage in record.export_stages.values() {
            if record.removed_materials.contains_key(&stage.path) {
                continue;
            }
            if record.file_deletions.contains_key(&stage.path) {
                paths.push(stage.path.clone());
                continue;
            }
            check_no_symlinks(&stage.path, true)?;
            if stage.path.exists() {
                if !stage
                    .parent_identity
                    .matches_path(stage.path.parent().unwrap())
                    .unwrap_or(false)
                    || !stage
                        .identity
                        .as_ref()
                        .is_some_and(|identity| identity.matches_path(&stage.path).unwrap_or(false))
                {
                    return Err(invalid("导出临时文件归属未核实。"));
                }
            } else {
                stage
                    .identity
                    .as_ref()
                    .unwrap_or(&stage.parent_identity)
                    .absent_on_expected_volume(&stage.path)
                    .map_err(|error| invalid(error.to_string()))?;
            }
            paths.push(stage.path.clone());
        }
        for target in record.exports.values() {
            if target.removed || record.removed_materials.contains_key(&target.path) {
                continue;
            }
            if record.file_deletions.contains_key(&target.path) {
                paths.push(target.path.clone());
                continue;
            }
            if let Some(identity) = &target.identity {
                check_no_symlinks(&target.path, true)?;
                if target.path.exists() && !identity.matches_path(&target.path).unwrap_or(false) {
                    if !target.published
                        && target
                            .stage_path
                            .as_ref()
                            .is_some_and(|stage| identity.matches_path(stage).unwrap_or(false))
                    {
                        continue;
                    }
                    return Err(invalid(format!(
                        "导出文件已被替换，未删除：{}",
                        target.path.display()
                    )));
                }
                if !target.path.exists() {
                    identity
                        .absent_on_expected_volume(&target.path)
                        .map_err(|error| invalid(error.to_string()))?;
                }
                paths.push(target.path.clone());
            }
        }
        paths.retain(|path| !record.removed_materials.contains_key(path));
        deduplicate(&mut paths);
        Ok(paths)
    }
    pub(crate) fn material_identity(
        &self,
        id: &str,
        path: &Path,
    ) -> Result<Option<PersistedIdentity>> {
        let record = self.read(id)?;
        if record.workspace_root != self.workspace {
            return Err(invalid("网页项目工作区归属已变化。"));
        }
        if let Some(bound) = record
            .project_root
            .iter()
            .chain(record.retained.iter())
            .find(|bound| bound.path == path)
        {
            return Ok(Some(bound.identity.clone()));
        }
        for (task, identity) in &record.tasks {
            if path == self.workspace.join(".platform/tasks").join(task) {
                self.verify_task_path(task, identity.as_ref())?;
                return Ok(identity
                    .clone()
                    .or(capture(path)?.map(|bound| bound.identity)));
            }
        }
        if let Some(stage) = record
            .export_stages
            .values()
            .find(|stage| stage.path == path)
        {
            return Ok(stage.identity.clone());
        }
        if let Some(export) = record
            .exports
            .values()
            .find(|export| export.path == path && !export.removed)
        {
            return Ok(export.identity.clone());
        }
        let fixed = fixed_paths(&self.workspace, id)
            .iter()
            .any(|fixed| fixed == path)
            || record
                .tasks
                .keys()
                .any(|task| path == self.workspace.join(".platform/operations").join(task));
        if !fixed {
            return Err(invalid("删除材料不在项目精确归属中。"));
        }
        Ok(capture(path)?.map(|bound| bound.identity))
    }
    pub(crate) fn all_owned_scopes(&self) -> Result<Vec<(PathBuf, Vec<PathBuf>)>> {
        check_no_symlinks(&self.root(), true)?;
        if !self.root().exists() {
            return Ok(vec![]);
        }
        let mut scopes = vec![];
        for entry in fs::read_dir(self.root())? {
            let id = entry?.file_name().to_string_lossy().into_owned();
            project_id(&id)?;
            let mut paths = self.material_paths(&id)?;
            for plan in self.file_deletion_plans(&id)? {
                paths.extend(crate::web_file_deletion::owned_scopes(&plan)?);
            }
            for (key, target) in self.read(&id)?.exports {
                let claim = crate::web_export_claims::directory(&self.data, &key)?;
                if let Some(identity) = target.claim_identity {
                    if claim.exists() {
                        if identity.matches_path(&claim).unwrap_or(false) {
                            paths.push(claim);
                        } else if !(target.removed && target.release_intent) {
                            return Err(invalid("导出占有记录身份变化。"));
                        }
                    } else {
                        identity
                            .absent_on_expected_volume(&claim)
                            .map_err(|error| invalid(error.to_string()))?;
                    }
                } else if claim.exists() {
                    return Err(invalid("导出占有目录尚未确认归属。"));
                }
            }
            if let Some(root) = self.read(&id)?.project_root {
                root.verify()?;
                paths.push(root.path);
            }
            paths.push(self.directory(&id)?);
            deduplicate(&mut paths);
            scopes.push((self.path(&id)?, paths));
        }
        scopes.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(scopes)
    }
    /// Admission reads exact ownership metadata and checks only the candidate's
    /// ancestors. A damaged unrelated material does not need to be opened.
    pub(crate) fn require_global_tool_unowned(&self, candidate: &Path) -> Result<()> {
        check_no_symlinks(&self.root(), true)?;
        if !self.root().try_exists()? {
            return Ok(());
        }
        for entry in fs::read_dir(self.root())? {
            let id = entry?.file_name().to_string_lossy().into_owned();
            let record = self.read(&id)?;
            let bound = record
                .project_root
                .iter()
                .chain(record.retained.iter())
                .any(|bound| {
                    candidate == bound.path
                        || (bound.directory && candidate.starts_with(&bound.path))
                        || (if bound.directory {
                            candidate.ancestors().any(|ancestor| {
                                bound.identity.matches_path(ancestor).unwrap_or(false)
                            })
                        } else {
                            bound.identity.matches_path(candidate).unwrap_or(false)
                        })
                });
            let fixed = fixed_paths(&record.workspace_root, &id)
                .iter()
                .any(|path| candidate.starts_with(path))
                || record.tasks.keys().any(|task| {
                    [".platform/tasks", ".platform/operations"]
                        .iter()
                        .any(|scope| {
                            candidate.starts_with(record.workspace_root.join(scope).join(task))
                        })
                });
            let stage = record.export_stages.values().any(|stage| {
                candidate == stage.path
                    || stage
                        .identity
                        .as_ref()
                        .is_some_and(|identity| identity.matches_path(candidate).unwrap_or(false))
            });
            let export = record
                .exports
                .values()
                .filter(|export| !export.removed)
                .any(|export| {
                    candidate == export.path
                        || export.identity.as_ref().is_some_and(|identity| {
                            identity.matches_path(candidate).unwrap_or(false)
                        })
                });
            if bound || fixed || stage || export {
                return Err(crate::project_tool_boundary::project_owned());
            }
        }
        Ok(())
    }
    pub(crate) fn rebind_after_migration(data: &Path, old: &Path, new: &Path) -> Result<()> {
        let old_owner = Self::new(data.into(), old.into());
        if !old_owner.root().exists() {
            return Ok(());
        }
        for entry in fs::read_dir(old_owner.root())? {
            let id = entry?.file_name().to_string_lossy().into_owned();
            project_id(&id)?;
            let record = old_owner.read(&id)?;
            if record.workspace_root == new {
                continue;
            }
            if record.workspace_root != old {
                return Err(invalid("网页项目迁移源工作区归属不符。"));
            }
            let mut paths = old_owner.material_paths(&id)?;
            if let Some(root) = &record.project_root {
                root.verify()?;
                paths.push(root.path.clone());
            }
            let state = old.join(".platform/project-state");
            paths.push(state.join(format!("{id}.json")));
            paths.push(state.join("reservations").join(format!("{id}.json")));
            paths.push(state.join("deleting").join(format!("{id}.json")));
            let mut retained = vec![];
            for path in paths {
                if !path.starts_with(old) {
                    continue;
                }
                if let Some(bound) = capture(&path)? {
                    retained.push(bound);
                }
            }
            for stage in record.export_stages.values() {
                if let Ok(relative) = stage.path.strip_prefix(old) {
                    if let Some(bound) = capture(&new.join(relative))? {
                        retained.push(bound);
                    }
                }
            }
            for export in record.exports.values().filter(|export| !export.removed) {
                if let Ok(relative) = export.path.strip_prefix(old) {
                    let target = new.join(relative);
                    if let Some(bound) = capture(&target)? {
                        if bound.directory {
                            return Err(invalid("迁移的导出副本不是文件。"));
                        }
                        old_owner.reserve_export_destination_inner(
                            &id,
                            &export.task_id,
                            &target,
                            Some(&bound.identity),
                        )?;
                    }
                }
            }
            old_owner.update(&id, false, |record| {
                for retained in retained {
                    if !record
                        .retained
                        .iter()
                        .any(|existing| existing.path == retained.path)
                    {
                        record.retained.push(retained);
                    }
                }
                if let Some(root) = &record.project_root {
                    if let Ok(relative) = root.path.strip_prefix(old) {
                        let target = new.join(relative);
                        let current = capture(&target)?
                            .ok_or_else(|| invalid("网页项目未包含在已核验迁移副本中。"))?;
                        record.project_root = Some(current);
                    }
                }
                for (task, identity) in &mut record.tasks {
                    let path = new.join(".platform/tasks").join(task);
                    if let Some(current) = capture(&path)? {
                        if !current.directory {
                            return Err(invalid("复制的任务根不是目录。"));
                        }
                        *identity = Some(current.identity);
                    }
                }
                record.workspace_root = new.into();
                Ok(())
            })?;
        }
        Ok(())
    }
    pub(crate) fn file_deletion_plans(
        &self,
        id: &str,
    ) -> Result<Vec<crate::web_file_deletion::FileDeletionPlan>> {
        Ok(self.read(id)?.file_deletions.into_values().collect())
    }
    pub(crate) fn reserve_export_destination(
        &self,
        id: &str,
        task: &str,
        path: &Path,
    ) -> Result<()> {
        self.reserve_export_destination_inner(id, task, path, None)
    }
    fn reserve_export_destination_inner(
        &self,
        id: &str,
        task: &str,
        path: &Path,
        verified_copy: Option<&PersistedIdentity>,
    ) -> Result<()> {
        use std::{
            io::Write,
            os::unix::fs::{DirBuilderExt, OpenOptionsExt},
        };
        absolute(path)?;
        task_id(task)?;
        crate::node_apps::development::with_source_ownership_admission(|| {
            check_no_symlinks(path, true)?;
            if path.exists()
                && !verified_copy
                    .is_some_and(|identity| identity.matches_path(path).unwrap_or(false))
            {
                return Err(ApiError::new(
                    "EXPORT_DESTINATION_EXISTS",
                    "请选择尚不存在的新导出文件名。",
                ));
            }
            let (key, parent_identity, ancestors) = crate::web_export_claims::key(path)?;
            if let Some(expected) = verified_copy {
                if let Some(previous) = self.read(id)?.exports.get(&key) {
                    if previous.path != path
                        || !previous
                            .identity
                            .as_ref()
                            .is_some_and(|identity| identity.matches_captured(expected))
                    {
                        return Err(invalid("迁移后的导出副本身份变化。"));
                    }
                    let claim = crate::web_export_claims::directory(&self.data, &key)?;
                    if previous
                        .claim_identity
                        .as_ref()
                        .is_some_and(|identity| identity.matches_path(&claim).unwrap_or(false))
                    {
                        return Ok(());
                    }
                }
            }
            if verified_copy.is_none() {
                for protected in [
                    self.data.clone(),
                    self.workspace.join("projects"),
                    self.workspace.join("backups"),
                    self.workspace.join(".platform"),
                    self.workspace.join("project-sources"),
                ] {
                    if let Some(bound) = capture(&protected)? {
                        if ancestors
                            .iter()
                            .any(|ancestor| ancestor.matches_captured(&bound.identity))
                        {
                            return Err(invalid("导出目标不能位于平台受管目录。"));
                        }
                    }
                }
            }
            crate::node_apps::development::require_export_location_unclaimed(
                &self.data,
                path.parent().unwrap(),
            )?;
            for entry in fs::read_dir(self.root())? {
                let other = entry?.file_name().to_string_lossy().into_owned();
                if other == id {
                    continue;
                }
                let record = self.read(&other)?;
                if record
                    .project_root
                    .iter()
                    .chain(record.retained.iter())
                    .any(|bound| {
                        bound.directory
                            && ancestors
                                .iter()
                                .any(|ancestor| ancestor.matches_captured(&bound.identity))
                    })
                {
                    return Err(invalid("导出目标位于另一个项目独占的目录。"));
                }
            }
            let claim_path = crate::web_export_claims::directory(&self.data, &key)?;
            check_no_symlinks(&claim_path, true)?;
            if claim_path.exists() {
                return Err(ApiError::new(
                    "EXPORT_PATH_OWNED",
                    "此导出位置已被项目占有，请选择新文件名。",
                ));
            }
            self.update(id, false, |record| {
                if !record.tasks.contains_key(task) {
                    return Err(invalid("导出任务尚未登记。"));
                }
                record.exports.insert(
                    key.clone(),
                    ExportTarget {
                        path: path.into(),
                        task_id: task.into(),
                        parent_identity: parent_identity.clone(),
                        claim_identity: None,
                        identity: verified_copy.cloned(),
                        stage_path: None,
                        published: verified_copy.is_some(),
                        removed: false,
                        release_intent: false,
                    },
                );
                Ok(())
            })?;
            let root = claim_path.parent().unwrap();
            check_no_symlinks(root, true)?;
            if !root.exists() {
                fs::DirBuilder::new().mode(0o700).create(root)?;
            }
            if let Err(error) = fs::DirBuilder::new().mode(0o700).create(&claim_path) {
                self.update(id, false, |record| {
                    record.exports.remove(&key);
                    Ok(())
                })?;
                return Err(error.into());
            }
            let identity = PersistedIdentity::capture(&claim_path)
                .map_err(|error| invalid(error.to_string()))?;
            self.update(id, false, |record| {
                record
                    .exports
                    .get_mut(&key)
                    .ok_or_else(|| invalid("导出归属丢失。"))?
                    .claim_identity = Some(identity);
                Ok(())
            })?;
            let claim = crate::web_export_claims::ExportClaim {
                version: 1,
                project_id: id.into(),
                task_id: task.into(),
                path: path.into(),
                parent_identity,
                ancestors,
            };
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(claim_path.join("owner.json"))?;
            file.write_all(&serde_json::to_vec(&claim)?)?;
            file.sync_all()?;
            fs::File::open(&claim_path)?.sync_all()?;
            fs::File::open(root)?.sync_all()?;
            Ok(())
        })
    }
    pub(crate) fn bind_export_destination(
        &self,
        id: &str,
        task: &str,
        path: &Path,
        stage: &Path,
    ) -> Result<()> {
        let identity =
            PersistedIdentity::capture(stage).map_err(|error| invalid(error.to_string()))?;
        self.update(id, false, |record| {
            let source = record
                .export_stages
                .get(task)
                .filter(|source| {
                    source.path == stage
                        && source
                            .identity
                            .as_ref()
                            .is_some_and(|expected| expected.matches_captured(&identity))
                })
                .ok_or_else(|| invalid("导出临时文件归属不符。"))?;
            let target = record
                .exports
                .values_mut()
                .find(|target| target.task_id == task && target.path == path)
                .ok_or_else(|| invalid("导出最终目标未预先独占登记。"))?;
            if !target
                .parent_identity
                .matches_captured(&source.parent_identity)
                || path.exists()
            {
                return Err(invalid("导出目标已被其他文件占用。"));
            }
            target.identity = Some(identity);
            target.stage_path = Some(stage.into());
            Ok(())
        })
    }
    pub(crate) fn commit_export_destination(&self, id: &str, task: &str) -> Result<()> {
        self.update(id, false, |record| {
            let target = record
                .exports
                .values_mut()
                .find(|target| target.task_id == task)
                .ok_or_else(|| invalid("导出目标归属缺失。"))?;
            if !target
                .identity
                .as_ref()
                .is_some_and(|identity| identity.matches_path(&target.path).unwrap_or(false))
            {
                return Err(invalid("已发布导出文件身份变化。"));
            }
            target.published = true;
            Ok(())
        })
    }
    pub(crate) fn cancel_unpublished_export(&self, id: &str, task: &str) -> Result<()> {
        let record = self.read(id)?;
        let Some((key, target)) = record
            .exports
            .iter()
            .find(|(_, target)| target.task_id == task)
        else {
            return Ok(());
        };
        if target.published {
            return Ok(());
        }
        if let Some(identity) = &target.identity {
            if identity.matches_path(&target.path).unwrap_or(false) {
                return Ok(());
            }
            if !target
                .stage_path
                .as_ref()
                .is_some_and(|stage| identity.matches_path(stage).unwrap_or(false))
            {
                return Err(invalid("无法确认导出未发布，保留当前归属。"));
            }
        }
        self.update(id, false, |record| {
            record
                .exports
                .get_mut(key)
                .ok_or_else(|| invalid("导出占有记录缺失。"))?
                .removed = true;
            Ok(())
        })
    }
    fn authorizes_file(
        &self,
        record: &Ownership,
        path: &Path,
        expected: &PersistedIdentity,
    ) -> bool {
        fixed_paths(&record.workspace_root, &record.project_id)
            .iter()
            .any(|fixed| fixed == path)
            || record.retained.iter().any(|bound| {
                !bound.directory && bound.path == path && bound.identity.matches_captured(expected)
            })
            || record.export_stages.values().any(|stage| {
                stage.path == path
                    && stage
                        .identity
                        .as_ref()
                        .is_some_and(|identity| identity.matches_captured(expected))
            })
            || record.exports.values().any(|export| {
                export.path == path
                    && export
                        .identity
                        .as_ref()
                        .is_some_and(|identity| identity.matches_captured(expected))
            })
            || record
                .removed_materials
                .get(path)
                .is_some_and(|identity| identity.matches_captured(expected))
    }
    pub(crate) fn save_file_deletion_plan(
        &self,
        id: &str,
        plan: &crate::web_file_deletion::FileDeletionPlan,
    ) -> Result<()> {
        plan.validate()?;
        let existing = self.read(id)?.file_deletions.get(&plan.original).cloned();
        if let Some(previous) = &existing {
            if !plan.same_authorization(previous) {
                return Err(invalid("文件删除计划不得改变归属。"));
            }
        } else {
            let expected = self
                .material_identity(id, &plan.original)?
                .ok_or_else(|| invalid("文件删除目标缺少项目归属。"))?;
            if !expected.matches_captured(&plan.expected)
                || !plan
                    .parent_identity
                    .matches_path(plan.original.parent().unwrap())
                    .unwrap_or(false)
            {
                return Err(invalid("文件删除计划与项目材料不符。"));
            }
        }
        self.update(id, false, |record| {
            if let Some(previous) = record.file_deletions.get(&plan.original) {
                if !plan.same_authorization(previous) {
                    return Err(invalid("文件删除计划归属已变化。"));
                }
            }
            record
                .file_deletions
                .insert(plan.original.clone(), plan.clone());
            Ok(())
        })
    }
    pub(crate) fn forget_file_deletion_plan(
        &self,
        id: &str,
        path: &Path,
        expected: &PersistedIdentity,
    ) -> Result<()> {
        self.update(id, false, |record| {
            if let Some(plan) = record.file_deletions.get(path) {
                if !plan.expected.matches_captured(expected) {
                    return Err(invalid("文件删除计划归属不符。"));
                }
                record.file_deletions.remove(path);
            }
            Ok(())
        })
    }
    pub(crate) fn mark_material_removed(
        &self,
        id: &str,
        path: &Path,
        expected: &PersistedIdentity,
    ) -> Result<()> {
        self.update(id, false, |record| {
            if !self.authorizes_file(record, path, expected) {
                return Err(invalid("已清理文件缺少项目归属。"));
            }
            record
                .removed_materials
                .insert(path.into(), expected.clone());
            for export in record
                .exports
                .values_mut()
                .filter(|export| export.path == path)
            {
                export.removed = true;
            }
            Ok(())
        })
    }
    pub(crate) fn finalize(&self, id: &str) -> Result<()> {
        let directory = self.directory(id)?;
        if !directory.exists() {
            return Ok(());
        }
        if self.path(id)?.exists() {
            let record = self.read(id)?;
            if !record.file_deletions.is_empty() {
                return Err(invalid("仍有未完成的文件删除计划。"));
            }
            for target in record.exports.values().filter(|target| !target.removed) {
                if let Some(identity) = &target.identity {
                    if !identity
                        .absent_on_expected_volume(&target.path)
                        .map_err(|error| invalid(error.to_string()))?
                    {
                        return Err(invalid("仍有未清理的项目导出文件。"));
                    }
                }
            }
            self.update(id, false, |record| {
                for target in record.exports.values_mut() {
                    target.removed = true;
                    target.release_intent = true;
                }
                Ok(())
            })?;
            for (key, target) in self.read(id)?.exports {
                let claim = crate::web_export_claims::directory(&self.data, &key)?;
                if let Some(identity) = target.claim_identity {
                    if claim.exists() {
                        if identity.matches_path(&claim).unwrap_or(false) {
                            crate::history_cleanup::remove_owned_directory(&claim, &identity)?;
                        }
                    } else {
                        identity
                            .absent_on_expected_volume(&claim)
                            .map_err(|error| invalid(error.to_string()))?;
                    }
                } else if claim.exists() {
                    return Err(invalid("未确认身份的导出占有目录仍存在。"));
                }
            }
        }
        let identity =
            PersistedIdentity::capture(&directory).map_err(|error| invalid(error.to_string()))?;
        crate::history_cleanup::remove_owned_directory(&directory, &identity).map(|_| ())
    }
}
pub(crate) fn fixed_paths(workspace: &Path, id: &str) -> Vec<PathBuf> {
    let state = workspace.join(".platform/project-state");
    let mut paths = vec![
        state.join("maintenance").join(format!("{id}.json")),
        state.join("builds").join(format!("{id}.json")),
        state.join("builds").join(format!("{id}.publication.json")),
        state.join("latest-build").join(format!("{id}.json")),
        workspace.join("backups").join(id),
        workspace.join(".platform/cache/npm").join(id),
        workspace.join("projects").join(format!(".creating-{id}")),
    ];
    for scope in [
        "git",
        "source-restore",
        "source-restore-staging",
        "data-restore",
    ] {
        paths.push(state.join(scope).join(id));
    }
    paths
}
fn deduplicate(paths: &mut Vec<PathBuf>) {
    let all = std::mem::take(paths);
    for path in &all {
        if !paths.contains(path)
            && !all
                .iter()
                .any(|ancestor| ancestor != path && path.starts_with(ancestor))
        {
            paths.push(path.clone());
        }
    }
}
fn validate_export_stage(task: &str, path: &Path) -> Result<()> {
    task_id(task)?;
    absolute(path)?;
    if path.file_name().and_then(|name| name.to_str())
        != Some(format!(".export-{task}.partial").as_str())
    {
        return Err(invalid("导出临时文件名与任务不符。"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, WebProjectOwnership, String) {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let data = base.join("data");
        let workspace = base.join("workspace");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&workspace).unwrap();
        let owner = WebProjectOwnership::new(data, workspace);
        let id = format!("prj_{}", uuid::Uuid::new_v4().simple());
        owner.initialize(&id).unwrap();
        (temporary, owner, id)
    }
    #[test]
    fn task_directory_identity_survives_partial_task_metadata_and_rejects_replacement() {
        let (_temporary, owner, id) = fixture();
        let task = format!("task_{}", uuid::Uuid::new_v4().simple());
        owner.record_task(&id, &task).unwrap();
        let path = owner.workspace.join(".platform/tasks").join(&task);
        fs::create_dir_all(&path).unwrap();
        owner
            .capture_task_directory(&id, &task, &PersistedIdentity::capture(&path).unwrap())
            .unwrap();
        fs::write(path.join("task.json"), "partial or missing task metadata").unwrap();
        fs::remove_file(path.join("task.json")).unwrap();
        assert!(owner.material_paths(&id).unwrap().contains(&path));
        fs::rename(&path, path.with_extension("retained-original")).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(owner.material_paths(&id).is_err());
        assert!(owner
            .capture_task_directory(&id, &task, &PersistedIdentity::capture(&path).unwrap())
            .is_err());
    }
    #[test]
    fn releasing_a_stage_does_not_adopt_an_unregistered_external_filename() {
        let (temporary, owner, id) = fixture();
        let task = format!("task_{}", uuid::Uuid::new_v4().simple());
        owner.record_task(&id, &task).unwrap();
        let external = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("external-delivery");
        fs::create_dir(&external).unwrap();
        let stage = external.join(format!(".export-{task}.partial"));
        owner.reserve_export_stage(&id, &task, &stage).unwrap();
        fs::write(&stage, []).unwrap();
        owner
            .capture_export_stage(
                &id,
                &task,
                &stage,
                &PersistedIdentity::capture(&stage).unwrap(),
            )
            .unwrap();
        fs::write(&stage, "project archive content").unwrap();
        assert!(owner.material_paths(&id).unwrap().contains(&stage));
        let delivered = external.join("user-delivered.zip");
        fs::rename(&stage, &delivered).unwrap();
        owner.forget_export_stage(&id, &task).unwrap();
        assert!(!owner.material_paths(&id).unwrap().contains(&delivered));
        assert!(delivered.exists());
    }
    #[test]
    fn migration_retains_only_this_projects_old_scope_and_keeps_new_identity_separate() {
        let (_temporary, owner, id) = fixture();
        let root = owner.workspace.join("projects/disposable-web-root");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("source"), "project owned source").unwrap();
        owner.record_project_root(&id, &root).unwrap();
        let state = owner.workspace.join(".platform/project-state");
        fs::create_dir_all(state.join("maintenance")).unwrap();
        fs::write(state.join(format!("{id}.json")), "old project identity").unwrap();
        fs::write(
            state.join("maintenance").join(format!("{id}.json")),
            "maintenance state",
        )
        .unwrap();
        let other = state.join("maintenance/prj_unrelated.json");
        fs::write(&other, "another project").unwrap();
        let task = format!("task_{}", uuid::Uuid::new_v4().simple());
        owner.record_task(&id, &task).unwrap();
        let old_task = owner.workspace.join(".platform/tasks").join(&task);
        fs::create_dir_all(&old_task).unwrap();
        owner
            .capture_task_directory(&id, &task, &PersistedIdentity::capture(&old_task).unwrap())
            .unwrap();
        fs::write(old_task.join("task.json"), "retained task metadata").unwrap();
        let next = owner.workspace.with_extension("new");
        fs::create_dir_all(next.join("projects/disposable-web-root")).unwrap();
        fs::write(
            next.join("projects/disposable-web-root/source"),
            "project owned source",
        )
        .unwrap();
        let new_task = next.join(".platform/tasks").join(&task);
        fs::create_dir_all(&new_task).unwrap();
        fs::write(new_task.join("task.json"), "retained task metadata").unwrap();
        WebProjectOwnership::rebind_after_migration(&owner.data, &owner.workspace, &next).unwrap();
        WebProjectOwnership::rebind_after_migration(&owner.data, &owner.workspace, &next).unwrap();
        let migrated = WebProjectOwnership::new(owner.data.clone(), next.clone());
        let paths = migrated.material_paths(&id).unwrap();
        assert!(paths.contains(&root));
        assert!(paths.contains(&old_task));
        assert!(paths.contains(&new_task));
        assert!(owner
            .record_task(&id, &format!("task_{}", uuid::Uuid::new_v4().simple()))
            .is_err());
        assert!(paths.contains(&state.join(format!("{id}.json"))));
        assert!(!paths.contains(&other));
        assert!(!paths.contains(
            &next
                .join(".platform/project-state")
                .join(format!("{id}.json"))
        ));
        assert_eq!(
            migrated.read(&id).unwrap().project_root.unwrap().path,
            next.join("projects/disposable-web-root")
        );
    }
    #[test]
    fn late_task_callbacks_cannot_recreate_a_removed_owner() {
        let (_temporary, owner, id) = fixture();
        owner.discard_unpublished(&id).unwrap();
        assert!(owner
            .record_task(&id, &format!("task_{}", uuid::Uuid::new_v4().simple()))
            .is_err());
        assert!(!owner.directory(&id).unwrap().exists());
    }
    fn publish_export(
        owner: &WebProjectOwnership,
        id: &str,
        parent: &Path,
        name: &str,
        commit: bool,
    ) -> (String, PathBuf, PersistedIdentity) {
        fs::create_dir_all(parent).unwrap();
        let task = format!("task_{}", uuid::Uuid::new_v4().simple());
        owner.record_task(id, &task).unwrap();
        let path = parent.join(name);
        owner.reserve_export_destination(id, &task, &path).unwrap();
        let stage = parent.join(format!(".export-{task}.partial"));
        owner.reserve_export_stage(id, &task, &stage).unwrap();
        fs::write(&stage, []).unwrap();
        let identity = PersistedIdentity::capture(&stage).unwrap();
        owner
            .capture_export_stage(id, &task, &stage, &identity)
            .unwrap();
        fs::write(&stage, b"disposable ZIP content").unwrap();
        owner
            .bind_export_destination(id, &task, &path, &stage)
            .unwrap();
        fs::rename(&stage, &path).unwrap();
        if commit {
            owner.commit_export_destination(id, &task).unwrap();
        }
        owner.forget_export_stage(id, &task).unwrap();
        (task, path, identity)
    }
    fn clear_export(
        owner: &WebProjectOwnership,
        id: &str,
        path: &Path,
        expected: &PersistedIdentity,
    ) {
        crate::web_file_deletion::remove_owned_file(owner, id, path, expected).unwrap();
        owner.mark_material_removed(id, path, expected).unwrap();
    }
    #[test]
    fn final_export_is_owned_before_rename_receipt_and_removed_with_claim() {
        let (temporary, owner, id) = fixture();
        let parent = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-exports");
        let (_, path, expected) = publish_export(&owner, &id, &parent, "snapshot.zip", false);
        assert!(owner.material_paths(&id).unwrap().contains(&path));
        clear_export(&owner, &id, &path, &expected);
        owner.finalize(&id).unwrap();
        assert!(!path.exists());
        assert!(!owner.directory(&id).unwrap().exists());
        assert!(fs::read_dir(owner.data.join("web-export-owners"))
            .unwrap()
            .next()
            .is_none());
    }
    #[test]
    fn replaced_final_export_is_never_adopted_or_deleted() {
        let (temporary, owner, id) = fixture();
        let parent = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-exports");
        let (_, path, _) = publish_export(&owner, &id, &parent, "snapshot.zip", true);
        fs::rename(&path, parent.join("kept-original.zip")).unwrap();
        fs::write(&path, b"foreign replacement").unwrap();
        assert!(owner.material_paths(&id).is_err());
        assert!(owner.finalize(&id).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"foreign replacement");
    }
    #[test]
    fn export_claim_survives_missing_file_and_rejects_case_and_unicode_aliases() {
        let (temporary, owner, id) = fixture();
        let parent = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-exports");
        let (_, path, expected) = publish_export(&owner, &id, &parent, "Caf\u{e9}.zip", true);
        fs::remove_file(&path).unwrap();
        let other = format!("prj_{}", uuid::Uuid::new_v4().simple());
        owner.initialize(&other).unwrap();
        let task = format!("task_{}", uuid::Uuid::new_v4().simple());
        owner.record_task(&other, &task).unwrap();
        let alias = parent.join("CAFE\u{301}.ZIP");
        assert_eq!(
            owner
                .reserve_export_destination(&other, &task, &alias)
                .unwrap_err()
                .code,
            "EXPORT_PATH_OWNED"
        );
        owner.mark_material_removed(&id, &path, &expected).unwrap();
        owner.finalize(&id).unwrap();
        owner
            .reserve_export_destination(&other, &task, &alias)
            .unwrap();
    }
    #[test]
    fn removed_progress_allows_a_retry_without_touching_a_new_owners_same_path() {
        let (temporary, owner, id) = fixture();
        let parent = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-exports");
        let (_, path, expected) = publish_export(&owner, &id, &parent, "snapshot.zip", true);
        clear_export(&owner, &id, &path, &expected);
        owner
            .update(&id, false, |record| {
                for target in record.exports.values_mut() {
                    target.release_intent = true;
                }
                Ok(())
            })
            .unwrap();
        let (key, _, _) = crate::web_export_claims::key(&path).unwrap();
        let claim = crate::web_export_claims::directory(&owner.data, &key).unwrap();
        let claim_identity = PersistedIdentity::capture(&claim).unwrap();
        crate::history_cleanup::remove_owned_directory(&claim, &claim_identity).unwrap();
        let other = format!("prj_{}", uuid::Uuid::new_v4().simple());
        owner.initialize(&other).unwrap();
        let (_, new_path, _) = publish_export(&owner, &other, &parent, "snapshot.zip", true);
        assert!(!owner.material_paths(&id).unwrap().contains(&new_path));
        owner.finalize(&id).unwrap();
        assert_eq!(fs::read(&new_path).unwrap(), b"disposable ZIP content");
        assert!(owner.material_paths(&other).unwrap().contains(&new_path));
    }
    #[test]
    fn export_and_node_source_admissions_are_exclusive_in_both_directions() {
        let (temporary, owner, id) = fixture();
        let parent = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-exports");
        let (_, _, _) = publish_export(&owner, &id, &parent, "snapshot.zip", true);
        let node = uuid::Uuid::new_v4().to_string();
        crate::node_project_ownership::NodeProjectOwnership::new(
            owner.data.clone(),
            owner.workspace.clone(),
        )
        .initialize(&node)
        .unwrap();
        assert!(
            crate::node_apps::development::claim_source_after_verified_migration(
                &owner.data,
                &owner.workspace,
                &node,
                &parent
            )
            .is_err()
        );
        let source = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-node-source");
        fs::create_dir(&source).unwrap();
        crate::node_apps::development::claim_source_after_verified_migration(
            &owner.data,
            &owner.workspace,
            &node,
            &source,
        )
        .unwrap();
        let task = format!("task_{}", uuid::Uuid::new_v4().simple());
        owner.record_task(&id, &task).unwrap();
        assert!(owner
            .reserve_export_destination(&id, &task, &source.join("forbidden.zip"))
            .is_err());
        assert!(crate::web_export_claims::require_source_unoccupied(
            &owner.data,
            &owner.workspace.join("projects/future-uncreated-root")
        )
        .is_ok());
    }
    #[test]
    fn missing_final_export_on_an_unavailable_expected_volume_blocks_scope_cleanup() {
        let (temporary, owner, id) = fixture();
        let parent = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-exports");
        let (_, path, _) = publish_export(&owner, &id, &parent, "snapshot.zip", true);
        fs::remove_file(&path).unwrap();
        owner.material_paths(&id).unwrap();
        owner
            .update(&id, false, |record| {
                let identity = record
                    .exports
                    .values_mut()
                    .next()
                    .unwrap()
                    .identity
                    .as_mut()
                    .unwrap();
                if identity.volume_uuid.is_some() {
                    identity.volume_uuid = Some(uuid::Uuid::new_v4().to_string());
                } else {
                    identity.device ^= 1;
                }
                Ok(())
            })
            .unwrap();
        assert!(owner.material_paths(&id).is_err());
        assert!(owner.finalize(&id).is_err());
    }
    #[test]
    fn migration_keeps_export_original_and_copied_file_exclusively_owned() {
        let (_temporary, owner, id) = fixture();
        let parent = owner.workspace.join("custom-exports");
        let (_, old_path, _) = publish_export(&owner, &id, &parent, "snapshot.zip", true);
        let next = owner.workspace.with_extension("new");
        fs::create_dir_all(next.join("custom-exports")).unwrap();
        let new_path = next.join("custom-exports/snapshot.zip");
        fs::copy(&old_path, &new_path).unwrap();
        WebProjectOwnership::rebind_after_migration(&owner.data, &owner.workspace, &next).unwrap();
        WebProjectOwnership::rebind_after_migration(&owner.data, &owner.workspace, &next).unwrap();
        let migrated = WebProjectOwnership::new(owner.data.clone(), next);
        let paths = migrated.material_paths(&id).unwrap();
        assert!(paths.contains(&old_path));
        assert!(paths.contains(&new_path));
        for path in [old_path, new_path] {
            let expected = migrated.material_identity(&id, &path).unwrap().unwrap();
            clear_export(&migrated, &id, &path, &expected);
        }
        migrated.finalize(&id).unwrap();
    }
}
