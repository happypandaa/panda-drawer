//! W2.1 projects only: new private roots and fully registered bundled samples.
//! No adoption, recovery, external SQL, build execution, deletion or path-based
//! SQLite opening is provided here. Failed creation evidence is retained.
use crate::windows_workspace::{
    WindowsObjectIdentity, WindowsWorkspace, WorkspaceDirectory, WorkspaceError, WorkspaceLock,
};
use crate::{APP_IDENTIFIER, ENVIRONMENT};
use platform_project_format::{
    input_digest, tree_digest, ApiError, CreateProjectInput, ProjectInfo, ProjectManifest,
    StarterDistribution, StarterProvenance, TreeFile, DIGEST_RULE_VERSION,
};
use rusqlite::{Connection, MAIN_DB};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const FILE_LIMIT: usize = 8_000_000;
const TOTAL_LIMIT: usize = 64_000_000;
const FILE_COUNT_LIMIT: usize = 1024;
const PROJECT_LIMIT: usize = 32;
const STAGE: &str = "windowsW2Sample";

#[derive(Clone, Debug)]
pub struct BundledProject {
    /// Installation-resource logical paths, never host filesystem paths.
    pub files: Vec<(String, Vec<u8>)>,
}

#[derive(Clone, Debug)]
pub struct OpenedProject {
    pub manifest: ProjectManifest,
    pub document_epoch: String,
    pub starter: StarterProvenance,
    pub project_root: PathBuf,
}

