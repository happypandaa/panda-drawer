//! Versioned, read-only author declarations shared by every Panda application.
//! Local instance identity, saved configuration and runtime state are not fields.
use crate::{app_windows::AppWindowConfig, runtime_policy::RuntimePolicy};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
};

mod configuration;
mod dependency_hint;
#[cfg(unix)]
#[path = "panda_manifest/unix_files.rs"]
mod file_io;
#[cfg(windows)]
#[path = "panda_manifest/windows_files.rs"]
mod file_io;
mod paths;
mod schema;
pub use configuration::{
    valid_business_variable, ConfigurationDecl, ConfigurationField, ConfigurationType, SelectOption,
};
pub use schema::export_schema;

pub const MANIFEST_FILE: &str = "panda.json";
pub const MANIFEST_VERSION: u32 = 1;
pub const NODE_MAJOR: u32 = 24;
pub const BACKEND_PROTOCOL_VERSION: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const BACKEND_API_PREFIX_PATTERN: &str = r"^/(?!_platform(?:/|$))(?!\.{1,2}(?:/|$))[A-Za-z0-9._~!$&'()*+,;=@-]+(?:/(?!\.{1,2}(?:/|$))[A-Za-z0-9._~!$&'()*+,;=@-]+)*$";
const AGENT_INSTRUCTIONS_PATTERN: &str =
    r"^(?!\s*$)[^./\\:\u0000][^/\\:\u0000]*(?:/[^./\\:\u0000][^/\\:\u0000]*)*$";
pub type ManifestResult<T> = Result<T, PandaManifestError>;

