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
#[cfg(windows)]
#[path = "web_project_ownership/windows_delivery.rs"]
pub(crate) mod windows_delivery;
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
    user_root: Option<OwnedPath>,
    instance_identity: Option<PersistedIdentity>,
    import_controls: BTreeMap<String, OwnedPath>,
    prior_project_roots: Vec<OwnedPath>,
    tasks: BTreeMap<String, Option<PersistedIdentity>>,
    retained: Vec<OwnedPath>,
    export_stages: BTreeMap<String, ExportStage>,
    file_deletions: BTreeMap<PathBuf, crate::web_file_deletion::FileDeletionPlan>,
    removed_materials: BTreeMap<PathBuf, PersistedIdentity>,
    #[cfg(windows)]
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    native_deliveries: BTreeMap<String, windows_delivery::Binding>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExportStage {
    path: PathBuf,
    parent_identity: PersistedIdentity,
    ancestor_identities: Vec<PersistedIdentity>,
    identity: Option<PersistedIdentity>,
    overwrite_identity: Option<PersistedIdentity>,
    overwrite_verified: bool,
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
fn import_control_path(data: &Path, id: &str, import: &str) -> Result<PathBuf> {
    let uuid = uuid::Uuid::parse_str(import).map_err(|_| invalid("导入控制标识无效。"))?;
    if uuid.to_string() != import || id != format!("prj_{}", uuid.simple()) {
        return Err(invalid("导入控制材料与网页实例不符。"));
    }
    Ok(data.join("node-project-ownership").join(import))
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
        if record.version != 2 || record.project_id != id {
            return Err(invalid("网页归属索引与项目不符。"));
        }
        for (import, bound) in &record.import_controls {
            if !bound.directory || bound.path != import_control_path(&self.data, id, import)? {
                return Err(invalid("导入控制目录归属无效。"));
            }
        }
        for task in record.tasks.keys() {
            task_id(task)?;
        }
        for path in record
            .project_root
            .iter()
            .chain(record.retained.iter())
            .chain(record.prior_project_roots.iter())
            .chain(record.user_root.iter())
        {
            absolute(&path.path)?;
        }
        for (task, stage) in &record.export_stages {
            validate_export_stage(task, &stage.path)?;
            if stage.overwrite_identity.is_some() && stage.identity.is_none() {
                return Err(invalid("覆盖临时文件缺少原始复制身份。"));
            }
            if stage.overwrite_verified && stage.overwrite_identity.is_none() {
                return Err(invalid("覆盖临时文件缺少已核验的旧目标身份。"));
            }
            if !stage
                .ancestor_identities
                .first()
                .is_some_and(|identity| identity.matches_captured(&stage.parent_identity))
            {
                return Err(invalid("临时文件的父目录归属链不完整。"));
            }
        }
        for (path, plan) in &record.file_deletions {
            plan.validate()?;
            if path != &plan.original || !self.authorizes_file(&record, path, &plan.expected) {
                return Err(invalid("文件删除计划不属于项目精确材料。"));
            }
        }
        #[cfg(windows)]
        for (task, binding) in &record.native_deliveries {
            windows_delivery::validate_binding(task, binding)?;
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
                    version: 2,
                    project_id: id.into(),
                    workspace_root: self.workspace.clone(),
                    project_root: None,
                    user_root: None,
                    instance_identity: None,
                    import_controls: BTreeMap::new(),
                    prior_project_roots: vec![],
                    tasks: BTreeMap::new(),
                    retained: vec![],
                    export_stages: BTreeMap::new(),
                    file_deletions: BTreeMap::new(),
                    removed_materials: BTreeMap::new(),
                    #[cfg(windows)]
                    native_deliveries: BTreeMap::new(),
                }
            }
            Err(error) => return Err(error),
        };
        if record.workspace_root != self.workspace {
            return Err(invalid("网页项目工作区归属已变化。"));
        }
        change(&mut record)?;
        for directory in [self.root(), self.directory(id)?] {
            crate::web_project_ownership_io::prepare_directory(&directory)?;
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
            || !record.import_controls.is_empty()
            || fixed_paths(&self.data, &self.workspace, id)
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
    pub(crate) fn source_roots(&self, id: &str) -> Result<Vec<PathBuf>> {
        let record = self.read(id)?;
        let mut roots = vec![];
        for root in record
            .project_root
            .iter()
            .chain(record.prior_project_roots.iter())
        {
            root.verify()?;
            if !roots.contains(&root.path) {
                roots.push(root.path.clone());
            }
        }
        // This is an exclusion/coordination path, never deletion authority.
        if let Some(root) = record.user_root {
            if !roots.contains(&root.path) {
                roots.push(root.path);
            }
        }
        Ok(roots)
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
    fn verify_task_path(
        &self,
        _id: &str,
        task: &str,
        identity: Option<&PersistedIdentity>,
    ) -> Result<()> {
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
        self.verify_task_path(id, task, identity.as_ref())
    }
    pub(crate) fn reserve_export_stage(&self, id: &str, task: &str, path: &Path) -> Result<()> {
        validate_export_stage(task, path)?;
        check_no_symlinks(path, true)?;
        if path.exists() {
            return Err(invalid("导出临时目标已存在，未接管。"));
        }
        let parent_identity = PersistedIdentity::capture(path.parent().unwrap())
            .map_err(|error| invalid(error.to_string()))?;
        let ancestor_identities = path
            .parent()
            .unwrap()
            .ancestors()
            .map(|path| {
                PersistedIdentity::capture(path).map_err(|error| invalid(error.to_string()))
            })
            .collect::<Result<Vec<_>>>()?;
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
                    ancestor_identities,
                    identity: None,
                    overwrite_identity: None,
                    overwrite_verified: false,
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
    pub(crate) fn prepare_export_stage_overwrite(
        &self,
        id: &str,
        task: &str,
        path: &Path,
        payload: &PersistedIdentity,
        destination: &Path,
        selected: &PersistedIdentity,
    ) -> Result<()> {
        validate_export_stage(task, path)?;
        check_no_symlinks(destination, false)?;
        if path.parent() != destination.parent()
            || !fs::symlink_metadata(destination)?.is_file()
            || !payload.matches_path(path).unwrap_or(false)
            || !selected.matches_path(destination).unwrap_or(false)
        {
            return Err(invalid("复制临时文件或用户选择的覆盖目标已变化。"));
        }
        self.update(id, false, |record| {
            let stage = record
                .export_stages
                .get_mut(task)
                .filter(|stage| stage.path == path)
                .ok_or_else(|| invalid("临时复制尚未登记。"))?;
            if !stage
                .identity
                .as_ref()
                .is_some_and(|identity| identity.matches_captured(payload))
                || !stage
                    .parent_identity
                    .matches_path(path.parent().unwrap())
                    .unwrap_or(false)
                || stage
                    .overwrite_identity
                    .as_ref()
                    .is_some_and(|identity| !identity.matches_captured(selected))
            {
                return Err(invalid("复制覆盖授权身份已变化。"));
            }
            stage.overwrite_identity = Some(selected.clone());
            stage.overwrite_verified = false;
            Ok(())
        })
    }
    pub(crate) fn verify_export_stage_overwrite(
        &self,
        id: &str,
        task: &str,
        path: &Path,
        payload: &PersistedIdentity,
        destination: &Path,
        selected: &PersistedIdentity,
    ) -> Result<()> {
        validate_export_stage(task, path)?;
        check_no_symlinks(path, false)?;
        check_no_symlinks(destination, false)?;
        if path.parent() != destination.parent()
            || !payload.matches_path(destination).unwrap_or(false)
            || !selected.matches_path(path).unwrap_or(false)
        {
            return Err(invalid("覆盖发布结果未确认，原目标仍受保护。"));
        }
        self.update(id, false, |record| {
            let stage = record
                .export_stages
                .get_mut(task)
                .filter(|stage| stage.path == path)
                .ok_or_else(|| invalid("覆盖临时材料未登记。"))?;
            if !stage
                .identity
                .as_ref()
                .is_some_and(|identity| identity.matches_captured(payload))
                || !stage
                    .overwrite_identity
                    .as_ref()
                    .is_some_and(|identity| identity.matches_captured(selected))
                || !stage
                    .parent_identity
                    .matches_path(path.parent().unwrap())
                    .unwrap_or(false)
            {
                return Err(invalid("覆盖发布授权或父目录身份变化。"));
            }
            stage.overwrite_verified = true;
            Ok(())
        })
    }
    /// Native copies reserve an ownership key without creating a BuildManager
    /// task directory. Only this empty reservation may disappear on delivery.
    pub(crate) fn forget_empty_task(&self, id: &str, task: &str) -> Result<()> {
        task_id(task)?;
        self.update(id, false, |record| {
            if record.export_stages.contains_key(task)
                || record.tasks.get(task).is_some_and(Option::is_some)
            {
                return Err(invalid("本次原生复制仍有未清理的任务材料。"));
            }
            for path in [
                self.workspace.join(".platform/tasks").join(task),
                self.workspace.join(".platform/operations").join(task),
            ] {
                match fs::symlink_metadata(path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                    Ok(_) => return Err(invalid("本次原生复制已有任务目录，未清除其归属。")),
                }
            }
            record.tasks.remove(task);
            Ok(())
        })
    }
    pub(crate) fn import_binding_matches(
        &self,
        id: &str,
        task: &str,
        path: &Path,
        expected: &PersistedIdentity,
    ) -> Result<bool> {
        let file = self.path(id)?;
        if !file.try_exists()? {
            return Ok(false);
        }
        let r = self.read(id)?;
        Ok(r.tasks.contains_key(task)
            && r.project_root
                .as_ref()
                .is_some_and(|b| b.path == path && b.identity.matches_captured(expected)))
    }
    pub(crate) fn retain_import_rollback_binding(
        &self,
        id: &str,
        task: &str,
        path: &Path,
        expected: &PersistedIdentity,
    ) -> Result<()> {
        self.update(id, false, |r| {
            if let Some(b) = &r.project_root {
                if b.path != path || !b.identity.matches_captured(expected) {
                    return Err(invalid("待撤销源码归属变化。"));
                }
            }
            r.project_root = Some(OwnedPath {
                path: path.into(),
                identity: expected.clone(),
                directory: true,
            });
            r.tasks.entry(task.into()).or_insert(None);
            Ok(())
        })
    }
    pub(crate) fn discard_imported_source(
        &self,
        id: &str,
        path: &Path,
        expected: &PersistedIdentity,
    ) -> Result<()> {
        let directory = self.directory(id)?;
        if !self.path(id)?.try_exists()? {
            if directory.try_exists()? {
                fs::remove_dir(directory)?;
            }
            return Ok(());
        }
        let record = self.read(id)?;
        if !record
            .project_root
            .as_ref()
            .is_some_and(|b| b.path == path && b.identity.matches_captured(expected))
            || !record.retained.is_empty()
            || !record.export_stages.is_empty()
            || !record.file_deletions.is_empty()
        {
            return Err(invalid("待撤销的源码事务还有其他材料。"));
        }
        crate::durable_writer::require_owner_clear(&crate::durable_writer::WriterOwner::web(
            &self.data, id,
        )?)?;
        // Keep the exact source/task binding until all other control material is gone.
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            if path == self.path(id)? {
                continue;
            }
            check_no_symlinks(&path, false)?;
            fs::remove_dir(&path)?;
        }
        fs::remove_file(self.path(id)?)?;
        fs::remove_dir(directory)?;
        Ok(())
    }
    pub(crate) fn record_import_control(
        &self,
        id: &str,
        import: &str,
        expected: &PersistedIdentity,
    ) -> Result<()> {
        let path = import_control_path(&self.data, id, import)?;
        self.update(id, false, |record| {
            if let Some(bound) = record.import_controls.get(import) {
                if bound.path != path || !bound.identity.matches_captured(expected) {
                    return Err(invalid("导入控制目录已有不同身份。"));
                }
                bound.verify()?;
                return Ok(());
            }
            check_no_symlinks(&path, false)?;
            if !path.is_dir() || !expected.matches_path(&path).unwrap_or(false) {
                return Err(invalid("导入控制目录身份已变化。"));
            }
            record.import_controls.insert(
                import.into(),
                OwnedPath {
                    path,
                    identity: expected.clone(),
                    directory: true,
                },
            );
            Ok(())
        })
    }
    pub(crate) fn release_import_control(
        &self,
        id: &str,
        import: &str,
        expected: &PersistedIdentity,
    ) -> Result<()> {
        let path = import_control_path(&self.data, id, import)?;
        check_no_symlinks(&path, true)?;
        if path.try_exists()?
            || !expected
                .absent_on_expected_volume(&path)
                .map_err(|e| invalid(e.to_string()))?
        {
            return Err(invalid("导入控制目录尚未确认消失。"));
        }
        if !self.path(id)?.try_exists()? {
            return Ok(());
        }
        self.update(id, false, |record| {
            if let Some(bound) = record.import_controls.get(import) {
                if bound.path != path || !bound.identity.matches_captured(expected) {
                    return Err(invalid("待释放导入控制目录身份不符。"));
                }
            }
            record.import_controls.remove(import);
            Ok(())
        })
    }
    pub(crate) fn discard_empty_admission(
        &self,
        id: &str,
        fresh_identity: Option<&PersistedIdentity>,
    ) -> Result<()> {
        if !self.path(id)?.try_exists()? {
            return Ok(());
        }
        let r = self.read(id)?;
        if r.project_root.is_some()
            || r.user_root.is_some()
            || !r.tasks.is_empty()
            || !r.retained.is_empty()
            || !r.export_stages.is_empty()
            || !r.import_controls.is_empty()
        {
            return Err(invalid("非空登记不能作为未接入事务清理。"));
        }
        let private = self.data.join("web-apps").join(id);
        if private.try_exists()? {
            let bound = r
                .instance_identity
                .as_ref()
                .or(fresh_identity)
                .ok_or_else(|| invalid("待撤销控制目录缺少精确创建身份。"))?;
            crate::history_cleanup::remove_owned_directory(&private, bound)?;
        }
        self.discard_unpublished(id)
    }
    pub(crate) fn discard_user_registration(
        &self,
        id: &str,
        source: &Path,
        expected: &PersistedIdentity,
    ) -> Result<()> {
        self.update(id, false, |r| {
            if r.project_root.is_some() || !r.tasks.is_empty() {
                return Err(invalid("待撤销的本地登记包含平台材料。"));
            }
            if let Some(bound) = &r.user_root {
                if bound.path != source || !bound.identity.matches_captured(expected) {
                    return Err(invalid("待撤销用户目录身份不符。"));
                }
            }
            r.user_root = None;
            Ok(())
        })?;
        self.discard_unpublished(id)
    }
    pub(crate) fn record_instance_root(&self, id: &str) -> Result<()> {
        let path = self.data.join("web-apps").join(id);
        let identity = PersistedIdentity::capture(&path).map_err(|e| invalid(e.to_string()))?;
        self.update(id, false, |r| {
            if r.instance_identity
                .as_ref()
                .is_some_and(|old| !old.matches_captured(&identity))
            {
                return Err(invalid("实例控制目录已被替换。"));
            }
            r.instance_identity = Some(identity);
            Ok(())
        })
    }
    pub(crate) fn instance_identity(&self, id: &str) -> Result<Option<PersistedIdentity>> {
        Ok(self.read(id)?.instance_identity)
    }
    pub(crate) fn record_user_root(&self, id: &str, path: &Path) -> Result<()> {
        absolute(path)?;
        let current = capture(path)?.ok_or_else(|| invalid("用户项目目录不存在。"))?;
        self.update(id, false, |record| {
            if record.project_root.is_some() {
                return Err(invalid("项目已登记平台源码归属。"));
            }
            record.user_root = Some(current);
            Ok(())
        })
    }
    pub(crate) fn user_source_roots(&self) -> Result<Vec<PathBuf>> {
        let mut roots = vec![];
        if !self.root().try_exists()? {
            return Ok(roots);
        }
        for entry in fs::read_dir(self.root())? {
            let id = entry?.file_name().to_string_lossy().into_owned();
            if let Some(root) = self.read(&id)?.user_root {
                roots.push(root.path);
            }
        }
        Ok(roots)
    }
    pub(crate) fn record_project_root(&self, id: &str, path: &Path) -> Result<()> {
        absolute(path)?;
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
        #[cfg(windows)]
        windows_delivery::recover_project(self, id)?;
        let record = self.read(id)?;
        if record.workspace_root != self.workspace {
            return Err(invalid("网页项目工作区归属已变化。"));
        }
        let mut paths = fixed_paths(&self.data, &self.workspace, id);
        for (import, bound) in &record.import_controls {
            bound.verify()?;
            crate::durable_writer::require_owner_clear(&crate::durable_writer::WriterOwner::node(
                &self.data, import,
            )?)?;
            paths.push(bound.path.clone());
        }
        if let Some(root) = &record.project_root {
            root.verify()?;
            paths.push(root.path.clone());
        }
        for (task, identity) in &record.tasks {
            self.verify_task_path(id, task, identity.as_ref())?;
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
                        .iter()
                        .chain(
                            stage
                                .overwrite_identity
                                .iter()
                                .filter(|_| stage.overwrite_verified),
                        )
                        .any(|identity| identity.matches_path(&stage.path).unwrap_or(false))
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
            .chain(record.import_controls.values())
            .find(|bound| bound.path == path)
        {
            return Ok(Some(bound.identity.clone()));
        }
        for (task, identity) in &record.tasks {
            if path == self.workspace.join(".platform/tasks").join(task) {
                self.verify_task_path(id, task, identity.as_ref())?;
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
            if stage.path.try_exists()? {
                return stage
                    .identity
                    .iter()
                    .chain(
                        stage
                            .overwrite_identity
                            .iter()
                            .filter(|_| stage.overwrite_verified),
                    )
                    .find(|identity| identity.matches_path(&stage.path).unwrap_or(false))
                    .cloned()
                    .map(Some)
                    .ok_or_else(|| invalid("临时文件已被替换或覆盖结果未确认，未接管其新身份。"));
            }
            return Ok(stage.identity.clone());
        }
        let fixed = fixed_paths(&self.data, &self.workspace, id)
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
            if let Some(root) = self.read(&id)?.project_root {
                root.verify()?;
                paths.push(root.path);
            }
            paths.push(self.directory(&id)?);
            if self.read(&id)?.instance_identity.is_some() {
                paths.push(self.data.join("web-apps").join(&id));
            }
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
                .chain(record.user_root.iter())
                .chain(record.import_controls.values())
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
            let fixed = fixed_paths(&self.data, &record.workspace_root, &id)
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
                        .iter()
                        .chain(stage.overwrite_identity.iter())
                        .any(|identity| identity.matches_path(candidate).unwrap_or(false))
            });
            if bound || fixed || stage {
                return Err(crate::project_tool_boundary::project_owned());
            }
        }
        Ok(())
    }
    /// Admission only: a newly claimed source root must not absorb another
    /// project's live external copy stage or an exact retained migration copy.
    pub(crate) fn require_source_unoccupied(data: &Path, source: &Path) -> Result<()> {
        check_no_symlinks(source, true)?;
        let source_identity = match fs::symlink_metadata(source) {
            Ok(_) => Some(
                PersistedIdentity::capture(source).map_err(|error| invalid(error.to_string()))?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let ownership = Self::new(data.into(), PathBuf::new());
        check_no_symlinks(&ownership.root(), true)?;
        if !ownership.root().try_exists()? {
            return Ok(());
        }
        for entry in fs::read_dir(ownership.root())? {
            let id = entry?.file_name().to_string_lossy().into_owned();
            let record = ownership.read(&id)?;
            let stages = record.export_stages.values().any(|stage| {
                stage.path.starts_with(source)
                    || source.starts_with(&stage.path)
                    || source_identity.as_ref().is_some_and(|identity| {
                        stage
                            .ancestor_identities
                            .iter()
                            .any(|ancestor| ancestor.matches_captured(identity))
                    })
            });
            let bound = record
                .project_root
                .iter()
                .chain(record.prior_project_roots.iter())
                .chain(record.user_root.iter())
                .chain(record.import_controls.values())
                .chain(record.retained.iter())
                .filter(|bound| !record.removed_materials.contains_key(&bound.path))
                .any(|bound| {
                    bound.path.starts_with(source)
                        || (bound.directory && source.starts_with(&bound.path))
                        || source_identity.as_ref().is_some_and(|identity| {
                            bound
                                .path
                                .ancestors()
                                .any(|ancestor| identity.matches_path(ancestor).unwrap_or(false))
                        })
                });
            if stages || bound {
                return Err(ApiError::new(
                    "PROJECT_DIRECTORY_OWNED",
                    "此目录包含另一个项目独占的代码或临时材料，请选择独立目录。",
                ));
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
                        if !record
                            .prior_project_roots
                            .iter()
                            .any(|prior| prior.path == root.path)
                        {
                            record.prior_project_roots.push(root.clone());
                        }
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
    fn authorizes_file(
        &self,
        record: &Ownership,
        path: &Path,
        expected: &PersistedIdentity,
    ) -> bool {
        fixed_paths(&self.data, &record.workspace_root, &record.project_id)
            .iter()
            .any(|fixed| fixed == path)
            || record.retained.iter().any(|bound| {
                !bound.directory && bound.path == path && bound.identity.matches_captured(expected)
            })
            || record.export_stages.values().any(|stage| {
                stage.path == path
                    && stage
                        .identity
                        .iter()
                        .chain(
                            stage
                                .overwrite_identity
                                .iter()
                                .filter(|_| stage.overwrite_verified),
                        )
                        .any(|identity| identity.matches_captured(expected))
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
            Ok(())
        })
    }
    pub(crate) fn finalize(&self, id: &str) -> Result<()> {
        let directory = self.directory(id)?;
        if !directory.exists() {
            return Ok(());
        }
        if self.path(id)?.exists() {
            #[cfg(windows)]
            windows_delivery::recover_project(self, id)?;
            let record = self.read(id)?;
            if !record.file_deletions.is_empty() {
                return Err(invalid("仍有未完成的文件删除计划。"));
            }
            // All remaining entries are platform material. Final deliveries
            // have never entered this ledger.
            self.material_paths(id)?;
        }
        let identity =
            PersistedIdentity::capture(&directory).map_err(|error| invalid(error.to_string()))?;
        crate::history_cleanup::remove_owned_directory(&directory, &identity).map(|_| ())
    }
}
pub(crate) fn fixed_paths(data: &Path, workspace: &Path, id: &str) -> Vec<PathBuf> {
    let private = data.join("web-apps").join(id);
    vec![
        private.join("control.json"),
        private.join("cache"),
        private.join("logs"),
        private.join("uploads"),
        private.join("builds"),
        private.join("npm-cache"),
        private.join("latest-build.json"),
        private.join("handoff"),
        private.join("create"),
        private.join("configuration"),
        workspace
            .join(".platform/cache/storage/projects")
            .join(format!("{id}.json")),
        workspace
            .join(".platform/cache/storage/invalidations/projects")
            .join(format!("{id}.json")),
    ]
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
        let state = owner.data.join("web-apps").join(&id);
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join("control.json"), "private project control").unwrap();
        owner.record_instance_root(&id).unwrap();
        let other = owner.data.join("web-apps/prj_unrelated/control.json");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
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
        let roots = migrated.source_roots(&id).unwrap();
        assert!(roots.contains(&root));
        assert!(roots.contains(&next.join("projects/disposable-web-root")));
        assert!(paths.contains(&root));
        assert!(paths.contains(&old_task));
        assert!(paths.contains(&new_task));
        assert!(owner
            .record_task(&id, &format!("task_{}", uuid::Uuid::new_v4().simple()))
            .is_err());
        assert!(paths.contains(&state.join("control.json")));
        assert!(!paths.contains(&other));
        assert!(!paths.iter().any(|p| p.starts_with(next.join("web-apps"))));
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
    fn publish_delivery(
        owner: &WebProjectOwnership,
        id: &str,
        parent: &Path,
        name: &str,
        forget: bool,
    ) -> (String, PathBuf, PathBuf) {
        fs::create_dir_all(parent).unwrap();
        let task = format!("task_{}", uuid::Uuid::new_v4().simple());
        owner.record_task(id, &task).unwrap();
        let path = parent.join(name);
        let stage = parent.join(format!(".export-{task}.partial"));
        owner.reserve_export_stage(id, &task, &stage).unwrap();
        fs::write(&stage, []).unwrap();
        owner
            .capture_export_stage(
                id,
                &task,
                &stage,
                &PersistedIdentity::capture(&stage).unwrap(),
            )
            .unwrap();
        fs::write(&stage, b"disposable user ZIP").unwrap();
        fs::rename(&stage, &path).unwrap();
        if forget {
            owner.forget_export_stage(id, &task).unwrap();
        }
        (task, path, stage)
    }
    #[test]
    fn delivered_zip_never_enters_deletion_or_uninstall_scope_even_if_replaced() {
        let (temporary, owner, id) = fixture();
        let parent = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("user-deliveries");
        let (_, path, _) = publish_delivery(&owner, &id, &parent, "snapshot.zip", true);
        assert!(!owner.material_paths(&id).unwrap().contains(&path));
        assert!(!owner
            .all_owned_scopes()
            .unwrap()
            .iter()
            .any(|(_, paths)| paths.contains(&path)));
        fs::write(&path, b"user replaced delivered ZIP").unwrap();
        owner.finalize(&id).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"user replaced delivered ZIP");
        assert!(!owner.data.join("web-export-owners").exists());
    }
    #[test]
    fn crash_after_delivery_before_forgetting_partial_never_adopts_the_final_zip() {
        let (temporary, owner, id) = fixture();
        let parent = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("user-deliveries");
        let (_, path, stage) = publish_delivery(&owner, &id, &parent, "snapshot.zip", false);
        let reopened = WebProjectOwnership::new(owner.data.clone(), owner.workspace.clone());
        let paths = reopened.material_paths(&id).unwrap();
        assert!(paths.contains(&stage));
        assert!(!paths.contains(&path));
        reopened.finalize(&id).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"disposable user ZIP");
    }
    #[test]
    fn a_delivered_user_zip_does_not_permanently_reserve_a_source_directory() {
        let (temporary, owner, id) = fixture();
        let parent = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("user-deliveries");
        let (_, path, _) = publish_delivery(&owner, &id, &parent, "snapshot.zip", true);
        let node = uuid::Uuid::new_v4().to_string();
        crate::node_project_ownership::NodeProjectOwnership::new(
            owner.data.clone(),
            owner.workspace.clone(),
        )
        .initialize(&node)
        .unwrap();
        crate::node_apps::source_ownership::claim_source_after_verified_migration(
            &owner.data,
            &owner.workspace,
            &node,
            &parent,
        )
        .unwrap();
        assert_eq!(fs::read(path).unwrap(), b"disposable user ZIP");
    }
    #[test]
    fn missing_partial_on_an_unavailable_expected_volume_stays_protected() {
        let (temporary, owner, id) = fixture();
        let parent = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("user-deliveries");
        let (_, path, stage) = publish_delivery(&owner, &id, &parent, "snapshot.zip", false);
        owner
            .update(&id, false, |record| {
                let identity = record
                    .export_stages
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
        assert!(!stage.exists());
        assert!(path.exists());
    }
    #[test]
    fn public_migration_does_not_register_user_deliveries_as_project_material() {
        let (_temporary, owner, id) = fixture();
        let (_, old, _) = publish_delivery(
            &owner,
            &id,
            &owner.workspace.join("custom-deliveries"),
            "snapshot.zip",
            true,
        );
        let next = owner.workspace.with_extension("new");
        fs::create_dir_all(next.join("custom-deliveries")).unwrap();
        let new = next.join("custom-deliveries/snapshot.zip");
        fs::copy(&old, &new).unwrap();
        WebProjectOwnership::rebind_after_migration(&owner.data, &owner.workspace, &next).unwrap();
        let migrated = WebProjectOwnership::new(owner.data.clone(), next);
        let paths = migrated.material_paths(&id).unwrap();
        assert!(!paths.contains(&old));
        assert!(!paths.contains(&new));
        migrated.finalize(&id).unwrap();
        assert_eq!(fs::read(old).unwrap(), b"disposable user ZIP");
        assert_eq!(fs::read(new).unwrap(), b"disposable user ZIP");
    }
}

#[cfg(test)]
mod import_control_tests {
    use super::*;
    fn fixture() -> (
        tempfile::TempDir,
        WebProjectOwnership,
        String,
        String,
        PathBuf,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let data = root.join("data");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&data).unwrap();
        let import = uuid::Uuid::new_v4();
        let id = format!("prj_{}", import.simple());
        let import = import.to_string();
        let path = data.join("node-project-ownership").join(&import);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("import-evidence"), b"disposable import control").unwrap();
        let owner = WebProjectOwnership::new(data, workspace);
        owner.initialize(&id).unwrap();
        (temp, owner, id, import, path)
    }
    #[test]
    fn exact_import_control_is_current_owned_material_and_release_leaves_no_scope() {
        let (_temp, owner, id, import, path) = fixture();
        let identity = PersistedIdentity::capture(&path).unwrap();
        owner
            .record_import_control(&id, &import, &identity)
            .unwrap();
        assert!(owner.material_paths(&id).unwrap().contains(&path));
        assert!(owner
            .material_identity(&id, &path)
            .unwrap()
            .unwrap()
            .matches_captured(&identity));
        crate::history_cleanup::remove_owned_directory(&path, &identity).unwrap();
        owner
            .record_import_control(&id, &import, &identity)
            .unwrap();
        owner
            .release_import_control(&id, &import, &identity)
            .unwrap();
        owner
            .release_import_control(&id, &import, &identity)
            .unwrap();
        assert!(owner.read(&id).unwrap().import_controls.is_empty());
        assert!(!owner.material_paths(&id).unwrap().contains(&path));
    }
    #[test]
    fn import_control_mapping_and_replacement_never_grant_other_directory_deletion() {
        let (_temp, owner, id, import, path) = fixture();
        let identity = PersistedIdentity::capture(&path).unwrap();
        assert!(owner
            .record_import_control(&id, &uuid::Uuid::new_v4().to_string(), &identity)
            .is_err());
        owner
            .record_import_control(&id, &import, &identity)
            .unwrap();
        fs::rename(&path, path.with_extension("held-original")).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("foreign"), b"keep replacement").unwrap();
        assert!(owner.material_paths(&id).is_err());
        assert!(owner
            .release_import_control(&id, &import, &identity)
            .is_err());
        assert_eq!(fs::read(path.join("foreign")).unwrap(), b"keep replacement");
    }
}