struct Bundle {
    files: BTreeMap<String, Vec<u8>>,
    distribution: StarterDistribution,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Marker {
    schema_version: u32,
    identifier: String,
    environment: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    schema_version: u32,
    identifier: String,
    environment: String,
    workspace_root: PathBuf,
    last_port: Option<u16>,
    migration_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stage {
    schema_version: u32,
    identifier: String,
    environment: String,
    stage: String,
    data_root_identity: WindowsObjectIdentity,
    ancestor_identities: Vec<WindowsObjectIdentity>,
    workspace_identity: WindowsObjectIdentity,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileStamp {
    identity: WindowsObjectIdentity,
    sha256: String,
}

/// This local control record explicitly stores Windows ID128, never Unix inode.
/// Portable project.json, SQLite, source, SDK and starter records are unchanged.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Registration {
    schema_version: u32,
    identity_kind: String,
    id: String,
    project_root: PathBuf,
    root_identity: WindowsObjectIdentity,
    database_path: String,
    database_identity: WindowsObjectIdentity,
    document_epoch: String,
    files: BTreeMap<String, FileStamp>,
}

struct ActiveRoot {
    workspace: WindowsWorkspace,
    _lock: WorkspaceLock,
}

pub struct WindowsProjectStore {
    bundle: Bundle,
    active: Mutex<Option<ActiveRoot>>,
    #[cfg(test)]
    test_root: Option<PathBuf>,
}

fn failure(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message).outcome("not_started")
}
fn fs_error(error: WorkspaceError) -> ApiError {
    failure(error.code, &error.message)
}
fn json_error(error: serde_json::Error) -> ApiError {
    failure("W2_RECORD_INVALID", &format!("W2.1 记录无法识别：{error}"))
}
fn sqlite_error(error: rusqlite::Error) -> ApiError {
    failure("DATABASE_INVALID", &format!("SQLite 样本检查失败：{error}"))
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn valid_id(id: &str) -> bool {
    id.starts_with("prj_") && id.len() == 36 && id.as_bytes()[4..].iter().all(u8::is_ascii_hexdigit)
}
fn uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn starter_id(value: &str) -> bool {
    value.starts_with("starter_")
        && value.len() == 40
        && value.as_bytes()[8..].iter().all(u8::is_ascii_hexdigit)
}
fn distribution_supported(value: &StarterDistribution) -> bool {
    value.schema_version == 1
        && value.origin == "bundledTemplate"
        && (value.template_version == "react-ts-vite-0.1.4"
            || cfg!(test) && value.template_version == "synthetic-w2")
        && value.sdk_version == "0.1.0"
        && value.entry_relative == "index.html"
        && digest(&value.source_digest)
        && digest(&value.artifact_digest)
}

fn logical(path: &str) -> Result<Vec<&str>, ApiError> {
    let parts: Vec<_> = path.split('/').collect();
    if parts.is_empty()
        || parts.len() > 32
        || parts.iter().any(|part| {
            let stem = part
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9'));
            part.is_empty()
                || *part == "."
                || *part == ".."
                || part.encode_utf16().count() > 255
                || part.ends_with([' ', '.'])
                || reserved
                || part.chars().any(|ch| {
                    ch < ' ' || matches!(ch, '\\' | ':' | '"' | '*' | '?' | '<' | '>' | '|')
                })
        })
    {
        return Err(failure(
            "INVALID_PATH",
            "仅接受无别名、无点片段的逻辑相对文件路径。",
        ));
    }
    Ok(parts)
}

fn read_json<T: DeserializeOwned>(
    directory: &WorkspaceDirectory,
    name: &str,
) -> Result<T, ApiError> {
    let file = directory.read_bounded(name, FILE_LIMIT).map_err(fs_error)?;
    strict_json(&file.bytes)
}

fn strict_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    platform_project_format::strict_json::from_slice(bytes).map_err(json_error)
}
fn write_json<T: Serialize>(
    directory: &WorkspaceDirectory,
    name: &str,
    value: &T,
) -> Result<(), ApiError> {
    directory
        .write_new(name, &serde_json::to_vec_pretty(value).map_err(json_error)?)
        .map_err(fs_error)?;
    Ok(())
}
fn exact_entries(
    directory: &WorkspaceDirectory,
    expected: &[(&str, bool)],
) -> Result<(), ApiError> {
    let entries = directory.list().map_err(fs_error)?;
    if entries.len() != expected.len()
        || entries.iter().any(|entry| {
            !expected
                .iter()
                .any(|(name, is_dir)| entry.name == *name && entry.is_directory == *is_dir)
        })
    {
        return Err(failure(
            "W2_RETAINED_EVIDENCE",
            "目录包含未知、缺失或未完成的现场；已保留，未自动修复或接管。",
        ));
    }
    Ok(())
}

fn directory_for(
    root: &WorkspaceDirectory,
    parts: &[&str],
    create: bool,
) -> Result<WorkspaceDirectory, ApiError> {
    let mut current = root.clone();
    for part in parts {
        current = if create {
            let exists = current
                .list()
                .map_err(fs_error)?
                .into_iter()
                .find(|entry| entry.name == *part);
            match exists {
                Some(entry) if entry.is_directory => {
                    current.open_directory(part).map_err(fs_error)?
                }
                Some(_) => return Err(failure("INVALID_PATH", "文件与目录路径冲突。")),
                None => current.create_directory(part).map_err(fs_error)?,
            }
        } else {
            current.open_directory(part).map_err(fs_error)?
        };
    }
    Ok(current)
}

fn write_logical(root: &WorkspaceDirectory, path: &str, bytes: &[u8]) -> Result<(), ApiError> {
    let parts = logical(path)?;
    let parent = directory_for(root, &parts[..parts.len() - 1], true)?;
    parent
        .write_new(parts[parts.len() - 1], bytes)
        .map_err(fs_error)?;
    Ok(())
}

fn collect(
    root: &WorkspaceDirectory,
) -> Result<BTreeMap<String, (Vec<u8>, WindowsObjectIdentity)>, ApiError> {
    fn visit(
        dir: &WorkspaceDirectory,
        prefix: &str,
        depth: usize,
        total: &mut usize,
        files: &mut BTreeMap<String, (Vec<u8>, WindowsObjectIdentity)>,
        directories: &mut BTreeSet<String>,
    ) -> Result<(), ApiError> {
        if depth > 32 {
            return Err(failure("W2_LIMIT", "样本目录深度超过内部检查上限。"));
        }
        for entry in dir.list().map_err(fs_error)? {
            let path = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            logical(&path)?;
            if entry.is_directory {
                directories.insert(path.clone());
                visit(
                    &dir.open_directory(&entry.name).map_err(fs_error)?,
                    &path,
                    depth + 1,
                    total,
                    files,
                    directories,
                )?;
            } else {
                if files.len() == FILE_COUNT_LIMIT {
                    return Err(failure("W2_LIMIT", "样本文件数超过内部检查上限。"));
                }
                let file = dir
                    .read_bounded(&entry.name, FILE_LIMIT)
                    .map_err(fs_error)?;
                *total = total
                    .checked_add(file.bytes.len())
                    .ok_or_else(|| failure("W2_LIMIT", "样本大小溢出。"))?;
                if *total > TOTAL_LIMIT {
                    return Err(failure("W2_LIMIT", "样本大小超过内部检查上限。"));
                }
                files.insert(path, (file.bytes, file.identity));
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    let mut directories = BTreeSet::new();
    visit(root, "", 0, &mut 0, &mut files, &mut directories)?;
    let mut expected = BTreeSet::from(["meta".to_owned(), "data/attachments".to_owned()]);
    for path in files.keys() {
        let mut parent = path.as_str();
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            expected.insert(prefix.to_owned());
            parent = prefix;
        }
    }
    if directories != expected {
        return Err(failure(
            "W2_RETAINED_EVIDENCE",
            "样本含未知空目录或缺少固定目录；现场保持不变。",
        ));
    }
    root.verify_binding().map_err(fs_error)?;
    Ok(files)
}

fn tree<'a>(files: &'a BTreeMap<String, Vec<u8>>, prefix: &str) -> Vec<TreeFile<'a>> {
    files
        .iter()
        .filter_map(|(path, bytes)| {
            path.strip_prefix(prefix).map(|label| TreeFile {
                logical_path: label,
                bytes,
            })
        })
        .collect()
}

impl Bundle {
    fn validate(input: BundledProject) -> Result<Self, ApiError> {
        let mut files = BTreeMap::new();
        let mut folded = BTreeSet::new();
        let mut total = 0usize;
        for (path, bytes) in input.files {
            logical(&path)?;
            total = total
                .checked_add(bytes.len())
                .ok_or_else(|| failure("INVALID_TEMPLATE", "资源大小溢出。"))?;
            if bytes.len() > FILE_LIMIT
                || total > TOTAL_LIMIT
                || files.len() >= FILE_COUNT_LIMIT
                || !folded.insert(path.to_lowercase())
            {
                return Err(failure(
                    "INVALID_TEMPLATE",
                    "资源超过内部上限或含名称冲突。",
                ));
            }
            files.insert(path, bytes);
        }
        for required in [
            "template/template.json",
            "template/starter.json",
            "template/starter/index.html",
            "template/app/package.json",
            "template/app/package-lock.json",
            "template/app/src/App.tsx",
            "template/app/README.md",
            "template/app/vendor/local-projects-sdk-0.1.0.tgz",
            "sdk/releases/local-projects-sdk-0.1.0.tgz",
            "sdk/README.md",
            "skills/local-projects-app/SKILL.md",
        ] {
            if !files.contains_key(required) {
                return Err(failure(
                    "INSTALLATION_RESOURCES_MISSING",
                    "内置项目资源不完整。",
                ));
            }
        }
        let distribution: StarterDistribution = strict_json(&files["template/starter.json"])?;
        let metadata: Value = strict_json(&files["template/template.json"])?;
        if !distribution_supported(&distribution)
            || metadata["templateVersion"].as_str() != Some(distribution.template_version.as_str())
            || metadata["sdkVersion"].as_str() != Some(distribution.sdk_version.as_str())
            || tree_digest(&tree(&files, "template/app/")) != distribution.source_digest
            || tree_digest(&tree(&files, "template/starter/")) != distribution.artifact_digest
            || files["sdk/releases/local-projects-sdk-0.1.0.tgz"]
                != files["template/app/vendor/local-projects-sdk-0.1.0.tgz"]
        {
            return Err(failure(
                "INVALID_TEMPLATE",
                "初始应用的源码、固定 SDK 与产物来源摘要不匹配。",
            ));
        }
        Ok(Self {
            files,
            distribution,
        })
    }
}

fn empty_database() -> Result<Vec<u8>, ApiError> {
    let connection = Connection::open_in_memory().map_err(sqlite_error)?;
    // Materialize a real SQLite header without inventing a platform table.
    connection
        .execute_batch("PRAGMA user_version=0; VACUUM;")
        .map_err(sqlite_error)?;
    let bytes = connection
        .serialize(MAIN_DB)
        .map_err(sqlite_error)?
        .to_vec();
    Ok(bytes)
}

fn empty_schema(bytes: &[u8]) -> Result<Value, ApiError> {
    if bytes.is_empty() || bytes.len() > FILE_LIMIT {
        return Err(failure(
            "DATABASE_INVALID",
            "SQLite 样本为空或超过检查上限。",
        ));
    }
    let mut connection = Connection::open_in_memory().map_err(sqlite_error)?;
    connection
        .deserialize_read_exact(MAIN_DB, bytes, bytes.len(), true)
        .map_err(sqlite_error)?;
    let check: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(sqlite_error)?;
    if check != "ok" {
        return Err(failure("DATABASE_INVALID", "SQLite 完整性检查未通过。"));
    }
    let objects: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(sqlite_error)?;
    if objects != 0 {
        return Err(failure(
            "W2_DATABASE_CHANGED",
            "当前阶段仅开放原登记空库；外部数据库修改已阻断，不返回空 schema。",
        ));
    }
    Ok(json!({"tables":[],"indexes":[],"views":[],"triggers":[]}))
}

impl WindowsProjectStore {
    /// Validate trusted embedded packaging bytes only. Performs no filesystem I/O.
    pub fn new(bundle: BundledProject) -> Result<Self, ApiError> {
        Ok(Self {
            bundle: Bundle::validate(bundle)?,
            active: Mutex::new(None),
            #[cfg(test)]
            test_root: None,
        })
    }

