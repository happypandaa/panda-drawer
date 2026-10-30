//! Development Node selection is independent of application runtime Node selection.
//! Installed resources default to verified private Node/npm; an explicit local choice
//! is preserved. Resource-free development/test managers retain system discovery.
use crate::{
    owned_process::{IoPlan, OwnedProcessRegistry, ProcessOptions, ProcessPipes},
    projects::check_no_symlinks,
    ApiError,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutableInfo {
    pub path: PathBuf,
    pub version: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentInfo {
    pub ready: bool,
    pub node: Option<ExecutableInfo>,
    pub npm: Option<ExecutableInfo>,
    pub selection: String,
    pub configured_node_path: Option<PathBuf>,
    pub managed_node_path: Option<PathBuf>,
    pub supported_node_range: String,
    pub recommended_node: String,
    pub download_url: String,
    pub issues: Vec<ApiError>,
}
#[derive(Clone, Debug)]
pub struct Toolchain {
    pub node: ExecutableInfo,
    pub npm: ExecutableInfo,
    pub path: String,
}
#[derive(Clone)]
pub struct EnvironmentManager {
    settings_path: PathBuf,
    managed_root: Option<PathBuf>,
    processes: OwnedProcessRegistry,
    project_boundary: Option<crate::project_tool_boundary::ProjectToolBoundary>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    schema_version: u32,
    node_path: Option<PathBuf>,
}
impl EnvironmentManager {
    pub fn new(data_root: PathBuf) -> Self {
        Self::with_processes(data_root, None, OwnedProcessRegistry::default())
    }
    pub fn with_managed_node(data_root: PathBuf, root: PathBuf) -> Self {
        Self::with_processes(data_root, Some(root), OwnedProcessRegistry::default())
    }
    pub(crate) fn with_processes(
        data_root: PathBuf,
        managed_root: Option<PathBuf>,
        processes: OwnedProcessRegistry,
    ) -> Self {
        Self {
            settings_path: data_root.join("toolchain.json"),
            managed_root,
            processes,
            project_boundary: None,
        }
    }
    pub(crate) fn with_project_boundary(mut self, workspace: PathBuf) -> Self {
        self.project_boundary = Some(crate::project_tool_boundary::ProjectToolBoundary::new(
            self.settings_path.parent().unwrap().into(),
            workspace,
        ));
        self
    }
    pub(crate) fn with_process_registry(&self, processes: OwnedProcessRegistry) -> Self {
        let mut scoped = self.clone();
        scoped.processes = processes;
        scoped
    }
    fn configured(&self) -> Result<Option<PathBuf>, ApiError> {
        check_no_symlinks(&self.settings_path, true)?;
        if !self.settings_path.exists() {
            return Ok(None);
        }
        let settings: Settings = serde_json::from_slice(&fs::read(&self.settings_path)?)?;
        if settings.schema_version != 1 {
            return Err(ApiError::new(
                "TOOLCHAIN_SETTINGS_INVALID",
                "Unsupported local toolchain settings",
            ));
        }
        Ok(settings.node_path)
    }
    pub async fn configure_node(&self, path: Option<PathBuf>) -> Result<EnvironmentInfo, ApiError> {
        crate::platform_support::require_runtime().map_err(ApiError::from)?;
        let tools = if let Some(path) = &path {
            if !path.is_absolute() {
                return Err(ApiError::new(
                    "INVALID_NODE_PATH",
                    "Choose an absolute path to the Node executable",
                ));
            }
            Some(
                probe_with_boundary(path, &self.processes, (), self.project_boundary.as_ref())
                    .await?,
            )
        } else {
            None
        };
        crate::node_apps::development::with_source_ownership_admission(|| {
            if let (Some(boundary), Some(path), Some(tools)) =
                (&self.project_boundary, &path, &tools)
            {
                for selected in [path, &tools.node.path, &tools.npm.path] {
                    boundary.require_path(selected)?;
                }
                if let Some(npm) = npm_entry_candidates(path, &tools.node.path)
                    .into_iter()
                    .find(|path| fs::canonicalize(path).is_ok_and(|path| path.is_file()))
                {
                    boundary.require_dependency_path(&npm)?;
                }
            }
            crate::builds::atomic_json(
                &self.settings_path,
                &Settings {
                    schema_version: 1,
                    node_path: path,
                },
            )
        })?;
        Ok(self.doctor().await)
    }
    pub async fn doctor(&self) -> EnvironmentInfo {
        let configured = self.configured();
        let mut info = EnvironmentInfo {
            ready: false,
            node: None,
            npm: None,
            selection: "detected".into(),
            configured_node_path: None,
            managed_node_path: self.managed_root.as_ref().map(|root| root.join("bin/node")),
            supported_node_range: "^22.12.0 || ^24.0.0".into(),
            recommended_node: "24 LTS".into(),
            download_url: "https://nodejs.org/en/download".into(),
            issues: vec![],
        };
        let configured = match configured {
            Ok(value) => value,
            Err(error) => {
                info.issues.push(error);
                return info;
            }
        };
        info.configured_node_path = configured.clone();
        if configured.is_some() {
            info.selection = "explicit".into();
        } else if self.managed_root.is_some() {
            info.selection = "managed".into();
        }
        match self.selected(configured, ()).await {
            Ok(toolchain) => {
                info.ready = true;
                info.node = Some(toolchain.node);
                info.npm = Some(toolchain.npm);
            }
            Err(error) => info.issues.push(error),
        }
        info
    }
    pub async fn resolve(&self) -> Result<Toolchain, ApiError> {
        self.resolve_with_keepalive(()).await
    }
    pub(crate) async fn resolve_with_keepalive<G: Clone + Send + 'static>(
        &self,
        keep_alive: G,
    ) -> Result<Toolchain, ApiError> {
        self.selected(self.configured()?, keep_alive).await
    }
    async fn selected<G: Clone + Send + 'static>(
        &self,
        configured: Option<PathBuf>,
        keep_alive: G,
    ) -> Result<Toolchain, ApiError> {
        if let Some(node) = configured {
            // Never silently change an explicit v1 selection when it disappears.
            return probe_owned(&node, &self.processes, keep_alive).await;
        }
        if let Some(root) = &self.managed_root {
            // A damaged installation must not silently execute a host toolchain.
            // Verify once per resolution, on a blocking worker, before either probe.
            let verified = crate::managed_toolchain::verify(root.clone()).await?;
            return probe_managed(verified, &self.processes, keep_alive).await;
        }
        let mut last_error = None;
        for candidate in candidates() {
            match probe_owned(&candidate, &self.processes, keep_alive.clone()).await {
                Ok(toolchain) => return Ok(toolchain),
                Err(error) => {
                    if error.code.starts_with("PROCESS_")
                        && error.details["operationOutcome"] != "not_started"
                    {
                        return Err(error);
                    }
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            ApiError::new(
                "NODE_NOT_FOUND",
                "Install Node 24 LTS, or select an existing Node executable in platform settings",
            )
        }))
    }
}
fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    // nvm installs are discoverable even when Finder supplies /usr/bin:/bin only.
    if let Some(home) = dirs::home_dir() {
        if let Ok(versions) = fs::read_dir(home.join(".nvm/versions/node")) {
            let mut found: Vec<_> = versions
                .filter_map(Result::ok)
                .map(|entry| entry.path().join("bin/node"))
                .collect();
            found.sort_by_key(|path| {
                std::cmp::Reverse(
                    version_tuple(
                        path.parent()
                            .and_then(Path::parent)
                            .and_then(Path::file_name)
                            .and_then(|x| x.to_str())
                            .unwrap_or_default(),
                    )
                    .unwrap_or((0, 0, 0)),
                )
            });
            paths.extend(found);
        }
        paths.extend([home.join(".volta/bin/node"), home.join(".local/bin/node")]);
    }
    for base in [
        "/opt/homebrew/opt/node@24/bin",
        "/opt/homebrew/opt/node@22/bin",
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
    ] {
        paths.push(Path::new(base).join("node"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path).map(|path| path.join("node")));
    }
    let mut seen = HashSet::new();
    paths.retain(|path| path.is_file() && seen.insert(path.clone()));
    paths
}
fn version_tuple(version: &str) -> Option<(u64, u64, u64)> {
    let version = version.trim().trim_start_matches('v');
    let mut components = version.split('.');
    let major = components.next()?.parse().ok()?;
    let minor = components.next()?.parse().ok()?;
    let patch = components.next()?.parse().ok()?;
    if components.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}
