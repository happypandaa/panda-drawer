use crate::{API_VERSION, APP_IDENTIFIER, ENVIRONMENT};
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use platform_native::filesystem as native_fs;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const MARKER: &str = ".environment.json";
/// Written into a new installation's visible home workspace. It ties that
/// folder to this data directory so another folder is never adopted silently.
const HOME_WORKSPACE_MARKER: &str = ".platform/workspace-default.json";

#[derive(Clone, Debug)]
pub struct RuntimePaths {
    data_root: PathBuf,
    workspace_root: PathBuf,
}

/// Keep this value alive until the service and its discovery files are gone.
/// The lock file itself is deliberately never removed.
pub struct InstanceLock {
    _file: File,
    root: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnvironmentMarker {
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
    #[serde(default)]
    migration_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HomeWorkspaceMarker {
    schema_version: u32,
    identifier: String,
    environment: String,
    data_root: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub api_version: u32,
    pub instance_id: String,
    pub pid: u32,
    pub base_url: String,
    pub workspace_root: PathBuf,
    pub environment: String,
    pub identifier: String,
}

impl RuntimePaths {
    #[cfg(test)]
    pub(crate) fn for_test() -> (tempfile::TempDir, Self) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("dev");
        let paths = Self::initialize_at(root).unwrap();
        (temp, paths)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn for_disposable_interruption_test(root: PathBuf) -> Result<Self> {
        Self::initialize_at(root)
    }

    #[cfg(test)]
    pub(crate) fn reinitialize_for_test(&self) -> Result<Self> {
        Self::initialize_at(self.data_root.clone())
    }

    /// There is intentionally no environment variable or public custom-root override.
    pub fn initialize() -> Result<Self> {
        crate::platform_support::require_runtime()?;
        Self::initialize_at(Self::default_root()?)
    }

    /// Offline, read-only preview; never initializes a missing installation.
    pub fn identity_repair_preview() -> Result<serde_json::Value> {
        crate::platform_support::require_runtime()?;
        let (paths, migration) = Self::identity_repair_paths(Self::default_root()?)?;
        crate::identity_upgrade::preview(&paths, migration.as_deref())
    }

    /// Explicit identity compatibility repair, serialized with desktop startup.
    pub fn identity_repair(input: serde_json::Value) -> Result<serde_json::Value> {
        crate::platform_support::require_runtime()?;
        Self::identity_repair_at(Self::default_root()?, input)
    }

    fn identity_repair_paths(root: PathBuf) -> Result<(Self, Option<String>)> {
        crate::platform_support::require_runtime()?;
        let mut paths = Self::at(root);
        paths.validate_marker()?;
        let settings: Settings = read_private_json(&paths.settings_path())?;
        Self::validate_settings_identity(&settings)?;
        paths.workspace_root = settings.workspace_root;
        Ok((paths, settings.migration_id))
    }

    fn identity_repair_at(root: PathBuf, input: serde_json::Value) -> Result<serde_json::Value> {
        let (paths, migration) = Self::identity_repair_paths(root)?;
        let file = open_private(&paths.data_root.join("instance.lock"), false, false)?;
        file.try_lock_exclusive()
            .context("Quit the desktop service before offline identity repair")?;
        crate::identity_upgrade::apply(&paths, migration.as_deref(), input)
    }

    /// Read an existing development runtime without creating directories or settings.
    pub fn discover() -> Result<Self> {
        crate::platform_support::require_runtime()?;
        let mut paths = Self::at(Self::default_root()?);
        paths.validate_marker()?;
        let settings: Settings = read_private_json(&paths.settings_path())?;
        Self::validate_settings_identity(&settings)?;
        crate::workspace::validate_selection(
            paths.data_root(),
            &settings.workspace_root,
            settings.migration_id.as_deref(),
        )?;
        paths.workspace_root = settings.workspace_root;
        paths.validate_existing()?;
        Ok(paths)
    }

    fn default_root() -> Result<PathBuf> {
        if crate::build_profile::PRODUCTION_PROFILE {
            crate::platform_support::require_runtime()?;
        }
        let base = dirs::data_local_dir()
            .context("Cannot locate this user's application data directory")?;
        Ok(base.join(APP_IDENTIFIER))
    }

    fn at(data_root: PathBuf) -> Self {
        Self {
            workspace_root: data_root.join("workspace"),
            data_root,
        }
    }

    fn initialize_at(root: PathBuf) -> Result<Self> {
        crate::platform_support::require_runtime()?;
        require_absolute_clean_path(&root)?;
        ensure_directory_tree(&root)?;
        let marker = root.join(MARKER);
        if !marker.try_exists()? {
            if fs::read_dir(&root)?.next().is_some() {
                bail!(
                    "Refusing to adopt an unmarked nonempty data directory: {}",
                    root.display()
                );
            }
            native_fs::make_directory_private(&root)?;
            create_private_json(
                &marker,
                &EnvironmentMarker {
                    schema_version: 1,
                    identifier: APP_IDENTIFIER.to_owned(),
                    environment: ENVIRONMENT.to_owned(),
                },
            )?;
        }
        let mut paths = Self::at(root);
        paths.validate_marker()?;
        paths.prepare_runtime_directories()?;
        let settings_path = paths.settings_path();
        if !settings_path.try_exists()? {
            if fs::read_dir(paths.migrations_dir())?.next().is_some() {
                bail!("Workspace selection settings are missing; retained migration evidence prevents initializing an old or empty workspace");
            }
            // Only a brand-new installation initializes the default workspace.
            // New installations use a visible folder in the user's home so
            // people and external AI tools can find their projects.
            paths.workspace_root = new_workspace_root(paths.data_root())?;
            let home_workspace = paths.workspace_root != paths.data_root.join("workspace");
            for dir in [
                paths.workspace_root.clone(),
                paths.workspace_root.join("projects"),
                paths.workspace_root.join(".platform"),
            ] {
                ensure_directory_tree(&dir)?;
            }
            if home_workspace {
                atomic_private_json(
                    &paths.workspace_root.join(HOME_WORKSPACE_MARKER),
                    &HomeWorkspaceMarker {
                        schema_version: 1,
                        identifier: APP_IDENTIFIER.to_owned(),
                        environment: ENVIRONMENT.to_owned(),
                        data_root: paths.data_root.clone(),
                    },
                )?;
            }
            create_private_json(
                &settings_path,
                &Settings {
                    schema_version: 1,
                    identifier: APP_IDENTIFIER.to_owned(),
                    environment: ENVIRONMENT.to_owned(),
                    workspace_root: paths.workspace_root.clone(),
                    last_port: None,
                    migration_id: None,
                },
            )?;
        } else {
            let settings: Settings = read_private_json(&settings_path)?;
            Self::validate_settings_identity(&settings)?;
            crate::workspace::validate_selection(
                paths.data_root(),
                &settings.workspace_root,
                settings.migration_id.as_deref(),
            )?;
            paths.workspace_root = settings.workspace_root;
        }
        paths.validate_existing()?;
        Ok(paths)
    }

    fn prepare_runtime_directories(&self) -> Result<()> {
        crate::workspace::require_supported_startup_inventory(self.data_root())?;
        ensure_directory_tree(&self.runtime_dir())?;
        ensure_directory_tree(&self.migrations_dir())?;
        Ok(())
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }
    /// Whole-installation cleanup must bind the selected workspace through the
    /// same public-migration evidence used by startup, never an arbitrary path.
    pub(crate) fn validate_cleanup_selection(&self) -> Result<()> {
        self.validate_existing()?;
        let settings = self.read_settings()?;
        crate::workspace::validate_selection(
            self.data_root(),
            self.workspace_root(),
            settings.migration_id.as_deref(),
        )
    }
    /// Read only: an older manager can resolve the current, already validated
    /// public-migration selection without initializing or switching anything.
    pub(crate) fn selected_for_cleanup(&self) -> Result<Self> {
        self.validate_marker()?;
        let settings: Settings = read_private_json(&self.settings_path())?;
        Self::validate_settings_identity(&settings)?;
        let selected = Self {
            data_root: self.data_root.clone(),
            workspace_root: settings.workspace_root,
        };
        selected.validate_cleanup_selection()?;
        Ok(selected)
    }
    pub fn runtime_dir(&self) -> PathBuf {
        self.data_root.join("runtime")
    }
    pub fn connection_path(&self) -> PathBuf {
        self.runtime_dir().join("connection.json")
    }
    pub fn token_path(&self) -> PathBuf {
        self.runtime_dir().join("control-token")
    }
    fn settings_path(&self) -> PathBuf {
        self.data_root.join("settings.json")
    }

    fn validate_marker(&self) -> Result<()> {
        reject_symlink_components(&self.data_root)?;
        if !native_fs::directory_is_private(&self.data_root)? {
            bail!(
                "Development data directory must be private ({})",
                native_fs::private_directory_requirement()
            );
        }
        let marker: EnvironmentMarker = read_private_json(&self.data_root.join(MARKER))?;
        if marker.schema_version != 1
            || marker.identifier != APP_IDENTIFIER
            || marker.environment != ENVIRONMENT
        {
            bail!("Development data directory has an incompatible environment marker");
        }
        Ok(())
    }

    fn validate_existing(&self) -> Result<()> {
        if crate::build_profile::PRODUCTION_PROFILE {
            crate::platform_support::require_runtime()?;
        }
        self.validate_marker()?;
        for dir in [
            self.runtime_dir(),
            self.workspace_root.clone(),
            self.workspace_root.join("projects"),
            self.workspace_root.join(".platform"),
        ] {
            reject_symlink_components(&dir)?;
            if !dir.is_dir() {
                bail!(
                    "Required development directory is missing: {}",
                    dir.display()
                );
            }
        }
        self.read_settings()?;
        Ok(())
    }

    fn validate_settings_identity(settings: &Settings) -> Result<()> {
        if settings.schema_version != 1
            || settings.identifier != APP_IDENTIFIER
            || settings.environment != ENVIRONMENT
        {
            bail!("Development settings have an incompatible identity");
        }
        Ok(())
    }
    fn read_settings(&self) -> Result<Settings> {
        let settings: Settings = read_private_json(&self.settings_path())?;
        Self::validate_settings_identity(&settings)?;
        if settings.workspace_root != self.workspace_root {
            bail!("Development workspace selection changed; refresh the runtime instance");
        }
        crate::workspace::validate_selection(
            self.data_root(),
            &settings.workspace_root,
            settings.migration_id.as_deref(),
        )?;
        Ok(settings)
    }
    pub(crate) fn migrations_dir(&self) -> PathBuf {
        self.data_root.join("migrations")
    }
    pub(crate) fn commit_workspace(&self, destination: &Path, task: &str) -> Result<Self> {
        let mut settings = self.read_settings()?;
        crate::workspace::validate_selection(self.data_root(), destination, Some(task))?;
        settings.workspace_root = destination.into();
        settings.migration_id = Some(task.into());
        atomic_private_json(&self.settings_path(), &settings)?;
        Ok(Self {
            data_root: self.data_root.clone(),
            workspace_root: destination.into(),
        })
    }

    pub fn acquire_lock(&self) -> Result<InstanceLock> {
        crate::platform_support::require_runtime()?;
        self.validate_existing()?;
        let path = self.data_root.join("instance.lock");
        let file = open_private(&path, true, false)?;
        file.try_lock_exclusive()
            .context("Another development instance already owns this workspace")?;
        Ok(InstanceLock {
            _file: file,
            root: self.data_root.clone(),
        })
    }

    pub(crate) fn verify_lock(&self, lock: &InstanceLock) -> Result<()> {
        if lock.root != self.data_root {
            bail!("Instance lock belongs to another data root");
        }
        self.validate_existing()
    }

    pub(crate) fn last_port(&self) -> Result<Option<u16>> {
        Ok(self.read_settings()?.last_port.filter(|port| *port != 0))
    }

    pub(crate) fn save_port(&self, port: u16) -> Result<()> {
        let mut settings = self.read_settings()?;
        settings.last_port = Some(port);
        atomic_private_json(&self.settings_path(), &settings)
    }

    pub(crate) fn publish_runtime(&self, discovery: &Discovery, token: &str) -> Result<()> {
        self.validate_existing()?;
        // Publish the credential before the discovery pointer. A reader observing the
        // transition fails authentication and may retry; it cannot target another root.
        atomic_private_bytes(&self.token_path(), token.as_bytes())?;
        atomic_private_json(&self.connection_path(), discovery)
    }

    pub fn read_discovery(&self) -> Result<Discovery> {
        self.validate_existing()?;
        let discovery: Discovery = read_private_json(&self.connection_path())?;
        if discovery.api_version != API_VERSION
            || discovery.identifier != APP_IDENTIFIER
            || discovery.environment != ENVIRONMENT
            || discovery.workspace_root != self.workspace_root
            || Uuid::parse_str(&discovery.instance_id).is_err()
        {
            bail!("Runtime discovery does not match this development environment");
        }
        discovery.port()?;
        Ok(discovery)
    }

    pub fn read_control_token(&self) -> Result<String> {
        self.validate_existing()?;
        let token = read_private_string(&self.token_path())?;
        if token.len() != 64 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
            bail!("Invalid local control credential");
        }
        Ok(token)
    }

    /// Called only after runtime shutdown has closed its resources. Settings
    /// may have committed before the replacement HttpState could be installed.
    /// An exact migration-source receipt then permits removing this instance's
    /// fixed discovery, without restoring the old workspace selection.
    pub(crate) fn remove_stopped_runtime_if_owned(
        &self,
        instance_id: &str,
        expected_token: &str,
    ) -> Result<bool> {
        let selected = self.selected_for_cleanup()?;
        if selected.workspace_root != self.workspace_root {
            crate::workspace::validate_stopped_runtime_source(self, &selected)?;
        }
        let discovery: Discovery = read_private_json(&self.connection_path())?;
        if discovery.api_version != API_VERSION
            || discovery.identifier != APP_IDENTIFIER
            || discovery.environment != ENVIRONMENT
            || discovery.workspace_root != self.workspace_root
            || discovery.instance_id != instance_id
            || Uuid::parse_str(&discovery.instance_id).is_err()
        {
            return Ok(false);
        }
        discovery.port()?;
        let token = read_private_string(&self.token_path())?;
        if token != expected_token {
            return Ok(false);
        }
        fs::remove_file(self.connection_path())?;
        fs::remove_file(self.token_path())?;
        Ok(true)
    }

    #[cfg(test)]
    pub(crate) fn remove_runtime_if_owned(
        &self,
        instance_id: &str,
        expected_token: &str,
    ) -> Result<()> {
        if !self.connection_path().try_exists()? {
            return Ok(());
        }
        let discovery = self.read_discovery()?;
        if discovery.instance_id != instance_id {
            return Ok(());
        }
        let token = self.read_control_token()?;
        if token != expected_token {
            return Ok(());
        }
        fs::remove_file(self.connection_path())?;
        fs::remove_file(self.token_path())?;
        Ok(())
    }
}

impl Discovery {
    /// Only a literal IPv4 loopback origin is accepted; no URL credentials,
    /// redirects, proxy hosts, paths, query strings or fragments are allowed.
    pub fn port(&self) -> Result<u16> {
        let value = self
            .base_url
            .strip_prefix("http://127.0.0.1:")
            .context("Runtime URL must be an HTTP IPv4 loopback origin")?;
        let port: u16 = value.parse().context("Invalid runtime port")?;
        if port == 0 || port.to_string() != value {
            bail!("Invalid runtime port");
        }
        Ok(port)
    }
}

#[cfg(all(test, feature = "production-profile"))]
mod production_profile_tests {
    use super::*;

