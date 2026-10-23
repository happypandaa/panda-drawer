//! Independent Windows development tools. A selected tool never silently falls
//! back to a different executable, and no unverified payload is executed.
use crate::ApiError;
use platform_host::{WindowsSourceDirectory, WorkspaceDirectory};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::watch;

type Result<T> = std::result::Result<T, ApiError>;
fn error(message: impl Into<String>) -> ApiError {
    ApiError::new("DEVELOPMENT_TOOL_UNAVAILABLE", message).outcome("not_started")
}

fn fs(error: platform_host::WorkspaceError) -> ApiError {
    ApiError::new(error.code, error.message).outcome("not_started")
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub executable: bool,
    pub pe_machine: Option<u16>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Release {
    kind: String,
    version: String,
    platform: String,
    arch: String,
    paths: BTreeMap<String, String>,
    files: Vec<ToolFile>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Selected {
    root: PathBuf,
    identity: platform_host::WindowsObjectIdentity,
    paths: BTreeMap<String, String>,
    files: Vec<ToolFile>,
    version: String,
    npm_version: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Selection {
    schema_version: u32,
    revision: u32,
    directory_identity: platform_host::WindowsObjectIdentity,
    node: Option<Selected>,
    git: Option<Selected>,
}
#[derive(Clone)]
pub struct DevelopmentTools {
    resources: PathBuf,
}
pub struct ToolLease {
    pub root: PathBuf,
    pub executable: PathBuf,
    pub npm_cli: Option<PathBuf>,
    pub exec_path: Option<PathBuf>,
    pub template_path: Option<PathBuf>,
    pub certificate_bundle: Option<PathBuf>,
    pub version: String,
    pub npm_version: Option<String>,
    pub selection: String,
    guards: Vec<WindowsSourceDirectory>,
    entries: BTreeMap<String, bool>,
    skipped_metadata: Option<String>,
}
impl ToolLease {
    pub fn verify(&self) -> Result<()> {
        for guard in &self.guards {
            guard.verify_binding().map_err(fs)?;
        }
        if tree_entries(&self.guards[0], self.skipped_metadata.as_deref())? != self.entries {
            return Err(error(
                "开发工具目录清单已变化，请重新核对；没有改用其他工具。",
            ));
        }
        Ok(())
    }
    pub fn environment(
        &self,
        home: &Path,
        temporary: &Path,
        node: Option<&ToolLease>,
    ) -> Result<BTreeMap<OsString, OsString>> {
        let windows = platform_host::windows_command::windows_directory()?;
        let system = windows.join("System32");
        let mut paths = vec![self.executable.parent().unwrap().to_owned()];
        if let Some(node) = node {
            paths.insert(0, node.executable.parent().unwrap().to_owned());
        }
        if let Some(exec) = &self.exec_path {
            paths.push(exec.clone());
            if let Some(parent) = exec.parent().and_then(Path::parent) {
                paths.push(parent.join("bin"));
            }
        }
        paths.push(system.clone());
        let path = std::env::join_paths(paths).map_err(|_| error("开发工具路径不能编码。"))?;
        let mut environment = BTreeMap::from([
            ("SYSTEMROOT".into(), windows.as_os_str().into()),
            ("WINDIR".into(), windows.as_os_str().into()),
            ("ComSpec".into(), system.join("cmd.exe").into_os_string()),
            ("PATH".into(), path),
            ("TEMP".into(), temporary.as_os_str().into()),
            ("TMP".into(), temporary.as_os_str().into()),
            ("HOME".into(), home.as_os_str().into()),
            ("USERPROFILE".into(), home.as_os_str().into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_CONFIG_SYSTEM".into(), "NUL".into()),
            ("GIT_CONFIG_GLOBAL".into(), "NUL".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("GCM_INTERACTIVE".into(), "Never".into()),
            ("GIT_ASKPASS".into(), "".into()),
            ("SSH_ASKPASS".into(), "".into()),
            (
                "NPM_CONFIG_USERCONFIG".into(),
                home.join("user.npmrc").into_os_string(),
            ),
            (
                "NPM_CONFIG_GLOBALCONFIG".into(),
                home.join("global.npmrc").into_os_string(),
            ),
            ("NPM_CONFIG_AUDIT".into(), "false".into()),
            ("NPM_CONFIG_FUND".into(), "false".into()),
            ("NPM_CONFIG_UPDATE_NOTIFIER".into(), "false".into()),
            (
                "NPM_CONFIG_CACHE".into(),
                temporary.join("npm-cache").into_os_string(),
            ),
            (
                "NPM_CONFIG_PREFIX".into(),
                temporary.join("npm-prefix").into_os_string(),
            ),
        ]);
        if let Some(exec) = &self.exec_path {
            environment.insert("GIT_EXEC_PATH".into(), exec.as_os_str().into());
        }
        if let Some(template) = &self.template_path {
            environment.insert("GIT_TEMPLATE_DIR".into(), template.as_os_str().into());
        }
        if let Some(bundle) = &self.certificate_bundle {
            environment.insert("GIT_SSL_CAINFO".into(), bundle.as_os_str().into());
        }
        Ok(environment)
    }
}
fn tree_entries(
    root: &WindowsSourceDirectory,
    skip: Option<&str>,
) -> Result<BTreeMap<String, bool>> {
    fn visit(
        root: &WindowsSourceDirectory,
        prefix: &str,
        skip: Option<&str>,
        entries: &mut BTreeMap<String, bool>,
    ) -> Result<()> {
        for item in root.list().map_err(fs)? {
            if prefix.is_empty() && skip == Some(item.name.as_str()) {
                continue;
            }
            let path = if prefix.is_empty() {
                item.name.clone()
            } else {
                format!("{prefix}/{}", item.name)
            };
            if entries.len() >= 60_000 {
                return Err(error("开发资源目录清单超限。"));
            }
            entries.insert(path.clone(), item.is_directory);
            if item.is_directory {
                visit(
                    &root.open_directory(&item.name).map_err(fs)?,
                    &path,
                    skip,
                    entries,
                )?;
            }
        }
        root.verify_binding().map_err(fs)
    }
    let mut entries = BTreeMap::new();
    visit(root, "", skip, &mut entries)?;
    Ok(entries)
}
fn expected_entries(files: &[ToolFile]) -> BTreeMap<String, bool> {
    let mut entries = BTreeMap::new();
    for file in files {
        entries.insert(file.path.clone(), false);
        let mut current = file.path.as_str();
        while let Some((parent, _)) = current.rsplit_once('/') {
            entries.insert(parent.to_owned(), true);
            current = parent;
        }
    }
    entries
}
fn valid_label(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 2048
        && !path.contains(['\\', ':', '\0'])
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with(['.', ' '])
                && !part.chars().any(char::is_control)
        })
}
fn child(root: &WindowsSourceDirectory, path: &str) -> Result<(WindowsSourceDirectory, String)> {
    if !valid_label(path) {
        return Err(error("开发资源路径无效。"));
    }
    let mut parts: Vec<_> = path.split('/').collect();
    let name = parts.pop().unwrap().to_owned();
    let mut directory = root.clone();
    for part in parts {
        directory = directory.open_directory(part).map_err(fs)?;
    }
    Ok((directory, name))
}
fn inventory(
    root: &WindowsSourceDirectory,
    skip: Option<&str>,
) -> Result<(Vec<ToolFile>, Vec<WindowsSourceDirectory>)> {
    fn visit(
        directory: &WindowsSourceDirectory,
        prefix: &str,
        skip: Option<&str>,
        files: &mut Vec<ToolFile>,
        guards: &mut Vec<WindowsSourceDirectory>,
        total: &mut u64,
    ) -> Result<()> {
        for entry in directory.list().map_err(fs)? {
            let label = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            if prefix.is_empty() && skip == Some(entry.name.as_str()) {
                continue;
            }
            if !valid_label(&label) {
                return Err(error("开发资源含不支持的名称。"));
            }
            if entry.is_directory {
                visit(
                    &directory.open_directory(&entry.name).map_err(fs)?,
                    &label,
                    skip,
                    files,
                    guards,
                    total,
                )?;
            } else {
                if files.len() >= 50_000 {
                    return Err(error("开发资源文件数量超限。"));
                }
                let pin = directory.pin_file(&entry.name).map_err(fs)?;
                let mut file = pin.open_file(&entry.name).map_err(fs)?;
                let mut hasher = Sha256::new();
                let mut size = 0u64;
                let mut first = Vec::new();
                let mut buffer = [0u8; 65536];
                loop {
                    let n = file.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    size += n as u64;
                    *total += n as u64;
                    if size > 256 * 1024 * 1024 || *total > 2 * 1024 * 1024 * 1024 {
                        return Err(error("开发资源字节数量超限。"));
                    }
                    if first.len() < 4096 {
                        first.extend_from_slice(&buffer[..n.min(4096 - first.len())]);
                    }
                    hasher.update(&buffer[..n]);
                }
                let pe_machine = if label.to_ascii_lowercase().ends_with(".exe")
                    || label.to_ascii_lowercase().ends_with(".dll")
                {
                    let offset = first
                        .get(60..64)
                        .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
                        .ok_or_else(|| error("PE头缺失。"))?;
                    if first.get(..2) != Some(b"MZ")
                        || first.get(offset..offset + 4) != Some(b"PE\0\0")
                    {
                        return Err(error("PE格式无效。"));
                    }
                    let machine = u16::from_le_bytes(
                        first
                            .get(offset + 4..offset + 6)
                            .ok_or_else(|| error("PE架构缺失。"))?
                            .try_into()
                            .unwrap(),
                    );
                    if ![0x8664, 0x14c].contains(&machine) {
                        return Err(error("开发资源PE架构不支持。"));
                    }
                    Some(machine)
                } else {
                    None
                };
                let executable = label.to_ascii_lowercase().ends_with(".exe");
                files.push(ToolFile {
                    path: label,
                    size,
                    sha256: format!("{:x}", hasher.finalize()),
                    executable,
                    pe_machine,
                });
                guards.push(pin);
            }
        }
        directory.verify_binding().map_err(fs)
    }
    let mut files = Vec::new();
    let mut guards = vec![root.clone()];
    visit(root, "", skip, &mut files, &mut guards, &mut 0)?;
    files.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    Ok((files, guards))
}
impl DevelopmentTools {
    pub fn new(resources: PathBuf) -> Self {
        Self { resources }
    }
    fn release(kind: &str) -> Result<Release> {
        let bytes = match kind {
            "node" => include_str!("../../../scripts/windows-development-node-release.json"),
            "git" => include_str!("../../../scripts/windows-development-git-release.json"),
            _ => return Err(error("未知开发工具。")),
        };
        serde_json::from_str(bytes).map_err(|_| error("开发资源固定声明无效。"))
    }
    fn selection(data: &WorkspaceDirectory) -> Result<Option<Selection>> {
        let Some(root) = platform_host::windows_development::area(data, "development-tools", false)
            .map_err(fs)?
        else {
            return Ok(None);
        };
        let mut names: Vec<_> = root
            .list()
            .map_err(fs)?
            .into_iter()
            .filter(|entry| entry.name.starts_with("selection-"))
            .map(|entry| entry.name)
            .collect();
        names.sort();
        let Some(last) = names.last() else {
            return Err(error(
                "开发工具设置目录缺少已确认选择，请保留设置现场排查；没有自动切回内置工具。",
            ));
        };
        for (index, name) in names.iter().enumerate() {
            if *name != format!("selection-{:08}.json", index + 1) {
                return Err(error("开发工具设置修订不连续，请保留记录。"));
            }
        }
        let selection: Selection =
            serde_json::from_slice(&root.read_bounded(last, 8_000_000).map_err(fs)?.bytes)
                .map_err(|_| error("开发工具设置格式无效。"))?;
        if selection.schema_version != 1
            || selection.revision as usize != names.len()
            || selection.directory_identity != *root.identity()
        {
            return Err(error("开发工具设置身份或修订不匹配。"));
        }
        Ok(Some(selection))
    }
    pub fn lease(&self, data: &WorkspaceDirectory, kind: &str) -> Result<ToolLease> {
        let settings = Self::selection(data)?;
        let selected = settings.as_ref().and_then(|s| {
            if kind == "node" {
                s.node.as_ref()
            } else {
                s.git.as_ref()
            }
        });
        self.load(kind, selected)
    }
    fn load(&self, kind: &str, selected: Option<&Selected>) -> Result<ToolLease> {
        let release = Self::release(kind)?;
        if release.kind != format!("development-{kind}")
            || release.platform != "win32"
            || release.arch != "x64"
        {
            return Err(error("开发资源平台不匹配。"));
        }
        let path = selected
            .map(|s| s.root.clone())
            .unwrap_or_else(|| self.resources.join(format!("development-{kind}")));
        let root = WindowsSourceDirectory::open(&path).map_err(fs)?;
        if let Some(selection) = selected {
            if selection.identity != *root.identity() {
                return Err(error("显式选择的开发工具目录身份已变化，请重新选择。"));
            }
        }
        let skip = if selected.is_none() {
            Some(format!("development-{kind}.json"))
        } else {
            None
        };
        if let Some(name) = &skip {
            let value: Value =
                serde_json::from_slice(&root.read_bounded(name, 8_000_000).map_err(fs)?.bytes)
                    .map_err(|_| error("开发资源声明无效。"))?;
            let expected: Value = serde_json::from_str(match kind {
                "node" => include_str!("../../../scripts/windows-development-node-release.json"),
                _ => include_str!("../../../scripts/windows-development-git-release.json"),
            })?;
            if value != expected {
                return Err(error("开发资源声明与程序固定声明不一致。"));
            }
        }
        let (files, guards) = inventory(&root, skip.as_deref())?;
        let expected = selected.map(|s| &s.files).unwrap_or(&release.files);
        if &files != expected {
            return Err(error(
                "显式选择或内置开发工具的完整文件树已变化；没有回退到系统工具。",
            ));
        }
        let paths = selected.map(|s| &s.paths).unwrap_or(&release.paths);
        let mut alternative = release.paths.clone();
        if kind == "git" {
            for value in alternative.values_mut() {
                *value = value.replace("ucrt64/", "mingw64/");
            }
        }
        if paths != &release.paths && (kind != "git" || paths != &alternative) {
            return Err(error("开发工具路径布局与确认的Node/npm或Git布局不匹配。"));
        }
        let executable = paths
            .get("executable")
            .ok_or_else(|| error("开发工具入口缺失。"))?;
        child(&root, executable)?;
        if !files
            .iter()
            .any(|f| f.path == *executable && f.executable && f.pe_machine == Some(0x8664))
            || paths.values().any(|p| !valid_label(p))
        {
            return Err(error("开发工具入口或路径无效。"));
        }
        let npm_version = if kind == "node" {
            let package = paths.get("npmCli").ok_or_else(|| error("npm入口缺失。"))?;
            let package = Path::new(package)
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| error("npm入口必须位于完整npm包内。"))?
                .join("package.json");
            let (directory, name) = child(&root, &package.to_string_lossy().replace('\\', "/"))?;
            let metadata: Value =
                serde_json::from_slice(&directory.read_bounded(&name, 65536).map_err(fs)?.bytes)?;
            Some(
                metadata["version"]
                    .as_str()
                    .ok_or_else(|| error("npm版本缺失。"))?
                    .to_owned(),
            )
        } else {
            None
        };
        let get = |key: &str| -> Option<PathBuf> { paths.get(key).map(|p| root.path().join(p)) };
        for key in if kind == "git" {
            vec!["execPath", "templatePath", "binPath"]
        } else {
            Vec::new()
        } {
            let mut directory = root.clone();
            for part in paths
                .get(key)
                .ok_or_else(|| error("Git布局缺失。"))?
                .split('/')
            {
                directory = directory.open_directory(part).map_err(fs)?;
            }
            directory.verify_binding().map_err(fs)?;
        }
        if kind == "git" {
            let helper = format!("{}/git-remote-https.exe", paths["binPath"]);
            if !files
                .iter()
                .any(|f| f.path == helper && f.executable && f.pe_machine == Some(0x8664))
            {
                return Err(error("Git HTTPS helper缺失或不是Windows x64。"));
            }
        }
        let entries = expected_entries(&files);
        let lease = ToolLease {
            root: root.path().into(),
            executable: root.path().join(executable),
            npm_cli: get("npmCli"),
            exec_path: get("execPath"),
            template_path: get("templatePath"),
            certificate_bundle: get("certificateBundle"),
            version: selected
                .map(|s| s.version.clone())
                .unwrap_or(release.version),
            npm_version,
            selection: if selected.is_some() {
                "configured"
            } else {
                "managed"
            }
            .into(),
            guards,
            entries,
            skipped_metadata: skip,
        };
        lease.verify()?;
        Ok(lease)
    }
    async fn probe_one(tool: &ToolLease) -> Result<()> {
        let scratch = WindowsSourceDirectory::create_private_in(
            &std::env::temp_dir(),
            &format!("disposable-development-probe-{}", uuid::Uuid::new_v4()),
        )
        .map_err(fs)?;
        let home = scratch.create_private_directory("home").map_err(fs)?;
        let temporary = scratch.create_private_directory("temporary").map_err(fs)?;
        home.write_new("user.npmrc", b"").map_err(fs)?;
        home.write_new("global.npmrc", b"").map_err(fs)?;
        let _configuration = home
            .pin_file("user.npmrc")
            .map_err(fs)?
            .pin_file("global.npmrc")
            .map_err(fs)?;
        let environment = tool.environment(home.path(), temporary.path(), None)?;
        async fn execute(
            tool: &ToolLease,
            args: Vec<OsString>,
            environment: BTreeMap<OsString, OsString>,
        ) -> Result<String> {
            let (_send, receive) = watch::channel(false);
            let result = platform_host::windows_command::run(
                platform_host::windows_command::CommandSpec {
                    executable: tool.executable.clone(),
                    args,
                    working_directory: tool.root.clone(),
                    environment,
                    stdin: Vec::new(),
                    timeout: Duration::from_secs(10),
                    stdout_limit: 4096,
                    stderr_limit: 4096,
                },
                receive,
            )
            .await;
            let output = match result {
                Ok(output) => output,
                Err(issue) => {
                    // A failed observation may leave native Job cleanup active.
                    // The caller still owns the complete ToolLease and config.
                    platform_host::windows_command::wait_for_idle().await;
                    return Err(issue);
                }
            };
            if !output.success() {
                return Err(error("开发工具执行检查失败，未改用其他工具。"));
            }
            Ok(String::from_utf8_lossy(&output.stdout.bytes)
                .trim()
                .to_owned())
        }
        let version = execute(tool, vec!["--version".into()], environment.clone()).await?;
        let expected = if tool.npm_cli.is_some() {
            if tool.version.starts_with('v') {
                tool.version.clone()
            } else {
                format!("v{}", tool.version)
            }
        } else if tool.version.starts_with("git version ") {
            tool.version.clone()
        } else {
            format!("git version {}", tool.version)
        };
        if version != expected {
            return Err(error("开发工具执行版本与已确认版本不符。"));
        }
        if let Some(npm) = &tool.npm_cli {
            let version = execute(
                tool,
                vec![npm.as_os_str().into(), "--version".into()],
                environment.clone(),
            )
            .await?;
            if Some(version.as_str()) != tool.npm_version.as_deref() {
                return Err(error("npm执行版本与已确认版本不符。"));
            }
        }
        if let Some(exec) = &tool.exec_path {
            let actual = execute(tool, vec!["--exec-path".into()], environment).await?;
            if !actual
                .replace('\\', "/")
                .eq_ignore_ascii_case(&exec.to_string_lossy().replace('\\', "/"))
            {
                return Err(error("Git helper路径不属于所选工具树。"));
            }
        }
        tool.verify()
    }
    pub async fn doctor(&self, data: &WorkspaceDirectory) -> Value {
        let mut issues = Vec::new();
        let mut result = json!({"ready":false,"node":null,"npm":null,"git":null,"supportedNodeRange":"^22.12.0 || ^24.0.0","issues":[]});
        for kind in ["node", "git"] {
            let check = async {
                let tool = self.lease(data, kind)?;
                Self::probe_one(&tool).await?;
                Ok::<_, ApiError>(tool)
            }
            .await;
            match check {
                Ok(tool) => {
                    result[kind] = json!({"path":tool.executable,"version":tool.version,"selection":tool.selection});
                    if kind == "node" {
                        result["npm"] = json!({"path":tool.npm_cli,"version":tool.npm_version});
                    }
                }
                Err(issue) => issues.push(issue),
            }
        }
        result["ready"] = json!(issues.is_empty());
        result["issues"] = json!(issues);
        result
    }
    /// Explicit choices are snapshotted before execution. Invalid choices do not
    /// alter the saved selection. None explicitly returns that tool to managed.
    pub async fn configure(
        &self,
        data: &WorkspaceDirectory,
        kind: &str,
        executable: Option<PathBuf>,
    ) -> Result<Value> {
        if !matches!(kind, "node" | "git") {
            return Err(error("未知开发工具选择。"));
        }
        let previous = Self::selection(data)?;
        let selection_guards;
        let chosen = if let Some(path) = executable {
            if !path.is_absolute()
                || path.file_name().and_then(|s| s.to_str())
                    != Some(if kind == "node" {
                        "node.exe"
                    } else {
                        "git.exe"
                    })
            {
                return Err(error("请选择开发工具的绝对exe路径。"));
            }
            let root_path = if kind == "node" {
                path.parent().unwrap()
            } else {
                path.parent()
                    .and_then(Path::parent)
                    .ok_or_else(|| error("请选择Git for Windows的cmd/git.exe。"))?
            };
            let root = WindowsSourceDirectory::open(root_path).map_err(fs)?;
            let release = Self::release(kind)?;
            let mut paths = release.paths;
            if kind == "git" && root.list().map_err(fs)?.iter().any(|e| e.name == "mingw64") {
                for value in paths.values_mut() {
                    *value = value.replace("ucrt64/", "mingw64/");
                }
            }
            let (files, guards) = inventory(&root, None)?;
            for key in if kind == "node" {
                vec!["executable", "npmCli"]
            } else {
                vec!["executable", "execPath", "certificateBundle"]
            } {
                let value = paths
                    .get(key)
                    .ok_or_else(|| error("所选工具布局不完整。"))?;
                if key != "execPath" && !files.iter().any(|f| f.path == *value) {
                    return Err(error("所选工具缺少必须文件。"));
                }
            }
            let expected_path = root
                .path()
                .join(paths.get("executable").ok_or_else(|| error("入口缺失。"))?);
            if !expected_path
                .to_string_lossy()
                .eq_ignore_ascii_case(&path.to_string_lossy())
                || !files.iter().any(|f| {
                    f.path == paths["executable"] && f.executable && f.pe_machine == Some(0x8664)
                })
            {
                return Err(error("所选入口必须是该工具树中的Windows x64主程序。"));
            }
            let mut selected = Selected {
                root: root.path().into(),
                identity: root.identity().clone(),
                paths,
                files,
                version: String::new(),
                npm_version: None,
            };
            // A user-selected executable is only probed after the complete tree
            // is read and pinned. It is never discovered through PATH.
            let temporary = root.path();
            let windows = platform_host::windows_command::windows_directory()?;
            let environment = BTreeMap::from([("SYSTEMROOT".into(), windows.into_os_string())]);
            let (_send, receive) = watch::channel(false);
            let result = platform_host::windows_command::run(
                platform_host::windows_command::CommandSpec {
                    executable: path,
                    args: vec!["--version".into()],
                    working_directory: temporary.into(),
                    environment,
                    stdin: Vec::new(),
                    timeout: Duration::from_secs(10),
                    stdout_limit: 4096,
                    stderr_limit: 4096,
                },
                receive,
            )
            .await;
            let output = match result {
                Ok(output) => output,
                Err(issue) => {
                    // `guards` pin the complete selected tree until even an
                    // uncertain process exit has finished its native cleanup.
                    platform_host::windows_command::wait_for_idle().await;
                    return Err(issue);
                }
            };
            if !output.success() {
                return Err(error("所选开发工具版本检查未成功。"));
            }
            let version = String::from_utf8_lossy(&output.stdout.bytes)
                .trim()
                .to_owned();
            if kind == "node" && !supported_node(&version) {
                return Err(error("所选Node必须满足22.12+或24.x。"));
            }
            if kind == "git" && !version.starts_with("git version ") {
                return Err(error("所选Git版本输出无效。"));
            }
            selected.version = version;
            if kind == "node" {
                let npm = selected.paths.get("npmCli").unwrap();
                let package = Path::new(npm)
                    .parent()
                    .and_then(Path::parent)
                    .unwrap()
                    .join("package.json");
                let (directory, name) =
                    child(&root, &package.to_string_lossy().replace('\\', "/"))?;
                let metadata: Value = serde_json::from_slice(
                    &directory.read_bounded(&name, 65536).map_err(fs)?.bytes,
                )?;
                selected.npm_version = metadata["version"].as_str().map(str::to_owned);
                if selected.npm_version.is_none() {
                    return Err(error("npm版本信息缺失。"));
                }
            }
            let verified = self.load(kind, Some(&selected))?;
            Self::probe_one(&verified).await?;
            for guard in &guards {
                guard.verify_binding().map_err(fs)?;
            }
            selection_guards = guards;
            drop(verified);
            Some(selected)
        } else {
            let verified = self.load(kind, None)?;
            Self::probe_one(&verified).await?;
            selection_guards = verified.guards;
            None
        };
        let root = platform_host::windows_development::area(data, "development-tools", true)
            .map_err(fs)?
            .unwrap();
        let mut selection = previous.unwrap_or(Selection {
            schema_version: 1,
            revision: 0,
            directory_identity: root.identity().clone(),
            node: None,
            git: None,
        });
        selection.revision += 1;
        if kind == "node" {
            selection.node = chosen;
        } else {
            selection.git = chosen;
        }
        root.write_new(
            &format!("selection-{:08}.json", selection.revision),
            &serde_json::to_vec(&selection)?,
        )
        .map_err(|e| fs(e).outcome("unknown"))?;
        for guard in &selection_guards {
            guard
                .verify_binding()
                .map_err(|e| fs(e).outcome("unknown"))?;
        }
        Ok(self.doctor(data).await)
    }
}
fn supported_node(value: &str) -> bool {
    let parts: Vec<_> = value.trim_start_matches('v').split('.').collect();
    parts.len() == 3
        && ((parts[0] == "22" && parts[1].parse::<u32>().is_ok_and(|v| v >= 12))
            || parts[0] == "24")
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_node_range_is_explicit() {
        for v in ["v22.12.0", "v22.99.1", "v24.22.1"] {
            assert!(supported_node(v));
        }
        for v in [
            "v22.11.9",
            "v23.1.0",
            "v25.0.0",
            "v24.0.x",
            "v24.0.0;whoami",
        ] {
            assert!(!supported_node(v));
        }
    }
    #[test]
    fn resource_names_reject_windows_aliases_and_traversal() {
        for s in [
            "../node.exe",
            "x\\node.exe",
            "node.exe:ads",
            "x/./a",
            "x//a",
            "x./a",
            "x /a",
        ] {
            assert!(!valid_label(s));
        }
        assert!(valid_label("node_modules/@npmcli/config/lib/index.js"));
    }
}

#[cfg(test)]
#[path = "development_tools_tests.rs"]
mod development_tools_tests;