pub fn supported_node(version: &str) -> bool {
    matches!(version_tuple(version),Some((22,minor,_)) if minor>=12)
        || matches!(version_tuple(version), Some((24, _, _)))
}
fn tool_path(node: &Path) -> String {
    format!(
        "{}:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin",
        node.parent().unwrap_or(Path::new("/usr/bin")).display()
    )
}
async fn output<G: Send + 'static>(
    command: Command,
    processes: &OwnedProcessRegistry,
    keep_alive: G,
) -> Result<String, ApiError> {
    let mut run = processes.start(
        command,
        ProcessOptions::new("node-environment/version-probe", Duration::from_secs(5)),
        keep_alive,
        |pipes| {
            let ProcessPipes {
                stdin,
                mut stdout,
                mut stderr,
                stop: _,
            } = pipes;
            drop(stdin);
            IoPlan::new(async move {
                let mut output = Vec::new();
                let mut bytes = [0u8; 4096];
                loop {
                    let count = stdout.read(&mut bytes).await?;
                    if count == 0 {
                        return Ok(output);
                    }
                    if output.len() + count > 16_384 {
                        return Err(ApiError::new(
                            "TOOLCHAIN_PROBE_FAILED",
                            "Node/npm version output exceeded its byte budget",
                        ));
                    }
                    output.extend_from_slice(&bytes[..count]);
                }
            })
            .drain("version-stderr", async move {
                let mut bytes = [0u8; 4096];
                while stderr.read(&mut bytes).await? != 0 {}
                Ok(())
            })
        },
    )?;
    let result = (&mut run).await.map_err(|mut error| {
        if error.code == "PROCESS_WORK_TIMEOUT" {
            error.code = "TOOLCHAIN_PROBE_TIMEOUT".into();
        }
        let receipt = run.snapshot();
        let closed = receipt.phase == crate::owned_process::ProcessPhase::Closed
            && receipt.leader_reaped
            && receipt.group_exited
            && receipt.io_joined;
        error = error.detail("processResourceClosed", closed);
        if !closed {
            error = error.outcome("unknown");
        }
        error
    })?;
    if !result.status.success() {
        return Err(ApiError::new(
            "TOOLCHAIN_PROBE_FAILED",
            "Node/npm version check failed",
        ));
    }
    Ok(String::from_utf8_lossy(&result.value).trim().to_owned())
}
#[cfg(test)]
async fn probe(candidate: &Path) -> Result<Toolchain, ApiError> {
    probe_owned(candidate, &OwnedProcessRegistry::default(), ()).await
}
async fn probe_owned<G: Clone + Send + 'static>(
    candidate: &Path,
    processes: &OwnedProcessRegistry,
    keep_alive: G,
) -> Result<Toolchain, ApiError> {
    probe_with_boundary(candidate, processes, keep_alive, None).await
}
async fn probe_with_boundary<G: Clone + Send + 'static>(
    candidate: &Path,
    processes: &OwnedProcessRegistry,
    keep_alive: G,
    boundary: Option<&crate::project_tool_boundary::ProjectToolBoundary>,
) -> Result<Toolchain, ApiError> {
    if !candidate.is_absolute() || !candidate.is_file() {
        return Err(ApiError::new(
            "NODE_NOT_FOUND",
            format!("Node executable not found: {}", candidate.display()),
        ));
    }
    if let Some(boundary) = boundary {
        boundary.require_path(candidate)?;
    }
    let node_path = fs::canonicalize(candidate)?;
    let mut command = Command::new(&node_path);
    command.arg("--version");
    let version = output(command, processes, keep_alive.clone()).await?;
    if !supported_node(&version) {
        return Err(ApiError::new(
            "NODE_VERSION_UNSUPPORTED",
            format!("Node {version} is unsupported; use 22.12+ on Node 22, or Node 24 LTS"),
        ));
    }
    // npm must belong to this installation, and is executed by the selected Node, not its shebang.
    // Prefer the JS entry before npm shims: some installations use ordinary shell files.
    let (npm_entry, npm) = npm_entry_candidates(candidate, &node_path)
        .into_iter()
        .filter_map(|path| fs::canonicalize(&path).ok().map(|actual| (path, actual)))
        .find(|(_, path)| path.is_file())
        .ok_or_else(|| {
            ApiError::new(
                "NPM_NOT_FOUND",
                "No npm JavaScript entry was found beside the selected Node",
            )
        })?;
    if let Some(boundary) = boundary {
        boundary.require_dependency_path(&npm_entry)?;
        boundary.require_path(&npm)?;
    }
    let path = tool_path(&node_path);
    let mut command = Command::new(&node_path);
    command.arg(&npm).arg("--version").env("PATH", &path);
    let npm_version = output(command, processes, keep_alive)
        .await
        .map_err(|mut error| {
            if !error.code.starts_with("PROCESS_") {
                error.code = "NPM_NOT_FOUND".into();
            }
            error
        })?;
    Ok(Toolchain {
        node: ExecutableInfo {
            path: node_path,
            version,
        },
        npm: ExecutableInfo {
            path: npm,
            version: npm_version,
        },
        path,
    })
}
pub(crate) fn npm_entry_candidates(candidate: &Path, node_path: &Path) -> [PathBuf; 4] {
    [
        candidate
            .parent()
            .unwrap()
            .join("../lib/node_modules/npm/bin/npm-cli.js"),
        node_path
            .parent()
            .unwrap()
            .join("../lib/node_modules/npm/bin/npm-cli.js"),
        candidate.parent().unwrap().join("npm"),
        node_path.parent().unwrap().join("npm"),
    ]
}

