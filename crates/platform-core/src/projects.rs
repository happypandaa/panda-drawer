//! A project folder is author data. Instance identity and controls are private platform state.
use crate::{
    database::{self, Budget, DatabaseWorker, QueueBudget, Work, WorkResult},
    file_identity::PersistedIdentity as FileIdentity,
    panda_manifest::{PandaManifest, PandaRuntime},
    ApiError, SchemaInfo, SqlCaller, SqlRequest, SqlResult, TransactionRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::RwLock;
use uuid::Uuid;

pub type ProjectManifest = PandaManifest;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateProjectInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub project_root: PathBuf,
    pub document_epoch: String,
    pub state: String,
    pub issues: Vec<String>,
    pub manifest: Option<PandaManifest>,
    pub build_info: Value,
    pub runtime: PandaRuntime,
    pub ownership: ProjectOwnership,
    pub source_preserved_on_delete: bool,
    pub version: Option<String>,
    pub icon_data_url: Option<String>,
    pub git_source: Option<Value>,
    pub is_agent: bool,
    pub address: Option<crate::FixedAppAddress>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectOwnership {
    User,
    Platform,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Registration {
    schema_version: u32,
    id: String,
    project_root: PathBuf,
    root_identity: FileIdentity,
    ownership: ProjectOwnership,
    document_epoch: String,
    workspace_root: PathBuf,
    address: crate::FixedAppAddress,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git_source: Option<Value>,
    #[serde(default)]
    is_agent: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentHint {
    is_agent: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Reservation {
    project_id: String,
    task_id: String,
    project_root: PathBuf,
    root_identity: Option<FileIdentity>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProjectDeletion {
    schema_version: u32,
    pub(crate) project_id: String,
    pub(crate) task_id: String,
    pub(crate) target_epoch: String,
    pub(crate) root_identity: FileIdentity,
    pub(crate) project_root: PathBuf,
    pub(crate) ownership: ProjectOwnership,
    pub(crate) source_epoch: Option<String>,
}
#[derive(Clone)]
pub struct ProjectHandle {
    pub id: String,
    pub root: PathBuf,
    pub manifest: PandaManifest,
    pub private_dir: PathBuf,
    pub address: crate::FixedAppAddress,
    pub document_epoch: String,
    pub operation_gate: Arc<RwLock<()>>,
    registration: Registration,
    state_path: PathBuf,
    pub(crate) control: Arc<crate::control::ProjectControl>,
    pub(crate) maintenance_id: Option<String>,
    pub(crate) diagnostic: bool,
    workspace_paused: Arc<AtomicBool>,
    deletion_path: PathBuf,
}
impl ProjectHandle {
    pub(crate) fn is_workspace_paused(&self) -> bool {
        self.workspace_paused.load(Ordering::Acquire)
    }
    pub fn web_origin(&self) -> String {
        self.address.origin()
    }
    pub fn sdk_enabled(&self) -> bool {
        self.manifest
            .platform
            .as_ref()
            .is_some_and(|p| p.sdk_api_version == 1)
    }
    pub fn database_path(&self) -> Result<PathBuf, ApiError> {
        self.require_sdk()?;
        Ok(self.root.join("data/main.sqlite"))
    }
    pub fn attachments_dir(&self) -> Result<PathBuf, ApiError> {
        self.require_sdk()?;
        Ok(self.root.join("data/attachments"))
    }
    pub(crate) fn require_sdk(&self) -> Result<(), ApiError> {
        if !self.sdk_enabled() {
            return Err(ApiError::new(
                "SDK_NOT_ENABLED",
                "这个应用没有启用平台 SDK。",
            ));
        }
        Ok(())
    }
    pub(crate) fn ensure_attachments_dir(&self) -> Result<PathBuf, ApiError> {
        self.check()?;
        let path = self.attachments_dir()?;
        ensure_user_directory(&self.root.join("data"))?;
        ensure_user_directory(&path)?;
        Ok(path)
    }
    pub(crate) fn prepare_database(&self) -> Result<PathBuf, ApiError> {
        self.check()?;
        let path = self.database_path()?;
        ensure_user_directory(&self.root.join("data"))?;
        check_no_symlinks(&path, true)?;
        if path.try_exists()?
            && (!path.is_file() || crate::project_file_io::link_count(&path)? != 1)
        {
            return Err(ApiError::new(
                "UNSAFE_DATABASE_PATH",
                "数据库必须是独立的普通文件。",
            ));
        }
        Ok(path)
    }
    pub(crate) fn build_target(&self) -> Result<crate::web_build::WebBuildTarget, ApiError> {
        self.check()?;
        let manifest = read_manifest(&self.root)?;
        Ok(crate::web_build::WebBuildTarget {
            project_root: self.root.clone(),
            manifest,
        })
    }
    pub fn check(&self) -> Result<(), ApiError> {
        if self.is_workspace_paused() {
            return Err(ApiError::new(
                "WORKSPACE_MIGRATING",
                "Workspace migration has paused project access",
            )
            .outcome("not_started"));
        }
        check_deletion_marker(&self.deletion_path)?;
        if !self.state_path.try_exists()? {
            return Err(removed_project_error(&self.id));
        }
        check_no_symlinks(&self.root, false)?;
        if !identity_matches(&self.registration.root_identity, &self.root)? {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Project directory has been replaced; remove and add the folder again",
            ));
        }
        let current: Registration = read_json(&self.state_path)?;
        if current.document_epoch != self.document_epoch
            || current.id != self.id
            || current.address != self.address
        {
            return Err(ApiError::new(
                "PROJECT_RELOAD_REQUIRED",
                "Project identity changed; reopen the project",
            ));
        }
        Ok(())
    }
    pub fn validate_epoch(&self, expected: Option<&str>) -> Result<(), ApiError> {
        if expected.is_some_and(|v| v != self.document_epoch) {
            return Err(ApiError::new(
                "PROJECT_RELOAD_REQUIRED",
                "This page belongs to an old project document; reopen the project",
            )
            .outcome("not_started"));
        }
        self.check()?;
        self.control.check(
            expected.is_some(),
            self.maintenance_id.as_deref(),
            self.diagnostic,
        )
    }
}
pub(crate) fn registration_path(data: &Path, id: &str) -> PathBuf {
    data.join("web-apps").join(id).join("registration.json")
}
fn directory_mapping_error() -> ApiError {
    ApiError::new(
        "PROJECT_DIRECTORY_MAPPING_INVALID",
        "项目登记的目录绑定无效，未接管其他目录。",
    )
}
fn validate_registration(id: &str, state: &Registration) -> Result<(), ApiError> {
    state.address.validate()?;
    if state.schema_version != 3
        || state.id != id
        || !valid_id(id)
        || !state.project_root.is_absolute()
        || state
            .project_root
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
        || Uuid::parse_str(&state.document_epoch).is_err()
    {
        return Err(directory_mapping_error());
    }
    Ok(())
}
pub(crate) fn resolve_project_root(
    data: &Path,
    _workspace: &Path,
    id: &str,
) -> Result<PathBuf, ApiError> {
    if !valid_id(id) {
        return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
    }
    let path = registration_path(data, id);
    check_no_symlinks(&path, true)?;
    if path.try_exists()? {
        let state: Registration = read_json(&path)?;
        validate_registration(id, &state)?;
        return Ok(state.project_root);
    }
    let path = data.join("web-apps").join(id).join("reservation.json");
    if path.try_exists()? {
        let record: Reservation = read_json(&path)?;
        if record.project_id != id
            || !valid_id(&record.task_id)
            || !record.project_root.is_absolute()
        {
            return Err(directory_mapping_error());
        }
        return Ok(record.project_root);
    }
    Err(ApiError::new(
        "PROJECT_NOT_REGISTERED",
        "Project identity is not registered",
    ))
}
pub(crate) fn enumerate_project_roots(
    data: &Path,
    workspace: &Path,
) -> Result<Vec<(String, PathBuf)>, ApiError> {
    let directory = data.join("web-apps");
    check_no_symlinks(&directory, true)?;
    if !directory.try_exists()? {
        return Ok(vec![]);
    }
    let mut roots = vec![];
    let mut owners = HashMap::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if !valid_id(&id) || !entry.path().join("registration.json").try_exists()? {
            continue;
        }
        let root = resolve_project_root(data, workspace, &id)?;
        if owners.insert(root.clone(), id.clone()).is_some() {
            return Err(directory_mapping_error());
        }
        roots.push((id, root));
    }
    roots.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(roots)
}
pub(crate) struct RegisteredProjectSource {
    pub(crate) id: String,
    pub(crate) root: PathBuf,
    pub(crate) identity: FileIdentity,
    pub(crate) ownership: ProjectOwnership,
}
pub(crate) fn registered_project_sources(
    data: &Path,
    workspace: &Path,
) -> Result<Vec<RegisteredProjectSource>, ApiError> {
    enumerate_project_roots(data, workspace)?
        .into_iter()
        .map(|(id, _)| {
            let r: Registration = read_json(&registration_path(data, &id))?;
            Ok(RegisteredProjectSource {
                id,
                root: r.project_root,
                identity: r.root_identity,
                ownership: r.ownership,
            })
        })
        .collect()
}
pub(crate) struct NodeDeletionBarrier {
    pending: Arc<Mutex<HashMap<String, String>>>,
    token: String,
}
impl Drop for NodeDeletionBarrier {
    fn drop(&mut self) {
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.token);
    }
}
type ProjectIdentityRemoved = Arc<dyn Fn(&str) + Send + Sync>;
pub struct ProjectStore {
    workspace: PathBuf,
    processes: crate::owned_process::OwnedProcessRegistry,
    registration_lock: Mutex<()>,
    gates: Mutex<HashMap<String, Arc<RwLock<()>>>>,
    workers: Mutex<HashMap<String, Arc<DatabaseWorker>>>,
    shutdown_issues: Mutex<Vec<crate::shutdown::ShutdownIssue>>,
    queue: Arc<QueueBudget>,
    stopping: AtomicBool,
    lifecycle_gate: RwLock<()>,
    workspace_migrating: AtomicBool,
    workspace_paused: Arc<AtomicBool>,
    controls: Mutex<HashMap<String, Arc<crate::control::ProjectControl>>>,
    active_tasks: Arc<Mutex<HashMap<String, ProjectOperation>>>,
    pending_node_deletions: Arc<Mutex<HashMap<String, String>>>,
    resources: Option<Arc<crate::resources::ProjectResources>>,
    identity_removed: Mutex<Vec<ProjectIdentityRemoved>>,
    node_source_data_root: Mutex<Option<PathBuf>>,
}
impl ProjectStore {
    pub fn new(workspace: PathBuf) -> Result<Self, ApiError> {
        Self::with_resources(workspace, None)
    }
    pub fn with_resources(
        workspace: PathBuf,
        resources: Option<Arc<crate::resources::ProjectResources>>,
    ) -> Result<Self, ApiError> {
        Self::with_resources_and_processes(workspace, resources, Default::default())
    }
    pub(crate) fn with_resources_and_processes(
        workspace: PathBuf,
        resources: Option<Arc<crate::resources::ProjectResources>>,
        processes: crate::owned_process::OwnedProcessRegistry,
    ) -> Result<Self, ApiError> {
        let data = if workspace.file_name().is_some_and(|n| n == "workspace") {
            workspace.parent().unwrap().to_path_buf()
        } else {
            workspace.join(".platform")
        };
        Self::with_paths(workspace, data, resources, processes)
    }
    pub(crate) fn with_paths(
        workspace: PathBuf,
        data: PathBuf,
        resources: Option<Arc<crate::resources::ProjectResources>>,
        processes: crate::owned_process::OwnedProcessRegistry,
    ) -> Result<Self, ApiError> {
        crate::platform_support::require_runtime().map_err(ApiError::from)?;
        check_no_symlinks(&workspace, false)?;
        for path in [workspace.join("projects"), data.join("web-apps")] {
            ensure_dir(&path)?;
        }
        Ok(Self {
            workspace,
            processes,
            registration_lock: Mutex::new(()),
            gates: Mutex::new(HashMap::new()),
            workers: Mutex::new(HashMap::new()),
            shutdown_issues: Mutex::new(vec![]),
            queue: Arc::new(QueueBudget::default()),
            stopping: AtomicBool::new(false),
            lifecycle_gate: RwLock::new(()),
            workspace_migrating: AtomicBool::new(false),
            workspace_paused: Arc::new(AtomicBool::new(false)),
            controls: Mutex::new(HashMap::new()),
            active_tasks: Arc::new(Mutex::new(HashMap::new())),
            pending_node_deletions: Arc::new(Mutex::new(HashMap::new())),
            resources,
            identity_removed: Mutex::new(vec![]),
            node_source_data_root: Mutex::new(Some(data)),
        })
    }
    pub(crate) fn set_node_source_data_root(&self, data: PathBuf) {
        *self.node_source_data_root.lock().unwrap() = Some(data);
    }
    pub(crate) fn data_root(&self) -> PathBuf {
        self.node_source_data_root
            .lock()
            .unwrap()
            .clone()
            .expect("ProjectStore application data root")
    }
    pub(crate) fn private_dir(&self, id: &str) -> Result<PathBuf, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        Ok(self.data_root().join("web-apps").join(id))
    }
    pub fn workspace_root(&self) -> &Path {
        &self.workspace
    }
    pub(crate) fn processes(&self) -> crate::owned_process::OwnedProcessRegistry {
        self.processes.clone()
    }
    pub(crate) fn project_processes(
        &self,
        id: &str,
    ) -> Result<crate::owned_process::OwnedProcessRegistry, ApiError> {
        Ok(self
            .processes()
            .with_writer_owner(crate::durable_writer::WriterOwner::web(
                &self.data_root(),
                id,
            )?))
    }
    fn accepting(&self) -> Result<(), ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping").outcome("not_started"))
        } else {
            Ok(())
        }
    }
    pub(crate) fn initialize_project_ownership(&self, id: &str) -> Result<(), ApiError> {
        crate::web_project_ownership::WebProjectOwnership::new(
            self.data_root(),
            self.workspace.clone(),
        )
        .initialize(id)
    }
    fn require_new_project_location(&self, root: &Path) -> Result<(), ApiError> {
        crate::node_apps::source_ownership::require_new_web_project_location(
            &self.data_root(),
            root,
        )
    }
    fn reservation_path(&self, id: &str) -> PathBuf {
        self.data_root()
            .join("web-apps")
            .join(id)
            .join("reservation.json")
    }
    pub(crate) fn is_registered(&self, id: &str) -> Result<bool, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        Ok(registration_path(&self.data_root(), id).try_exists()?)
    }
    pub(crate) fn reserve_managed_project(
        &self,
        id: &str,
        task: &str,
        name: &str,
    ) -> Result<PathBuf, ApiError> {
        let _lock = self.registration_lock.lock().unwrap();
        self.accepting()?;
        self.check_workspace_available()?;
        if !valid_id(id) || !valid_id(task) {
            return Err(directory_mapping_error());
        }
        let path = self.reservation_path(id);
        if path.try_exists()? {
            let r: Reservation = read_json(&path)?;
            if r.project_id == id && r.task_id == task {
                return Ok(r.project_root);
            }
            return Err(directory_mapping_error());
        }
        if self.is_registered(id)? {
            return Err(ApiError::new(
                "PROJECT_ID_CONFLICT",
                "Project identity is already registered",
            ));
        }
        let base = crate::project_directory_name::readable_directory_name(name, id);
        let root = crate::node_apps::source_ownership::with_source_ownership_admission(|| {
            for suffix in 1..10001 {
                let leaf = if suffix == 1 {
                    base.clone()
                } else {
                    format!("{base}-{suffix}")
                };
                let root = self.workspace.join("projects").join(leaf);
                if root.try_exists()? {
                    continue;
                }
                let mut reserved = false;
                for entry in fs::read_dir(self.data_root().join("web-apps"))? {
                    let path = entry?.path().join("reservation.json");
                    if path.try_exists()? && read_json::<Reservation>(&path)?.project_root == root {
                        reserved = true;
                        break;
                    }
                }
                if reserved {
                    continue;
                }
                self.require_new_project_location(&root)?;
                self.initialize_project_ownership(id)?;
                ensure_dir(&self.private_dir(id)?)?;
                let ownership = crate::web_project_ownership::WebProjectOwnership::new(
                    self.data_root(),
                    self.workspace.clone(),
                );
                ownership.record_instance_root(id)?;
                ownership.record_task(id, task)?;
                crate::app_address::reserve_app_address(
                    &self.data_root(),
                    crate::app_address::AppAddressKind::Web,
                    id,
                )?;
                crate::builds::atomic_json(
                    &path,
                    &Reservation {
                        project_id: id.into(),
                        task_id: task.into(),
                        project_root: root.clone(),
                        root_identity: None,
                    },
                )?;
                return Ok(root);
            }
            Err(ApiError::new(
                "PROJECT_DIRECTORY_CONFLICT",
                "No unused project directory was available",
            ))
        })?;
        Ok(root)
    }
    pub(crate) fn reserve_managed_source(
        &self,
        id: &str,
        task: &str,
        root: &Path,
        expected: &FileIdentity,
    ) -> Result<(), ApiError> {
        let _lock = self.registration_lock.lock().unwrap();
        self.accepting()?;
        self.check_workspace_available()?;
        if !valid_id(id)
            || !valid_id(task)
            || !root.is_absolute()
            || !expected.matches_path(root).unwrap_or(false)
        {
            return Err(directory_mapping_error());
        }
        let state = self.web_import_state(id, task, root, expected)?;
        if state == WebImportState::Registered {
            return Ok(());
        }
        let owner = crate::web_project_ownership::WebProjectOwnership::new(
            self.data_root(),
            self.workspace.clone(),
        );
        let mut fresh_private = None;
        let result = crate::node_apps::source_ownership::with_source_ownership_admission(|| {
            if state == WebImportState::Absent {
                crate::web_project_ownership::WebProjectOwnership::require_source_unoccupied(
                    &self.data_root(),
                    root,
                )?;
                owner.initialize(id)?;
                let private = self.private_dir(id)?;
                if !private.try_exists()? {
                    ensure_dir(&private)?;
                    fresh_private = Some(identity(&private)?);
                } else if owner
                    .instance_identity(id)?
                    .as_ref()
                    .is_none_or(|i| !i.matches_path(&private).unwrap_or(false))
                {
                    return Err(directory_mapping_error());
                }
                owner.record_instance_root(id)?;
                crate::app_address::reserve_app_address(
                    &self.data_root(),
                    crate::app_address::AppAddressKind::Web,
                    id,
                )?;
                // Reservation is the durable transfer point, including errors in the following ledger writes.
                crate::builds::atomic_json(
                    &self.reservation_path(id),
                    &Reservation {
                        project_id: id.into(),
                        task_id: task.into(),
                        project_root: root.into(),
                        root_identity: Some(expected.clone()),
                    },
                )?;
            }
            owner.record_project_root(id, root)?;
            owner.record_task(id, task)
        });
        if let Err(error) = result {
            if !self.reservation_path(id).try_exists()? && !self.is_registered(id)? {
                let cleanup = owner.discard_empty_admission(id, fresh_private.as_ref());
                return Err(error.detail("cleanupError", cleanup.err().map(|e| json!(e))));
            }
            return Err(error);
        }
        Ok(())
    }
    pub(crate) fn discard_empty_managed_reservation(&self, id: &str) -> Result<(), ApiError> {
        let _lock = self.registration_lock.lock().unwrap();
        if self.is_registered(id)? || self.reservation_path(id).try_exists()? {
            return Err(directory_mapping_error());
        }
        crate::web_project_ownership::WebProjectOwnership::new(
            self.data_root(),
            self.workspace.clone(),
        )
        .discard_empty_admission(id, None)
    }
    pub(crate) fn web_import_state(
        &self,
        id: &str,
        task: &str,
        root: &Path,
        expected: &FileIdentity,
    ) -> Result<WebImportState, ApiError> {
        if self.is_registered(id)? {
            let r: Registration = read_json(&registration_path(&self.data_root(), id))?;
            if r.project_root == root
                && r.ownership == ProjectOwnership::Platform
                && r.root_identity.matches_captured(expected)
            {
                return Ok(WebImportState::Registered);
            }
            return Err(directory_mapping_error());
        }
        let p = self.reservation_path(id);
        if p.try_exists()? {
            let r: Reservation = read_json(&p)?;
            if r.project_id == id
                && r.task_id == task
                && r.project_root == root
                && r.root_identity
                    .as_ref()
                    .is_none_or(|i| i.matches_captured(expected))
            {
                return Ok(WebImportState::Reserved);
            }
            return Err(directory_mapping_error());
        }
        let owner = crate::web_project_ownership::WebProjectOwnership::new(
            self.data_root(),
            self.workspace.clone(),
        );
        if owner.import_binding_matches(id, task, root, expected)? {
            return Ok(WebImportState::Reserved);
        }
        let directory = owner.directory(id)?;
        if directory.try_exists()? && !owner.path(id)?.try_exists()? {
            if !root.try_exists()?
                && !self.private_dir(id)?.try_exists()?
                && fs::read_dir(directory)?.next().is_none()
            {
                return Ok(WebImportState::Reserved);
            }
            return Err(directory_mapping_error());
        }
        Ok(WebImportState::Absent)
    }
    pub(crate) fn discard_unpublished_project(
        &self,
        id: &str,
        task: &str,
        root: &Path,
        expected: &FileIdentity,
    ) -> Result<(), ApiError> {
        let _lock = self.registration_lock.lock().unwrap();
        match self.web_import_state(id, task, root, expected)? {
            WebImportState::Registered => {
                return Err(ApiError::new(
                    "PROJECT_ALREADY_REGISTERED",
                    "Registered source must be preserved",
                ))
            }
            WebImportState::Absent => return Ok(()),
            WebImportState::Reserved => {}
        }
        let owner = crate::web_project_ownership::WebProjectOwnership::new(
            self.data_root(),
            self.workspace.clone(),
        );
        if owner.path(id)?.try_exists()? {
            owner.retain_import_rollback_binding(id, task, root, expected)?;
        }
        if root.try_exists()? {
            crate::history_cleanup::remove_owned_directory(root, expected)?;
        }
        let private = self.private_dir(id)?;
        if private.try_exists()? {
            let identity = owner
                .instance_identity(id)?
                .ok_or_else(directory_mapping_error)?;
            crate::history_cleanup::remove_owned_directory(&private, &identity)?;
        }
        owner.discard_imported_source(id, root, expected)
    }
    pub(crate) fn registered_source(
        &self,
        root: &Path,
        expected: &FileIdentity,
    ) -> Result<Option<String>, ApiError> {
        for (id, _) in enumerate_project_roots(&self.data_root(), &self.workspace)? {
            let r: Registration = read_json(&registration_path(&self.data_root(), &id))?;
            if r.project_root == root || r.root_identity.matches_captured(expected) {
                if !r.root_identity.matches_captured(expected) {
                    return Err(ApiError::new(
                        "PROJECT_IDENTITY_CHANGED",
                        "已登记的项目目录被替换。",
                    ));
                }
                return Ok(Some(id));
            }
        }
        Ok(None)
    }
    pub async fn register_local(&self, root: &Path) -> Result<ProjectInfo, ApiError> {
        check_no_symlinks(root, false)?;
        let root = root.canonicalize()?;
        let expected = identity(&root)?;
        let manifest = read_manifest(&root)?;
        self.register_local_checked(&root, &expected, &manifest.source_digest)
            .await
    }
    pub(crate) async fn register_local_checked(
        &self,
        root: &Path,
        expected: &FileIdentity,
        digest: &str,
    ) -> Result<ProjectInfo, ApiError> {
        let _lifecycle = self.lifecycle_gate.read().await;
        self.accepting()?;
        self.check_workspace_available()?;
        let id = format!("prj_{}", Uuid::new_v4().simple());
        let admission = {
            let _lock = self.registration_lock.lock().unwrap();
            crate::node_apps::source_ownership::with_source_ownership_admission(|| {
                check_no_symlinks(root, false)?;
                if !expected.matches_path(root).unwrap_or(false)
                    || read_manifest(root)?.source_digest != digest
                {
                    return Err(ApiError::new(
                        "PROJECT_SOURCE_CHANGED",
                        "项目文件夹或panda.json已变化，请重新检查。",
                    ));
                }
                self.require_new_project_location(root)?;
                self.initialize_project_ownership(&id)?;
                crate::web_project_ownership::WebProjectOwnership::new(
                    self.data_root(),
                    self.workspace.clone(),
                )
                .record_user_root(&id, root)?;
                self.write_registration(
                    &id,
                    root,
                    ProjectOwnership::User,
                    Uuid::new_v4().to_string(),
                    None,
                )
            })
        };
        if let Err(error) = admission {
            if self.is_registered(&id).unwrap_or(true) {
                return Err(error
                    .detail("registrationCommitted", true)
                    .detail("projectId", id));
            }
            let owner = crate::web_project_ownership::WebProjectOwnership::new(
                self.data_root(),
                self.workspace.clone(),
            );
            if owner.path(&id)?.try_exists()? {
                let cleanup = (|| {
                    let private = self.private_dir(&id)?;
                    if private.try_exists()? {
                        if let Some(identity) = owner.instance_identity(&id)? {
                            crate::history_cleanup::remove_owned_directory(&private, &identity)?;
                        }
                    }
                    owner.discard_user_registration(&id, root, expected)
                })();
                return Err(error.detail("cleanupError", cleanup.err().map(|e| json!(e))));
            }
            return Err(error);
        }
        self.info(&id).await.map_err(|e| {
            e.detail("registrationCommitted", true)
                .detail("projectId", id)
        })
    }
    pub(crate) async fn register_managed(
        &self,
        id: &str,
        task: &str,
        root: &Path,
    ) -> Result<ProjectInfo, ApiError> {
        self.register_managed_with_source(id, task, root, None)
            .await
    }
    pub(crate) async fn register_managed_git(
        &self,
        id: &str,
        task: &str,
        root: &Path,
        url: &str,
        branch: Option<&str>,
        catalog_id: Option<&str>,
    ) -> Result<ProjectInfo, ApiError> {
        let mut source = json!({"url":url,"branch":branch});
        if let Some(id) = catalog_id {
            source["catalogId"] = json!(id);
        }
        self.register_managed_with_source(id, task, root, Some(source))
            .await
    }
    async fn register_managed_with_source(
        &self,
        id: &str,
        task: &str,
        root: &Path,
        git_source: Option<Value>,
    ) -> Result<ProjectInfo, ApiError> {
        {
            let _lock = self.registration_lock.lock().unwrap();
            self.check_workspace_available()?;
            if self.is_registered(id)? {
                let r: Registration = read_json(&registration_path(&self.data_root(), id))?;
                if r.project_root != root || r.ownership != ProjectOwnership::Platform {
                    return Err(directory_mapping_error());
                }
            } else {
                let r: Reservation = read_json(&self.reservation_path(id))?;
                if r.project_id != id || r.task_id != task || r.project_root != root {
                    return Err(directory_mapping_error());
                }
                if r.root_identity
                    .as_ref()
                    .is_some_and(|i| !i.matches_path(root).unwrap_or(false))
                {
                    return Err(directory_mapping_error());
                }
                read_manifest(root)?;
                crate::web_project_ownership::WebProjectOwnership::new(
                    self.data_root(),
                    self.workspace.clone(),
                )
                .record_project_root(id, root)?;
                self.write_registration(
                    id,
                    root,
                    ProjectOwnership::Platform,
                    Uuid::new_v4().to_string(),
                    git_source,
                )?;
            }
            let path = self.reservation_path(id);
            if path.try_exists()? {
                fs::remove_file(path)?;
            }
        }
        self.info(id).await.map_err(|e| {
            e.detail("registrationCommitted", true)
                .detail("projectId", id)
        })
    }
    fn write_registration(
        &self,
        id: &str,
        root: &Path,
        ownership: ProjectOwnership,
        epoch: String,
        git_source: Option<Value>,
    ) -> Result<(), ApiError> {
        check_no_symlinks(root, false)?;
        if !root.is_dir() {
            return Err(directory_mapping_error());
        }
        ensure_dir(&self.private_dir(id)?)?;
        crate::web_project_ownership::WebProjectOwnership::new(
            self.data_root(),
            self.workspace.clone(),
        )
        .record_instance_root(id)?;
        let address = crate::app_address::reserve_app_address(
            &self.data_root(),
            crate::app_address::AppAddressKind::Web,
            id,
        )?;
        publish_registration(
            &registration_path(&self.data_root(), id),
            &Registration {
                schema_version: 3,
                id: id.into(),
                project_root: root.into(),
                root_identity: identity(root)?,
                ownership,
                document_epoch: epoch,
                workspace_root: self.workspace.clone(),
                address,
                git_source,
                is_agent: read_manifest(root)?.agent.is_some(),
            },
        )
        .map_err(|e| {
            e.detail(
                "registrationCommitted",
                self.is_registered(id).unwrap_or(true),
            )
            .detail("projectId", id)
        })
    }
    pub async fn create(&self, input: CreateProjectInput) -> Result<ProjectInfo, ApiError> {
        let _lifecycle = self.lifecycle_gate.read().await;
        let name = input.name.trim();
        if name.is_empty() || name.chars().count() > 200 {
            return Err(ApiError::new(
                "INVALID_PROJECT_NAME",
                "Project name must contain 1–200 characters",
            ));
        }
        let id = format!("prj_{}", Uuid::new_v4().simple());
        let task = format!("task_{}", Uuid::new_v4().simple());
        let root = self.reserve_managed_project(&id, &task, name)?;
        fs::create_dir(&root)?;
        let expected = identity(&root)?;
        crate::web_project_ownership::WebProjectOwnership::new(
            self.data_root(),
            self.workspace.clone(),
        )
        .record_project_root(&id, &root)?;
        let created=async {
            let app=crate::panda_manifest::AppDecl{name:name.into(),description:input.description,version:None,icon:None};
            if let Some(resources)=&self.resources {
                let manifest=resources.install(&root,&app)?;
                let (_stop,stop)=tokio::sync::watch::channel(false);let (_cancel,cancel)=tokio::sync::watch::channel(false);
                let processes=self.project_processes(&id)?;
                let environment=crate::environment::EnvironmentManager::with_processes(self.data_root(),Some(resources.development_node_root()),processes.clone());
                let context=crate::web_build::WebBuildContext{environment,operation_root:self.private_dir(&id)?.join("create").join(&task),cache_root:self.private_dir(&id)?.join("cache/npm"),processes,owner:resources.clone(),stop,cancel,progress:Arc::new(|_|Ok(()))};
                crate::web_build::WebBuildRunner::run(crate::web_build::WebBuildTarget{project_root:root.clone(),manifest},context).await?;
            } else {
                // A resource-free core fixture has no bundled React template or toolchain.
                let value=json!({"manifestVersion":1,"runtime":"web","app":app,"web":{"root":".","entry":"index.html"},"platform":{"sdkApiVersion":1}});
                write_new(&root.join("panda.json"),&serde_json::to_vec_pretty(&value)?)?;
                write_new(&root.join("index.html"),b"<!doctype html><html><body><main>Panda project</main></body></html>")?;
                write_new(&root.join("README.md"),b"# Panda project\n\nApplication data is in data/.\n")?;
                write_new(&root.join(".gitignore"),b"node_modules/\n# Remove data/ to version application data.\ndata/\n")?;
            }
            self.register_managed(&id,&task,&root).await
        }.await;
        match created {
            Ok(info) => {
                crate::web_project_ownership::WebProjectOwnership::new(
                    self.data_root(),
                    self.workspace.clone(),
                )
                .forget_empty_task(&id, &task)
                .map_err(|e| {
                    e.detail("registrationCommitted", true)
                        .detail("projectId", id.clone())
                })?;
                Ok(info)
            }
            Err(error) => {
                if self.is_registered(&id).unwrap_or(true) {
                    Err(error
                        .detail("registrationCommitted", true)
                        .detail("projectId", id))
                } else {
                    let cleanup = self.discard_unpublished_project(&id, &task, &root, &expected);
                    Err(error
                        .detail("projectId", id)
                        .detail("cleanupError", cleanup.err().map(|e| json!(e))))
                }
            }
        }
    }
    pub(crate) fn storage_root(
        &self,
        id: &str,
    ) -> Result<(PathBuf, Option<PandaManifest>), ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        self.check_not_deleting(id)?;
        let r: Registration = read_json(&registration_path(&self.data_root(), id))?;
        validate_registration(id, &r)?;
        check_no_symlinks(&r.project_root, false)?;
        if !r
            .root_identity
            .matches_path(&r.project_root)
            .unwrap_or(false)
        {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Project directory identity changed",
            ));
        }
        let manifest = PandaManifest::load(&r.project_root).ok();
        Ok((r.project_root, manifest))
    }
    #[cfg(test)]
    pub(crate) fn project_root(&self, id: &str) -> Result<PathBuf, ApiError> {
        resolve_project_root(&self.data_root(), &self.workspace, id)
    }
    pub async fn list(&self) -> Result<Vec<ProjectInfo>, ApiError> {
        let directory = self.data_root().join("web-apps");
        check_no_symlinks(&directory, true)?;
        if !directory.try_exists()? {
            return Ok(vec![]);
        }
        let mut ids = vec![];
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if valid_id(&id) && entry.path().join("registration.json").try_exists()? {
                ids.push(id);
            }
        }
        ids.sort();
        let mut out = vec![];
        for id in ids {
            match self.info(&id).await {
                Ok(info) => out.push(info),
                Err(error) => out.push(ProjectInfo {
                    id: id.clone(),
                    name: id.clone(),
                    description: String::new(),
                    project_root: registration_path(&self.data_root(), &id),
                    document_epoch: String::new(),
                    state: "error".into(),
                    issues: vec![error.message],
                    manifest: None,
                    build_info: json!({}),
                    runtime: PandaRuntime::Web,
                    version: None,
                    ownership: ProjectOwnership::User,
                    source_preserved_on_delete: true,
                    icon_data_url: None,
                    git_source: None,
                    is_agent: false,
                    address: None,
                }),
            }
        }
        Ok(out)
    }
    pub async fn info(&self, id: &str) -> Result<ProjectInfo, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let r: Registration = read_json(&registration_path(&self.data_root(), id))?;
        validate_registration(id, &r)?;
        let mut info = ProjectInfo {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            project_root: r.project_root.clone(),
            document_epoch: r.document_epoch.clone(),
            state: "error".into(),
            issues: vec![],
            manifest: None,
            build_info: json!({}),
            runtime: PandaRuntime::Web,
            version: None,
            ownership: r.ownership,
            source_preserved_on_delete: r.ownership == ProjectOwnership::User,
            icon_data_url: None,
            git_source: None,
            is_agent: self.remembered_agent_hint(&r),
            address: Some(r.address),
        };
        info.git_source = r.git_source.clone();
        match self.project(id) {
            Ok(p) => {
                info.is_agent = p.manifest.agent.is_some();
                // This is display metadata, never registration identity or deletion authority.
                let _ = self.remember_agent_hint(&p.registration, info.is_agent);
                info.name = p.manifest.app.name.clone();
                info.description = p.manifest.app.description.clone().unwrap_or_default();
                info.icon_data_url = p.manifest.icon_data_url(&p.root);
                info.version = p.manifest.app.version.clone();
                let entry = p.manifest.resolve_web_entry(&p.root).is_ok();
                match self.web_configuration_can_start(&p) {
                    Ok(false) => info.state = "needsConfiguration".into(),
                    Ok(true) => {
                        info.state = if entry {
                            "ready"
                        } else if p.manifest.development.is_some() {
                            "needsBuild"
                        } else {
                            "entryMissing"
                        }
                        .into()
                    }
                    Err(error) => {
                        info.state = "error".into();
                        info.issues.push(error.message);
                    }
                }
                info.build_info =
                    json!({"canBuild":p.manifest.development.is_some(),"entryExists":entry});
                info.manifest = Some(p.manifest);
            }
            Err(e) => {
                info.issues.push(e.message);
                if !r.project_root.exists() {
                    info.state = "missing".into();
                }
            }
        }
        Ok(info)
    }
    fn remembered_agent_hint(&self, registration: &Registration) -> bool {
        self.private_dir(&registration.id)
            .and_then(|root| read_json::<AgentHint>(&root.join("agent-hint.json")))
            .map(|hint| hint.is_agent)
            .unwrap_or(registration.is_agent)
    }
    fn remember_agent_hint(&self, observed: &Registration, is_agent: bool) -> Result<(), ApiError> {
        if self.remembered_agent_hint(observed) == is_agent {
            return Ok(());
        }
        let _lock = self.registration_lock.lock().unwrap();
        self.accepting()?;
        self.check_workspace_available()?;
        self.check_not_deleting(&observed.id)?;
        let current: Registration = read_json(&registration_path(&self.data_root(), &observed.id))?;
        validate_registration(&observed.id, &current)?;
        if current.document_epoch != observed.document_epoch
            || current.project_root != observed.project_root
            || current.address != observed.address
            || !current
                .root_identity
                .matches_captured(&observed.root_identity)
            || !identity_matches(&current.root_identity, &current.project_root)?
        {
            return Err(directory_mapping_error());
        }
        crate::builds::atomic_json(
            &self.private_dir(&observed.id)?.join("agent-hint.json"),
            &AgentHint { is_agent },
        )
    }
    pub fn web_address(&self, id: &str) -> Result<crate::FixedAppAddress, ApiError> {
        Ok(self.project(id)?.address)
    }
    pub fn project(&self, id: &str) -> Result<ProjectHandle, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        self.accepting()?;
        self.check_not_deleting(id)?;
        let state_path = registration_path(&self.data_root(), id);
        let r: Registration = read_json(&state_path)?;
        validate_registration(id, &r)?;
        check_no_symlinks(&r.project_root, false)?;
        if !identity_matches(&r.root_identity, &r.project_root)? {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Project directory changed; remove and add it again",
            ));
        }
        crate::app_address::require_app_address(
            &self.data_root(),
            crate::app_address::AppAddressKind::Web,
            id,
            r.address,
        )?;
        let manifest = read_manifest(&r.project_root)?;
        Ok(ProjectHandle {
            id: id.into(),
            root: r.project_root.clone(),
            manifest,
            private_dir: self.private_dir(id)?,
            address: r.address,
            document_epoch: r.document_epoch.clone(),
            operation_gate: self.gate(id),
            registration: r,
            state_path,
            control: self.control(id)?,
            maintenance_id: None,
            diagnostic: false,
            workspace_paused: self.workspace_paused.clone(),
            deletion_path: self.deletion_path(id),
        })
    }
    pub(crate) fn reserve_web_open(&self, id: &str) -> Result<ProjectTaskLease, ApiError> {
        self.accepting()?;
        self.check_workspace_available()?;
        self.project(id)?;
        let mut active = self.active_tasks.lock().unwrap();
        let key = format!("web-open:{id}");
        if active.contains_key(id) || active.contains_key(&key) {
            return Err(ApiError::new(
                "PROJECT_BUSY",
                "项目已有窗口或正在处理任务。",
            ));
        }
        active.insert(key.clone(), ProjectOperation::Transient("webOpen".into()));
        Ok(ProjectTaskLease {
            active: self.active_tasks.clone(),
            id: key,
        })
    }
    pub(crate) fn reserve_web_build(
        &self,
        id: &str,
        task: &str,
    ) -> Result<ProjectTaskLease, ApiError> {
        self.accepting()?;
        self.check_workspace_available()?;
        self.project(id)?;
        let mut active = self.active_tasks.lock().unwrap();
        if active.contains_key(&format!("web-open:{id}")) {
            return Err(ApiError::new(
                "WEB_APP_OPEN",
                "先关闭应用窗口，再构建项目。",
            ));
        }
        if active.contains_key(id) {
            return Err(ApiError::new("PROJECT_BUSY", "项目正在处理其他任务。"));
        }
        active.insert(id.into(), ProjectOperation::Task(task.into()));
        Ok(ProjectTaskLease {
            active: self.active_tasks.clone(),
            id: id.into(),
        })
    }
    async fn submit(
        &self,
        id: &str,
        epoch: Option<&str>,
        work: Work,
        bytes: usize,
        maintenance_id: Option<&str>,
    ) -> Result<WorkResult, ApiError> {
        let result = self
            .submit_snapshot(self.project(id)?, epoch, work, bytes, maintenance_id, false)
            .await;
        let queued = result
            .as_ref()
            .is_err_and(|e| e.code == "SQL_QUEUE_TIMEOUT" || e.code == "SQL_QUEUE_FULL");
        let opened = self
            .active_tasks
            .lock()
            .unwrap()
            .contains_key(&format!("web-open:{id}"));
        if !opened && !queued {
            if let Err(error) = self.close_idle_database(id).await {
                return Err(error.outcome(if result.is_ok() {
                    "committed"
                } else {
                    "unknown"
                }));
            }
        }
        result
    }
    async fn submit_snapshot(
        &self,
        mut project: ProjectHandle,
        epoch: Option<&str>,
        work: Work,
        bytes: usize,
        maintenance_id: Option<&str>,
        snapshot_connection: bool,
    ) -> Result<WorkResult, ApiError> {
        let id = project.id.clone();
        let id = id.as_str();
        if bytes > database::JSON_BODY_MAX_BYTES {
            return Err(ApiError::new(
                "REQUEST_TOO_LARGE",
                "SQL request exceeds the JSON input budget",
            )
            .outcome("not_started"));
        }
        project.maintenance_id = maintenance_id.map(str::to_owned);
        project.diagnostic = epoch.is_none() && matches!(work, Work::Schema);
        project.validate_epoch(epoch)?;
        project.require_sdk()?;
        for attempt in 0..2 {
            let operation = project.operation_gate.clone().read_owned().await;
            project.validate_epoch(epoch)?;
            let retain_connection = snapshot_connection
                || self
                    .active_tasks
                    .lock()
                    .unwrap()
                    .contains_key(&format!("web-open:{id}"));
            let (worker, retired) = {
                let mut workers = self.workers.lock().unwrap();
                self.accepting()?;
                if self.workspace_paused() {
                    return Err(
                        ApiError::new("WORKSPACE_MIGRATING", "Workspace access is paused")
                            .outcome("not_started"),
                    );
                }
                let retired = if workers.get(id).is_none_or(|worker| worker.is_finished()) {
                    workers.insert(id.into(), Arc::new(DatabaseWorker::spawn()?))
                } else {
                    None
                };
                (workers[id].clone(), retired)
            };
            if let Some(worker) = retired {
                let issues = self.join_database_workers(vec![worker]).await;
                if issues.iter().any(|issue| !issue.resource_closed) {
                    self.request_stop();
                    return Err(database_close_error(&issues));
                }
            }
            let result = worker
                .submit(
                    project.clone(),
                    epoch.map(str::to_owned),
                    work.clone(),
                    self.queue.clone(),
                    bytes,
                    database::DatabaseAdmission {
                        retain_connection,
                        operation,
                    },
                )
                .await;
            if attempt == 0
                && result.as_ref().is_err_and(|error| {
                    error.code == "DATABASE_WORKER_STOPPED"
                        && error.details["operationOutcome"] == "not_started"
                })
            {
                let retired = {
                    let mut workers = self.workers.lock().unwrap();
                    if workers
                        .get(id)
                        .is_some_and(|cached| Arc::ptr_eq(cached, &worker))
                    {
                        workers.remove(id)
                    } else {
                        None
                    }
                };
                if let Some(worker) = retired {
                    let issues = self.join_database_workers(vec![worker]).await;
                    if issues.iter().any(|issue| !issue.resource_closed) {
                        self.request_stop();
                        return Err(database_close_error(&issues));
                    }
                }
                continue;
            }
            return result;
        }
        unreachable!()
    }
    pub async fn execute(
        &self,
        id: &str,
        request: SqlRequest,
        epoch: Option<&str>,
        caller: SqlCaller,
    ) -> Result<SqlResult, ApiError> {
        self.execute_managed(id, request, epoch, caller, None).await
    }
    pub async fn execute_managed(
        &self,
        id: &str,
        request: SqlRequest,
        epoch: Option<&str>,
        caller: SqlCaller,
        maintenance_id: Option<&str>,
    ) -> Result<SqlResult, ApiError> {
        let bytes = serde_json::to_vec(&request)?.len();
        let budget = Budget::new(&request.options, caller)?;
        match self
            .submit(
                id,
                epoch,
                Work::Statements(
                    vec![database::SqlStatement {
                        sql: request.sql,
                        params: request.params,
                    }],
                    budget,
                ),
                bytes,
                maintenance_id,
            )
            .await?
        {
            WorkResult::Statements(mut result) => Ok(result.remove(0)),
            _ => unreachable!(),
        }
    }
    pub async fn transaction(
        &self,
        id: &str,
        request: TransactionRequest,
        epoch: Option<&str>,
        caller: SqlCaller,
    ) -> Result<Vec<SqlResult>, ApiError> {
        self.transaction_managed(id, request, epoch, caller, None)
            .await
    }
    pub async fn transaction_managed(
        &self,
        id: &str,
        request: TransactionRequest,
        epoch: Option<&str>,
        caller: SqlCaller,
        maintenance_id: Option<&str>,
    ) -> Result<Vec<SqlResult>, ApiError> {
        if request.statements.is_empty()
            || request.statements.len() > database::TRANSACTION_MAX_STATEMENTS
        {
            return Err(ApiError::new(
                "INVALID_TRANSACTION",
                "A transaction requires 1 to 100 statements",
            )
            .outcome("not_started"));
        }
        let bytes = serde_json::to_vec(&request)?.len();
        let budget = Budget::new(&request.options, caller)?;
        match self
            .submit(
                id,
                epoch,
                Work::Statements(request.statements, budget),
                bytes,
                maintenance_id,
            )
            .await?
        {
            WorkResult::Statements(results) => Ok(results),
            _ => unreachable!(),
        }
    }
    pub async fn schema(&self, id: &str, epoch: Option<&str>) -> Result<SchemaInfo, ApiError> {
        match self.submit(id, epoch, Work::Schema, 0, None).await? {
            WorkResult::Schema(schema) => Ok(schema),
            _ => unreachable!(),
        }
    }
    pub(crate) async fn execute_snapshot(
        &self,
        project: &ProjectHandle,
        request: SqlRequest,
        epoch: Option<&str>,
        caller: SqlCaller,
    ) -> Result<SqlResult, ApiError> {
        let bytes = serde_json::to_vec(&request)?.len();
        let budget = Budget::new(&request.options, caller)?;
        match self
            .submit_snapshot(
                project.clone(),
                epoch,
                Work::Statements(
                    vec![database::SqlStatement {
                        sql: request.sql,
                        params: request.params,
                    }],
                    budget,
                ),
                bytes,
                None,
                true,
            )
            .await?
        {
            WorkResult::Statements(mut r) => Ok(r.remove(0)),
            _ => unreachable!(),
        }
    }
    pub(crate) async fn transaction_snapshot(
        &self,
        project: &ProjectHandle,
        request: TransactionRequest,
        epoch: Option<&str>,
        caller: SqlCaller,
    ) -> Result<Vec<SqlResult>, ApiError> {
        if request.statements.is_empty()
            || request.statements.len() > database::TRANSACTION_MAX_STATEMENTS
        {
            return Err(ApiError::new(
                "INVALID_TRANSACTION",
                "A transaction requires 1 to 100 statements",
            )
            .outcome("not_started"));
        }
        let bytes = serde_json::to_vec(&request)?.len();
        let budget = Budget::new(&request.options, caller)?;
        match self
            .submit_snapshot(
                project.clone(),
                epoch,
                Work::Statements(request.statements, budget),
                bytes,
                None,
                true,
            )
            .await?
        {
            WorkResult::Statements(r) => Ok(r),
            _ => unreachable!(),
        }
    }
    pub(crate) async fn schema_snapshot(
        &self,
        project: &ProjectHandle,
        epoch: Option<&str>,
        _caller: SqlCaller,
    ) -> Result<SchemaInfo, ApiError> {
        match self
            .submit_snapshot(project.clone(), epoch, Work::Schema, 0, None, true)
            .await?
        {
            WorkResult::Schema(r) => Ok(r),
            _ => unreachable!(),
        }
    }
    pub(crate) fn control(
        &self,
        id: &str,
    ) -> Result<Arc<crate::control::ProjectControl>, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        if !registration_path(&self.data_root(), id).try_exists()?
            && !self.reservation_path(id).try_exists()?
            && !self.deletion_path(id).try_exists()?
        {
            return Err(removed_project_error(id));
        }
        let mut controls = self.controls.lock().unwrap();
        if let Some(control) = controls.get(id) {
            return Ok(control.clone());
        }
        let control = crate::control::ProjectControl::load_private(&self.private_dir(id)?)?;
        controls.insert(id.into(), control.clone());
        Ok(control)
    }
    pub(crate) fn control_for_deletion(
        &self,
        id: &str,
    ) -> Result<Arc<crate::control::ProjectControl>, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        if !registration_path(&self.data_root(), id).try_exists()?
            && !self.reservation_path(id).try_exists()?
            && !self.deletion_path(id).try_exists()?
        {
            return Err(removed_project_error(id));
        }
        let mut controls = self.controls.lock().unwrap();
        if let Some(control) = controls.get(id) {
            return Ok(control.clone());
        }
        match crate::control::ProjectControl::load_private(&self.private_dir(id)?) {
            Ok(control) => {
                controls.insert(id.into(), control.clone());
                Ok(control)
            }
            Err(error) if error.code == "INVALID_JSON" => {
                crate::control::ProjectControl::volatile_private_for_deletion(
                    &self.private_dir(id)?,
                )
            }
            Err(error) => Err(error),
        }
    }
    pub(crate) fn gate(&self, id: &str) -> Arc<RwLock<()>> {
        self.gates
            .lock()
            .unwrap()
            .entry(id.into())
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone()
    }
    pub(crate) fn active_task(&self, id: &str) -> Option<String> {
        match self.active_tasks.lock().unwrap().get(id) {
            Some(ProjectOperation::Task(task)) => Some(task.clone()),
            _ => None,
        }
    }
    pub(crate) fn reserve_task(&self, id: &str, task: &str) -> Result<ProjectTaskLease, ApiError> {
        // Real project keys always receive the development gate, including the
        // target lease of a lifecycle copy. Source/Node task namespaces retain
        // their existing non-project coordination without inspecting user paths.
        if valid_id(id) {
            return self.reserve_development_task(id, task);
        }
        self.reserve_operation(id, ProjectOperation::Task(task.into()))
    }
    pub(crate) fn check_development_available(&self, id: &str) -> Result<(), ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        self.check_not_deleting(id)?;

        Ok(())
    }
    pub(crate) fn reserve_development_task(
        &self,
        id: &str,
        task: &str,
    ) -> Result<ProjectTaskLease, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let lease = self.reserve_operation(id, ProjectOperation::Task(task.into()))?;
        self.check_development_available(id)?;
        Ok(lease)
    }
    pub(crate) fn reserve_delete_task(
        &self,
        id: &str,
        task: &str,
    ) -> Result<ProjectTaskLease, ApiError> {
        let lease = self.reserve_operation(id, ProjectOperation::Task(task.into()))?;
        self.deletion_root(id)?;
        Ok(lease)
    }
    pub(crate) fn reserve_transient_operation(
        &self,
        id: &str,
        label: &str,
    ) -> Result<ProjectTaskLease, ApiError> {
        self.reserve_operation(id, ProjectOperation::Transient(label.into()))
    }
    fn reserve_operation(
        &self,
        id: &str,
        operation: ProjectOperation,
    ) -> Result<ProjectTaskLease, ApiError> {
        self.accepting()?;
        let mut active = self.active_tasks.lock().unwrap();
        self.check_workspace_available()?;
        if let Some(operation) = active.get(id) {
            let error = ApiError::new(
                "PROJECT_BUSY",
                "This project already has an active operation",
            );
            return Err(match operation {
                ProjectOperation::Task(task) => error.detail("taskId", task.clone()),
                ProjectOperation::Transient(label) => error.detail("operation", label.clone()),
            });
        }
        let _writer_owner = if id.starts_with("prj_") {
            self.node_source_data_root
                .lock()
                .unwrap()
                .as_ref()
                .map(|data| crate::durable_writer::WriterOwner::web(data, id))
                .transpose()?
        } else {
            None
        };
        active.insert(id.into(), operation);
        Ok(ProjectTaskLease {
            active: self.active_tasks.clone(),
            id: id.into(),
        })
    }
    #[cfg(test)]
    pub(crate) fn fixture_has_database_worker(&self, id: &str) -> bool {
        self.workers.lock().unwrap().contains_key(id)
    }
    #[cfg(test)]
    pub(crate) fn fixture_panicked_database_worker(&self, id: &str) {
        let project = self.project(id).unwrap();
        let worker = DatabaseWorker::fixture_panicked(project.database_path().unwrap());
        assert!(self
            .workers
            .lock()
            .unwrap()
            .insert(id.into(), Arc::new(worker))
            .is_none());
    }
    /// Inject a genuine failed SQLite join without opening a recovery-protected
    /// live slot. The caller owns the separate disposable database fixture.
    async fn close_idle_database(&self, id: &str) -> Result<(), ApiError> {
        let gate = self.gate(id);
        let _drain = gate.write().await;
        if self
            .active_tasks
            .lock()
            .unwrap()
            .contains_key(&format!("web-open:{id}"))
        {
            return Ok(());
        }
        self.close_database(id).await
    }
    /// Stop browser/session admission before calling this lifecycle boundary.
    /// It drains accepted SQL/file operations and joins the SQLite worker.
    pub async fn drain_and_close_database(&self, id: &str) -> Result<(), ApiError> {
        let gate = self.gate(id);
        let _drain = gate.write().await;
        self.close_database(id).await
    }
    pub(crate) async fn close_database(&self, id: &str) -> Result<(), ApiError> {
        let worker = self.workers.lock().unwrap().remove(id);
        if let Some(worker) = worker {
            worker.stop();
            let issues = self.join_database_workers(vec![worker]).await;
            if !issues.is_empty() {
                return Err(database_close_error(&issues));
            }
        }
        Ok(())
    }
    async fn join_database_workers(
        &self,
        workers: Vec<Arc<DatabaseWorker>>,
    ) -> Vec<crate::shutdown::ShutdownIssue> {
        let issues = match tokio::task::spawn_blocking(move || {
            workers
                .into_iter()
                .filter_map(|worker| {
                    worker.join().err().map(|error| {
                        crate::shutdown::ShutdownIssue::new(
                            "database",
                            "DATABASE_THREAD_JOIN_FAILED",
                            error,
                            true,
                            false,
                        )
                    })
                })
                .collect::<Vec<_>>()
        })
        .await
        {
            Ok(issues) => issues,
            Err(error) => vec![crate::shutdown::ShutdownIssue::join(
                "databaseJoinWorker",
                error,
            )],
        };
        self.shutdown_issues.lock().unwrap().extend(issues.clone());
        issues
    }
    pub(crate) fn check_workspace_available(&self) -> Result<(), ApiError> {
        if self.workspace_migrating.load(Ordering::Acquire) {
            Err(ApiError::new(
                "WORKSPACE_MIGRATING",
                "A workspace migration is in progress",
            )
            .outcome("not_started"))
        } else {
            Ok(())
        }
    }
    pub(crate) fn workspace_paused(&self) -> bool {
        self.workspace_paused.load(Ordering::Acquire)
    }
    pub(crate) fn begin_workspace_migration(&self) -> Result<(), ApiError> {
        self.accepting()?;

        let _active = self.active_tasks.lock().unwrap();
        let _registration = self.registration_lock.lock().unwrap();
        let pending = self
            .pending_node_deletions
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if !pending.is_empty() {
            return Err(ApiError::new(
                "NODE_DELETION_PENDING",
                "还有未完成的应用删除任务，请先完成或在允许时放弃，再迁移或清理工作区。",
            )
            .detail(
                "appIds",
                serde_json::json!(pending.values().collect::<Vec<_>>()),
            ));
        }
        check_pending_deletions(&self.data_root())?;
        if self.workspace_migrating.swap(true, Ordering::AcqRel) {
            return Err(ApiError::new(
                "WORKSPACE_MIGRATING",
                "A workspace migration is already in progress",
            ));
        }
        Ok(())
    }
    pub(crate) fn reserve_node_deletion_barrier(
        &self,
        app_id: &str,
    ) -> Result<NodeDeletionBarrier, ApiError> {
        let mut pending = self
            .pending_node_deletions
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        self.accepting()?;
        self.check_workspace_available()?;
        let token = uuid::Uuid::new_v4().to_string();
        pending.insert(token.clone(), app_id.into());
        Ok(NodeDeletionBarrier {
            pending: self.pending_node_deletions.clone(),
            token,
        })
    }
    pub(crate) fn has_active_tasks(&self) -> bool {
        self.active_tasks
            .lock()
            .unwrap()
            .keys()
            .any(|key| !key.starts_with("web-open:"))
    }
    pub(crate) async fn pause_workspace(&self) -> Result<(), ApiError> {
        let _lifecycle = self.lifecycle_gate.write().await;
        self.workspace_paused.store(true, Ordering::Release);
        let workers = self
            .workers
            .lock()
            .unwrap()
            .drain()
            .map(|(_, worker)| worker)
            .collect::<Vec<_>>();
        for worker in &workers {
            worker.stop();
        }
        let issues = self.join_database_workers(workers).await;
        if !issues.is_empty() {
            self.stopping.store(true, Ordering::Release);
            return Err(database_close_error(&issues));
        }
        Ok(())
    }
    pub(crate) fn resume_workspace(&self) {
        self.workspace_paused.store(false, Ordering::Release);
        self.workspace_migrating.store(false, Ordering::Release);
    }
    pub(crate) fn request_stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.request_stop();
        let _lifecycle = self.lifecycle_gate.write().await;
        let workers = self
            .workers
            .lock()
            .unwrap()
            .drain()
            .map(|(_, worker)| worker)
            .collect::<Vec<_>>();
        for worker in &workers {
            worker.stop();
        }
        self.join_database_workers(workers).await;
        self.shutdown_issues.lock().unwrap().clone()
    }

    fn deletion_path(&self, id: &str) -> PathBuf {
        self.data_root()
            .join("web-apps")
            .join(id)
            .join("deleting.json")
    }
    pub(crate) fn pending_deletion(&self, id: &str) -> Result<Option<ProjectDeletion>, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let p = self.deletion_path(id);
        if !p.try_exists()? {
            return Ok(None);
        }
        let m: ProjectDeletion = read_json(&p)?;
        if m.schema_version != 2
            || m.project_id != id
            || !valid_id(&m.task_id)
            || Uuid::parse_str(&m.target_epoch).is_err()
            || !m.project_root.is_absolute()
            || m.project_root
                .components()
                .any(|p| matches!(p, Component::ParentDir | Component::CurDir))
        {
            return Err(directory_mapping_error());
        }
        Ok(Some(m))
    }
    fn check_not_deleting(&self, id: &str) -> Result<(), ApiError> {
        check_deletion_marker(&self.deletion_path(id))
    }
    pub(crate) fn deletion_root(&self, id: &str) -> Result<(PathBuf, FileIdentity), ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let path = registration_path(&self.data_root(), id);
        let registered = if path.try_exists()? {
            let r: Registration = read_json(&path)?;
            validate_registration(id, &r)?;
            Some(r)
        } else {
            None
        };
        let marker = self.pending_deletion(id)?;
        let (root, identity, ownership) = if let Some(m) = marker {
            if let Some(r) = registered {
                if r.project_root != m.project_root
                    || r.ownership != m.ownership
                    || !r.root_identity.matches_captured(&m.root_identity)
                    || !(m.source_epoch.as_deref() == Some(r.document_epoch.as_str())
                        || m.target_epoch == r.document_epoch)
                {
                    return Err(directory_mapping_error());
                }
            }
            (m.project_root, m.root_identity, m.ownership)
        } else {
            let r = registered.ok_or_else(|| removed_project_error(id))?;
            (r.project_root, r.root_identity, r.ownership)
        };
        // User-folder deletion only retires this instance. Its current path may
        // be absent, replaced or linked; none of those objects is adopted.
        if ownership == ProjectOwnership::Platform {
            let owner = crate::web_project_ownership::WebProjectOwnership::new(
                self.data_root(),
                self.workspace.clone(),
            );
            if !owner
                .material_identity(id, &root)?
                .as_ref()
                .is_some_and(|bound| bound.matches_captured(&identity))
            {
                return Err(directory_mapping_error());
            }
            check_no_symlinks(&root, true)?;
            if root.try_exists()? && !identity_matches(&identity, &root)? {
                return Err(directory_mapping_error());
            }
        }
        Ok((root, identity))
    }
    pub(crate) fn project_ownership(&self, id: &str) -> Result<ProjectOwnership, ApiError> {
        if let Some(m) = self.pending_deletion(id)? {
            return Ok(m.ownership);
        }
        Ok(read_json::<Registration>(&registration_path(&self.data_root(), id))?.ownership)
    }
    pub(crate) fn validate_deletion_epoch(
        &self,
        id: &str,
        expected: Option<&str>,
    ) -> Result<(), ApiError> {
        self.deletion_root(id)?;
        let m = self.pending_deletion(id)?;
        let current = self.registered_epoch(id)?;
        if expected.is_some_and(|e| {
            current.as_deref() == Some(e)
                || m.as_ref()
                    .is_some_and(|m| m.source_epoch.as_deref() == Some(e) || m.target_epoch == e)
        }) {
            Ok(())
        } else {
            Err(ApiError::new(
                "PROJECT_DELETE_CONFIRMATION_CHANGED",
                "Read the current project and confirm its deletion again",
            )
            .outcome("not_started"))
        }
    }
    pub(crate) fn begin_deletion(
        &self,
        id: &str,
        task: &str,
        epoch: &str,
        expected: &FileIdentity,
    ) -> Result<ProjectDeletion, ApiError> {
        let (root, actual) = self.deletion_root(id)?;
        if !expected.matches_captured(&actual) {
            return Err(directory_mapping_error());
        }
        let m = ProjectDeletion {
            schema_version: 2,
            project_id: id.into(),
            task_id: task.into(),
            target_epoch: epoch.into(),
            root_identity: expected.clone(),
            project_root: root,
            ownership: self.project_ownership(id)?,
            source_epoch: self.registered_epoch(id)?,
        };
        crate::builds::atomic_json(&self.deletion_path(id), &m)?;
        Ok(m)
    }
    /// A registration file exists but this version cannot read it (older
    /// format or damaged). Such a project is still deletable: its platform
    /// data is removed by ID and its folder is kept.
    pub(crate) fn registration_unreadable(&self, id: &str) -> Result<bool, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let path = registration_path(&self.data_root(), id);
        if !path.try_exists()? {
            return Ok(false);
        }
        Ok(read_json::<Registration>(&path)
            .and_then(|record| validate_registration(id, &record))
            .is_err())
    }
    /// Removes an unreadable registration so the remaining platform data can
    /// be finalized by ID. The project folder it named is never touched.
    pub(crate) fn retire_unreadable_registration(&self, id: &str) -> Result<(), ApiError> {
        let _lock = self.registration_lock.lock().unwrap();
        if !self.registration_unreadable(id)? {
            return Ok(());
        }
        if self.workers.lock().unwrap().contains_key(id) || self.has_active_project_work(id) {
            return Err(ApiError::new(
                "PROJECT_BUSY",
                "Project work has not finished.",
            ));
        }
        fs::remove_file(registration_path(&self.data_root(), id))?;
        Ok(())
    }
    pub(crate) fn deleted_project_identity_absent(&self, id: &str) -> Result<bool, ApiError> {
        Ok(!self.is_registered(id)? && !self.reservation_path(id).try_exists()?)
    }
    pub(crate) fn has_active_project_work(&self, id: &str) -> bool {
        self.active_tasks.lock().unwrap().contains_key(id)
    }
    pub(crate) fn on_project_identity_removed(&self, observer: ProjectIdentityRemoved) {
        self.identity_removed.lock().unwrap().push(observer);
    }
    pub(crate) fn complete_deleted_project_identity(
        &self,
        m: &ProjectDeletion,
    ) -> Result<(), ApiError> {
        let _lock = self.registration_lock.lock().unwrap();
        let id = &m.project_id;
        if self.workers.lock().unwrap().contains_key(id) || self.has_active_project_work(id) {
            return Err(ApiError::new(
                "PROJECT_BUSY",
                "Project writers must finish before removing its identity",
            ));
        }
        if m.ownership == ProjectOwnership::Platform && m.project_root.try_exists()? {
            return Err(ApiError::new(
                "PROJECT_DELETE_INCOMPLETE",
                "Project files remain",
            ));
        }
        if let Some(current) = self.pending_deletion(id)? {
            if current.task_id != m.task_id
                || current.target_epoch != m.target_epoch
                || !current.root_identity.matches_captured(&m.root_identity)
            {
                return Err(directory_mapping_error());
            }
        }
        for p in [
            registration_path(&self.data_root(), id),
            self.reservation_path(id),
        ] {
            if p.try_exists()? {
                fs::remove_file(&p)?;
            }
        }
        self.controls.lock().unwrap().remove(id);
        self.gates.lock().unwrap().remove(id);
        for observer in self.identity_removed.lock().unwrap().clone() {
            observer(id);
        }
        let p = self.deletion_path(id);
        if p.try_exists()? {
            fs::remove_file(p)?;
        }
        Ok(())
    }
    pub(crate) fn registered_epoch(&self, id: &str) -> Result<Option<String>, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let p = registration_path(&self.data_root(), id);
        if !p.try_exists()? {
            return Ok(None);
        }
        let r: Registration = read_json(&p)?;
        validate_registration(id, &r)?;
        Ok(Some(r.document_epoch))
    }
    pub(crate) fn with_registered_epoch<T>(
        &self,
        id: &str,
        epoch: &str,
        admit: impl FnOnce() -> Result<T, ApiError>,
    ) -> Result<Option<T>, ApiError> {
        let _l = self.registration_lock.lock().unwrap();
        if self.registered_epoch(id)?.as_deref() != Some(epoch) {
            return Ok(None);
        }
        admit().map(Some)
    }
    pub(crate) fn rebind_migrated(&self, id: &str, epoch: &str) -> Result<(), ApiError> {
        let p = registration_path(&self.data_root(), id);
        let mut r: Registration = read_json(&p)?;
        validate_registration(id, &r)?;
        if r.ownership == ProjectOwnership::Platform {
            let relative = r
                .project_root
                .strip_prefix(&r.workspace_root)
                .map_err(|_| directory_mapping_error())?
                .to_path_buf();
            r.project_root = self.workspace.join(relative);
            if r.project_root.try_exists()? {
                r.root_identity = identity(&r.project_root)?;
            }
        }
        r.workspace_root = self.workspace.clone();
        r.document_epoch = epoch.into();
        crate::builds::atomic_json(&p, &r)
    }
    pub(crate) fn retain_migrated_diagnostic(
        &self,
        id: &str,
        error: ApiError,
    ) -> Result<(), ApiError> {
        let c = self.control(id)?;
        let mut s = c.get();
        s.last_error = Some(error);
        c.set(s)
    }
}
fn database_close_error(issues: &[crate::shutdown::ShutdownIssue]) -> ApiError {
    ApiError::new("DATABASE_CLOSE_FAILED", "数据库线程未能确认正常关闭")
        .detail("shutdownIssues", json!(issues))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebImportState {
    Absent,
    Reserved,
    Registered,
}
enum ProjectOperation {
    Task(String),
    Transient(String),
}
pub(crate) struct ProjectTaskLease {
    active: Arc<Mutex<HashMap<String, ProjectOperation>>>,
    id: String,
}
impl Drop for ProjectTaskLease {
    fn drop(&mut self) {
        self.active.lock().unwrap().remove(&self.id);
    }
}
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_lowercase()
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}
pub(crate) fn validate_manifest(m: &PandaManifest, _id: &str, root: &Path) -> Result<(), ApiError> {
    if m.runtime != PandaRuntime::Web {
        return Err(ApiError::new(
            "APP_RUNTIME_CHANGED",
            "应用类型变了，请删除后重新添加。",
        ));
    }
    m.inspect_local(root)
        .map(|_| ())
        .map_err(|e| ApiError::new("INVALID_PROJECT_MANIFEST", e.to_string()))
}
fn read_manifest(root: &Path) -> Result<PandaManifest, ApiError> {
    let m = PandaManifest::load(root)
        .map_err(|e| ApiError::new("INVALID_PROJECT_MANIFEST", e.to_string()))?;
    validate_manifest(&m, "", root)?;
    Ok(m)
}
fn identity(path: &Path) -> Result<FileIdentity, ApiError> {
    FileIdentity::capture(path).map_err(|e| ApiError::new("FILESYSTEM_ERROR", e.to_string()))
}
fn identity_matches(expected: &FileIdentity, path: &Path) -> Result<bool, ApiError> {
    expected
        .matches_path(path)
        .map_err(|e| ApiError::new("FILESYSTEM_ERROR", e.to_string()))
}
fn check_deletion_marker(path: &Path) -> Result<(), ApiError> {
    if path.try_exists()? {
        Err(ApiError::new(
            "PROJECT_DELETING",
            "Project deletion is pending",
        ))
    } else {
        Ok(())
    }
}
fn removed_project_error(id: &str) -> ApiError {
    ApiError::new(
        "PROJECT_REMOVED",
        "Project instance is no longer registered",
    )
    .detail("projectId", id)
}
pub(crate) fn check_pending_deletions(data: &Path) -> Result<(), ApiError> {
    let root = data.join("web-apps");
    if root.is_dir() {
        for e in fs::read_dir(root)? {
            if e?.path().join("deleting.json").try_exists()? {
                return Err(ApiError::new(
                    "PROJECT_DELETION_PENDING",
                    "A project deletion is pending",
                ));
            }
        }
    }
    Ok(())
}
pub fn checked_relative(
    root: &Path,
    relative: &str,
    allow_missing: bool,
) -> Result<PathBuf, ApiError> {
    let p = Path::new(relative);
    if p.is_absolute()
        || p.components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(ApiError::new(
            "UNSAFE_PATH",
            "Expected project-relative path",
        ));
    }
    let path = root.join(p);
    check_no_symlinks(&path, allow_missing)?;
    Ok(path)
}
pub use crate::path_checks::check_no_symlinks;
fn ensure_dir(path: &Path) -> Result<(), ApiError> {
    crate::project_file_io::ensure_directory(path)
}
fn ensure_user_directory(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    if path.try_exists()? {
        if !path.is_dir() {
            return Err(ApiError::new("UNSAFE_PATH", "Expected data directory"));
        }
    } else {
        fs::create_dir(path)?;
    }
    check_no_symlinks(path, false)
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ApiError> {
    check_no_symlinks(path, false)?;
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|e| ApiError::new("INVALID_JSON", e.to_string()))
}
fn write_new(path: &Path, content: &[u8]) -> Result<(), ApiError> {
    let mut f = crate::project_file_io::create_new_file(path)?;
    f.write_all(content)?;
    f.sync_all()?;
    Ok(())
}
fn publish_registration(path: &Path, r: &Registration) -> Result<(), ApiError> {
    crate::builds::atomic_json(path, r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SqlOptions, SqlStatement};
    fn store() -> (tempfile::TempDir, ProjectStore) {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().canonicalize().unwrap();
        let store = ProjectStore::new(workspace).unwrap();
        (temp, store)
    }
    async fn create(store: &ProjectStore, name: &str) -> ProjectInfo {
        let info = store
            .create(CreateProjectInput {
                name: name.into(),
                description: None,
            })
            .await
            .unwrap();
        store.schema(&info.id, None).await.unwrap();
        store.close_database(&info.id).await.unwrap();
        info
    }
    #[tokio::test]
    async fn agent_hint_tracks_valid_edits_and_survives_missing_source_without_rebinding() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let workspace = root.join("platform/workspace");
        fs::create_dir_all(&workspace).unwrap();
        let store = ProjectStore::new(workspace).unwrap();
        let source = root.join("disposable-agent-source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("index.html"), "disposable").unwrap();
        fs::write(source.join("AGENTS.md"), "Disposable agent instructions").unwrap();
        let mut manifest = json!({
            "manifestVersion":1,"runtime":"web","app":{"name":"Disposable agent"},
            "web":{"root":".","entry":"index.html"},"agent":{"instructions":"AGENTS.md"}
        });
        fs::write(
            source.join("panda.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let original = store.register_local(&source).await.unwrap();
        assert!(original.is_agent);
        assert_eq!(original.ownership, ProjectOwnership::User);
        let registration = registration_path(&store.data_root(), &original.id);
        let before = fs::read(&registration).unwrap();
        let mut legacy: Value = serde_json::from_slice(&before).unwrap();
        assert_eq!(legacy["isAgent"], true);
        legacy.as_object_mut().unwrap().remove("isAgent");
        assert!(
            !serde_json::from_value::<Registration>(legacy)
                .unwrap()
                .is_agent
        );

        manifest.as_object_mut().unwrap().remove("agent");
        fs::write(
            source.join("panda.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(!store.info(&original.id).await.unwrap().is_agent);
        assert!(!store.list().await.unwrap()[0].is_agent);
        assert_eq!(fs::read(&registration).unwrap(), before);
        let moved = root.join("disposable-moved-agent-source");
        fs::rename(&source, &moved).unwrap();
        let missing = store.info(&original.id).await.unwrap();
        assert_eq!(missing.state, "missing");
        assert!(!missing.is_agent);
        assert_eq!(missing.document_epoch, original.document_epoch);
        assert_eq!(missing.ownership, ProjectOwnership::User);
        assert!(missing.source_preserved_on_delete);

        fs::rename(&moved, &source).unwrap();
        manifest["agent"] = json!({"instructions":"AGENTS.md"});
        fs::write(
            source.join("panda.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(store.info(&original.id).await.unwrap().is_agent);
        fs::remove_file(source.join("AGENTS.md")).unwrap();
        let invalid = store.info(&original.id).await.unwrap();
        assert!(invalid.manifest.is_none());
        assert!(invalid.is_agent);
        assert_eq!(fs::read(&registration).unwrap(), before);
        store.shutdown().await;
    }
    async fn exec(
        store: &ProjectStore,
        id: &str,
        sql: &str,
        params: Vec<Value>,
    ) -> Result<SqlResult, ApiError> {
        store
            .execute(
                id,
                SqlRequest {
                    sql: sql.into(),
                    params,
                    options: SqlOptions::default(),
                },
                None,
                SqlCaller::Management,
            )
            .await
    }
    #[tokio::test]
    async fn projects_are_independent_and_survive_reopening() {
        let (temp, store) = store();
        let a = create(&store, "English").await;
        let b = create(&store, "Math").await;
        assert_eq!(a.state, "ready");
        assert!(a.project_root.join("README.md").is_file());
        for project in [&a, &b] {
            exec(
                &store,
                &project.id,
                "CREATE TABLE notes(id INTEGER PRIMARY KEY, value TEXT NOT NULL)",
                vec![],
            )
            .await
            .unwrap();
        }
        exec(
            &store,
            &a.id,
            "INSERT INTO notes(value) VALUES (?)",
            vec![json!("hello")],
        )
        .await
        .unwrap();
        assert!(exec(&store, &b.id, "SELECT * FROM notes", vec![])
            .await
            .unwrap()
            .rows
            .is_empty());
        let schema = store.schema(&a.id, None).await.unwrap();
        assert_eq!(schema["tables"][0]["columns"][1]["name"], "value");
        let epoch = a.document_epoch.clone();
        store.shutdown().await;
        drop(store);
        let reopened = ProjectStore::new(temp.path().canonicalize().unwrap()).unwrap();
        assert_eq!(reopened.info(&a.id).await.unwrap().document_epoch, epoch);
        assert_eq!(
            exec(&reopened, &a.id, "SELECT value FROM notes", vec![])
                .await
                .unwrap()
                .rows[0]["value"],
            "hello"
        );
    }

    #[tokio::test]
    async fn transaction_and_returning_budget_failures_roll_back() {
        let (_temp, store) = store();
        let p = create(&store, "Rollback").await;
        exec(
            &store,
            &p.id,
            "CREATE TABLE notes(id INTEGER PRIMARY KEY, value TEXT UNIQUE)",
            vec![],
        )
        .await
        .unwrap();
        let request = TransactionRequest {
            statements: vec![
                SqlStatement {
                    sql: "INSERT INTO notes(value) VALUES ('a')".into(),
                    params: vec![],
                },
                SqlStatement {
                    sql: "INSERT INTO notes(value) VALUES ('a')".into(),
                    params: vec![],
                },
            ],
            options: SqlOptions::default(),
        };
        let error = store
            .transaction(&p.id, request, None, SqlCaller::Management)
            .await
            .unwrap_err();
        assert_eq!(error.details["operationOutcome"], "rolled_back");
        let request = SqlRequest {
            sql: "INSERT INTO notes(value) VALUES ('a'),('b') RETURNING value".into(),
            params: vec![],
            options: SqlOptions {
                max_rows: Some(1),
                timeout_ms: None,
            },
        };
        let error = store
            .execute(&p.id, request, None, SqlCaller::Browser)
            .await
            .unwrap_err();
        assert_eq!(error.code, "SQL_RESULT_LIMIT_EXCEEDED");
        assert_eq!(error.details["operationOutcome"], "rolled_back");
        assert_eq!(
            exec(&store, &p.id, "SELECT count(*) AS count FROM notes", vec![])
                .await
                .unwrap()
                .rows[0]["count"],
            0
        );
    }
    #[tokio::test]
    async fn values_roundtrip_without_integer_or_blob_loss() {
        let (_temp, store) = store();
        let p = create(&store, "Values").await;
        let row = exec(
            &store,
            &p.id,
            "SELECT ? AS integer, ? AS bytes, ? AS flag",
            vec![
                json!({"$int64":"9223372036854775807"}),
                json!({"$bytes":"AP8BAg=="}),
                json!(true),
            ],
        )
        .await
        .unwrap()
        .rows
        .remove(0);
        assert_eq!(row["integer"], json!({"$int64":"9223372036854775807"}));
        assert_eq!(row["bytes"], json!({"$bytes":"AP8BAg=="}));
        assert_eq!(row["flag"], 1);
        for value in [
            json!({"$int64":"9223372036854775808"}),
            json!(9007199254740992_i64),
            json!({"$bytes":"%%%"}),
            json!({"not":"sql"}),
        ] {
            assert_eq!(
                exec(&store, &p.id, "SELECT ?", vec![value])
                    .await
                    .unwrap_err()
                    .code,
                "INVALID_SQL_VALUE"
            );
        }
        assert_eq!(
            exec(&store, &p.id, "SELECT 1 AS same, 2 AS same", vec![])
                .await
                .unwrap_err()
                .code,
            "SQL_DUPLICATE_COLUMN"
        );
    }
    #[tokio::test]
    async fn authorizer_blocks_connection_escape_and_preserves_configuration() {
        let (_temp, store) = store();
        let p = create(&store, "Guard").await;
        for sql in [
            "/* hidden */ ATTACH DATABASE ':memory:' AS other",
            "DETACH main",
            "BEGIN",
            "COMMIT",
            "SAVEPOINT p",
            "PRAGMA foreign_keys=OFF",
            "PRAGMA journal_mode=DELETE",
            "PRAGMA writable_schema=ON",
            "SELECT load_extension('/tmp/nope')",
            "CREATE TEMP TABLE bad(x)",
            "VACUUM INTO '/tmp/platform-should-never-write.sqlite'",
        ] {
            assert!(
                exec(&store, &p.id, sql, vec![]).await.is_err(),
                "must reject {sql}"
            );
        }
        assert_eq!(
            exec(&store, &p.id, "PRAGMA foreign_keys", vec![])
                .await
                .unwrap()
                .rows[0]["foreign_keys"],
            1
        );
        assert_eq!(
            exec(&store, &p.id, "PRAGMA journal_mode", vec![])
                .await
                .unwrap()
                .rows[0]["journal_mode"],
            "wal"
        );
        assert!(exec(&store, &p.id, "SELECT 1; SELECT 2", vec![])
            .await
            .is_err());
        exec(
            &store,
            &p.id,
            "CREATE TABLE good(id INTEGER PRIMARY KEY)",
            vec![],
        )
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn old_epoch_is_rejected_and_missing_database_is_recreated() {
        let (_temp, store) = store();
        let p = create(&store, "Identity").await;
        let request = SqlRequest {
            sql: "CREATE TABLE bad(x)".into(),
            params: vec![],
            options: SqlOptions::default(),
        };
        assert_eq!(
            store
                .execute(&p.id, request, Some("old-document"), SqlCaller::Browser)
                .await
                .unwrap_err()
                .code,
            "PROJECT_RELOAD_REQUIRED"
        );
        let db = p.project_root.join("data/main.sqlite");
        fs::remove_file(&db).unwrap();
        exec(&store, &p.id, "SELECT 1", vec![]).await.unwrap();
        assert!(db.exists());
        assert_eq!(store.info(&p.id).await.unwrap().state, "ready");
    }
    #[tokio::test]
    async fn timeout_cleans_connection_without_cancelling_next_request() {
        let (_temp, store) = store();
        let p = create(&store, "Timeout").await;
        let request=SqlRequest{sql:"WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n".into(),params:vec![],options:SqlOptions{max_rows:None,timeout_ms:Some(5)}};
        let error = store
            .execute(&p.id, request, None, SqlCaller::Browser)
            .await
            .unwrap_err();
        assert_eq!(error.code, "SQL_TIMEOUT");
        assert_eq!(error.details["operationOutcome"], "rolled_back");
        assert_eq!(
            exec(&store, &p.id, "SELECT 42 AS answer", vec![])
                .await
                .unwrap()
                .rows[0]["answer"],
            42
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_data_is_rejected_and_replaced_database_is_accepted_after_close() {
        let (temp, store) = store();
        let p = create(&store, "Paths").await;
        let data = p.project_root.join("data");
        let held = temp.path().join("held-data");
        fs::rename(&data, &held).unwrap();
        std::os::unix::fs::symlink(&held, &data).unwrap();
        assert!(exec(&store, &p.id, "SELECT 1", vec![]).await.is_err());
        fs::remove_file(&data).unwrap();
        fs::rename(&held, &data).unwrap();
        store.close_database(&p.id).await.unwrap();
        let replacement = data.join("replacement.sqlite");
        database::initialize_database(&replacement).unwrap();
        fs::rename(replacement, data.join("main.sqlite")).unwrap();
        exec(&store, &p.id, "SELECT 1", vec![]).await.unwrap();
    }
    #[tokio::test]
    async fn replaced_project_directory_is_rejected_even_with_the_same_manifest_and_database() {
        let (_temp, store) = store();
        let project = create(&store, "Disposable replaced directory").await;
        let handle = store.project(&project.id).unwrap();
        let retained = project.project_root.with_file_name("retained-original");
        fs::rename(&project.project_root, &retained).unwrap();
        fs::create_dir_all(project.project_root.join("data")).unwrap();
        fs::copy(
            retained.join("panda.json"),
            project.project_root.join("panda.json"),
        )
        .unwrap();
        fs::copy(
            retained.join("data/main.sqlite"),
            project.project_root.join("data/main.sqlite"),
        )
        .unwrap();
        assert_eq!(handle.check().unwrap_err().code, "PROJECT_IDENTITY_CHANGED");
        assert_eq!(
            store.project(&project.id).err().unwrap().code,
            "PROJECT_IDENTITY_CHANGED"
        );
        assert!(store.registered_epoch(&project.id).unwrap().is_some());
        assert!(retained.join("data/main.sqlite").is_file());
        store.shutdown().await;
    }
    #[tokio::test]
    async fn transaction_rows_share_one_budget() {
        let (_temp, store) = store();
        let p = create(&store, "Shared budget").await;
        let request = TransactionRequest {
            statements: vec![
                SqlStatement {
                    sql: "SELECT 1 AS x".into(),
                    params: vec![],
                },
                SqlStatement {
                    sql: "SELECT 2 AS x".into(),
                    params: vec![],
                },
            ],
            options: SqlOptions {
                max_rows: Some(1),
                timeout_ms: None,
            },
        };
        assert_eq!(
            store
                .transaction(&p.id, request, None, SqlCaller::Browser)
                .await
                .unwrap_err()
                .code,
            "SQL_RESULT_LIMIT_EXCEEDED"
        );
        assert!(store
            .execute(
                &p.id,
                SqlRequest {
                    sql: "SELECT 1".into(),
                    params: vec![],
                    options: SqlOptions {
                        max_rows: Some(1001),
                        timeout_ms: None
                    }
                },
                None,
                SqlCaller::Browser
            )
            .await
            .is_err());
    }
    #[tokio::test]
    async fn an_unregistered_directory_is_ignored_without_hiding_valid_projects() {
        let (_temp, store) = store();
        let p = create(&store, "Valid").await;
        fs::create_dir(store.workspace.join("projects/Bad directory")).unwrap();
        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert!(list
            .iter()
            .any(|info| info.id == p.id && info.state == "ready"));
        assert!(!list.iter().any(|info| info.id == "Bad directory"));
    }
    #[tokio::test]
    async fn queued_request_expires_promptly_and_never_executes() {
        let (_temp, store) = store();
        let store = Arc::new(store);
        let p = create(&store, "Queue").await;
        let running_store = store.clone();
        let id = p.id.clone();
        let running = tokio::spawn(async move {
            running_store.execute(&id,SqlRequest{
            sql:"WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n".into(),params:vec![],options:SqlOptions{max_rows:None,timeout_ms:Some(6500)}
        },None,SqlCaller::Management).await
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let start = std::time::Instant::now();
        let queued = exec(&store, &p.id, "CREATE TABLE must_not_exist(x)", vec![])
            .await
            .unwrap_err();
        assert_eq!(queued.code, "SQL_QUEUE_TIMEOUT");
        assert_eq!(queued.details["operationOutcome"], "not_started");
        assert!(start.elapsed() < std::time::Duration::from_millis(6100));
        assert_eq!(running.await.unwrap().unwrap_err().code, "SQL_TIMEOUT");
        assert!(store.schema(&p.id, None).await.unwrap()["tables"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    #[tokio::test]
    async fn foreign_keys_and_schema_details_are_enforced() {
        let (_temp, store) = store();
        let p = create(&store, "Relations").await;
        exec(
            &store,
            &p.id,
            "CREATE TABLE parent(id INTEGER PRIMARY KEY)",
            vec![],
        )
        .await
        .unwrap();
        exec(&store,&p.id,"CREATE TABLE child(id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parent(id), name TEXT UNIQUE)",vec![]).await.unwrap();
        exec(
            &store,
            &p.id,
            "CREATE VIEW child_names AS SELECT name FROM child",
            vec![],
        )
        .await
        .unwrap();
        assert!(exec(
            &store,
            &p.id,
            "INSERT INTO child(parent_id,name) VALUES(9,'orphan')",
            vec![]
        )
        .await
        .is_err());
        let schema = store.schema(&p.id, None).await.unwrap();
        let child = schema["tables"]
            .as_array()
            .unwrap()
            .iter()
            .find(|table| table["name"] == "child")
            .unwrap();
        assert_eq!(child["foreignKeys"][0]["table"], "parent");
        assert_eq!(child["indexes"][0]["columns"][0]["name"], "name");
        assert_eq!(schema["views"][0]["columns"][0]["name"], "name");
    }
    #[tokio::test]
    async fn hardlinked_databases_are_rejected_before_opening() {
        let (_temp, store) = store();
        let p = create(&store, "Link").await;
        fs::hard_link(
            p.project_root.join("data/main.sqlite"),
            p.project_root.join("data/copy.sqlite"),
        )
        .unwrap();
        assert_eq!(
            exec(&store, &p.id, "CREATE TABLE bad(x)", vec![])
                .await
                .unwrap_err()
                .code,
            "UNSAFE_DATABASE_PATH"
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn final_identity_removal_requires_joined_writers_and_clears_every_current_identity_cache(
    ) {
        let (temporary, store) = store();
        let project = create(&store, "Disposable complete identity").await;
        let retained_handle = store.project(&project.id).unwrap();
        let _opened = store.reserve_web_open(&project.id).unwrap();
        store.schema(&project.id, None).await.unwrap();
        assert!(store.workers.lock().unwrap().contains_key(&project.id));
        let marker = store
            .begin_deletion(
                &project.id,
                "task_disposable_identity_delete",
                &Uuid::new_v4().to_string(),
                &identity(&project.project_root).unwrap(),
            )
            .unwrap();
        let moved = temporary.path().join("disposable-owned-deletion-stage");
        fs::rename(&project.project_root, &moved).unwrap();
        assert_eq!(
            store
                .complete_deleted_project_identity(&marker)
                .unwrap_err()
                .code,
            "PROJECT_BUSY"
        );
        store.close_database(&project.id).await.unwrap();
        let active = store
            .reserve_transient_operation(&project.id, "disposable actual writer owner")
            .unwrap();
        assert_eq!(
            store
                .complete_deleted_project_identity(&marker)
                .unwrap_err()
                .code,
            "PROJECT_BUSY"
        );
        drop(active);
        fs::remove_dir_all(moved).unwrap();
        store.complete_deleted_project_identity(&marker).unwrap();
        store.complete_deleted_project_identity(&marker).unwrap();
        assert!(store.deleted_project_identity_absent(&project.id).unwrap());
        assert!(store.registered_epoch(&project.id).unwrap().is_none());
        assert!(!store.controls.lock().unwrap().contains_key(&project.id));
        assert!(!store.gates.lock().unwrap().contains_key(&project.id));
        assert!(!store.workers.lock().unwrap().contains_key(&project.id));
        assert!(!store.active_tasks.lock().unwrap().contains_key(&project.id));
        assert_eq!(retained_handle.check().unwrap_err().code, "PROJECT_REMOVED");
        assert_eq!(
            store.control(&project.id).err().unwrap().code,
            "PROJECT_REMOVED"
        );
        for name in ["registration.json", "reservation.json", "deleting.json"] {
            assert!(!store.private_dir(&project.id).unwrap().join(name).exists());
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_identity_finalization_waits_for_real_sqlite_close_before_namespace_move() {
        let (temporary, store) = store();
        let project = create(&store, "Disposable complete identity").await;
        let retained_handle = store.project(&project.id).unwrap();
        let _opened = store.reserve_web_open(&project.id).unwrap();
        store.schema(&project.id, None).await.unwrap();
        assert!(store.workers.lock().unwrap().contains_key(&project.id));
        let marker = store
            .begin_deletion(
                &project.id,
                "task_disposable_identity_delete",
                &Uuid::new_v4().to_string(),
                &identity(&project.project_root).unwrap(),
            )
            .unwrap();
        let moved = temporary.path().join("disposable-owned-deletion-stage");
        assert!(
            fs::rename(&project.project_root, &moved).is_err(),
            "a live SQLite HANDLE must prevent the Windows namespace move"
        );
        assert_eq!(
            store
                .complete_deleted_project_identity(&marker)
                .unwrap_err()
                .code,
            "PROJECT_DELETE_INCOMPLETE"
        );
        assert!(store.workers.lock().unwrap().contains_key(&project.id));
        store.close_database(&project.id).await.unwrap();
        fs::rename(&project.project_root, &moved).unwrap();
        let active = store
            .reserve_transient_operation(&project.id, "disposable actual writer owner")
            .unwrap();
        assert_eq!(
            store
                .complete_deleted_project_identity(&marker)
                .unwrap_err()
                .code,
            "PROJECT_BUSY"
        );
        drop(active);
        fs::remove_dir_all(moved).unwrap();
        store.complete_deleted_project_identity(&marker).unwrap();
        store.complete_deleted_project_identity(&marker).unwrap();
        assert!(store.deleted_project_identity_absent(&project.id).unwrap());
        assert!(store.registered_epoch(&project.id).unwrap().is_none());
        assert!(!store.controls.lock().unwrap().contains_key(&project.id));
        assert!(!store.gates.lock().unwrap().contains_key(&project.id));
        assert!(!store.workers.lock().unwrap().contains_key(&project.id));
        assert!(!store.active_tasks.lock().unwrap().contains_key(&project.id));
        assert_eq!(retained_handle.check().unwrap_err().code, "PROJECT_REMOVED");
        assert_eq!(
            store.control(&project.id).err().unwrap().code,
            "PROJECT_REMOVED"
        );
        for name in ["registration.json", "reservation.json", "deleting.json"] {
            assert!(!store.private_dir(&project.id).unwrap().join(name).exists());
        }
    }
}

#[cfg(test)]
#[path = "project_instance_tests.rs"]
mod instance_tests;