    fn rejected<T>(result: anyhow::Result<T>) {
        let error = match result {
            Ok(_) => panic!("metadata-only production runtime was admitted"),
            Err(error) => error,
        };
        let gate = error
            .downcast_ref::<crate::platform_support::RuntimeSupportError>()
            .expect("admission must run before filesystem/resource validation");
        assert_eq!(
            gate.code,
            crate::platform_support::runtime_support()
                .error_code
                .unwrap()
        );
        let api_error = crate::ApiError::from(*gate);
        assert_eq!(api_error.details["operationOutcome"], "not_started");
    }

    #[test]
    fn production_profile_public_paths_refuse_before_root_resolution() {
        rejected(RuntimePaths::default_root());
        rejected(RuntimePaths::initialize());
        rejected(RuntimePaths::discover());
        rejected(RuntimePaths::identity_repair_preview());
        rejected(RuntimePaths::identity_repair(serde_json::json!({})));
    }

    #[tokio::test]
    async fn production_profile_existing_and_new_paths_resources_and_credentials_are_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("production-must-not-be-created");
        rejected(RuntimePaths::initialize_at(missing.clone()));
        rejected(RuntimePaths::identity_repair_paths(missing.clone()));
        rejected(RuntimePaths::identity_repair_at(
            missing.clone(),
            serde_json::json!({}),
        ));
        let node_error = crate::node_apps::NodeApps::new(missing.clone(), missing.join("node"))
            .err()
            .expect("production Node manager must not initialize");
        assert_eq!(
            node_error.code,
            crate::platform_support::runtime_support()
                .error_code
                .unwrap()
        );
        assert_eq!(node_error.details["operationOutcome"], "not_started");
        assert!(!missing.exists());