    fn with_root<T>(
        &self,
        create: bool,
        work: impl FnOnce(&WindowsWorkspace) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| failure("W2_STORE_POISONED", "上次样本操作异常终止；现场保留。"))?;
        if active.is_none() {
            match self.open_workspace(false) {
                Ok(workspace) => {
                    // Never create even a lock file inside an unrecognized old root.
                    self.validate_root(&workspace)?;
                    let lock = workspace
                        .root()
                        .lock_file("instance.lock")
                        .map_err(fs_error)?;
                    *active = Some(ActiveRoot {
                        workspace,
                        _lock: lock,
                    });
                }
                Err(error) if create && error.code == "FIXED_ROOT_MISSING" => {
                    let workspace = self.open_workspace(true).map_err(fs_error)?;
                    let lock = workspace
                        .root()
                        .lock_file("instance.lock")
                        .map_err(fs_error)?;
                    *active = Some(ActiveRoot {
                        workspace,
                        _lock: lock,
                    });
                    self.initialize_on_explicit_create(&active.as_ref().unwrap().workspace)?;
                }
                Err(error) => return Err(fs_error(error)),
            }
        }
        let workspace = &active.as_ref().unwrap().workspace;
        self.validate_root(workspace)?;
        work(workspace)
    }

    fn open_workspace(&self, create: bool) -> Result<WindowsWorkspace, WorkspaceError> {
        #[cfg(test)]
        if let Some(parent) = &self.test_root {
            return WindowsWorkspace::for_test_path(parent.clone(), create);
        }
        if create {
            WindowsWorkspace::create_fixed()
        } else {
            WindowsWorkspace::open_fixed()
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(bundle: BundledProject, parent: PathBuf) -> Result<Self, ApiError> {
        let mut store = Self::new(bundle)?;
        store.test_root = Some(parent);
        Ok(store)
    }

    fn initialize_on_explicit_create(&self, workspace: &WindowsWorkspace) -> Result<(), ApiError> {
        let root = workspace.root();
        exact_entries(&root, &[("instance.lock", false)])?;
        write_json(
            &root,
            ".environment.json",
            &Marker {
                schema_version: 1,
                identifier: APP_IDENTIFIER.into(),
                environment: ENVIRONMENT.into(),
            },
        )?;
        root.create_directory("runtime").map_err(fs_error)?;
        root.create_directory("migrations").map_err(fs_error)?;
        let work = root.create_directory("workspace").map_err(fs_error)?;
        work.create_directory("projects").map_err(fs_error)?;
        work.create_directory("backups").map_err(fs_error)?;
        let platform = work.create_directory(".platform").map_err(fs_error)?;
        platform
            .create_directory("project-state")
            .map_err(fs_error)?;
        write_json(
            &root,
            "settings.json",
            &Settings {
                schema_version: 1,
                identifier: APP_IDENTIFIER.into(),
                environment: ENVIRONMENT.into(),
                workspace_root: work.path().into(),
                last_port: None,
                migration_id: None,
            },
        )?;
        write_json(
            &platform,
            "windows-stage.json",
            &Stage {
                schema_version: 1,
                identifier: APP_IDENTIFIER.into(),
                environment: ENVIRONMENT.into(),
                stage: STAGE.into(),
                data_root_identity: workspace.identity().clone(),
                ancestor_identities: workspace.ancestor_identities(),
                workspace_identity: work.identity().clone(),
            },
        )?;
        workspace.verify_binding().map_err(fs_error)
    }

    fn validate_root(&self, workspace: &WindowsWorkspace) -> Result<(), ApiError> {
        let root = workspace.root();
        exact_entries(
            &root,
            &[
                (".environment.json", false),
                ("settings.json", false),
                ("instance.lock", false),
                ("runtime", true),
                ("migrations", true),
                ("workspace", true),
            ],
        )?;
        let marker: Marker = read_json(&root, ".environment.json")?;
        let settings_value: Value = read_json(&root, "settings.json")?;
        let settings_fields = [
            "schemaVersion",
            "identifier",
            "environment",
            "workspaceRoot",
            "lastPort",
            "migrationId",
        ];
        if !settings_value.as_object().is_some_and(|object| {
            object.len() == settings_fields.len()
                && settings_fields
                    .iter()
                    .all(|field| object.contains_key(*field))
        }) {
            return Err(failure(
                "W2_RECORD_INVALID",
                "工作区设置字段不完整；没有使用缺失字段默认值。",
            ));
        }
        let settings: Settings = serde_json::from_value(settings_value).map_err(json_error)?;
        let work = root.open_directory("workspace").map_err(fs_error)?;
        if marker.schema_version != 1
            || marker.identifier != APP_IDENTIFIER
            || marker.environment != ENVIRONMENT
            || settings.schema_version != 1
            || settings.identifier != APP_IDENTIFIER
            || settings.environment != ENVIRONMENT
            || settings.workspace_root != work.path()
            || settings.migration_id.is_some()
            || settings.last_port.is_some()
        {
            return Err(failure(
                "W2_ROOT_IDENTITY_MISMATCH",
                "固定根身份或工作区选择不属于当前 W2.1 样本范围。",
            ));
        }
        exact_entries(&root.open_directory("runtime").map_err(fs_error)?, &[])?;
        exact_entries(&root.open_directory("migrations").map_err(fs_error)?, &[])?;
        exact_entries(
            &work,
            &[("projects", true), ("backups", true), (".platform", true)],
        )?;
        exact_entries(&work.open_directory("backups").map_err(fs_error)?, &[])?;
        let platform = work.open_directory(".platform").map_err(fs_error)?;
        exact_entries(
            &platform,
            &[("windows-stage.json", false), ("project-state", true)],
        )?;
        let stage: Stage = read_json(&platform, "windows-stage.json")?;
        if stage.schema_version != 1
            || stage.identifier != APP_IDENTIFIER
            || stage.environment != ENVIRONMENT
            || stage.stage != STAGE
            || stage.data_root_identity != *workspace.identity()
            || stage.ancestor_identities != workspace.ancestor_identities()
            || stage.workspace_identity != *work.identity()
        {
            return Err(failure(
                "W2_ROOT_IDENTITY_MISMATCH",
                "未识别当前工作区内部阶段或完整 Windows 身份。",
            ));
        }
        let projects = work
            .open_directory("projects")
            .map_err(fs_error)?
            .list()
            .map_err(fs_error)?;
        let registrations = platform
            .open_directory("project-state")
            .map_err(fs_error)?
            .list()
            .map_err(fs_error)?;
        if projects.len() > PROJECT_LIMIT
            || projects.len() != registrations.len()
            || projects.iter().any(|project| {
                !project.is_directory
                    || !valid_id(&project.name)
                    || !registrations.iter().any(|record| {
                        !record.is_directory && record.name == format!("{}.json", project.name)
                    })
            })
        {
            return Err(failure(
                "W2_RETAINED_EVIDENCE",
                "发现未登记项目、创建残留或未知控制记录；没有自动补登记或删除。",
            ));
        }
        workspace.verify_binding().map_err(fs_error)
    }

    pub fn create(&self, input: CreateProjectInput) -> Result<ProjectInfo, ApiError> {
        let name = input.name.trim();
        if name.is_empty()
            || name.chars().count() > 200
            || input
                .description
                .as_ref()
                .is_some_and(|text| text.chars().count() > 2000)
        {
            return Err(failure(
                "INVALID_PROJECT_METADATA",
                "项目名称需含 1–200 字符，说明最多 2000 字符。",
            ));
        }
        let mut phase = "opening_or_initializing";
        let mut sample_id = None;
        let mut staging_name = None;
        let mut published = false;
        let result = self.with_root(true, |workspace| {
            // An existing damaged sample blocks further mutation too.
            self.list_in(workspace)?;
            let work = workspace
                .root()
                .open_directory("workspace")
                .map_err(fs_error)?;
            let projects = work.open_directory("projects").map_err(fs_error)?;
            if projects.list().map_err(fs_error)?.len() >= PROJECT_LIMIT {
                return Err(failure("W2_LIMIT", "内部样本数量已达上限。"));
            }
            let id = format!("prj_{}", Uuid::new_v4().simple());
            let temporary = format!(".creating-{id}");
            sample_id = Some(id.clone());
            staging_name = Some(temporary.clone());
            phase = "preparing";
            let staging = projects.create_directory(&temporary).map_err(fs_error)?;
            let registration = self.prepare(
                &staging,
                &id,
                name,
                input.description.as_deref().unwrap_or(""),
                &projects.path().join(&id),
            )?;
            drop(staging);
            // Publish payload first, registration last. Interruptions remain
            // visibly unregistered; startup never repairs or adopts them.
            phase = "publishing";
            projects
                .publish_noreplace_checked(&temporary, &id, true, &registration.root_identity)
                .map_err(fs_error)?;
            published = true;
            phase = "registering";
            let state = work
                .open_directory(".platform")
                .map_err(fs_error)?
                .open_directory("project-state")
                .map_err(fs_error)?;
            write_json(&state, &format!("{id}.json"), &registration)?;
            phase = "committed";
            self.open_in(workspace, &id).map(|opened| self.info(opened))
        });
        result.map_err(|error| {
            error
                .outcome(match phase {
                    "preparing" => "not_committed",
                    "committed" => "committed",
                    _ => "unknown",
                })
                .detail("sampleId", json!(sample_id))
                .detail("stagingName", json!(staging_name))
                .detail(
                    "projectPublished",
                    if published { json!(true) } else { Value::Null },
                )
                .detail("creationPhase", phase)
        })
    }

    fn prepare(
        &self,
        directory: &WorkspaceDirectory,
        id: &str,
        name: &str,
        description: &str,
        target: &std::path::Path,
    ) -> Result<Registration, ApiError> {
        let manifest = ProjectManifest {
            manifest_version: 1,
            id: id.into(),
            name: name.into(),
            description: description.into(),
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
            extra: serde_json::Map::new(),
        };
        let build_id = format!("starter_{}", Uuid::new_v4().simple());
        let mut files = BTreeMap::new();
        for (path, bytes) in &self.bundle.files {
            if let Some(relative) = path.strip_prefix("template/app/") {
                files.insert(format!("app/{relative}"), bytes.clone());
            }
            if let Some(relative) = path.strip_prefix("template/starter/") {
                files.insert(format!("dist/{relative}"), bytes.clone());
                files.insert(
                    format!(".platform/artifacts/{build_id}/dist/{relative}"),
                    bytes.clone(),
                );
            }
            if let Some(relative) = path.strip_prefix("skills/local-projects-app/") {
                files.insert(
                    format!(".agents/skills/local-projects-app/{relative}"),
                    bytes.clone(),
                );
            }
        }
        let input = input_digest(&manifest, &tree(&files, "app/")).map_err(json_error)?;
        let starter = StarterProvenance {
            schema_version: 1,
            origin: "bundledTemplate".into(),
            build_id,
            installed_at: now(),
            digest_rule_version: DIGEST_RULE_VERSION,
            input_digest: input,
            distribution: self.bundle.distribution.clone(),
        };
        files.insert(
            "project.json".into(),
            serde_json::to_vec_pretty(&manifest).map_err(json_error)?,
        );
        files.insert(
            ".platform/starter.json".into(),
            serde_json::to_vec_pretty(&starter).map_err(json_error)?,
        );
        files.insert("data/main.sqlite".into(), empty_database()?);
        files.insert("README.md".into(), format!("# {name}\n\n{description}\n\nProject ID: `{id}`\n\n这是 Windows W2.1 可丢弃内部样本。源码、固定 SDK、初始网页和独立 SQLite 已保存。当前仅允许创建、列表、打开、真实只读 schema 和会话事件；SQL 执行、附件、构建、备份、恢复、迁移及 CLI 均未开放。不要把此阶段用于真实数据，也不要手改目录或控制记录来修复失败。\n\n先阅读 project.json、docs/database.md、docs/platform.md 与 .agents/skills/local-projects-app/SKILL.md。共享 skill 中描述的后续能力仍受本阶段门禁约束。\n").into_bytes());
        files.insert("docs/database.md".into(), b"# Database contract\n\nDatabase: data/main.sqlite. This is a real independent SQLite database with no business tables. W2.1 permits a verified read-only schema snapshot only. SQL and attachment writes remain unavailable. Never replace the database or its control records.\n".to_vec());
        let sdk_guide = std::str::from_utf8(&self.bundle.files["sdk/README.md"])
            .map_err(|_| failure("INVALID_TEMPLATE", "SDK 文档不是 UTF-8。"))?;
        files.insert("docs/platform.md".into(), format!("# Windows W2.1 integration\n\nSDK 0.1.0 and API v1 are unchanged. This internal stage exposes context, read-only empty schema and events only. CLI, SQL execution, files and builds remain unavailable; the pinned guide below describes the complete API, not this stage's permissions.\n\n{sdk_guide}").into_bytes());
        let agent = b"# Project development instructions\n\nRead README.md and its Windows W2.1 capability restrictions first, then project.json, docs/database.md, docs/platform.md and .agents/skills/local-projects-app/SKILL.md. Do not edit this disposable sample in the current read-only stage or invoke unopened CLI/build/SQL/lifecycle operations.\n";
        files.insert("AGENTS.md".into(), agent.to_vec());
        files.insert("CLAUDE.md".into(), agent.to_vec());
        for empty in ["meta", "data/attachments"] {
            directory_for(directory, &logical(empty)?, true)?;
        }
        for (path, bytes) in files {
            write_logical(directory, &path, &bytes)?;
        }
        let actual = collect(directory)?;
        empty_schema(&actual["data/main.sqlite"].0)?;
        let database_identity = actual["data/main.sqlite"].1.clone();
        let stamps = actual
            .into_iter()
            .map(|(path, (bytes, identity))| {
                (
                    path,
                    FileStamp {
                        identity,
                        sha256: sha(&bytes),
                    },
                )
            })
            .collect();
        Ok(Registration {
            schema_version: 1,
            identity_kind: "windowsFileId128".into(),
            id: id.into(),
            project_root: target.into(),
            root_identity: directory.identity().clone(),
            database_path: "data/main.sqlite".into(),
            database_identity,
            document_epoch: Uuid::new_v4().to_string(),
            files: stamps,
        })
    }

    pub fn list(&self) -> Result<Vec<ProjectInfo>, ApiError> {
        self.with_root(false, |workspace| self.list_in(workspace))
    }
    fn list_in(&self, workspace: &WindowsWorkspace) -> Result<Vec<ProjectInfo>, ApiError> {
        let work = workspace
            .root()
            .open_directory("workspace")
            .map_err(fs_error)?;
        let projects = work.open_directory("projects").map_err(fs_error)?;
        let mut result = Vec::new();
        for entry in projects.list().map_err(fs_error)? {
            result.push(self.info(self.open_in(workspace, &entry.name)?));
        }
        result.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(result)
    }
    pub fn open(&self, id: &str) -> Result<OpenedProject, ApiError> {
        self.with_root(false, |workspace| self.open_in(workspace, id))
    }
    pub fn project_info(&self, id: &str) -> Result<ProjectInfo, ApiError> {
        self.open(id).map(|opened| self.info(opened))
    }

    /// Called after the runtime stops admission and drains store users. Drop
    /// only owning handles; retain the lock file and every project/evidence byte.
    pub(crate) fn close(&self) -> Result<(), ApiError> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| failure("W2_STORE_POISONED", "无法在异常状态下释放样本工作区句柄。"))?;
        drop(active.take());
        Ok(())
    }

