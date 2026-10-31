//! Project files are portable; local registration and document epochs belong to the workspace.
use crate::{
    database::{self, Budget, DatabaseWorker, QueueBudget, Work, WorkResult},
    file_identity::PersistedIdentity as FileIdentity,
    ApiError, SchemaInfo, SqlCaller, SqlRequest, SqlResult, TransactionRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectManifest {
    pub manifest_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(
        default,
        deserialize_with = "display_icon",
        skip_serializing_if = "Option::is_none"
    )]
    pub icon: Option<String>,
    pub sdk_api_version: u32,
    pub source_dir: String,
    pub output_dir: String,
    pub entry: String,
    pub database: String,
    pub attachments_dir: String,
    pub build: Value,
    pub docs: Value,
    #[serde(default)]
    pub runtime_requirements: Value,
    #[serde(flatten)]
    pub extra: MapExtras,
}
type MapExtras = serde_json::Map<String, Value>;
#[path = "project_data_restore_registration.rs"]
pub(crate) mod data_restore_registration;
pub(crate) use data_restore_registration::{DataRestoreEpochState, DataRestoreRegistrationPlan};
fn display_icon<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    // Invalid display metadata must not disable the project's business app.
    // The manager reads the raw value independently and reports its problem.
    let value = Value::deserialize(deserializer)?;
    Ok(value.as_str().map(str::to_owned))
}
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
    pub manifest: Option<ProjectManifest>,
    pub build_info: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registration {
    schema_version: u32,
    id: String,
    project_root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    directory_name: Option<String>,
    root_identity: FileIdentity,
    database_path: String,
    database_identity: FileIdentity,
    document_epoch: String,
}

/// A failed permanent removal stays quarantined until an explicit delete retry.
/// This marker is outside the partially removable project and is not a backup.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProjectDeletion {
    schema_version: u32,
    pub(crate) project_id: String,
    pub(crate) task_id: String,
    pub(crate) target_epoch: String,
    pub(crate) root_identity: FileIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    directory_name: Option<String>,
    pub(crate) source_epoch: Option<String>,
}

#[derive(Clone)]
pub struct ProjectHandle {
    workspace: PathBuf,
    pub root: PathBuf,
    pub manifest: ProjectManifest,
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
    pub fn check(&self) -> Result<(), ApiError> {
        crate::data_restore_guard::check_business_available(&self.workspace, &self.manifest.id)?;
        if self.workspace_paused.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "WORKSPACE_MIGRATING",
                "Workspace migration has paused project access",
            )
            .outcome("not_started"));
        }
        check_deletion_marker(&self.deletion_path)?;
        if !self.state_path.try_exists()? {
            return Err(removed_project_error(&self.manifest.id));
        }
        check_no_symlinks(&self.root, false)?;
        if !identity_matches(&self.registration.root_identity, &self.root)? {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Project directory has been replaced; explicit re-registration is required",
            ));
        }
        let db = checked_relative(&self.root, &self.manifest.database, false)?;
        if !db.is_file() {
            return Err(ApiError::new(
                "DATABASE_MISSING",
                "The project's registered database is missing; no empty replacement was created",
            ));
        }
        if crate::project_file_io::link_count(&db)? != 1 {
            return Err(ApiError::new(
                "UNSAFE_DATABASE_PATH",
                "Database hard links are not supported",
            ));
        }
        if !identity_matches(&self.registration.database_identity, &db)? {
            return Err(ApiError::new("PROJECT_IDENTITY_CHANGED","The registered database file has been replaced; explicit re-registration is required"));
        }
        let current: Registration = read_json(&self.state_path)?;
        if current.document_epoch != self.document_epoch {
            return Err(ApiError::new(
                "PROJECT_RELOAD_REQUIRED",
                "Project identity changed; reopen the project",
            ));
        }
        let manifest: ProjectManifest = read_json(&self.root.join("project.json"))?;
        if manifest.id != self.manifest.id || manifest.database != self.manifest.database {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Project identity or database path changed; reopen through the platform",
            ));
        }
        Ok(())
    }
    pub fn validate_epoch(&self, expected: Option<&str>) -> Result<(), ApiError> {
        if expected.is_some_and(|value| value != self.document_epoch) {
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

fn registration_path(workspace: &Path, id: &str) -> PathBuf {
    workspace
        .join(".platform/project-state")
        .join(format!("{id}.json"))
}

fn directory_mapping_error() -> ApiError {
    ApiError::new(
        "PROJECT_DIRECTORY_MAPPING_INVALID",
        "The retained project directory binding is invalid; no other directory was adopted",
    )
}

fn mapping_diagnostic(workspace: &Path, id: &str, error: ApiError) -> ProjectInfo {
    ProjectInfo {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        // This is the retained control record to inspect, never a guessed
        // replacement project directory. All actions still resolve by ID.
        project_root: registration_path(workspace, id),
        document_epoch: String::new(),
        state: "error".into(),
        issues: vec![format!("{} ({})", error.message, error.code)],
        manifest: None,
        build_info: json!({"sourceState":"unknown","checkedAt":null,"stale":true,"reason":"Project directory binding needs inspection"}),
    }
}

fn checked_directory_name(name: &str) -> Result<&str, ApiError> {
    if name.is_empty()
        || name.len() > 128
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
    {
        return Err(directory_mapping_error());
    }
    Ok(name)
}

fn bound_root(workspace: &Path, name: &str) -> Result<PathBuf, ApiError> {
    let root = workspace
        .join("projects")
        .join(checked_directory_name(name)?);
    check_no_symlinks(&root, true)?;
    if root.try_exists()? && !fs::symlink_metadata(&root)?.is_dir() {
        return Err(directory_mapping_error());
    }
    Ok(root)
}

fn registration_root(
    workspace: &Path,
    id: &str,
    state: &Registration,
) -> Result<PathBuf, ApiError> {
    if state.schema_version != 1
        || state.id != id
        || Uuid::parse_str(&state.document_epoch)
            .map(|epoch| epoch.to_string() != state.document_epoch)
            .unwrap_or(true)
    {
        return Err(directory_mapping_error());
    }
    let name = state
        .directory_name
        .as_deref()
        .ok_or_else(directory_mapping_error)?;
    let root = bound_root(workspace, name)?;
    if state.project_root != root {
        return Err(directory_mapping_error());
    }
    Ok(root)
}

/// Collect names for allocation only. This is not a binding resolver and never
/// inspects an old absolute target: a stale/malformed binding remains blocked by
/// registration_root for every actual operation on that project.
fn retain_directory_claims(
    names: &mut std::collections::HashSet<String>,
    owner: &str,
    directory_name: Option<&str>,
    project_root: Option<&Path>,
) -> Result<(), ApiError> {
    names.insert(checked_directory_name(owner)?.to_owned());
    if let Some(name) = directory_name {
        names.insert(checked_directory_name(name)?.to_owned());
    }
    if let Some(root) = project_root {
        let name = crate::project_directory_io::retained_directory_leaf(root)
            .ok_or_else(directory_mapping_error)?;
        // Conflicting but safe declarations reserve both names. Neither becomes
        // a valid binding, and no path is opened, repaired, or adopted here.
        names.insert(checked_directory_name(&name)?.to_owned());
    }
    Ok(())
}

/// Resolve only current platform-owned bindings. A directory or manifest cannot
/// introduce or revive an identity without explicit registration.
pub(crate) fn resolve_project_root(workspace: &Path, id: &str) -> Result<PathBuf, ApiError> {
    if !valid_id(id) {
        return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
    }
    let path = registration_path(workspace, id);
    check_no_symlinks(&path, true)?;
    let registered = if path.try_exists()? {
        Some(registration_root(
            workspace,
            id,
            &read_json::<Registration>(&path)?,
        )?)
    } else {
        None
    };
    for kind in ["reservations", "deleting"] {
        let path = workspace
            .join(".platform/project-state")
            .join(kind)
            .join(format!("{id}.json"));
        check_no_symlinks(&path, true)?;
        if !path.try_exists()? {
            continue;
        }
        let record: Value = read_json(&path)?;
        if record["projectId"] != id {
            return Err(directory_mapping_error());
        }
        if kind == "reservations" && record["taskId"].as_str().is_none_or(|task| !valid_id(task)) {
            return Err(directory_mapping_error());
        }
        // A durable reservation owns its fresh destination until registration
        // commits. Existing project identities are never imported again.
        if kind != "reservations" {
            if let Some(root) = &registered {
                return Ok(root.clone());
            }
        }
        if let Some(value) = record.get("directoryName") {
            return bound_root(
                workspace,
                value.as_str().ok_or_else(directory_mapping_error)?,
            );
        }
    }
    match registered {
        Some(root) => Ok(root),
        None => Err(ApiError::new(
            "PROJECT_NOT_REGISTERED",
            "Project identity is not registered",
        )),
    }
}

/// Current registrations are the complete discovery index. Unregistered
/// directories never become projects through inspection.
pub(crate) fn enumerate_project_roots(
    workspace: &Path,
) -> Result<Vec<(String, PathBuf)>, ApiError> {
    let states = workspace.join(".platform/project-state");
    check_no_symlinks(&states, false)?;
    let mut roots = HashMap::<String, PathBuf>::new();
    let mut owners = HashMap::<PathBuf, String>::new();
    for entry in fs::read_dir(&states)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|id| valid_id(id))
        else {
            continue;
        };
        let root = resolve_project_root(workspace, id)?;
        if owners.insert(root.clone(), id.into()).is_some() {
            return Err(directory_mapping_error());
        }
        roots.insert(id.into(), root);
    }
    let mut roots: Vec<_> = roots.into_iter().collect();
    roots.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(roots)
}