#[derive(Debug)]
pub struct PandaManifestError {
    field: String,
    reason: String,
}
impl fmt::Display for PandaManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}：{}", self.field, self.reason)
    }
}
impl std::error::Error for PandaManifestError {}
pub(crate) fn invalid(field: &str, reason: &str) -> PandaManifestError {
    PandaManifestError {
        field: field.into(),
        reason: reason.into(),
    }
}
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}
fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.contains('\0')
}
fn https(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    })
}
/// `.` is meaningful for a root directory, never for an entry file.
fn relative(value: &str) -> bool {
    text(value, 1024)
        && value.split('/').any(|part| part != ".")
        && !value.contains(['\\', ':'])
        && !value.split('/').any(|part| part.is_empty() || part == "..")
}
fn relative_root(value: &str) -> bool {
    value == "." || relative(value)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PandaRuntime {
    Web,
    Node,
}
impl PandaRuntime {
    pub fn label(self) -> &'static str {
        match self {
            Self::Web => "网页项目",
            Self::Node => "Node 服务项目",
        }
    }
}
struct RuntimeRule {
    runtime: PandaRuntime,
    forbidden: &'static [&'static str],
    required: &'static [&'static str],
    allow_secrets: bool,
}
const RUNTIME_RULES: [RuntimeRule; 2] = [
    RuntimeRule {
        runtime: PandaRuntime::Web,
        forbidden: &["backend"],
        required: &[],
        allow_secrets: false,
    },
    RuntimeRule {
        runtime: PandaRuntime::Node,
        forbidden: &["platform", "development", "runtimeRequirements"],
        required: &["backend"],
        allow_secrets: true,
    },
];

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PandaManifest {
    #[serde(skip)]
    pub(crate) source_digest: String,
    #[serde(
        rename = "$schema",
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub schema: Option<String>,
    pub manifest_version: u32,
    pub runtime: PandaRuntime,
    pub app: AppDecl,
    pub web: WebDecl,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub backend: Option<BackendDecl>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub platform: Option<PlatformDecl>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub development: Option<DevelopmentDecl>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub configuration: Option<ConfigurationDecl>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub documentation: Option<DocumentationDecl>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub agent: Option<AgentDecl>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub window: Option<AppWindowConfig>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub runtime_requirements: Option<RuntimePolicy>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppDecl {
    pub name: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub version: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub icon: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentDecl {
    pub instructions: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebDecl {
    pub root: String,
    pub entry: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendDecl {
    pub root: String,
    pub entry: String,
    pub node_major: u32,
    pub protocol_version: u32,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub api_prefixes: Option<Vec<String>>,
}
impl BackendDecl {
    pub fn effective_api_prefixes(&self) -> Vec<String> {
        self.api_prefixes
            .clone()
            .unwrap_or_else(default_backend_api_prefixes)
    }
}
pub(crate) fn default_backend_api_prefixes() -> Vec<String> {
    vec!["/api".into(), "/tts".into()]
}
pub(crate) fn validate_backend_api_prefixes(prefixes: &[String]) -> ManifestResult<()> {
    let mut seen = std::collections::BTreeSet::new();
    for prefix in prefixes {
        let valid = prefix.strip_prefix('/').is_some_and(|path| {
            let mut parts = path.split('/');
            parts.clone().next() != Some("_platform")
                && parts.all(|part| {
                    !part.is_empty()
                        && part != "."
                        && part != ".."
                        && part.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || b"._~!$&'()*+,;=@-".contains(&byte)
                        })
                })
        });
        if !valid {
            return Err(invalid(
                "backend.apiPrefixes",
                "须为以 / 开头的完整路径前缀；禁止根路径、平台保留路径、空段、点路径、编码或查询参数",
            ));
        }
        if !seen.insert(prefix) {
            return Err(invalid("backend.apiPrefixes", "路径前缀不能重复"));
        }
    }
    Ok(())
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformDecl {
    pub sdk_api_version: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DevelopmentDecl {
    #[serde(default = "default_source_dir")]
    pub source_dir: String,
    pub build: BuildDecl,
}
fn default_source_dir() -> String {
    ".".into()
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum BuildDecl {
    Npm { script: String },
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentationDecl {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub readme: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub development: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub database: Option<String>,
}

/// Shared declared-path resolver; it never reads entry file contents.
pub fn checked_project_child(
    root: &Path,
    relative_path: &str,
    directory: bool,
) -> ManifestResult<PathBuf> {
    file_io::checked_child(root, relative_path, directory)
}
fn web_path_allowed(path: &Path, project_relative: bool) -> bool {
    let project_data = project_relative
        && path.components().find_map(|part| match part {
            std::path::Component::Normal(name) => Some(name.eq_ignore_ascii_case("data")),
            _ => None,
        }) == Some(true);
    !project_data
        && !path.components().any(|part| match part {
            std::path::Component::Normal(name) => name.to_str().is_none_or(|name| {
                name.starts_with('.')
                    || ["node_modules", MANIFEST_FILE]
                        .iter()
                        .any(|protected| name.eq_ignore_ascii_case(protected))
            }),
            std::path::Component::CurDir => false,
            _ => true,
        })
}
pub(crate) fn ensure_web_path(app_dir: &Path, path: &Path, field: &str) -> ManifestResult<()> {
    let root = app_dir
        .canonicalize()
        .map_err(|_| invalid(field, "项目目录不可读取"))?;
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| invalid(field, "网页路径越界"))?;
    if !web_path_allowed(relative, true) {
        return Err(invalid(field, "网页路径指向受保护的项目内容"));
    }
    Ok(())
}

impl PandaManifest {
    pub fn parse(bytes: &[u8]) -> ManifestResult<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(invalid(MANIFEST_FILE, "文件超过 64 KiB 读取上限"));
        }
        let mut deserializer = serde_json::Deserializer::from_slice(bytes);
        let mut result: Self =
            serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
                let context = error.path().to_string();
                let message = error.inner().to_string();
                for (prefix, reason) in [
                    ("unknown field `", "未知字段"),
                    ("duplicate field `", "重复字段"),
                    ("missing field `", "缺少必填字段"),
                ] {
                    if let Some(rest) = message.strip_prefix(prefix) {
                        let field = rest.split('`').next().unwrap_or("");
                        let path: String = if context.is_empty() || context == "." {
                            field.into()
                        } else if prefix == "unknown field `"
                            && matches!(error.path().iter().next_back(),
                                Some(serde_path_to_error::Segment::Map { key }) if key == field)
                        {
                            // Unknown keys fail while the key is deserialized,
                            // so its segment is already included in the path.
                            context.clone()
                        } else {
                            format!("{context}.{field}")
                        };
                        return invalid(&path, reason);
                    }
                }
                invalid(
                    if context.is_empty() || context == "." {
                        MANIFEST_FILE
                    } else {
                        &context
                    },
                    "JSON 格式、字段类型或取值无效",
                )
            })?;
        deserializer
            .end()
            .map_err(|_| invalid(MANIFEST_FILE, "JSON 对象后含有多余内容"))?;
        result.source_digest = format!("{:x}", Sha256::digest(bytes));
        result.validate()?;
        Ok(result)
    }
    /// Internal callers use exactly the same declaration reader at startup.
    pub(crate) fn parse_runtime(bytes: &[u8]) -> ManifestResult<Self> {
        Self::parse(bytes)
    }
    pub fn load(app_dir: &Path) -> ManifestResult<Self> {
        Self::parse(&Self::read_bytes(app_dir)?)
    }
    pub(crate) fn read_bytes(app_dir: &Path) -> ManifestResult<Vec<u8>> {
        file_io::read_bytes(app_dir)
    }
    pub fn validate(&self) -> ManifestResult<()> {
        if self.manifest_version != MANIFEST_VERSION {
            return Err(invalid("manifestVersion", "仅支持版本 1"));
        }
        if !text(&self.app.name, 240) {
            return Err(invalid(
                "app.name",
                "应用名称不能为空、含 NUL 或超过读取上限",
            ));
        }
        for (field, value, limit) in [
            ("app.version", self.app.version.as_ref(), 80),
            ("app.description", self.app.description.as_ref(), 4000),
        ] {
            if value.is_some_and(|v| !text(v, limit)) {
                return Err(invalid(field, "文本不能为空、含 NUL 或超过读取上限"));
            }
        }
        if !relative_root(&self.web.root) {
            return Err(invalid("web.root", "须为项目内相对目录或 ."));
        }
        if !relative(&self.web.entry) {
            return Err(invalid("web.entry", "须为相对网页目录的文件路径"));
        }
        let value =
            serde_json::to_value(self).map_err(|_| invalid(MANIFEST_FILE, "声明无法读取"))?;
        let rule = RUNTIME_RULES
            .iter()
            .find(|rule| rule.runtime == self.runtime)
            .expect("all runtime variants have rules");
        for field in rule.forbidden {
            if value.get(field).is_some() {
                return Err(invalid(field, "当前应用类型禁止此字段"));
            }
        }
        for field in rule.required {
            if value.get(field).is_none() {
                return Err(invalid(field, "当前应用类型缺少必填字段"));
            }
        }
        if let Some(backend) = &self.backend {
            if backend.node_major != NODE_MAJOR {
                return Err(invalid("backend.nodeMajor", "仅支持 Node 24"));
            }
            if backend.protocol_version != BACKEND_PROTOCOL_VERSION {
                return Err(invalid("backend.protocolVersion", "仅支持控制协议 1"));
            }
            if !relative_root(&backend.root) {
                return Err(invalid("backend.root", "须为项目内相对目录或 ."));
            }
            if !relative(&backend.entry) {
                return Err(invalid("backend.entry", "须为相对后端目录的文件路径"));
            }
            if let Some(prefixes) = &backend.api_prefixes {
                validate_backend_api_prefixes(prefixes)?;
            }
        }
        if let Some(platform) = &self.platform {
            if platform.sdk_api_version != crate::API_VERSION {
                return Err(invalid("platform.sdkApiVersion", "SDK API 版本不受支持"));
            }
        }
        if let Some(development) = &self.development {
            if !relative_root(&development.source_dir) {
                return Err(invalid("development.sourceDir", "须为项目内相对目录或 ."));
            }
            let BuildDecl::Npm { script } = &development.build;
            if script.is_empty()
                || script.len() > 100
                || script.starts_with('-')
                || !script
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_:-".contains(&b))
            {
                return Err(invalid(
                    "development.build.script",
                    "须为有效的 npm 脚本名称",
                ));
            }
        }
        if let Some(config) = &self.configuration {
            config.validate()?;
            if !rule.allow_secrets && config.fields.iter().any(|field| field.secret) {
                return Err(invalid(
                    "configuration.fields.secret",
                    "网页配置不允许秘密值",
                ));
            }
        }
        if let Some(docs) = &self.documentation {
            if self.runtime == PandaRuntime::Node && docs.database.is_some() {
                return Err(invalid("documentation.database", "Node 应用禁止此字段"));
            }
            for (field, path) in [
                ("documentation.readme", &docs.readme),
                ("documentation.development", &docs.development),
                ("documentation.database", &docs.database),
            ] {
                if path.as_ref().is_some_and(|path| !relative(path)) {
                    return Err(invalid(field, "须为项目内相对文件路径"));
                }
            }
        }
        if let Some(agent) = &self.agent {
            if !relative(&agent.instructions)
                || agent
                    .instructions
                    .split('/')
                    .any(|part| part.starts_with('.'))
            {
                return Err(invalid(
                    "agent.instructions",
                    "须为项目内非隐藏普通文件的相对路径",
                ));
            }
        }
        if self.window.is_some_and(|window| !window.valid()) {
            return Err(invalid("window", "窗口尺寸无效"));
        }
        if let Some(requirements) = &self.runtime_requirements {
            RuntimePolicy::parse(&json!(requirements))
                .map_err(|_| invalid("runtimeRequirements", "网络或设备声明无效"))?;
        }
        Ok(())
    }
    /// Optional display metadata never turns a valid application into a failure.
    pub fn icon_data_url(&self, app_dir: &Path) -> Option<String> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let relative = self.app.icon.as_deref()?;
        if !Path::new(relative).extension()?.eq_ignore_ascii_case("png") {
            return None;
        }
        let path = checked_project_child(app_dir, relative, false).ok()?;
        let bytes = crate::independent_file_io::read_file(&path, crate::png_icon::ICON_MAX).ok()?;
        crate::png_icon::decode_png(&bytes).ok()?;
        Some(format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
    }
    pub fn resolve_backend(&self, app_dir: &Path) -> ManifestResult<(PathBuf, PathBuf)> {
        let backend = self
            .backend
            .as_ref()
            .ok_or_else(|| invalid("backend", "网页项目不启动 Node 后端"))?;
        let cwd = file_io::checked_child(app_dir, &backend.root, true)?;
        let entry = file_io::checked_child(&cwd, &backend.entry, false)?;
        Ok((entry, cwd))
    }
    fn web_root_if_present(&self, app_dir: &Path) -> ManifestResult<Option<PathBuf>> {
        if !web_path_allowed(Path::new(&self.web.root), true) {
            return Err(invalid("web.root", "网页目录不能位于受保护的项目内容中"));
        }
        let root = paths::resolve(app_dir, &self.web.root, true, true)
            .map_err(|_| invalid("web.root", "目录不存在、越界或无法安全读取"))?;
        if let Some(root) = &root {
            ensure_web_path(app_dir, root, "web.root")?;
        }
        Ok(root)
    }
    pub fn resolve_web_root(&self, app_dir: &Path) -> ManifestResult<PathBuf> {
        self.web_root_if_present(app_dir)?
            .ok_or_else(|| invalid("web.root", "网页目录不存在"))
    }
    fn web_entry_if_present(&self, app_dir: &Path, root: &Path) -> ManifestResult<Option<PathBuf>> {
        if !web_path_allowed(Path::new(&self.web.entry), false) {
            return Err(invalid("web.entry", "网页入口不能位于受保护的项目内容中"));
        }
        let entry = paths::resolve(root, &self.web.entry, false, true)
            .map_err(|_| invalid("web.entry", "文件越界或无法安全读取"))?;
        if let Some(entry) = &entry {
            ensure_web_path(app_dir, entry, "web.entry")?;
        }
        Ok(entry)
    }
    pub fn resolve_web_entry(&self, app_dir: &Path) -> ManifestResult<PathBuf> {
        let root = self.resolve_web_root(app_dir)?;
        self.web_entry_if_present(app_dir, &root)?
            .ok_or_else(|| invalid("web.entry", "网页入口不存在"))
    }
    pub fn inspect(&self, app_dir: &Path) -> ManifestResult<Value> {
        self.inspect_with_policy(app_dir, false)
    }
    /// Local web registration accepts an unfinished output directory; no code
    /// is executed and a missing page remains an explicit application state.
    pub fn inspect_local(&self, app_dir: &Path) -> ManifestResult<Value> {
        self.inspect_with_policy(app_dir, true)
    }
    fn inspect_with_policy(
        &self,
        app_dir: &Path,
        allow_missing_web: bool,
    ) -> ManifestResult<Value> {
        self.validate()?;
        let web_root = self.web_root_if_present(app_dir)?;
        let web_entry = match web_root {
            Some(root) => self.web_entry_if_present(app_dir, &root)?,
            None => None,
        };
        let missing_web = web_entry.is_none();
        if missing_web
            && (self.runtime == PandaRuntime::Node
                || (!allow_missing_web && self.development.is_none()))
        {
            return Err(invalid(
                "web.entry",
                &format!("文件 {}/{} 不存在", self.web.root, self.web.entry),
            ));
        }
        let mut checked = Vec::new();
        if let Some(agent) = &self.agent {
            let path =
                file_io::checked_child(app_dir, &agent.instructions, false).map_err(|_| {
                    invalid(
                        "agent.instructions",
                        &format!(
                            "规则文件 {} 不存在或无法安全读取，请补上项目内的非隐藏普通文件",
                            agent.instructions
                        ),
                    )
                })?;
            let root = app_dir
                .canonicalize()
                .map_err(|_| invalid("agent.instructions", "项目目录无法安全读取"))?;
            let visible = path.strip_prefix(root).is_ok_and(|relative| {
                relative.components().all(|component| match component {
                    std::path::Component::Normal(name) => {
                        name.to_str().is_some_and(|name| !name.starts_with('.'))
                    }
                    _ => false,
                })
            });
            if !visible {
                return Err(invalid(
                    "agent.instructions",
                    "规则文件须位于项目内，且不能是隐藏文件或位于隐藏目录",
                ));
            }
            checked.push(json!({"role":"agent-instructions","path":agent.instructions}));
        }
        if !missing_web {
            checked.push(
                json!({"role":"web-entry","path":format!("{}/{}",self.web.root,self.web.entry)}),
            );
        }
        let mut warnings = Vec::new();
        if self.backend.is_some() {
            let (entry, _) = self
                .resolve_backend(app_dir)
                .map_err(|_| invalid("backend.entry", "后端入口不存在或无法安全读取"))?;
            warnings = dependency_hint::warnings(app_dir, &entry);
            checked.push(json!({"role":"backend-entry","path":entry.strip_prefix(app_dir).unwrap_or(&entry)}));
        }
        if let Some(docs) = &self.documentation {
            for (field, role, path) in [
                ("documentation.readme", "user-documentation", &docs.readme),
                (
                    "documentation.development",
                    "development-documentation",
                    &docs.development,
                ),
                (
                    "documentation.database",
                    "database-documentation",
                    &docs.database,
                ),
            ] {
                if let Some(path) = path {
                    file_io::checked_child(app_dir, path, false)
                        .map_err(|_| invalid(field, "文档文件不存在或无法安全读取"))?;
                    checked.push(json!({"role":role,"path":path}));
                }
            }
        }
        let mut fields = Vec::new();
        let mut missing_required = Vec::new();
        if let Some(config) = &self.configuration {
            for field in &config.fields {
                let mut preview = json!(field);
                if field.secret {
                    preview
                        .as_object_mut()
                        .expect("field object")
                        .remove("default");
                }
                let status = if field.default.is_some() {
                    "author-default"
                } else if field.required {
                    missing_required.push(field.name.clone());
                    "missing-required"
                } else {
                    "not-set"
                };
                preview["valueStatus"] = json!(status);
                fields.push(preview);
            }
        }
        Ok(
            json!({"inspectionKind":"read-only","runtime":self.runtime,"runtimeLabel":self.runtime.label(),"app":self.app,"checkedFiles":checked,"documentationDeclared":self.documentation.is_some(),"dependencyWarnings":warnings,"needsBuild":missing_web && self.development.is_some(),"entryMissing":missing_web,"configuration":{"declarationVersion":self.configuration.as_ref().map(|config|config.version),"fields":fields,"missingRequired":missing_required,"valueSource":"author-defaults-only-no-user-values-read","status":if missing_required.is_empty(){"no-missing-required-values-in-defaults"}else{"needs-configuration"}}}),
        )
    }
    pub fn business_defaults(&self) -> ManifestResult<BTreeMap<String, String>> {
        let mut result = BTreeMap::new();
        if let Some(config) = &self.configuration {
            for field in &config.fields {
                match &field.default {
                    Some(value) if !field.secret => {
                        let value = match value {
                            Value::String(value) => value.clone(),
                            Value::Number(value) => value.to_string(),
                            Value::Bool(value) => value.to_string(),
                            _ => return Err(invalid("configuration.default", "默认值类型无效")),
                        };
                        result.insert(field.name.clone(), value);
                    }
                    None if field.required => {
                        return Err(invalid(
                            "configuration",
                            "缺少必填配置，不能从宿主环境借用凭据",
                        ))
                    }
                    _ => {}
                }
            }
        }
        Ok(result)
    }
}