    fn open_in(&self, workspace: &WindowsWorkspace, id: &str) -> Result<OpenedProject, ApiError> {
        if !valid_id(id) {
            return Err(failure("INVALID_PROJECT_ID", "无效的 W2.1 项目 ID。"));
        }
        let work = workspace
            .root()
            .open_directory("workspace")
            .map_err(fs_error)?;
        let project = work
            .open_directory("projects")
            .map_err(fs_error)?
            .open_directory(id)
            .map_err(fs_error)?;
        let state = work
            .open_directory(".platform")
            .map_err(fs_error)?
            .open_directory("project-state")
            .map_err(fs_error)?;
        let registration: Registration = read_json(&state, &format!("{id}.json"))?;
        if registration.schema_version != 1
            || registration.identity_kind != "windowsFileId128"
            || registration.id != id
            || registration.project_root != project.path()
            || registration.root_identity != *project.identity()
            || registration.database_path != "data/main.sqlite"
            || !uuid(&registration.document_epoch)
        {
            return Err(failure(
                "PROJECT_IDENTITY_CHANGED",
                "项目目录与完整 Windows 登记身份不匹配。",
            ));
        }
        let actual = collect(&project)?;
        if actual.len() != registration.files.len()
            || actual.iter().any(|(path, (bytes, identity))| {
                !registration
                    .files
                    .get(path)
                    .is_some_and(|stamp| stamp.identity == *identity && stamp.sha256 == sha(bytes))
            })
        {
            return Err(failure(
                "W2_PROJECT_CHANGED",
                "样本文件集合、字节或对象身份已改变；本阶段不自动修复或接受外部修改。",
            ));
        }
        let Some((database, db_id)) = actual.get("data/main.sqlite") else {
            return Err(failure(
                "DATABASE_MISSING",
                "样本数据库缺失；没有创建空替代库。",
            ));
        };
        if *db_id != registration.database_identity {
            return Err(failure(
                "PROJECT_IDENTITY_CHANGED",
                "数据库对象身份已变化。",
            ));
        }
        empty_schema(database)?;
        let manifest: ProjectManifest = strict_json(
            &actual
                .get("project.json")
                .ok_or_else(|| failure("INVALID_PROJECT", "项目清单缺失。"))?
                .0,
        )?;
        let starter: StarterProvenance = strict_json(
            &actual
                .get(".platform/starter.json")
                .ok_or_else(|| failure("INVALID_PROJECT", "初始应用来源缺失。"))?
                .0,
        )?;
        let expected_manifest = manifest.id == id
            && manifest.manifest_version == 1
            && manifest.sdk_api_version == 1
            && manifest.source_dir == "app"
            && manifest.output_dir == "dist"
            && manifest.entry == "dist/index.html"
            && manifest.database == "data/main.sqlite"
            && manifest.attachments_dir == "data/attachments"
            && manifest.runtime_requirements
                == json!({"network":{"connect":[],"images":[],"media":[]},"devices":{}});
        let bytes: BTreeMap<_, _> = actual
            .iter()
            .map(|(path, (bytes, _))| (path.clone(), bytes.clone()))
            .collect();
        if !expected_manifest
            || starter.schema_version != 1
            || starter.origin != "bundledTemplate"
            || starter.digest_rule_version != DIGEST_RULE_VERSION
            || !starter_id(&starter.build_id)
            || !distribution_supported(&starter.distribution)
            || !digest(&starter.input_digest)
            || bytes.get("app/vendor/local-projects-sdk-0.1.0.tgz")
                != self
                    .bundle
                    .files
                    .get("sdk/releases/local-projects-sdk-0.1.0.tgz")
            || tree_digest(&tree(&bytes, "app/")) != starter.distribution.source_digest
            || tree_digest(&tree(&bytes, "dist/")) != starter.distribution.artifact_digest
            || tree_digest(&tree(
                &bytes,
                &format!(".platform/artifacts/{}/dist/", starter.build_id),
            )) != starter.distribution.artifact_digest
            || input_digest(&manifest, &tree(&bytes, "app/")).map_err(json_error)?
                != starter.input_digest
        {
            return Err(failure(
                "PROJECT_SOURCE_MISMATCH",
                "样本源码、固定 SDK、初始产物及来源记录不再匹配。",
            ));
        }
        project.verify_binding().map_err(fs_error)?;
        Ok(OpenedProject {
            manifest,
            document_epoch: registration.document_epoch,
            starter,
            project_root: project.path().into(),
        })
    }