/// A failed deletion still binds registration identity. Migration/cleanup must
/// not rewrite that identity until the task completes or is safely abandoned.
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
    workspace_cleaning: AtomicBool,
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
        crate::platform_support::require_runtime().map_err(ApiError::from)?;
        check_no_symlinks(&workspace, false)?;
        for path in [
            workspace.join("projects"),
            workspace.join(".platform/project-state"),
        ] {
            ensure_dir(&path)?;
        }
        #[cfg(test)]
        let default_data_root = Some(
            if workspace
                .file_name()
                .is_some_and(|name| name == "workspace")
            {
                workspace.parent().unwrap().to_path_buf()
            } else {
                workspace.clone()
            },
        );
        #[cfg(not(test))]
        let default_data_root = None;
        Ok(Self {
            workspace,
            processes,
            registration_lock: Mutex::new(()),
            gates: Mutex::new(HashMap::new()),
            workers: Mutex::new(HashMap::new()),
            shutdown_issues: Mutex::new(Vec::new()),
            queue: Arc::new(QueueBudget::default()),
            stopping: AtomicBool::new(false),
            lifecycle_gate: RwLock::new(()),
            workspace_migrating: AtomicBool::new(false),
            workspace_cleaning: AtomicBool::new(false),
            workspace_paused: Arc::new(AtomicBool::new(false)),
            resources,
            controls: Mutex::new(HashMap::new()),
            active_tasks: Arc::new(Mutex::new(HashMap::new())),
            pending_node_deletions: Arc::new(Mutex::new(HashMap::new())),
            identity_removed: Mutex::new(vec![]),
            node_source_data_root: Mutex::new(default_data_root),
        })
    }
    pub(crate) fn set_node_source_data_root(&self, data_root: PathBuf) {
        *self.node_source_data_root.lock().unwrap() = Some(data_root);
    }
    fn require_new_project_location(&self, location: &Path) -> Result<(), ApiError> {
        if let Some(data_root) = self.node_source_data_root.lock().unwrap().as_ref() {
            crate::node_apps::development::require_new_web_project_location(data_root, location)?;
        }
        Ok(())
    }
    pub(crate) fn initialize_project_ownership(&self, id: &str) -> Result<(), ApiError> {
        if let Some(data_root) = self.node_source_data_root.lock().unwrap().as_ref() {
            crate::web_project_ownership::WebProjectOwnership::new(
                data_root.clone(),
                self.workspace.clone(),
            )
            .initialize(id)?;
        }
        Ok(())
    }
    fn record_project_root_ownership(&self, id: &str, root: &Path) -> Result<(), ApiError> {
        if let Some(data_root) = self.node_source_data_root.lock().unwrap().as_ref() {
            crate::web_project_ownership::WebProjectOwnership::new(
                data_root.clone(),
                self.workspace.clone(),
            )
            .record_project_root(id, root)?;
        }
        Ok(())
    }
    pub(crate) fn discard_unpublished_ownership(&self, id: &str) -> Result<(), ApiError> {
        if let Some(data_root) = self.node_source_data_root.lock().unwrap().as_ref() {
            crate::web_project_ownership::WebProjectOwnership::new(
                data_root.clone(),
                self.workspace.clone(),
            )
            .discard_unpublished(id)?;
        }
        Ok(())
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
        let data = self.node_source_data_root.lock().unwrap();
        let data = data.as_ref().ok_or_else(|| {
            ApiError::new(
                "PROJECT_WRITER_UNCONFIRMED",
                "项目写入者缺少固定数据根归属。",
            )
        })?;
        Ok(self
            .processes
            .with_writer_owner(crate::durable_writer::WriterOwner::web(data, id)?))
    }
    fn accepting(&self) -> Result<(), ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping").outcome("not_started"))
        } else {
            Ok(())
        }
    }
    pub async fn create(&self, input: CreateProjectInput) -> Result<ProjectInfo, ApiError> {
        let _creation_lease = self.lifecycle_gate.read().await;
        self.accepting()?;
        self.check_workspace_available()?;
        let name = input.name.trim();
        if name.is_empty() {
            return Err(ApiError::new(
                "INVALID_PROJECT_NAME",
                "Project name must not be empty",
            ));
        }
        if name.chars().count() > 200
            || input
                .description
                .as_ref()
                .is_some_and(|text| text.chars().count() > 2000)
        {
            return Err(ApiError::new(
                "INVALID_PROJECT_METADATA",
                "Project name or description is too long",
            ));
        }
        let _creation = self.registration_lock.lock().unwrap();
        self.check_workspace_available()?;
        let id = format!("prj_{}", Uuid::new_v4().simple());
        let creation_task = format!("task_create_{}", Uuid::new_v4().simple());
        self.initialize_project_ownership(&id)?;
        let root = self
            .reserve_named_identity_locked(&id, &creation_task, name)
            .map_err(|error| {
                let cleanup = self.discard_unpublished_ownership(&id);
                error.detail("cleanupError", cleanup.err().map(|error| json!(error)))
            })?;
        let staging = self
            .workspace
            .join("projects")
            .join(format!(".creating-{id}"));
        check_no_symlinks(&staging, true)?;
        crate::project_file_io::create_private_directory(&staging)?;
        let manifest = ProjectManifest {
            manifest_version: 1,
            id: id.clone(),
            name: name.to_owned(),
            description: input.description.unwrap_or_default(),
            icon: None,
            sdk_api_version: 1,
            source_dir: "app".into(),
            output_dir: "dist".into(),
            entry: "dist/index.html".into(),
            database: "data/main.sqlite".into(),
            attachments_dir: "data/attachments".into(),
            build: json!({"script":"build"}),
            docs: json!({"readme":"README.md","database":"docs/database.md","platform":"docs/platform.md"}),
            runtime_requirements: json!({"network":{"connect":[],"images":[],"media":[]},"devices":{}}),
            extra: MapExtras::new(),
        };
        let created = (|| {
            for dir in [
                "meta",
                "docs",
                "app/src",
                "app/public",
                "data/attachments",
                ".platform",
            ] {
                ensure_dir(&staging.join(dir))?;
            }
            write_new(
                &staging.join("project.json"),
                &serde_json::to_vec_pretty(&manifest)?,
            )?;
            write_new(&staging.join("README.md"),format!("# {}\n\n项目 ID：`{}`\n\n{}\n\n## 给 AI 的交接\n\n- 先阅读 `project.json`、`docs/platform.md` 与 `docs/database.md`。\n- 源码放在 `app/`，用户数据保存在 `data/`；不要以构建、刷新或清理为由删除数据。\n- 当前是 M1 空项目：数据库已建立，尚无网页产物或已发布 SDK。M2 才提供完整 React 模板与构建命令。\n- 记录项目用途、用户操作流程、源码入口、依赖、构建方式和已知限制。\n- 新增或变更表时同步 `docs/database.md`，说明字段、关联、单位、枚举及附件逻辑路径。\n- 不在项目文件中保存平台管理令牌、当前服务端口或机器绝对路径。\n\n## 本次交接\n\n- 功能：待实现。\n- 验证：待填写。\n- 已知问题与下次工作：待填写。\n",manifest.name,id,manifest.description).as_bytes())?;
            write_new(&staging.join("docs/database.md"),b"# Database contract\n\nDatabase: `data/main.sqlite` (SQLite; independent per project).\n\nThe database is initially empty. Document every table, column, type, nullability, primary/foreign key, index, unit, enum, timestamp convention and attachment path here. Describe compatibility steps for schema changes. The platform enables foreign keys on its own connections. External tools must do the same. Never replace a registered database while the app is running.\n")?;
            write_new(&staging.join("docs/platform.md"),b"# Platform API v1 (M1 development)\n\nThis project runs in Panda Drawer. Use its bundled `local-projects-dev` CLI: `service status --json`, `project info --project ID --json`, `db schema --project ID --json`, `db execute --project ID --input request.json --json`. No SDK package/build service is published in M1.\n\nThe platform owns sessions, document epochs and each project's SQLite connection. Browser routes under `/p/ID/api/v1/` require a project session and `X-Platform-Document-Epoch`. Do not hard-code ports or credentials. SQL accepts bound positional parameters. Large integers use `{\"$int64\":\"decimal\"}`; blobs use `{\"$bytes\":\"base64\"}`. Requests accept one SQL statement or one managed transaction; BEGIN/COMMIT/ATTACH and file export are disallowed.\n\nAttachments are logical paths under `data/attachments/`. Keep business data and attachments outside `dist/`. Budgets currently use internal test parameters and are not yet formal SDK defaults.\n")?;
            write_new(&staging.join("app/README.md"),b"# Project source\n\nM1 creates storage and runtime access only. Add project source here; the standard React template, vendored SDK and build workflow arrive in M2. A controlled verification fixture may be installed explicitly for M1 runtime tests.\n")?;
            if let Some(resources) = &self.resources {
                resources.install(&staging, &manifest)?;
            }
            database::initialize_database(&staging.join("data/main.sqlite"))?;
            crate::project_file_io::make_file_private(&staging.join("data/main.sqlite"))?;
            publish_directory_noreplace(&staging, &root)?;
            self.record_project_root_ownership(&id, &root)?;
            platform_native::filesystem::sync_directory(root.parent().unwrap())?;
            let state = Registration {
                schema_version: 1,
                id: id.clone(),
                project_root: root.clone(),
                directory_name: Some(root.file_name().unwrap().to_string_lossy().into_owned()),
                root_identity: identity(&root)?,
                database_path: manifest.database.clone(),
                database_identity: identity(&root.join(&manifest.database))?,
                document_epoch: Uuid::new_v4().to_string(),
            };
            publish_registration(&registration_path(&self.workspace, &id), &state)?;
            self.release_identity_locked(&id, &creation_task)?;
            Ok::<(), ApiError>(())
        })();
        // A failed or interrupted creation is never admitted by scanning its
        // manifest. The reservation and any candidate are retained for review.
        created.map_err(|error| {
            error
                .detail("projectId", id.clone())
                .detail("taskId", creation_task)
                .detail("candidatePath", staging.to_string_lossy().into_owned())
                .detail("projectRoot", root.to_string_lossy().into_owned())
        })?;
        drop(_creation);
        self.info(&id).await
    }
    pub(crate) fn project_root(&self, id: &str) -> Result<PathBuf, ApiError> {
        resolve_project_root(&self.workspace, id)
    }
    pub async fn list(&self) -> Result<Vec<ProjectInfo>, ApiError> {
        let mut results = Vec::new();
        let mut ids = std::collections::BTreeSet::new();
        for entry in fs::read_dir(self.workspace.join(".platform/project-state"))? {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            if let Some(id) = path
                .file_stem()
                .and_then(|name| name.to_str())
                .filter(|id| valid_id(id))
            {
                ids.insert(id.to_owned());
            }
        }
        for id in ids {
            match self.is_active_identity(&id) {
                Ok(true) => results.push(self.info(&id).await?),
                Ok(false) => (),
                Err(error) => results.push(mapping_diagnostic(&self.workspace, &id, error)),
            }
        }
        results.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        Ok(results)
    }
    pub async fn info(&self, id: &str) -> Result<ProjectInfo, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let root = match self.project_root(id) {
            Ok(root) => root,
            Err(error) => return Ok(mapping_diagnostic(&self.workspace, id, error)),
        };
        let manifest = checked_relative(&root, "project.json", false)
            .and_then(|path| read_json::<ProjectManifest>(&path));
        let mut info = ProjectInfo {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            project_root: root.clone(),
            document_epoch: self.registered_epoch(id).ok().flatten().unwrap_or_default(),
            state: "error".into(),
            issues: vec![],
            manifest: None,
            build_info: json!({"sourceState":"unknown","checkedAt":null,"stale":true,"reason":"M1 does not verify build provenance"}),
        };
        match manifest {
            Ok(manifest) => {
                info.name = manifest.name.clone();
                info.description = manifest.description.clone();
                info.manifest = Some(manifest.clone());
                match self.project(id) {
                    Ok(project) => {
                        info.document_epoch = project.document_epoch;
                        match checked_relative(&root, &manifest.entry, true) {
                            Ok(entry) if entry.is_file() => info.state = "ready".into(),
                            Ok(_) => {
                                info.state = "unbuilt".into();
                                info.build_info = json!({"sourceState":"unbuilt","checkedAt":null,"stale":false,"reason":"No webpage artifact is present"});
                            }
                            Err(error) => info.issues.push(error.message),
                        }
                    }
                    Err(error) => {
                        if error.code.starts_with("PROJECT_DATA_RESTORE_") {
                            info.build_info = json!({"sourceState":"unknown","checkedAt":null,"stale":true,"dataRecoveryRequired":true,"dataRecoveryState":if error.code=="PROJECT_DATA_RESTORE_PENDING"{"pending"}else{"blocked"},"reason":error.message});
                        }
                        if matches!(error.code.as_str(), "DATABASE_MISSING" | "PROJECT_MISSING") {
                            info.state = "missing".into();
                        }
                        info.issues.push(error.message);
                    }
                }
            }
            Err(error) => {
                if !root.exists() {
                    info.state = "missing".into();
                    info.issues
                        .push("Registered project directory is missing".into());
                } else {
                    info.issues.push(error.message);
                }
            }
        }
        Ok(info)
    }
    pub fn project(&self, id: &str) -> Result<ProjectHandle, ApiError> {
        self.accepting()?;
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        crate::data_restore_guard::check_business_available(&self.workspace, id)?;
        self.check_not_deleting(id)?;
        self.require_active_identity(id)?;
        let root = self.project_root(id)?;
        if !root.exists() {
            return Err(ApiError::new(
                "PROJECT_MISSING",
                "Project directory is missing",
            ));
        }
        check_no_symlinks(&root, false)?;
        let manifest: ProjectManifest =
            read_json(&checked_relative(&root, "project.json", false)?)?;
        validate_manifest(&manifest, id, &root)?;
        let database = checked_relative(&root, &manifest.database, true)?;
        if !database.is_file() {
            return Err(ApiError::new(
                "DATABASE_MISSING",
                "The registered database is missing; no empty replacement was created",
            ));
        }
        if crate::project_file_io::link_count(&database)? != 1 {
            return Err(ApiError::new(
                "UNSAFE_DATABASE_PATH",
                "Each project must own an independent database file; hard links are not supported",
            ));
        }
        let state_path = self
            .workspace
            .join(".platform/project-state")
            .join(format!("{id}.json"));
        let _registration = self.registration_lock.lock().unwrap();
        self.require_active_identity(id)?;
        let registration = if state_path.exists() {
            let state: Registration = read_json(&state_path)?;
            if state.schema_version != 1
                || state.id != id
                || Uuid::parse_str(&state.document_epoch)
                    .map(|value| value.to_string() != state.document_epoch)
                    .unwrap_or(true)
                || state.project_root != root
                || state.database_path != manifest.database
                || !identity_matches(&state.root_identity, &root)?
                || !identity_matches(&state.database_identity, &database)?
            {
                return Err(ApiError::new("PROJECT_IDENTITY_CHANGED","Project root or database identity differs from its registration; explicit re-registration is required"));
            }
            state
        } else {
            return Err(ApiError::new(
                "PROJECT_NOT_REGISTERED",
                "Project identity is not registered",
            ));
        };
        let operation_gate = self
            .gates
            .lock()
            .unwrap()
            .entry(id.into())
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone();
        Ok(ProjectHandle {
            workspace: self.workspace.clone(),
            root,
            manifest,
            document_epoch: registration.document_epoch.clone(),
            operation_gate,
            registration,
            state_path,
            control: self.control(id)?,
            maintenance_id: None,
            diagnostic: false,
            workspace_paused: self.workspace_paused.clone(),
            deletion_path: self.deletion_path(id),
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
        if bytes > database::JSON_BODY_MAX_BYTES {
            return Err(ApiError::new(
                "REQUEST_TOO_LARGE",
                "SQL request exceeds the JSON input budget",
            )
            .outcome("not_started"));
        }
        let mut project = self.project(id)?;
        project.maintenance_id = maintenance_id.map(str::to_owned);
        project.diagnostic = epoch.is_none() && matches!(work, Work::Schema);
        project.validate_epoch(epoch)?;
        for attempt in 0..2 {
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
    pub(crate) fn control(
        &self,
        id: &str,
    ) -> Result<Arc<crate::control::ProjectControl>, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        if !registration_path(&self.workspace, id).try_exists()?
            && !self.reservation_path(id).try_exists()?
            && !self.deletion_path(id).try_exists()?
        {
            return Err(removed_project_error(id));
        }
        let mut controls = self.controls.lock().unwrap();
        if let Some(control) = controls.get(id) {
            return Ok(control.clone());
        }
        let control = crate::control::ProjectControl::load(&self.workspace, id)?;
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
        if !registration_path(&self.workspace, id).try_exists()?
            && !self.reservation_path(id).try_exists()?
            && !self.deletion_path(id).try_exists()?
        {
            return Err(removed_project_error(id));
        }
        let mut controls = self.controls.lock().unwrap();
        if let Some(control) = controls.get(id) {
            return Ok(control.clone());
        }
        match crate::control::ProjectControl::load(&self.workspace, id) {
            Ok(control) => {
                controls.insert(id.into(), control.clone());
                Ok(control)
            }
            Err(error) if error.code == "INVALID_JSON" => {
                crate::control::ProjectControl::volatile_for_deletion(&self.workspace, id)
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
        crate::data_restore_guard::check_development_available(&self.workspace, id)?;
        crate::project_git::check_data_operations_available(&self.workspace, id)?;
        crate::source_restore_guard::check_development_available(&self.workspace, id)
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
    pub(crate) fn reserve_source_restore_recovery_task(
        &self,
        id: &str,
        task: &str,
        permit: &crate::source_restore_guard::SourceRestoreRecoveryPermit,
    ) -> Result<ProjectTaskLease, ApiError> {
        self.check_not_deleting(id)?;
        let lease = self.reserve_operation(id, ProjectOperation::Task(task.into()))?;
        crate::data_restore_guard::check_development_available(&self.workspace, id)?;
        // Revalidate after admission: a previously displayed recovery summary is
        // not permission to bypass a newer journal or another pending operation.
        crate::project_git::check_data_operations_available(&self.workspace, id)?;
        permit.revalidate(&self.workspace, id)?;
        Ok(lease)
    }
    pub(crate) fn reserve_transient_operation(
        &self,
        id: &str,
        label: &str,
    ) -> Result<ProjectTaskLease, ApiError> {
        self.reserve_operation(id, ProjectOperation::Transient(label.into()))
    }
    pub(crate) fn reserve_data_save_finalization_task(
        &self,
        id: &str,
        task: &str,
        input: &crate::project_git::GitDataSaveFinalizeInput,
    ) -> Result<ProjectTaskLease, ApiError> {
        self.check_not_deleting(id)?;
        let lease = self.reserve_operation(id, ProjectOperation::Task(task.into()))?;
        crate::data_restore_guard::check_development_available(&self.workspace, id)?;
        crate::source_restore_guard::check_development_available(&self.workspace, id)?;
        crate::project_git::check_data_recovery_record(&self.workspace, id, input)?;
        Ok(lease)
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
        let writer_owner = if id.starts_with("prj_") {
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
            writer_owner,
        })
    }
    #[cfg(test)]
    pub(crate) fn fixture_has_database_worker(&self, id: &str) -> bool {
        self.workers.lock().unwrap().contains_key(id)
    }
    #[cfg(test)]
    pub(crate) fn fixture_panicked_database_worker(&self, id: &str) {
        let project = self.project(id).unwrap();
        let worker =
            DatabaseWorker::fixture_panicked(project.root.join(&project.manifest.database));
        assert!(self
            .workers
            .lock()
            .unwrap()
            .insert(id.into(), Arc::new(worker))
            .is_none());
    }
    /// Inject a genuine failed SQLite join without opening a recovery-protected
    /// live slot. The caller owns the separate disposable database fixture.
    #[cfg(test)]
    pub(crate) fn fixture_panicked_recovery_database_worker(&self, id: &str, database: PathBuf) {
        assert!(!crate::data_restore_guard::inspect(&self.workspace, id)
            .pending
            .is_empty());
        let worker = DatabaseWorker::fixture_panicked(database);
        assert!(self
            .workers
            .lock()
            .unwrap()
            .insert(id.into(), Arc::new(worker))
            .is_none());
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
    /// Re-register a project whose root or database identity changed. The
    /// retired maintenance workflow was its only caller; an explicit
    /// re-registration entry has not been designed yet, so keep the checked
    /// primitive instead of inventing an implicit recovery path.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn rebind(&self, id: &str, advance: bool) -> Result<String, ApiError> {
        crate::data_restore_guard::check_development_available(&self.workspace, id)?;
        let _lock = self.registration_lock.lock().unwrap();
        let root = self.project_root(id)?;
        check_no_symlinks(&root, false)?;
        let manifest: ProjectManifest = read_json(&root.join("project.json"))?;
        validate_manifest(&manifest, id, &root)?;
        let path = self
            .workspace
            .join(".platform/project-state")
            .join(format!("{id}.json"));
        let old: Registration = read_json(&path)?;
        let epoch = if advance {
            Uuid::new_v4().to_string()
        } else {
            old.document_epoch
        };
        let state = Registration {
            schema_version: 1,
            id: id.into(),
            project_root: root.clone(),
            directory_name: Some(root.file_name().unwrap().to_string_lossy().into_owned()),
            root_identity: identity(&root)?,
            database_path: manifest.database.clone(),
            database_identity: identity(&root.join(&manifest.database))?,
            document_epoch: epoch.clone(),
        };
        crate::builds::atomic_json(&path, &state)?;
        Ok(epoch)
    }
    pub(crate) fn check_workspace_available(&self) -> Result<(), ApiError> {
        if self.workspace_migrating.load(Ordering::Acquire) {
            if self.workspace_cleaning.load(Ordering::Acquire) {
                return Err(ApiError::new("WORKSPACE_BUSY", "History cleanup is in progress; wait for its final result before another management write").outcome("not_started"));
            }
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
        crate::data_restore_guard::check_workspace_available(&self.workspace)?;
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
        check_pending_deletions(&self.workspace)?;
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
        !self.active_tasks.lock().unwrap().is_empty()
    }
    pub(crate) fn begin_workspace_cleanup(&self) -> Result<(), ApiError> {
        self.begin_workspace_migration()?;
        self.workspace_cleaning.store(true, Ordering::Release);
        Ok(())
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
        self.workspace_cleaning.store(false, Ordering::Release);
        self.workspace_migrating.store(false, Ordering::Release);
    }
    fn deletion_path(&self, id: &str) -> PathBuf {
        self.workspace
            .join(".platform/project-state/deleting")
            .join(format!("{id}.json"))
    }
    pub(crate) fn pending_deletion(&self, id: &str) -> Result<Option<ProjectDeletion>, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let path = self.deletion_path(id);
        check_no_symlinks(&path, true)?;
        if !path.try_exists()? {
            return Ok(None);
        }
        let marker: ProjectDeletion = read_json(&path)?;
        if marker.schema_version != 1
            || marker.project_id != id
            || !valid_id(&marker.task_id)
            || Uuid::parse_str(&marker.target_epoch).is_err()
            || marker
                .source_epoch
                .as_deref()
                .is_some_and(|epoch| Uuid::parse_str(epoch).is_err())
        {
            return Err(ApiError::new(
                "PROJECT_DELETE_INCOMPLETE",
                "The retained deletion marker needs inspection",
            ));
        }
        Ok(Some(marker))
    }
    fn check_not_deleting(&self, id: &str) -> Result<(), ApiError> {
        check_deletion_marker(&self.deletion_path(id))
    }
    /// Validate only the registered root, so missing/corrupt SQLite remains deletable.
    pub(crate) fn deletion_root(&self, id: &str) -> Result<(PathBuf, FileIdentity), ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let _lock = self.registration_lock.lock().unwrap();
        let marker = self.pending_deletion(id)?;
        if marker.is_none() {
            self.require_active_identity(id)?;
        }
        let root = self.project_root(id)?;
        check_no_symlinks(&root, true)?;
        let actual = if root.try_exists()? {
            if !fs::symlink_metadata(&root)?.is_dir() {
                return Err(ApiError::new(
                    "PROJECT_IDENTITY_CHANGED",
                    "Deletion target is not an ordinary project directory",
                ));
            }
            Some(identity(&root)?)
        } else {
            None
        };
        let expected = marker
            .as_ref()
            .map(|m| &m.root_identity)
            .or(actual.as_ref())
            .ok_or_else(|| ApiError::new("PROJECT_MISSING", "Project directory is missing"))?;
        if let Some(actual) = &actual {
            if !expected.matches_captured(actual) {
                return Err(ApiError::new(
                    "PROJECT_IDENTITY_CHANGED",
                    "The deletion target directory was replaced; no files were removed",
                ));
            }
        }
        let registration = self
            .workspace
            .join(".platform/project-state")
            .join(format!("{id}.json"));
        check_no_symlinks(&registration, true)?;
        if registration.try_exists()? {
            let state: Registration = read_json(&registration)?;
            let epoch_matches = marker.as_ref().is_none_or(|m| {
                m.source_epoch.as_deref() == Some(state.document_epoch.as_str())
                    || m.target_epoch == state.document_epoch
            });
            if state.schema_version != 1
                || state.id != id
                || state.project_root != root
                || !state.root_identity.matches_captured(expected)
                || !epoch_matches
            {
                return Err(ApiError::new("PROJECT_IDENTITY_CHANGED", "The registered project no longer owns this deletion; existing files and records were retained"));
            }
        } else if actual.is_some() && marker.as_ref().is_some_and(|m| m.source_epoch.is_some()) {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "The registered deletion identity is missing; retained files need inspection",
            ));
        }
        Ok((root, expected.clone()))
    }
    pub(crate) fn validate_deletion_epoch(
        &self,
        id: &str,
        expected: Option<&str>,
    ) -> Result<(), ApiError> {
        self.deletion_root(id)?;
        let expected = expected.filter(|epoch| Uuid::parse_str(epoch).is_ok()).ok_or_else(|| {
            ApiError::new("PROJECT_DELETE_CONFIRMATION_REQUIRED", "Read the current project identity and explicitly supply expectedDocumentEpoch before permanent deletion").outcome("not_started")
        })?;
        let _lock = self.registration_lock.lock().unwrap();
        let path = self
            .workspace
            .join(".platform/project-state")
            .join(format!("{id}.json"));
        check_no_symlinks(&path, true)?;
        let current = if path.try_exists()? {
            Some(read_json::<Registration>(&path)?.document_epoch)
        } else {
            None
        };
        let pending = self.pending_deletion(id)?;
        if current.as_deref() == Some(expected)
            || pending.as_ref().is_some_and(|marker| {
                marker.source_epoch.as_deref() == Some(expected) || marker.target_epoch == expected
            })
        {
            Ok(())
        } else {
            Err(ApiError::new("PROJECT_DELETE_CONFIRMATION_CHANGED", "The project identity changed after it was displayed; read the current project and confirm permanent deletion again").outcome("not_started"))
        }
    }
    pub(crate) fn begin_deletion(
        &self,
        id: &str,
        task: &str,
        epoch: &str,
        expected: &FileIdentity,
    ) -> Result<ProjectDeletion, ApiError> {
        let (_, actual) = self.deletion_root(id)?;
        if !expected.matches_captured(&actual) {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Project directory changed before deletion",
            ));
        }
        let _lock = self.registration_lock.lock().unwrap();
        let registration = self
            .workspace
            .join(".platform/project-state")
            .join(format!("{id}.json"));
        let source_epoch = if registration.try_exists()? {
            Some(read_json::<Registration>(&registration)?.document_epoch)
        } else {
            None
        };
        let marker = ProjectDeletion {
            schema_version: 1,
            project_id: id.into(),
            task_id: task.into(),
            target_epoch: epoch.into(),
            root_identity: expected.clone(),
            directory_name: Some(
                self.project_root(id)?
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ),
            source_epoch,
        };
        crate::builds::atomic_json(&self.deletion_path(id), &marker)?;
        Ok(marker)
    }
    pub(crate) fn deleted_project_identity_absent(&self, id: &str) -> Result<bool, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let root = match self.project_root(id) {
            Ok(root) => root,
            Err(error) if error.code == "PROJECT_NOT_REGISTERED" => {
                bound_root(&self.workspace, id)?
            }
            Err(error) => return Err(error),
        };
        for path in [
            root,
            registration_path(&self.workspace, id),
            self.reservation_path(id),
            self.deletion_path(id),
        ] {
            check_no_symlinks(&path, true)?;
            match fs::symlink_metadata(path) {
                Ok(_) => return Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(true)
    }
    pub(crate) fn has_active_project_work(&self, id: &str) -> bool {
        self.workers.lock().unwrap().contains_key(id)
            || self.active_tasks.lock().unwrap().contains_key(id)
    }
    pub(crate) fn on_project_identity_removed(&self, observer: ProjectIdentityRemoved) {
        self.identity_removed.lock().unwrap().push(observer);
    }
    /// All writers and their actual database worker must already have joined.
    /// The deletion marker stays authoritative until every identity is cleared.
    pub(crate) fn complete_deleted_project_identity(
        &self,
        marker: &ProjectDeletion,
    ) -> Result<(), ApiError> {
        let id = &marker.project_id;
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let _lock = self.registration_lock.lock().unwrap();
        let current = self.pending_deletion(id)?;
        if let Some(current) = &current {
            if current.task_id != marker.task_id
                || current.target_epoch != marker.target_epoch
                || !current
                    .root_identity
                    .matches_captured(&marker.root_identity)
            {
                return Err(ApiError::new(
                    "PROJECT_IDENTITY_CHANGED",
                    "A different deletion owns this project",
                ));
            }
            if self.project_root(id)?.try_exists()? {
                return Err(ApiError::new(
                    "PROJECT_DELETE_INCOMPLETE",
                    "Project files remain; retry deletion",
                ));
            }
        } else if registration_path(&self.workspace, id).try_exists()?
            || self.reservation_path(id).try_exists()?
        {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Deletion ownership marker is missing",
            ));
        }
        let workers = self.workers.lock().unwrap();
        let active = self.active_tasks.lock().unwrap();
        if workers.contains_key(id) || active.contains_key(id) {
            return Err(ApiError::new(
                "PROJECT_BUSY",
                "Project writers must finish before removing its identity",
            ));
        }
        for path in [
            registration_path(&self.workspace, id),
            self.reservation_path(id),
        ] {
            check_no_symlinks(&path, true)?;
            if path.try_exists()? {
                fs::remove_file(&path)?;
                platform_native::filesystem::sync_directory(path.parent().unwrap())?;
            }
        }
        self.controls.lock().unwrap().remove(id);
        self.gates.lock().unwrap().remove(id);
        for observer in self.identity_removed.lock().unwrap().clone() {
            observer(id);
        }
        let path = self.deletion_path(id);
        if path.try_exists()? {
            fs::remove_file(&path)?;
            platform_native::filesystem::sync_directory(path.parent().unwrap())?;
        }
        Ok(())
    }
    fn reservation_path(&self, id: &str) -> PathBuf {
        self.workspace
            .join(".platform/project-state/reservations")
            .join(format!("{id}.json"))
    }
    fn is_active_identity(&self, id: &str) -> Result<bool, ApiError> {
        let path = self.reservation_path(id);
        check_no_symlinks(&path, true)?;
        Ok(!path.exists())
    }
    fn require_active_identity(&self, id: &str) -> Result<(), ApiError> {
        if !self.is_active_identity(id)? {
            return Err(ApiError::new(
                "PROJECT_REGISTRATION_PENDING",
                "Project registration is being committed",
            )
            .outcome("not_started"));
        }
        Ok(())
    }
    pub(crate) fn registered_epoch(&self, id: &str) -> Result<Option<String>, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let path = self
            .workspace
            .join(".platform/project-state")
            .join(format!("{id}.json"));
        check_no_symlinks(&path, true)?;
        if !path.is_file() || !self.is_active_identity(id)? || self.pending_deletion(id)?.is_some()
        {
            return Ok(None);
        }
        let registration: Registration = read_json(&path)?;
        let root = self.project_root(id)?;
        if !root.is_dir()
            || registration.project_root != root
            || !identity_matches(&registration.root_identity, &root)?
        {
            return Ok(None);
        }
        Ok(Some(registration.document_epoch))
    }
    /// Keep identity validation and a synchronous cache admission ordered with
    /// permanent removal. The callback may acquire cache locks, but must not
    /// re-enter registration methods or await while this lock is held.
    pub(crate) fn with_registered_epoch<T>(
        &self,
        id: &str,
        epoch: &str,
        admit: impl FnOnce() -> Result<T, ApiError>,
    ) -> Result<Option<T>, ApiError> {
        let _registration = self.registration_lock.lock().unwrap();
        if self.registered_epoch(id)?.as_deref() != Some(epoch) {
            return Ok(None);
        }
        admit().map(Some)
    }
    pub(crate) fn metadata_root(
        &self,
        id: &str,
    ) -> Result<(PathBuf, Option<ProjectManifest>), ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        self.require_active_identity(id)?;
        let root = self.project_root(id)?;
        check_no_symlinks(&root, false)?;
        if !root.is_dir() {
            return Err(ApiError::new(
                "PROJECT_MISSING",
                "Project directory is missing",
            ));
        }
        let manifest = read_json::<ProjectManifest>(&root.join("project.json")).ok();
        Ok((root, manifest))
    }
    fn check_identity_locked(&self, id: &str, except_task: Option<&str>) -> Result<(), ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        crate::data_restore_guard::check_development_available(&self.workspace, id)?;
        if let Some(marker) = self.pending_deletion(id)? {
            return Err(ApiError::new("PROJECT_ID_CONFLICT", "Project deletion is incomplete; explicitly retry its deletion before reusing this ID").detail("taskId", marker.task_id));
        }
        let reservation = self.reservation_path(id);
        check_no_symlinks(&reservation, true)?;
        if reservation.exists() {
            let record: Value = read_json(&reservation)?;
            if record["taskId"].as_str() != except_task {
                return Err(ApiError::new(
                    "PROJECT_ID_CONFLICT",
                    "Project ID is reserved by an unfinished operation",
                )
                .detail("conflict", record));
            }
        }
        let root = match self.project_root(id) {
            Ok(root) => root,
            Err(error) if error.code == "PROJECT_NOT_REGISTERED" => {
                bound_root(&self.workspace, id)?
            }
            Err(error) => return Err(error),
        };
        let registered = self
            .workspace
            .join(".platform/project-state")
            .join(format!("{id}.json"));
        if root.exists() || registered.exists() {
            let name = read_json::<ProjectManifest>(&root.join("project.json"))
                .map(|m| m.name)
                .unwrap_or_else(|_| id.into());
            return Err(
                ApiError::new("PROJECT_ID_CONFLICT", "Project ID is already registered").detail(
                    "conflict",
                    json!({"projectId":id,"name":name,"path":root,"state":"active"}),
                ),
            );
        }
        Ok(())
    }
    pub(crate) fn check_identity_available(
        &self,
        id: &str,
        except_task: Option<&str>,
    ) -> Result<(), ApiError> {
        let _lock = self.registration_lock.lock().unwrap();
        self.check_identity_locked(id, except_task)
    }
    pub(crate) fn reserve_identity(&self, id: &str, task: &str) -> Result<(), ApiError> {
        let _lock = self.registration_lock.lock().unwrap();
        self.check_workspace_available()?;
        if !valid_id(task) {
            return Err(ApiError::new("INVALID_TASK_ID", "Invalid task ID"));
        }
        let reservation_path = self.reservation_path(id);
        check_no_symlinks(&reservation_path, true)?;
        if reservation_path.try_exists()? {
            let reservation: Value = read_json(&reservation_path)?;
            if reservation["projectId"] == id
                && reservation["taskId"] == task
                && reservation.get("directoryName").is_some()
            {
                self.check_not_deleting(id)?;
                self.project_root(id)?;
                return Ok(());
            }
        }
        self.check_identity_locked(id, Some(task))?;
        crate::builds::atomic_json(
            &self.reservation_path(id),
            &json!({"projectId":id,"taskId":task}),
        )
    }
    pub(crate) fn reserve_named_identity(
        &self,
        id: &str,
        task: &str,
        name: &str,
    ) -> Result<PathBuf, ApiError> {
        let _lock = self.registration_lock.lock().unwrap();
        self.reserve_named_identity_locked(id, task, name)
    }
    fn retained_directory_names(
        &self,
        except_owner: Option<&str>,
        current_reservation: &Path,
    ) -> Result<std::collections::HashSet<String>, ApiError> {
        let mut names = std::collections::HashSet::new();
        let states = self.workspace.join(".platform/project-state");
        for kind in ["", "reservations", "deleting"] {
            let directory = states.join(kind);
            check_no_symlinks(&directory, true)?;
            if !directory.try_exists()? {
                continue;
            }
            for entry in fs::read_dir(&directory)? {
                let path = entry?.path();
                if path == current_reservation
                    || path.extension().is_none_or(|extension| extension != "json")
                {
                    continue;
                }
                let Some(owner) = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .filter(|owner| valid_id(owner))
                else {
                    continue;
                };
                if Some(owner) == except_owner {
                    continue;
                }
                if kind.is_empty() {
                    let record: Registration = read_json(&path)?;
                    if record.schema_version != 1
                        || record.id != owner
                        || Uuid::parse_str(&record.document_epoch)
                            .map(|epoch| epoch.to_string() != record.document_epoch)
                            .unwrap_or(true)
                    {
                        return Err(directory_mapping_error());
                    }
                    retain_directory_claims(
                        &mut names,
                        owner,
                        record.directory_name.as_deref(),
                        Some(&record.project_root),
                    )?;
                } else {
                    let record: Value = read_json(&path)?;
                    if record["projectId"] != owner
                        || (kind == "reservations"
                            && record["taskId"].as_str().is_none_or(|task| !valid_id(task)))
                    {
                        return Err(directory_mapping_error());
                    }
                    let name = record
                        .get("directoryName")
                        .map(|value| value.as_str().ok_or_else(directory_mapping_error))
                        .transpose()?;
                    let root = record
                        .get("projectRoot")
                        .map(|value| {
                            value
                                .as_str()
                                .map(Path::new)
                                .ok_or_else(directory_mapping_error)
                        })
                        .transpose()?;
                    retain_directory_claims(&mut names, owner, name, root)?;
                }
            }
        }
        Ok(names)
    }
    fn reserve_named_identity_locked(
        &self,
        id: &str,
        task: &str,
        name: &str,
    ) -> Result<PathBuf, ApiError> {
        self.check_workspace_available()?;
        if !valid_id(id) || !valid_id(task) {
            return Err(ApiError::new(
                "INVALID_PROJECT_ID",
                "Invalid project or task ID",
            ));
        }
        let reservation_path = self.reservation_path(id);
        check_no_symlinks(&reservation_path, true)?;
        if reservation_path.try_exists()? {
            let reservation: Value = read_json(&reservation_path)?;
            if reservation["projectId"] != id || reservation["taskId"] != task {
                return Err(ApiError::new(
                    "PROJECT_ID_CONFLICT",
                    "Project ID is reserved by a different operation",
                ));
            }
            if reservation.get("directoryName").is_some() {
                // Reinspection or a display-name edit never changes a task's
                // already durable destination, including after publication.
                self.check_not_deleting(id)?;
                let root = self.project_root(id)?;
                self.require_new_project_location(&root)?;
                return Ok(root);
            }
        }
        self.check_identity_locked(id, Some(task))?;
        // Every newly allocated identity receives a fresh readable destination.
        let base = crate::project_directory_name::readable_directory_name(name, id);
        checked_directory_name(&base)?;
        let retained_names = self.retained_directory_names(None, &reservation_path)?;
        let mut directory_name = base.clone();
        let mut suffix = 1u32;
        loop {
            let root = self.workspace.join("projects").join(&directory_name);
            // A claimed basename is occupied even when its entity is missing,
            // inaccessible, or a link. Do not inspect an unrelated old target.
            let occupied = retained_names.contains(&directory_name)
                || bound_root(&self.workspace, &directory_name)?.try_exists()?;
            if !occupied {
                self.require_new_project_location(&root)?;
                crate::builds::atomic_json(
                    &reservation_path,
                    &json!({"projectId":id,"taskId":task,"directoryName":directory_name}),
                )?;
                return Ok(root);
            }
            suffix += 1;
            if suffix > 10_000 {
                return Err(ApiError::new(
                    "PROJECT_DIRECTORY_CONFLICT",
                    "No unused project directory name was available",
                ));
            }
            directory_name = format!("{base}-{suffix}");
            if directory_name.len() > 128 {
                return Err(ApiError::new(
                    "PROJECT_DIRECTORY_CONFLICT",
                    "The retained directory name cannot accept a safe conflict suffix",
                ));
            }
        }
    }
    pub(crate) fn release_identity(&self, id: &str, task: &str) -> Result<(), ApiError> {
        let _lock = self.registration_lock.lock().unwrap();
        self.release_identity_locked(id, task)
    }
    fn release_identity_locked(&self, id: &str, task: &str) -> Result<(), ApiError> {
        let path = self.reservation_path(id);
        check_no_symlinks(&path, true)?;
        if path.exists() {
            let record: Value = read_json(&path)?;
            if record["taskId"] != task {
                return Err(ApiError::new(
                    "PROJECT_ID_CONFLICT",
                    "Reservation belongs to a different operation",
                ));
            }
            fs::remove_file(&path)?;
            platform_native::filesystem::sync_directory(path.parent().unwrap())?;
        }
        Ok(())
    }
    pub(crate) fn register_lifecycle(
        &self,
        id: &str,
        epoch: &str,
        task: &str,
        allow_damaged: bool,
    ) -> Result<(), ApiError> {
        crate::data_restore_guard::check_development_available(&self.workspace, id)?;
        let _lock = self.registration_lock.lock().unwrap();
        let reservation_path = self.reservation_path(id);
        check_no_symlinks(&reservation_path, true)?;
        if !reservation_path.exists() {
            let path = self
                .workspace
                .join(".platform/project-state")
                .join(format!("{id}.json"));
            let registration: Registration = read_json(&path)?;
            let root = self.project_root(id)?;
            if registration.document_epoch == epoch
                && registration.project_root == root
                && identity_matches(&registration.root_identity, &root)?
            {
                let matches = if registration.database_path.is_empty() {
                    allow_damaged
                } else {
                    let db = checked_relative(&root, &registration.database_path, true)?;
                    if db.is_file() {
                        identity_matches(&registration.database_identity, &db)?
                            || (allow_damaged
                                && is_missing_identity(&registration.database_identity))
                    } else {
                        allow_damaged && is_missing_identity(&registration.database_identity)
                    }
                };
                if matches {
                    return Ok(());
                }
            }
            return Err(ApiError::new(
                "PROJECT_ID_CONFLICT",
                "An already completed registration no longer matches this operation",
            ));
        }
        let reservation: Value = read_json(&reservation_path)?;
        if reservation["taskId"] != task {
            return Err(ApiError::new(
                "PROJECT_ID_CONFLICT",
                "Registration does not own the ID reservation",
            ));
        }
        self.write_lifecycle_registration(id, epoch, allow_damaged)?;
        self.release_identity_locked(id, task)
    }
    fn write_lifecycle_registration(
        &self,
        id: &str,
        epoch: &str,
        allow_damaged: bool,
    ) -> Result<(), ApiError> {
        let root = self.project_root(id)?;
        self.record_project_root_ownership(id, &root)?;
        self.write_lifecycle_registration_at(id, epoch, allow_damaged, root)
    }
    fn write_lifecycle_registration_at(
        &self,
        id: &str,
        epoch: &str,
        allow_damaged: bool,
        root: PathBuf,
    ) -> Result<(), ApiError> {
        crate::data_restore_guard::check_development_available(&self.workspace, id)?;
        if !valid_id(id) || Uuid::parse_str(epoch).is_err() {
            return Err(ApiError::new(
                "INVALID_PROJECT_ID",
                "Invalid project identity",
            ));
        }
        check_no_symlinks(&root, false)?;
        let manifest = read_json::<ProjectManifest>(&root.join("project.json"));
        let database_path = match manifest {
            Ok(ref manifest) => {
                let valid = validate_manifest(manifest, id, &root);
                if valid.is_err() && !allow_damaged {
                    return valid;
                }
                if valid.is_ok() {
                    manifest.database.clone()
                } else {
                    String::new()
                }
            }
            Err(error) if !allow_damaged => return Err(error),
            _ => String::new(),
        };
        let db = if database_path.is_empty() {
            None
        } else {
            Some(checked_relative(&root, &database_path, true)?)
        };
        let db_identity = match db.as_ref().filter(|p| p.is_file()) {
            Some(path) => {
                if crate::project_file_io::link_count(path)? != 1 {
                    if allow_damaged {
                        FileIdentity::unavailable()
                    } else {
                        return Err(ApiError::new(
                            "UNSAFE_DATABASE_PATH",
                            "Database hard links are not supported",
                        ));
                    }
                } else {
                    identity(path)?
                }
            }
            None if allow_damaged => FileIdentity::unavailable(),
            None => {
                return Err(ApiError::new(
                    "DATABASE_MISSING",
                    "No database was created for the missing registered file",
                ))
            }
        };
        let state = Registration {
            schema_version: 1,
            id: id.into(),
            project_root: root.clone(),
            directory_name: Some(root.file_name().unwrap().to_string_lossy().into_owned()),
            root_identity: identity(&root)?,
            database_path,
            database_identity: db_identity,
            document_epoch: epoch.into(),
        };
        crate::builds::atomic_json(
            &self
                .workspace
                .join(".platform/project-state")
                .join(format!("{id}.json")),
            &state,
        )?;
        Ok(())
    }
    pub(crate) fn retain_migrated_diagnostic(
        &self,
        id: &str,
        error: ApiError,
    ) -> Result<(), ApiError> {
        crate::data_restore_guard::check_development_available(&self.workspace, id)?;
        let _lock = self.registration_lock.lock().unwrap();
        let path = self
            .workspace
            .join(".platform/project-state")
            .join(format!("{id}.json"));
        if path.is_file() {
            let mut record: Registration = read_json(&path)?;
            record.database_identity = FileIdentity::unavailable();
            crate::builds::atomic_json(&path, &record)?;
        }
        let control = self.control(id)?;
        let mut state = control.get();
        state.last_error=Some(ApiError::new("MIGRATED_PROJECT_DIAGNOSTIC","Source project was already blocked; migration preserved its files and did not declare it healthy").detail("sourceIssue",serde_json::json!(error)));
        control.set(state)
    }
    pub(crate) fn rebind_migrated(&self, id: &str, epoch: &str) -> Result<(), ApiError> {
        crate::data_restore_guard::check_development_available(&self.workspace, id)?;
        let _lock = self.registration_lock.lock().unwrap();
        if !valid_id(id) || Uuid::parse_str(epoch).is_err() {
            return Err(directory_mapping_error());
        }
        let path = registration_path(&self.workspace, id);
        check_no_symlinks(&path, true)?;
        // Only the verified migration workflow may rebase a copied absolute
        // root. Normal lookup deliberately rejects an old-workspace path.
        if !path.try_exists()? {
            return Err(ApiError::new(
                "PROJECT_NOT_REGISTERED",
                "Migration requires a current project registration",
            ));
        }
        let mut previous: Registration = read_json(&path)?;
        if previous.schema_version != 1 || previous.id != id {
            return Err(directory_mapping_error());
        }
        let name = previous
            .directory_name
            .as_deref()
            .ok_or_else(directory_mapping_error)?;
        checked_directory_name(name)?;
        if previous
            .project_root
            .file_name()
            .and_then(|name| name.to_str())
            != Some(name)
        {
            return Err(directory_mapping_error());
        }
        let root = bound_root(&self.workspace, name)?;
        if !root.exists() {
            previous.project_root = root;
            previous.document_epoch = epoch.into();
            return crate::builds::atomic_json(&path, &previous);
        }
        self.write_lifecycle_registration_at(id, epoch, true, root)
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
}