        let root = temp.path().join("existing-production-sentinel");
        fs::create_dir(&root).unwrap();
        let sentinel = root.join(MARKER);
        fs::write(
            &sentinel,
            b"retained evidence; intentionally not valid JSON",
        )
        .unwrap();
        let paths = RuntimePaths::at(root.clone());
        rejected(paths.acquire_lock());
        rejected(paths.read_discovery());
        rejected(paths.read_control_token());
        assert!(!root.join("instance.lock").exists());
        let owned_file = tempfile::tempfile().unwrap();
        let lock = InstanceLock {
            _file: owned_file,
            root: root.clone(),
        };
        rejected(
            Box::pin(crate::RuntimeService::start_with_resources(
                paths,
                lock,
                missing.join("resources"),
                missing.join("cli"),
            ))
            .await,
        );
        assert_eq!(
            fs::read(&sentinel).unwrap(),
            b"retained evidence; intentionally not valid JSON"
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        assert!(!missing.exists());
    }
}

/// Workspace for a brand-new installation. Tests keep the private default so
/// they never touch the real home folder.
fn new_workspace_root(data_root: &Path) -> Result<PathBuf> {
    #[cfg(test)]
    {
        Ok(data_root.join("workspace"))
    }
    #[cfg(not(test))]
    {
        let base = if ENVIRONMENT == "production" {
            "PandaDrawer"
        } else {
            "PandaDrawer-Dev"
        };
        match dirs::home_dir() {
            Some(home) if reject_symlink_components(&home).is_ok() => {
                home_workspace_candidate(&home, base, data_root)
            }
            _ => Ok(data_root.join("workspace")),
        }
    }
}

/// `~/PandaDrawer`, or `~/PandaDrawer-2`… when an earlier name is taken by a
/// file or a nonempty folder. Existing content is never adopted, except a
/// folder this same data directory created before an interrupted first start.
fn home_workspace_candidate(home: &Path, base: &str, data_root: &Path) -> Result<PathBuf> {
    for index in 1..=99u32 {
        let candidate = if index == 1 {
            home.join(base)
        } else {
            home.join(format!("{base}-{index}"))
        };
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Ok(meta)
                if meta.is_dir()
                    && !native_fs::is_reparse(&meta)
                    && fs::read_dir(&candidate)?.next().is_none() =>
            {
                return Ok(candidate)
            }
            Ok(meta) if meta.is_dir() && is_home_workspace(data_root, &candidate) => {
                return Ok(candidate)
            }
            Ok(_) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    bail!("No free project folder name under {}", home.display())
}

/// A workspace outside the data directory without a migration is accepted only
/// when it is the visible home workspace this installation created.
pub(crate) fn is_home_workspace(data_root: &Path, workspace_root: &Path) -> bool {
    if workspace_root.starts_with(data_root) {
        return false;
    }
    read_private_json::<HomeWorkspaceMarker>(&workspace_root.join(HOME_WORKSPACE_MARKER)).is_ok_and(
        |marker| {
            marker.schema_version == 1
                && marker.identifier == APP_IDENTIFIER
                && marker.environment == ENVIRONMENT
                && marker.data_root == data_root
        },
    )
}

fn require_absolute_clean_path(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        bail!("Runtime paths must be absolute and normalized");
    }
    Ok(())
}

fn reject_symlink_components(path: &Path) -> Result<()> {
    require_absolute_clean_path(path)?;
    for current in native_fs::path_components(path) {
        let metadata = fs::symlink_metadata(&current)
            .with_context(|| format!("Cannot inspect {}", current.display()))?;
        if native_fs::is_reparse(&metadata) {
            bail!(
                "Symlink is not permitted in runtime paths: {}",
                current.display()
            );
        }
    }
    Ok(())
}

fn ensure_directory_tree(path: &Path) -> Result<()> {
    require_absolute_clean_path(path)?;
    for current in native_fs::path_components(path) {
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() && !native_fs::is_reparse(&meta) => (),
            Ok(_) => bail!("Expected an ordinary directory: {}", current.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match native_fs::create_private_directory(&current) {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        reject_symlink_components(&current)?;
                        if !current.is_dir() {
                            return Err(error.into());
                        }
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn open_private(path: &Path, create: bool, create_new: bool) -> Result<File> {
    reject_symlink_components(path.parent().context("File has no parent")?)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_file() || native_fs::is_reparse(&metadata) {
            bail!("Expected an ordinary private file: {}", path.display());
        }
    }
    let file = native_fs::open_private_file(path, create, create_new)
        .with_context(|| format!("Cannot open {}", path.display()))?;
    #[cfg(unix)]
    if file.metadata()?.permissions().mode() & 0o077 != 0 {
        bail!(
            "Runtime file is not private (expected mode 0600): {}",
            path.display()
        );
    }
    Ok(file)
}

fn read_private_string(path: &Path) -> Result<String> {
    let mut value = String::new();
    open_private(path, false, false)?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut value)?;
    if value.len() > 1024 * 1024 {
        bail!("Runtime metadata file is unexpectedly large");
    }
    Ok(value)
}

fn read_private_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_str(&read_private_string(path)?)
        .with_context(|| format!("Invalid runtime metadata: {}", path.display()))
}

fn create_private_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut file = open_private(path, false, true)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    Ok(())
}

fn atomic_private_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    atomic_private_bytes(path, &serde_json::to_vec_pretty(value)?)
}