    fn info(&self, opened: OpenedProject) -> ProjectInfo {
        ProjectInfo {
            id: opened.manifest.id.clone(),
            name: opened.manifest.name.clone(),
            description: opened.manifest.description.clone(),
            project_root: opened.project_root,
            document_epoch: opened.document_epoch,
            state: "ready".into(),
            issues: vec![],
            manifest: Some(opened.manifest),
            build_info: json!({"sourceState":"matched","stale":false,"origin":"bundledTemplate","templateVersion":opened.starter.distribution.template_version,"starter":opened.starter}),
        }
    }

    pub fn read_static(&self, id: &str, relative: &str) -> Result<Vec<u8>, ApiError> {
        logical(relative)?;
        self.with_root(false, |workspace| {
            let opened = self.open_in(workspace, id)?;
            let root = workspace
                .root()
                .open_directory("workspace")
                .map_err(fs_error)?
                .open_directory("projects")
                .map_err(fs_error)?
                .open_directory(id)
                .map_err(fs_error)?;
            let full = format!(
                ".platform/artifacts/{}/dist/{relative}",
                opened.starter.build_id
            );
            let parts = logical(&full)?;
            let directory = directory_for(&root, &parts[..parts.len() - 1], false)?;
            let file = directory
                .read_bounded(parts[parts.len() - 1], FILE_LIMIT)
                .map_err(fs_error)?;
            let state = workspace
                .root()
                .open_directory("workspace")
                .map_err(fs_error)?
                .open_directory(".platform")
                .map_err(fs_error)?
                .open_directory("project-state")
                .map_err(fs_error)?;
            let registration: Registration = read_json(&state, &format!("{id}.json"))?;
            if !registration.files.get(&full).is_some_and(|stamp| {
                stamp.identity == file.identity && stamp.sha256 == sha(&file.bytes)
            }) {
                return Err(failure(
                    "W2_PROJECT_CHANGED",
                    "静态文件在校验后发生变化；未提供替代内容。",
                ));
            }
            Ok(file.bytes)
        })
    }

