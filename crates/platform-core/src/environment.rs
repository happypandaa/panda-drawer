//! System Node discovery is independent of the shell that launched the desktop app.
use crate::{projects::check_no_symlinks, ApiError};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
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
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    schema_version: u32,
    node_path: Option<PathBuf>,
}
impl EnvironmentManager {
    pub fn new(data_root: PathBuf) -> Self {
        Self {
            settings_path: data_root.join("toolchain.json"),
        }
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
        if let Some(path) = &path {
            if !path.is_absolute() {
                return Err(ApiError::new(
                    "INVALID_NODE_PATH",
                    "Choose an absolute path to the Node executable",
                ));
            }
            probe(path).await?;
        }
        crate::builds::atomic_json(
            &self.settings_path,
            &Settings {
                schema_version: 1,
                node_path: path,
            },
        )?;
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
        }
        let candidates = configured.map_or_else(candidates, |path| vec![path]);
        let mut last_error = None;
        for candidate in candidates {
            match probe(&candidate).await {
                Ok(toolchain) => {
                    info.ready = true;
                    info.node = Some(toolchain.node);
                    info.npm = Some(toolchain.npm);
                    return info;
                }
                Err(error) => {
                    last_error = Some(error);
                }
            }
        }
        info.issues.push(last_error.unwrap_or_else(|| {
            ApiError::new(
                "NODE_NOT_FOUND",
                "Install Node 24 LTS, or select an existing Node executable in platform settings",
            )
        }));
        info
    }
    pub async fn resolve(&self) -> Result<Toolchain, ApiError> {
        let report = self.doctor().await;
        if !report.ready {
            return Err(report
                .issues
                .into_iter()
                .next()
                .unwrap_or_else(|| ApiError::new("NODE_NOT_FOUND", "Node is unavailable")));
        }
        let node = report.node.unwrap();
        let npm = report.npm.unwrap();
        let path = tool_path(&node.path);
        Ok(Toolchain { node, npm, path })
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
async fn output(mut command: Command) -> Result<String, ApiError> {
    command.kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .map_err(|_| {
            ApiError::new(
                "TOOLCHAIN_PROBE_TIMEOUT",
                "Node/npm version check timed out",
            )
        })?
        .map_err(|error| ApiError::new("NODE_NOT_FOUND", error.to_string()))?;
    if !output.status.success() || output.stdout.len() > 16_384 {
        return Err(ApiError::new(
            "TOOLCHAIN_PROBE_FAILED",
            "Node/npm version check failed",
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
async fn probe(candidate: &Path) -> Result<Toolchain, ApiError> {
    if !candidate.is_absolute() || !candidate.is_file() {
        return Err(ApiError::new(
            "NODE_NOT_FOUND",
            format!("Node executable not found: {}", candidate.display()),
        ));
    }
    let node_path = fs::canonicalize(candidate)?;
    let mut command = Command::new(&node_path);
    command.arg("--version");
    let version = output(command).await?;
    if !supported_node(&version) {
        return Err(ApiError::new(
            "NODE_VERSION_UNSUPPORTED",
            format!("Node {version} is unsupported; use 22.12+ on Node 22, or Node 24 LTS"),
        ));
    }
    // npm must belong to this installation, and is executed by the selected Node, not its shebang.
    let npm_candidates = [
        candidate.parent().unwrap().join("npm"),
        node_path.parent().unwrap().join("npm"),
        node_path
            .parent()
            .unwrap()
            .join("../lib/node_modules/npm/bin/npm-cli.js"),
    ];
    let npm = npm_candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            ApiError::new(
                "NPM_NOT_FOUND",
                "No npm executable was found beside the selected Node",
            )
        })?;
    let npm = fs::canonicalize(npm)?;
    let path = tool_path(&node_path);
    let mut command = Command::new(&node_path);
    command.arg(&npm).arg("--version").env("PATH", &path);
    let npm_version = output(command)
        .await
        .map_err(|error| ApiError::new("NPM_NOT_FOUND", error.message))?;
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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supported_versions_are_explicit() {
        for v in ["v22.12.0", "22.22.1", "24.0.0", "v24.14.1"] {
            assert!(supported_node(v));
        }
        for v in ["22.11.9", "20.19.0", "23.0.0", "25.0.0", "24.0.0-rc.1"] {
            assert!(!supported_node(v));
        }
    }
}