fn atomic_private_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("File has no parent")?;
    reject_symlink_components(parent)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_file() || native_fs::is_reparse(&metadata) {
            bail!(
                "Refusing to replace non-file runtime metadata: {}",
                path.display()
            );
        }
    }
    let temp = parent.join(format!(".write-{}", Uuid::new_v4()));
    let result = (|| {
        let mut file = open_private(&temp, false, true)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        native_fs::replace_file(&temp, path)?;
        native_fs::sync_directory(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    pub(super) fn test_root() -> (TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("dev");
        (temp, root)
    }

    #[test]
    fn refuses_unmarked_nonempty_directory_without_touching_contents() {
        let (_temp, root) = test_root();
        fs::create_dir(&root).unwrap();
        fs::write(root.join("precious.txt"), "keep").unwrap();
        assert!(RuntimePaths::initialize_at(root.clone()).is_err());
        assert_eq!(
            fs::read_to_string(root.join("precious.txt")).unwrap(),
            "keep"
        );
        assert!(!root.join(MARKER).exists());
    }

    #[test]
    fn refuses_settings_redirect_to_any_other_workspace() {
        let (_temp, root) = test_root();
        let paths = RuntimePaths::initialize_at(root.clone()).unwrap();
        let mut settings = paths.read_settings().unwrap();
        settings.workspace_root = root.join("production");
        atomic_private_json(&paths.settings_path(), &settings).unwrap();
        assert!(RuntimePaths::initialize_at(root.clone()).is_err());
        assert!(!root.join("production").exists());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_workspace_and_runtime_file() {
        use std::os::unix::fs::symlink;
        let (_temp, root) = test_root();
        let paths = RuntimePaths::initialize_at(root.clone()).unwrap();
        fs::rename(paths.workspace_root(), root.join("saved")).unwrap();
        symlink(root.join("saved"), paths.workspace_root()).unwrap();
        assert!(RuntimePaths::initialize_at(root.clone()).is_err());
        fs::remove_file(paths.workspace_root()).unwrap();
        fs::rename(root.join("saved"), paths.workspace_root()).unwrap();
        symlink(paths.settings_path(), paths.token_path()).unwrap();
        assert!(paths.read_control_token().is_err());
        assert!(atomic_private_bytes(&paths.token_path(), b"replacement").is_err());
    }

    #[test]
    fn lock_blocks_second_instance_and_is_reusable_after_release() {
        let (_temp, root) = test_root();
        let paths = RuntimePaths::initialize_at(root).unwrap();
        let lock = paths.acquire_lock().unwrap();
        assert!(paths.acquire_lock().is_err());
        drop(lock);
        assert!(paths.acquire_lock().is_ok());
        assert!(paths.data_root().join("instance.lock").exists());
    }

    #[test]
    fn offline_identity_repair_never_initializes_missing_data_and_respects_instance_lock() {
        let (_temp, root) = test_root();
        assert!(RuntimePaths::identity_repair_paths(root.clone()).is_err());
        assert!(!root.exists());
        let paths = RuntimePaths::initialize_at(root.clone()).unwrap();
        let lock = paths.acquire_lock().unwrap();
        let error =
            RuntimePaths::identity_repair_at(root.clone(), serde_json::json!({})).unwrap_err();
        assert!(error.to_string().contains("Quit the desktop service"));
        assert!(!root.join("identity-repairs").exists());
        drop(lock);
        assert!(RuntimePaths::identity_repair_at(root.clone(), serde_json::json!({})).is_err());
        assert!(!root.join("identity-repairs").exists());
    }

    #[test]
    fn cleanup_preserves_another_instance_and_credentials_are_private() {
        let (_temp, root) = test_root();
        let paths = RuntimePaths::initialize_at(root).unwrap();
        let instance_id = Uuid::new_v4().to_string();
        let discovery = Discovery {
            api_version: API_VERSION,
            instance_id: instance_id.clone(),
            pid: std::process::id(),
            base_url: "http://127.0.0.1:12345".into(),
            workspace_root: paths.workspace_root().into(),
            environment: ENVIRONMENT.into(),
            identifier: APP_IDENTIFIER.into(),
        };
        let token = "a".repeat(64);
        paths.publish_runtime(&discovery, &token).unwrap();
        #[cfg(unix)]
        for path in [
            paths.connection_path(),
            paths.token_path(),
            paths.settings_path(),
        ] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        paths
            .remove_runtime_if_owned(&Uuid::new_v4().to_string(), &token)
            .unwrap();
        assert!(paths.connection_path().exists());
        paths
            .remove_runtime_if_owned(&instance_id, &"b".repeat(64))
            .unwrap();
        assert!(paths.token_path().exists());
        assert!(!paths
            .remove_stopped_runtime_if_owned(&Uuid::new_v4().to_string(), &token)
            .unwrap());
        assert!(!paths
            .remove_stopped_runtime_if_owned(&instance_id, &"b".repeat(64))
            .unwrap());
        assert!(paths.connection_path().exists());
        assert!(paths.token_path().exists());
        paths.remove_runtime_if_owned(&instance_id, &token).unwrap();
        assert!(!paths.connection_path().exists());
        assert!(!paths.token_path().exists());
    }

    #[test]
    fn discovery_rejects_nonliteral_or_ambiguous_loopback_urls() {
        let mut discovery = Discovery {
            api_version: API_VERSION,
            instance_id: Uuid::new_v4().to_string(),
            pid: 1,
            base_url: String::new(),
            workspace_root: PathBuf::new(),
            environment: ENVIRONMENT.into(),
            identifier: APP_IDENTIFIER.into(),
        };
        for url in [
            "http://localhost:1234",
            "http://127.0.0.1:0",
            "http://127.0.0.1:01234",
            "http://127.0.0.1:1234/",
            "http://127.0.0.1:1234@evil.test",
            "https://127.0.0.1:1234",
        ] {
            discovery.base_url = url.into();
            assert!(discovery.port().is_err(), "{url}");
        }
    }

    #[test]
    fn new_installations_pick_a_free_visible_home_folder_without_adopting_content() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().canonicalize().unwrap();
        let data = home.join("Library/Application Support/disposable.identifier");
        assert_eq!(
            home_workspace_candidate(&home, "PandaDrawer", &data).unwrap(),
            home.join("PandaDrawer")
        );
        fs::create_dir(home.join("PandaDrawer")).unwrap();
        assert_eq!(
            home_workspace_candidate(&home, "PandaDrawer", &data).unwrap(),
            home.join("PandaDrawer"),
            "an empty folder is reused"
        );
        fs::write(home.join("PandaDrawer/notes.txt"), "user content").unwrap();
        fs::write(home.join("PandaDrawer-2"), "a file, not a folder").unwrap();
        assert_eq!(
            home_workspace_candidate(&home, "PandaDrawer", &data).unwrap(),
            home.join("PandaDrawer-3")
        );
        assert_eq!(
            fs::read_to_string(home.join("PandaDrawer/notes.txt")).unwrap(),
            "user content"
        );
    }

    #[test]
    fn a_home_workspace_is_accepted_only_with_this_installation_marker() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().canonicalize().unwrap();
        let data = home.join("data");
        let workspace = home.join("PandaDrawer");
        ensure_directory_tree(&workspace.join(".platform")).unwrap();
        assert!(!is_home_workspace(&data, &workspace));
        assert!(crate::workspace::validate_selection(&data, &workspace, None).is_err());
        let marker = |data_root: &Path| HomeWorkspaceMarker {
            schema_version: 1,
            identifier: APP_IDENTIFIER.to_owned(),
            environment: ENVIRONMENT.to_owned(),
            data_root: data_root.to_path_buf(),
        };
        atomic_private_json(
            &workspace.join(HOME_WORKSPACE_MARKER),
            &marker(&home.join("other")),
        )
        .unwrap();
        assert!(
            !is_home_workspace(&data, &workspace),
            "another installation's folder"
        );
        atomic_private_json(&workspace.join(HOME_WORKSPACE_MARKER), &marker(&data)).unwrap();
        assert!(is_home_workspace(&data, &workspace));
        crate::workspace::validate_selection(&data, &workspace, None).unwrap();
        assert_eq!(
            home_workspace_candidate(&home, "PandaDrawer", &data).unwrap(),
            workspace,
            "an interrupted first start resumes its own folder"
        );
        assert!(!is_home_workspace(&data, &data.join("workspace")));
    }
}

#[cfg(all(test, windows))]
#[path = "paths/windows_startup_tests.rs"]
mod windows_startup_tests;