async fn probe_managed<G: Clone + Send + 'static>(
    verified: crate::managed_toolchain::VerifiedToolchain,
    processes: &OwnedProcessRegistry,
    keep_alive: G,
) -> Result<Toolchain, ApiError> {
    let path = std::env::join_paths([
        verified.root.join("bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ])
    .map_err(|_| {
        ApiError::new(
            "MANAGED_NODE_INVALID",
            "受管开发工具路径不能用于隔离的 PATH。",
        )
    })?
    .into_string()
    .map_err(|_| ApiError::new("MANAGED_NODE_INVALID", "受管开发工具路径不是有效文本。"))?;
    let command = || {
        let mut command = Command::new(&verified.node);
        command
            .env_clear()
            .current_dir(&verified.root)
            .env("PATH", &path)
            .env("LC_ALL", "C")
            .env("HOME", &verified.root)
            .env(
                "NPM_CONFIG_USERCONFIG",
                verified.root.join(".unused-user-npmrc"),
            )
            .env(
                "NPM_CONFIG_GLOBALCONFIG",
                verified.root.join(".unused-global-npmrc"),
            );
        command
    };
    let mut node = command();
    node.arg("--version");
    let version = output(node, processes, keep_alive.clone()).await?;
    if version != format!("v{}", verified.node_version) {
        return Err(ApiError::new(
            "MANAGED_NODE_VERSION_MISMATCH",
            "受管开发 Node 的实际版本不符合固定发行声明。",
        ));
    }
    let mut npm = command();
    npm.arg(&verified.npm)
        .arg("--prefix")
        .arg(&verified.root)
        .arg("--version");
    let npm_version = output(npm, processes, keep_alive).await?;
    if npm_version != verified.npm_version {
        return Err(ApiError::new(
            "MANAGED_NODE_VERSION_MISMATCH",
            "受管开发 npm 的实际版本不符合固定发行声明。",
        ));
    }
    Ok(Toolchain {
        node: ExecutableInfo {
            path: verified.node,
            version,
        },
        npm: ExecutableInfo {
            path: verified.npm,
            version: npm_version,
        },
        path,
    })
}
#[cfg(all(test, unix))]
#[path = "environment_owned_process_tests.rs"]
mod owned_process_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::{Builder, TempDir};

    fn disposable() -> (TempDir, PathBuf) {
        let temp = Builder::new()
            .prefix("disposable-development-environment-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        (temp, root)
    }

    // A probe stub tests selection routing only, never real Node/npm compatibility.
    #[cfg(unix)]
    fn probe_stub(root: &Path) -> PathBuf {
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("lib/node_modules/npm/bin")).unwrap();
        let node = root.join("bin/node");
        fs::write(&node, "#!/bin/sh\nif [ \"$1\" = '--version' ]; then printf 'v24.22.0\\n'; else case \"$1\" in */npm-cli.js) printf '11.19.1\\n';; *) exit 87;; esac; fi\n").unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(root.join("bin/npm"), "#!/bin/sh\nexit 87\n").unwrap();
        fs::write(
            root.join("lib/node_modules/npm/bin/npm-cli.js"),
            "synthetic entry, not executed",
        )
        .unwrap();
        node
    }

    fn save_selection(data: &Path, node: Option<&Path>) {
        fs::create_dir_all(data).unwrap();
        fs::write(
            data.join("toolchain.json"),
            serde_json::to_vec(&Settings {
                schema_version: 1,
                node_path: node.map(Path::to_path_buf),
            })
            .unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn supported_versions_are_explicit() {
        for v in ["v22.12.0", "22.22.1", "24.0.0", "v24.14.1"] {
            assert!(supported_node(v));
        }
        for v in ["22.11.9", "20.19.0", "23.0.0", "25.0.0", "24.0.0-rc.1"] {
            assert!(!supported_node(v));
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn global_node_selection_rejects_project_paths_and_project_owned_npm_before_execution() {
        use std::os::unix::fs::symlink;
        let (_temporary, root) = disposable();
        let data = root.join("data");
        let workspace = root.join("workspace");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&workspace).unwrap();
        let manager =
            EnvironmentManager::new(data.clone()).with_project_boundary(workspace.clone());
        let external = probe_stub(&root.join("external-tools"));
        manager
            .configure_node(Some(external.clone()))
            .await
            .unwrap();
        let settings = fs::read(data.join("toolchain.json")).unwrap();
        let project_node = probe_stub(&workspace.join("projects/disposable"));
        let marker = root.join("project-tool-was-executed");
        fs::write(
            &project_node,
            format!(
                "#!/bin/sh\ntouch '{}'\nprintf 'v24.22.0\\n'\n",
                marker.display()
            ),
        )
        .unwrap();
        let outside_alias = root.join("outside-node-alias");
        symlink(&project_node, &outside_alias).unwrap();
        let inside_alias = project_node.with_extension("outside-alias");
        symlink(&external, &inside_alias).unwrap();
        for selected in [&project_node, &outside_alias, &inside_alias] {
            assert_eq!(
                manager
                    .configure_node(Some(selected.clone()))
                    .await
                    .unwrap_err()
                    .code,
                "GLOBAL_TOOL_PROJECT_OWNED"
            );
            assert_eq!(fs::read(data.join("toolchain.json")).unwrap(), settings);
        }
        assert!(!marker.exists(), "a rejected project Node must not execute");
        let npm_target = workspace.join("projects/disposable/project-npm.js");
        fs::write(&npm_target, "not executed").unwrap();
        let npm_entry = root.join("external-tools/lib/node_modules/npm/bin/npm-cli.js");
        fs::remove_file(&npm_entry).unwrap();
        symlink(&npm_target, npm_entry).unwrap();
        assert_eq!(
            manager
                .configure_node(Some(external))
                .await
                .unwrap_err()
                .code,
            "GLOBAL_TOOL_PROJECT_OWNED"
        );
        assert_eq!(fs::read(data.join("toolchain.json")).unwrap(), settings);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn explicit_v1_selection_is_preserved_over_managed_default() {
        let (_temp, root) = disposable();
        let node = probe_stub(&root.join("explicit"));
        let data = root.join("data");
        save_selection(&data, Some(&node));
        let before = fs::read(data.join("toolchain.json")).unwrap();
        let managed = root.join("missing-managed");
        let manager = EnvironmentManager::with_managed_node(data.clone(), managed.clone());
        let report = manager.doctor().await;
        assert!(report.ready, "{:?}", report.issues);
        assert_eq!(report.selection, "explicit");
        assert_eq!(report.configured_node_path, Some(node.clone()));
        assert_eq!(report.managed_node_path, Some(managed.join("bin/node")));
        assert_eq!(manager.resolve().await.unwrap().node.path, node);
        assert_eq!(fs::read(data.join("toolchain.json")).unwrap(), before);
    }

    #[tokio::test]
    async fn missing_explicit_selection_never_falls_back_to_host_or_managed_tools() {
        let (_temp, root) = disposable();
        let data = root.join("data");
        let node = root.join("missing-explicit/bin/node");
        save_selection(&data, Some(&node));
        let manager = EnvironmentManager::with_managed_node(data, root.join("missing-managed"));
        let report = manager.doctor().await;
        assert!(!report.ready);
        assert_eq!(report.selection, "explicit");
        assert_eq!(report.issues[0].code, "NODE_NOT_FOUND");
        assert_eq!(manager.resolve().await.unwrap_err().code, "NODE_NOT_FOUND");
    }

    #[tokio::test]
    async fn missing_managed_resources_fail_closed_without_persisting_auto_selection() {
        let (_temp, root) = disposable();
        let data = root.join("data");
        let managed = root.join("missing-managed");
        let manager = EnvironmentManager::with_managed_node(data.clone(), managed.clone());
        let report = manager.doctor().await;
        assert!(!report.ready);
        assert_eq!(report.selection, "managed");
        assert_eq!(report.managed_node_path, Some(managed.join("bin/node")));
        assert!(report.issues[0].code.starts_with("MANAGED_NODE_"));
        assert!(manager.resolve().await.is_err());
        assert!(!data.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn npm_javascript_entry_is_preferred_to_an_ordinary_shell_shim() {
        let (_temp, root) = disposable();
        let node = probe_stub(&root);
        let tools = probe(&node).await.unwrap();
        assert_eq!(
            tools.npm.path,
            root.join("lib/node_modules/npm/bin/npm-cli.js")
        );
        fs::remove_file(root.join("lib/node_modules/npm/bin/npm-cli.js")).unwrap();
        assert_eq!(probe(&node).await.unwrap_err().code, "NPM_NOT_FOUND");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn extensionless_local_npm_is_probed_by_the_selected_node() {
        let (_temp, root) = disposable();
        let node = probe_stub(&root);
        fs::remove_file(root.join("lib/node_modules/npm/bin/npm-cli.js")).unwrap();
        fs::write(
            root.join("bin/npm"),
            "#!/usr/bin/env node\nconsole.log('11.19.1')\n",
        )
        .unwrap();
        // This stub verifies routing, not real npm compatibility. The selected
        // Node must be asked to execute the fallback entry even without .js.
        fs::write(&node, "#!/bin/sh\nif [ \"$1\" = '--version' ]; then printf 'v24.22.0\\n'; else case \"$1\" in */bin/npm) printf '11.19.1\\n';; *) exit 87;; esac; fi\n").unwrap();
        let tools = probe(&node).await.unwrap();
        assert_eq!(tools.npm.path, root.join("bin/npm"));
        assert_eq!(tools.npm.version, "11.19.1");
    }

    #[tokio::test]
    #[ignore = "Requires an explicit official development-node sample under .test-artifacts"]
    async fn official_managed_sample_resolves_without_user_settings_or_host_path() {
        let sample = PathBuf::from(
            std::env::var_os("PANDA_DRAWER_DEVELOPMENT_NODE_TEST_ROOT")
                .expect("explicit official development-node sample"),
        );
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let artifacts = repository.join(".test-artifacts");
        assert!(sample.is_absolute() && sample.canonicalize().unwrap().starts_with(&artifacts));
        let data = Builder::new()
            .prefix("disposable-managed-environment-")
            .tempdir_in(&artifacts)
            .unwrap();
        let manager =
            EnvironmentManager::with_managed_node(data.path().to_path_buf(), sample.clone());
        let tools = manager.resolve().await.unwrap();
        assert_eq!(tools.node.path, sample.join("bin/node"));
        assert_eq!(tools.node.version, "v24.22.0");
        assert_eq!(tools.npm.version, "11.19.1");
        assert_eq!(
            tools.npm.path,
            sample.join("lib/node_modules/npm/bin/npm-cli.js")
        );
        assert_eq!(
            tools.path,
            format!(
                "{}:/usr/bin:/bin:/usr/sbin:/sbin",
                sample.join("bin").display()
            )
        );
        assert!(!data.path().join("toolchain.json").exists());
        // A fresh check after the real version probes also detects any cache/log
        // or other unexpected package mutation performed by those probes.
        crate::managed_toolchain::verify(sample).await.unwrap();

        // Also exercise an extensionless ordinary JS wrapper with a real Node.
        // This copy lives only in the disposable test data, never in the package.
        let local_bin = data.path().join("explicit/bin");
        fs::create_dir_all(&local_bin).unwrap();
        fs::copy(&tools.node.path, local_bin.join("node")).unwrap();
        fs::write(
            local_bin.join("npm"),
            "#!/usr/bin/env node\nconsole.log('11.19.1')\n",
        )
        .unwrap();
        let local = probe(&local_bin.join("node")).await.unwrap();
        assert_eq!(local.npm.path, local_bin.join("npm"));
        assert_eq!(local.npm.version, "11.19.1");
    }
}