fn database_close_error(issues: &[crate::shutdown::ShutdownIssue]) -> ApiError {
    ApiError::new(
        "DATABASE_CLOSE_FAILED",
        "数据库线程未能确认正常关闭；保留实例与恢复现场",
    )
    .detail("shutdownIssues", serde_json::json!(issues))
}

enum ProjectOperation {
    Task(String),
    Transient(String),
}
pub(crate) struct ProjectTaskLease {
    active: Arc<Mutex<HashMap<String, ProjectOperation>>>,
    id: String,
    writer_owner: Option<crate::durable_writer::WriterOwner>,
}
impl ProjectTaskLease {
    pub(crate) fn writer_owner(&self) -> Option<crate::durable_writer::WriterOwner> {
        self.writer_owner.clone()
    }
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
pub(crate) fn validate_manifest(
    manifest: &ProjectManifest,
    id: &str,
    root: &Path,
) -> Result<(), ApiError> {
    let invalid = |message: &str| ApiError::new("INVALID_PROJECT_MANIFEST", message);
    if manifest.manifest_version != 1 || manifest.sdk_api_version != 1 {
        return Err(ApiError::new(
            "UNSUPPORTED_PROJECT_VERSION",
            "Only manifestVersion 1 and sdkApiVersion 1 are supported",
        ));
    }
    if manifest.id != id {
        return Err(invalid("Project directory and manifest IDs differ"));
    }
    if manifest.name.trim().is_empty() {
        return Err(invalid("Project name is empty"));
    }
    for path in [
        &manifest.source_dir,
        &manifest.output_dir,
        &manifest.entry,
        &manifest.database,
        &manifest.attachments_dir,
    ] {
        checked_relative(root, path, true)?;
    }
    let source = Path::new(&manifest.source_dir);
    let output = Path::new(&manifest.output_dir);
    let data = Path::new("data");
    let meta = Path::new("meta");
    let platform = Path::new(".platform");
    for path in [source, output] {
        if [data, meta, platform]
            .into_iter()
            .any(|fixed| path.starts_with(fixed) || fixed.starts_with(path))
        {
            return Err(invalid(
                "Source/output cannot overlap data, meta or platform control files",
            ));
        }
    }
    if source.starts_with(output) || output.starts_with(source) {
        return Err(invalid("Source and output directories cannot overlap"));
    }
    if !Path::new(&manifest.entry).starts_with(output) || Path::new(&manifest.entry) == output {
        return Err(invalid("Entry must be a file inside outputDir"));
    }
    if !Path::new(&manifest.database).starts_with(data)
        || !Path::new(&manifest.attachments_dir).starts_with(data)
        || Path::new(&manifest.attachments_dir) == data
        || Path::new(&manifest.database).starts_with(&manifest.attachments_dir)
    {
        return Err(invalid(
            "Database and attachments must be separate paths inside data/",
        ));
    }
    if manifest
        .build
        .get("script")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(invalid("build.script must name a package script"));
    }
    for key in ["readme", "database", "platform"] {
        let path = manifest
            .docs
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("docs must provide readme, database and platform paths"))?;
        checked_relative(root, path, true)?;
    }
    crate::runtime_policy::RuntimePolicy::parse(&manifest.runtime_requirements)?;
    Ok(())
}
fn identity(path: &Path) -> Result<FileIdentity, ApiError> {
    FileIdentity::capture(path)
        .map_err(|error| ApiError::new("FILESYSTEM_ERROR", error.to_string()))
}
fn identity_matches(expected: &FileIdentity, path: &Path) -> Result<bool, ApiError> {
    // A preserved damaged database has no registered file identity. It must
    // remain blocked until an explicit maintenance/lifecycle rebind occurs.
    if is_missing_identity(expected) {
        return Ok(false);
    }
    expected
        .matches_path(path)
        .map_err(|error| ApiError::new("FILESYSTEM_ERROR", error.to_string()))
}
fn is_missing_identity(identity: &FileIdentity) -> bool {
    identity.is_legacy() && identity.legacy_pair() == (0, 0)
}
fn check_deletion_marker(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    if path.try_exists()? {
        let marker: ProjectDeletion = read_json(path)?;
        return Err(ApiError::new("PROJECT_DELETE_INCOMPLETE", "This project is quarantined after permanent deletion started; explicitly retry deletion to remove the remaining files")
            .detail("taskId", marker.task_id)
            .detail("restoreAvailable", false));
    }
    Ok(())
}
fn removed_project_error(id: &str) -> ApiError {
    ApiError::new(
        "PROJECT_REMOVED",
        "This project was permanently removed; open another project from the platform",
    )
    .detail("projectId", id)
}
pub(crate) fn check_pending_deletions(workspace: &Path) -> Result<(), ApiError> {
    let deleting = workspace.join(".platform/project-state/deleting");
    check_no_symlinks(&deleting, true)?;
    if deleting.try_exists()? && fs::read_dir(&deleting)?.next().is_some() {
        return Err(ApiError::new("PROJECT_DELETE_INCOMPLETE", "Explicitly finish the retained project deletion before migrating or cleaning the workspace"));
    }
    Ok(())
}
pub fn checked_relative(
    root: &Path,
    relative: &str,
    allow_missing: bool,
) -> Result<PathBuf, ApiError> {
    let path = Path::new(relative);
    if relative.is_empty()
        || relative.contains('\\')
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ApiError::new(
            "INVALID_PATH",
            "Paths must be clean project-relative paths",
        ));
    }
    let result = root.join(path);
    check_no_symlinks(&result, allow_missing)?;
    Ok(result)
}
pub use crate::path_checks::check_no_symlinks;
fn ensure_dir(path: &Path) -> Result<(), ApiError> {
    crate::project_file_io::ensure_directory(path)
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ApiError> {
    check_no_symlinks(path, false)?;
    let metadata = fs::metadata(path)?;
    if metadata.len() > database::JSON_BODY_MAX_BYTES as u64 {
        return Err(ApiError::new(
            "REQUEST_TOO_LARGE",
            "Project control file is too large",
        ));
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn write_new(path: &Path, content: &[u8]) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    let mut file = crate::project_file_io::create_new_file(path)?;
    file.write_all(content)?;
    file.sync_all()?;
    Ok(())
}

fn publish_registration(path: &Path, state: &Registration) -> Result<(), ApiError> {
    let staging = path.with_file_name(format!(".registration-{}", Uuid::new_v4()));
    let publication = (|| {
        write_new(&staging, &serde_json::to_vec_pretty(state)?)?;
        // An atomic create-if-absent publication never replaces an unexpected existing identity.
        fs::hard_link(&staging, path)?;
        fs::remove_file(&staging)?;
        platform_native::filesystem::sync_directory(path.parent().unwrap())?;
        Ok::<(), ApiError>(())
    })();
    if publication.is_err() {
        let _ = fs::remove_file(&staging);
    }
    publication
}

/// Publish a complete candidate without replacing even an empty destination.
/// Open parent descriptors keep the rename bound to the checked directories.
pub(crate) fn publish_directory_noreplace(from: &Path, to: &Path) -> Result<(), ApiError> {
    crate::project_directory_io::publish_directory_noreplace(from, to, directory_mapping_error)
}

#[cfg(all(test, unix))]
#[path = "project_directory_mapping_tests.rs"]
mod project_directory_mapping_tests;

#[cfg(all(test, windows))]
#[path = "project_directory_mapping_windows_tests.rs"]
mod project_directory_mapping_windows_tests;

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
        store
            .create(CreateProjectInput {
                name: name.into(),
                description: None,
            })
            .await
            .unwrap()
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
        assert_eq!(a.state, "unbuilt");
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
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn stable_project_registration_survives_device_renumbering_without_rewriting() {
        let (temp, store) = store();
        let project = create(&store, "Disposable stable registration").await;
        let path = store
            .workspace
            .join(".platform/project-state")
            .join(format!("{}.json", project.id));
        let mut registration: Registration = read_json(&path).unwrap();
        assert!(!registration.root_identity.is_legacy());
        assert!(!registration.database_identity.is_legacy());
        registration.root_identity.device += 1;
        registration.database_identity.device += 1;
        let bytes = serde_json::to_vec_pretty(&registration).unwrap();
        fs::write(&path, &bytes).unwrap();

        let mut handle = store.project(&project.id).unwrap();
        handle.check().unwrap();
        assert_eq!(
            store.registered_epoch(&project.id).unwrap(),
            Some(project.document_epoch.clone())
        );
        store
            .register_lifecycle(
                &project.id,
                &project.document_epoch,
                "completed-task",
                false,
            )
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);

        handle.registration.root_identity.inode += 1;
        assert_eq!(handle.check().unwrap_err().code, "PROJECT_IDENTITY_CHANGED");
        handle.registration.root_identity = registration.root_identity.clone();
        handle.registration.database_identity.inode += 1;
        assert_eq!(handle.check().unwrap_err().code, "PROJECT_IDENTITY_CHANGED");
        handle.registration.database_identity = registration.database_identity.clone();
        handle.registration.database_identity.volume_uuid = Some(Uuid::new_v4().to_string());
        assert_eq!(handle.check().unwrap_err().code, "PROJECT_IDENTITY_CHANGED");

        store.shutdown().await;
        drop(store);
        let reopened = ProjectStore::new(temp.path().canonicalize().unwrap()).unwrap();
        reopened.project(&project.id).unwrap().check().unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        registration.root_identity.volume_uuid = Some(Uuid::new_v4().to_string());
        fs::write(&path, serde_json::to_vec_pretty(&registration).unwrap()).unwrap();
        assert_eq!(
            reopened.project(&project.id).err().unwrap().code,
            "PROJECT_IDENTITY_CHANGED"
        );
        assert!(reopened.registered_epoch(&project.id).unwrap().is_none());
        reopened.shutdown().await;
    }
    #[tokio::test]
    async fn legacy_project_registration_stays_strict_and_is_not_implicitly_upgraded() {
        let (_temp, store) = store();
        let project = create(&store, "Disposable legacy registration").await;
        let path = store
            .workspace
            .join(".platform/project-state")
            .join(format!("{}.json", project.id));
        let mut registration: Registration = read_json(&path).unwrap();
        registration.root_identity.volume_uuid = None;
        registration.database_identity.volume_uuid = None;
        let bytes = serde_json::to_vec_pretty(&registration).unwrap();
        fs::write(&path, &bytes).unwrap();
        let mut handle = store.project(&project.id).unwrap();
        handle.check().unwrap();
        assert_eq!(
            store.registered_epoch(&project.id).unwrap(),
            Some(project.document_epoch.clone())
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);

        handle.registration.root_identity.device += 1;
        assert_eq!(handle.check().unwrap_err().code, "PROJECT_IDENTITY_CHANGED");
        registration.root_identity.device += 1;
        fs::write(&path, serde_json::to_vec_pretty(&registration).unwrap()).unwrap();
        assert_eq!(
            store.project(&project.id).err().unwrap().code,
            "PROJECT_IDENTITY_CHANGED"
        );
        assert!(store.registered_epoch(&project.id).unwrap().is_none());

        registration.root_identity.device -= 1;
        registration.database_identity.device += 1;
        let rejected = serde_json::to_vec_pretty(&registration).unwrap();
        fs::write(&path, &rejected).unwrap();
        assert_eq!(
            store.project(&project.id).err().unwrap().code,
            "PROJECT_IDENTITY_CHANGED"
        );
        assert_eq!(
            store
                .register_lifecycle(
                    &project.id,
                    &project.document_epoch,
                    "completed-task",
                    false
                )
                .unwrap_err()
                .code,
            "PROJECT_ID_CONFLICT"
        );
        assert_eq!(fs::read(&path).unwrap(), rejected);
        store.shutdown().await;
    }
    #[tokio::test]
    async fn damaged_database_identity_requires_an_explicit_rebind() {
        let (_temp, store) = store();
        let project = create(&store, "Disposable damaged registration").await;
        let path = store
            .workspace
            .join(".platform/project-state")
            .join(format!("{}.json", project.id));
        store
            .retain_migrated_diagnostic(
                &project.id,
                ApiError::new("PROJECT_IDENTITY_CHANGED", "Disposable source diagnostic"),
            )
            .unwrap();
        let bytes = fs::read(&path).unwrap();
        let registration: Registration = serde_json::from_slice(&bytes).unwrap();
        assert!(is_missing_identity(&registration.database_identity));
        assert!(!identity_matches(
            &registration.database_identity,
            &project.project_root.join("data/main.sqlite")
        )
        .unwrap());
        assert_eq!(
            store.project(&project.id).err().unwrap().code,
            "PROJECT_IDENTITY_CHANGED"
        );
        store
            .register_lifecycle(&project.id, &project.document_epoch, "completed-task", true)
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);

        assert_eq!(
            store.rebind(&project.id, false).unwrap(),
            project.document_epoch
        );
        let rebound: Registration = read_json(&path).unwrap();
        assert!(!is_missing_identity(&rebound.database_identity));
        #[cfg(target_os = "macos")]
        {
            assert!(!rebound.root_identity.is_legacy());
            assert!(!rebound.database_identity.is_legacy());
        }
        store.project(&project.id).unwrap().check().unwrap();
        store.shutdown().await;
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
    async fn old_epoch_and_missing_database_never_write_or_create() {
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
        assert_eq!(
            exec(&store, &p.id, "SELECT 1", vec![])
                .await
                .unwrap_err()
                .code,
            "DATABASE_MISSING"
        );
        assert!(!db.exists());
        assert_eq!(store.info(&p.id).await.unwrap().state, "missing");
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
    async fn symlinks_and_replaced_database_are_rejected() {
        let (temp, store) = store();
        let p = create(&store, "Paths").await;
        std::os::unix::fs::symlink(temp.path(), p.project_root.join("dist")).unwrap();
        assert_eq!(store.info(&p.id).await.unwrap().state, "error");
        fs::remove_file(p.project_root.join("dist")).unwrap();
        let db = p.project_root.join("data/main.sqlite");
        let replacement = p.project_root.join("data/replacement.sqlite");
        database::initialize_database(&replacement).unwrap();
        fs::rename(replacement, &db).unwrap();
        assert_eq!(
            exec(&store, &p.id, "SELECT 1", vec![])
                .await
                .unwrap_err()
                .code,
            "PROJECT_IDENTITY_CHANGED"
        );
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
            retained.join("project.json"),
            project.project_root.join("project.json"),
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
        assert!(store.registered_epoch(&project.id).unwrap().is_none());
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
            .any(|info| info.id == p.id && info.state == "unbuilt"));
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
        for kind in ["", "reservations", "retired", "deleting"] {
            assert!(!store
                .workspace
                .join(".platform/project-state")
                .join(kind)
                .join(format!("{}.json", project.id))
                .exists());
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_identity_finalization_waits_for_real_sqlite_close_before_namespace_move() {
        let (temporary, store) = store();
        let project = create(&store, "Disposable complete identity").await;
        let retained_handle = store.project(&project.id).unwrap();
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
        for kind in ["", "reservations", "retired", "deleting"] {
            assert!(!store
                .workspace
                .join(".platform/project-state")
                .join(kind)
                .join(format!("{}.json", project.id))
                .exists());
        }
    }
}