    pub fn schema(&self, id: &str) -> Result<Value, ApiError> {
        self.with_root(false, |workspace| {
            self.open_in(workspace, id)?;
            let project = workspace
                .root()
                .open_directory("workspace")
                .map_err(fs_error)?
                .open_directory("projects")
                .map_err(fs_error)?
                .open_directory(id)
                .map_err(fs_error)?;
            let file = project
                .open_directory("data")
                .map_err(fs_error)?
                .read_bounded("main.sqlite", FILE_LIMIT)
                .map_err(fs_error)?;
            let state = workspace
                .root()
                .open_directory("workspace")
                .map_err(fs_error)?
                .open_directory(".platform")
                .map_err(fs_error)?
                .open_directory("project-state")
                .map_err(fs_error)?;
            let registration: Registration = read_json(&state, &format!("{id}.json"))?;
            if !registration
                .files
                .get("data/main.sqlite")
                .is_some_and(|stamp| {
                    stamp.identity == file.identity && stamp.sha256 == sha(&file.bytes)
                })
            {
                return Err(failure(
                    "W2_DATABASE_CHANGED",
                    "只读 schema 快照与原登记数据库不一致。",
                ));
            }
            empty_schema(&file.bytes)
        })
    }
}

#[cfg(test)]
pub(crate) fn test_bundle() -> BundledProject {
    tests::test_bundle()
}

#[cfg(test)]
#[path = "windows_projects_tests.rs"]
mod tests;
