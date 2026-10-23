//! Experimental, read-only declarations for disposable Node runtime trials.
//! This module does not register projects or extend the existing v1 installer.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_MANIFEST_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub struct TrialManifestError(&'static str);

impl fmt::Display for TrialManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for TrialManifestError {}

type Result<T> = std::result::Result<T, TrialManifestError>;

fn invalid(message: &'static str) -> TrialManifestError {
    TrialManifestError(message)
}

// An absent optional field differs from an explicitly invalid/null field.
fn present<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TrialRuntime {
    StaticWeb,
    NodeService,
}

impl TrialRuntime {
    pub fn label(self) -> &'static str {
        match self {
            Self::StaticWeb => "网页项目",
            Self::NodeService => "Node 服务项目",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrialManifest {
    pub package_format: String,
    pub format_version: u32,
    pub runtime: TrialRuntime,
    pub app: AppDecl,
    pub web: WebDecl,
    #[serde(default, deserialize_with = "present")]
    pub window: Option<crate::app_windows::AppWindowConfig>,
    #[serde(default, deserialize_with = "present")]
    pub backend: Option<BackendDecl>,
    #[serde(default, deserialize_with = "present")]
    pub configuration: Option<ConfigurationDecl>,
    #[serde(default, deserialize_with = "present")]
    pub data_contract: Option<DataContractDecl>,
    #[serde(default, deserialize_with = "present")]
    pub documentation: Option<DocumentationDecl>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppDecl {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default, deserialize_with = "present")]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebDecl {
    pub root: String,
    pub entry: String,
    pub routing: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendDecl {
    pub protocol_version: u32,
    pub root: String,
    pub entry: String,
    pub node_major: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentationDecl {
    pub readme: String,
    pub development: String,
}

/// Author-declared compatibility of the application's APP_DATA_DIR contents.
/// Matching declarations are not a platform inspection of business data.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DataContractDecl {
    pub id: String,
    pub version: u32,
}

impl DataContractDecl {
    fn validate(&self) -> Result<()> {
        let bytes = self.id.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 128
            || !bytes[0].is_ascii_alphanumeric()
            || !bytes
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            || !(1..=i32::MAX as u32).contains(&self.version)
        {
            return Err(invalid("应用数据契约标识或版本无效。"));
        }
        Ok(())
    }

    pub(crate) fn from_manifest(manifest: &Value) -> Result<Option<Self>> {
        manifest
            .get("dataContract")
            .map(|value| {
                let declaration: Self = serde_json::from_value(value.clone())
                    .map_err(|_| invalid("应用数据契约声明无效。"))?;
                declaration.validate()?;
                Ok(declaration)
            })
            .transpose()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationDecl {
    pub version: u32,
    pub fields: Vec<ConfigurationField>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConfigurationType {
    String,
    Url,
    Integer,
    Boolean,
    Select,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationField {
    pub name: String,
    pub label: String,
    pub r#type: ConfigurationType,
    pub required: bool,
    pub secret: bool,
    pub description: String,
    pub how_to_get: Vec<String>,
    #[serde(default, deserialize_with = "present")]
    pub placeholder: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub default: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    pub endpoint_field: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub help_url: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub options: Option<Vec<SelectOption>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

/// A declaration preview, never a saved configuration or an installation plan.
/// Only explicitly declared metadata and ordinary author defaults are exposed.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialManifestInspection<'a> {
    inspection_kind: &'static str,
    runtime: TrialRuntime,
    runtime_label: &'static str,
    app: InspectedApp<'a>,
    checked_files: Vec<InspectedFile>,
    documentation_declared: bool,
    configuration: InspectedConfiguration<'a>,
    data_contract: Option<&'a DataContractDecl>,
    limitations: [&'static str; 3],
}

#[derive(Debug, Serialize)]
struct InspectedApp<'a> {
    id: &'a str,
    name: &'a str,
    version: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct InspectedFile {
    role: &'static str,
    path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InspectedConfiguration<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    declaration_version: Option<u32>,
    value_source: &'static str,
    status: &'static str,
    missing_required: Vec<&'a str>,
    fields: Vec<InspectedConfigurationField<'a>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InspectedConfigurationField<'a> {
    name: &'a str,
    label: &'a str,
    r#type: ConfigurationType,
    required: bool,
    secret: bool,
    description: &'a str,
    how_to_get: &'a [String],
    value_status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    placeholder: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    endpoint_field: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    help_url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Vec<InspectedSelectOption<'a>>>,
}

#[derive(Debug, Serialize)]
struct InspectedSelectOption<'a> {
    value: &'a str,
    label: &'a str,
}

fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.contains('\0')
}

pub fn valid_business_variable(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 96
        || !bytes[0].is_ascii_uppercase()
        || !bytes
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
    {
        return false;
    }
    if ["APP_", "PLATFORM_", "NODE_", "LD_", "DYLD_", "VITE_", "LC_"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return false;
    }
    ![
        "PATH",
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "TMP",
        "TEMP",
        "TMPDIR",
        "HOST",
        "PORT",
        "LANG",
        "USER",
        "LOGNAME",
        "SHELL",
        "COMSPEC",
        "SYSTEMROOT",
        "WINDIR",
        "BASH_ENV",
        "ENV",
        "PYTHONPATH",
        "PYTHONHOME",
    ]
    .contains(&name)
}

fn relative(value: &str) -> bool {
    text(value, 1024)
        && !value.contains(['\\', ':'])
        && !value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
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

impl TrialManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(invalid("应用声明超过实验读取上限。"));
        }
        // Deserialize typed structures directly: duplicate/unknown fields fail,
        // and diagnostics deliberately omit untrusted configuration contents.
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|_| invalid("应用声明不是受支持的严格实验格式。"))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn load(app_dir: &Path) -> Result<Self> {
        if !app_dir.is_absolute() {
            return Err(invalid("应用目录必须为绝对路径。"));
        }
        let path = checked_child(app_dir, "local-app.json", false)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(path)
            .map_err(|_| invalid("无法读取应用声明。"))?;
        let mut bytes = Vec::new();
        file.take((MAX_MANIFEST_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid("无法读取应用声明。"))?;
        Self::parse(&bytes)
    }

    fn validate(&self) -> Result<()> {
        if self.window.is_some_and(|window| !window.valid()) {
            return Err(invalid("应用窗口尺寸声明无效。"));
        }
        if self.package_format != "local-web-app"
            || !text(&self.app.id, 160)
            || !text(&self.app.name, 240)
            || !text(&self.app.version, 80)
            || self
                .app
                .description
                .as_ref()
                .is_some_and(|v| !text(v, 4000))
            || !relative(&self.web.root)
            || !relative(&self.web.entry)
            || self.web.routing != "files"
        {
            return Err(invalid("应用基础声明或网页入口无效。"));
        }
        match self.runtime {
            TrialRuntime::StaticWeb => {
                if self.format_version != 1
                    || self.backend.is_some()
                    || self.configuration.is_some()
                    || self.data_contract.is_some()
                {
                    return Err(invalid(
                        "网页项目不能声明 Node 后端、启动配置或 Node 数据契约。",
                    ));
                }
            }
            TrialRuntime::NodeService => {
                let backend = self
                    .backend
                    .as_ref()
                    .ok_or_else(|| invalid("Node 服务缺少入口声明。"))?;
                if self.format_version != 2
                    || backend.protocol_version != 1
                    || backend.node_major != 24
                    || !relative(&backend.root)
                    || !relative(&backend.entry)
                {
                    return Err(invalid("Node 服务版本或入口不受当前实验支持。"));
                }
            }
        }
        if let Some(docs) = &self.documentation {
            if !relative(&docs.readme)
                || !relative(&docs.development)
                || docs.readme == docs.development
            {
                return Err(invalid("应用使用和开发说明必须为独立的相对路径。"));
            }
        }
        if let Some(config) = &self.configuration {
            config.validate()?;
        }
        if let Some(data_contract) = &self.data_contract {
            data_contract.validate()?;
        }
        Ok(())
    }

    /// Resolves only this experiment's entry; does not certify a distributable package.
    pub fn resolve_backend(&self, app_dir: &Path) -> Result<(PathBuf, PathBuf)> {
        let backend = self
            .backend
            .as_ref()
            .ok_or_else(|| invalid("网页项目不启动 Node 后端。"))?;
        let cwd = checked_child(app_dir, &backend.root, true)?;
        let entry = checked_child(&cwd, &backend.entry, false)?;
        Ok((entry, cwd))
    }

    /// Checks only declared ordinary entry/document files and previews metadata.
    /// This never loads application code, dependencies, user values or credentials.
    pub fn inspect(&self, app_dir: &Path) -> Result<TrialManifestInspection<'_>> {
        self.validate()?;
        let web_root = checked_child(app_dir, &self.web.root, true)?;
        checked_child(&web_root, &self.web.entry, false)?;
        let mut checked_files = vec![InspectedFile {
            role: "web-entry",
            path: format!("{}/{}", self.web.root, self.web.entry),
        }];
        if let Some(backend) = &self.backend {
            self.resolve_backend(app_dir)?;
            checked_files.push(InspectedFile {
                role: "backend-entry",
                path: format!("{}/{}", backend.root, backend.entry),
            });
        }
        if let Some(documentation) = &self.documentation {
            for (role, path) in [
                ("user-documentation", &documentation.readme),
                ("development-documentation", &documentation.development),
            ] {
                checked_child(app_dir, path, false)?;
                checked_files.push(InspectedFile {
                    role,
                    path: path.clone(),
                });
            }
        }

        let mut fields = Vec::new();
        let mut missing_required = Vec::new();
        if let Some(configuration) = &self.configuration {
            for field in &configuration.fields {
                // validate() already rejects secret defaults. Keep this explicit
                // allow-list so a future declaration change cannot expose one.
                let default = if field.secret {
                    None
                } else {
                    field.default.as_ref()
                };
                let value_status = if default.is_some() {
                    "author-default"
                } else if field.required {
                    missing_required.push(field.name.as_str());
                    "missing-required"
                } else {
                    "not-set"
                };
                fields.push(InspectedConfigurationField {
                    name: &field.name,
                    label: &field.label,
                    r#type: field.r#type,
                    required: field.required,
                    secret: field.secret,
                    description: &field.description,
                    how_to_get: &field.how_to_get,
                    value_status,
                    placeholder: field.placeholder.as_deref(),
                    default,
                    endpoint_field: field.endpoint_field.as_deref(),
                    help_url: field.help_url.as_deref(),
                    options: field.options.as_ref().map(|options| {
                        options
                            .iter()
                            .map(|option| InspectedSelectOption {
                                value: &option.value,
                                label: &option.label,
                            })
                            .collect()
                    }),
                });
            }
        }
        let status = if missing_required.is_empty() {
            "no-missing-required-values-in-defaults"
        } else {
            "needs-configuration"
        };
        Ok(TrialManifestInspection {
            inspection_kind: "read-only-trial",
            runtime: self.runtime,
            runtime_label: self.runtime.label(),
            app: InspectedApp {
                id: &self.app.id,
                name: &self.app.name,
                version: &self.app.version,
                description: self.app.description.as_deref(),
            },
            checked_files,
            documentation_declared: self.documentation.is_some(),
            data_contract: self.data_contract.as_ref(),
            configuration: InspectedConfiguration {
                declaration_version: self.configuration.as_ref().map(|config| config.version),
                value_source: "author-defaults-only-no-user-values-read",
                status,
                missing_required,
                fields,
            },
            limitations: [
                "仅检查实验声明与已声明入口、说明文件的存在性，不读取文件内容或执行应用。",
                "未检查完整依赖、可安装性、业务兼容或完整静态 v1 格式。",
                "未读取或保存用户配置、密钥；本地默认值完整不代表服务可连接或凭据有效。",
            ],
        })
    }

    /// The trial has no credential store or UI. Only non-secret author defaults
    /// are used; required missing values block execution instead of borrowing
    /// credentials from the developer's shell.
    pub fn business_defaults(&self) -> Result<BTreeMap<String, String>> {
        let mut result = BTreeMap::new();
        if let Some(configuration) = &self.configuration {
            for field in &configuration.fields {
                match &field.default {
                    Some(value) => {
                        let value = match value {
                            Value::String(value) => value.clone(),
                            Value::Number(value) => value.to_string(),
                            Value::Bool(value) => value.to_string(),
                            _ => return Err(invalid("配置默认值无效。")),
                        };
                        result.insert(field.name.clone(), value);
                    }
                    None if field.required => {
                        return Err(invalid("本实验缺少必填配置，请勿从宿主环境借用凭据。"))
                    }
                    None => {}
                }
            }
        }
        Ok(result)
    }
}

fn checked_child(root: &Path, relative_path: &str, directory: bool) -> Result<PathBuf> {
    if !relative(relative_path) || !root.is_absolute() {
        return Err(invalid("应用路径超出允许范围。"));
    }
    let root_info = fs::symlink_metadata(root).map_err(|_| invalid("应用目录不可读取。"))?;
    if !root_info.is_dir() || root_info.file_type().is_symlink() {
        return Err(invalid("应用目录必须是普通目录。"));
    }
    let mut path = root
        .canonicalize()
        .map_err(|_| invalid("应用目录不可解析。"))?;
    let parts: Vec<_> = relative_path.split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        path.push(part);
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| invalid("声明文件或目录不存在。"))?;
        if metadata.file_type().is_symlink()
            || if index + 1 == parts.len() {
                if directory {
                    !metadata.is_dir()
                } else {
                    !metadata.is_file()
                }
            } else {
                !metadata.is_dir()
            }
        {
            return Err(invalid("应用入口不能经过链接或特殊文件。"));
        }
    }
    Ok(path)
}

impl ConfigurationDecl {
    fn validate(&self) -> Result<()> {
        if self.version != 1 || self.fields.len() > 32 {
            return Err(invalid("配置声明版本或字段数量不受实验支持。"));
        }
        let mut names = HashSet::new();
        for field in &self.fields {
            if !valid_business_variable(&field.name)
                || !names.insert(&field.name)
                || !text(&field.label, 160)
                || !text(&field.description, 4000)
                || field.how_to_get.is_empty()
                || field.how_to_get.len() > 12
                || field.how_to_get.iter().any(|v| !text(v, 2000))
                || field.placeholder.as_ref().is_some_and(|v| !text(v, 1000))
                || field.help_url.as_ref().is_some_and(|v| !https(v))
                || (field.secret
                    && (field.r#type != ConfigurationType::String || field.default.is_some()))
            {
                return Err(invalid("配置字段名称、说明或秘密属性无效。"));
            }
            match (field.r#type, &field.options) {
                (ConfigurationType::Select, Some(options))
                    if !options.is_empty() && options.len() <= 32 =>
                {
                    let mut values = HashSet::new();
                    if options.iter().any(|option| {
                        !text(&option.label, 160)
                            || !text(&option.value, 1000)
                            || !values.insert(&option.value)
                    }) {
                        return Err(invalid("配置选项必须具有独立值和可读名称。"));
                    }
                }
                (ConfigurationType::Select, _) | (_, Some(_)) => {
                    return Err(invalid("配置选项与字段类型不匹配。"))
                }
                _ => {}
            }
            if let Some(default) = &field.default {
                let valid = match field.r#type {
                    ConfigurationType::String => default.as_str().is_some_and(|v| text(v, 4000)),
                    ConfigurationType::Url => default.as_str().is_some_and(https),
                    ConfigurationType::Integer => default.as_i64().is_some(),
                    ConfigurationType::Boolean => default.is_boolean(),
                    ConfigurationType::Select => default.as_str().is_some_and(|v| {
                        field
                            .options
                            .as_ref()
                            .is_some_and(|o| o.iter().any(|o| o.value == v))
                    }),
                };
                if !valid {
                    return Err(invalid("配置默认值与字段类型不匹配。"));
                }
            }
        }
        for field in &self.fields {
            if let Some(endpoint) = &field.endpoint_field {
                if !field.secret
                    || !self.fields.iter().any(|f| {
                        &f.name == endpoint && f.r#type == ConfigurationType::Url && !f.secret
                    })
                {
                    return Err(invalid("秘密凭据必须引用有效的服务地址字段。"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest() -> Value {
        json!({"packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":"org.example.trial","name":"人工测试","version":"0.1.0"},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"root":"server","entry":"managed.mjs","protocolVersion":1,"nodeMajor":24}})
    }
    fn field() -> Value {
        json!({"name":"AI_API_KEY","label":"AI 访问密钥","type":"string","required":false,"secret":true,
            "description":"仅用于人工配置声明测试","howToGet":["本测试不填写真实凭据。"]})
    }
    fn parse(value: Value) -> Result<TrialManifest> {
        TrialManifest::parse(&serde_json::to_vec(&value).unwrap())
    }

    #[test]
    fn optional_native_window_declaration_is_strict_without_changing_old_manifests() {
        assert!(parse(manifest()).unwrap().window.is_none());
        let mut value = manifest();
        value["window"] = json!({"width":960,"height":640,"rememberSize":false});
        let config = parse(value.clone()).unwrap().window.unwrap();
        assert_eq!(
            (config.width, config.height, config.remember_size),
            (960, 640, false)
        );
        for invalid in [
            json!(null),
            json!({"width":0,"height":640}),
            json!({"width":960,"height":640,"nativePermission":true}),
            json!({"width":960,"height":640,"rememberSize":null}),
        ] {
            value["window"] = invalid;
            assert!(parse(value.clone()).is_err());
        }
    }

    #[test]
    fn data_contract_is_optional_strict_and_exposed_as_an_author_declaration() {
        let (app, mut value) = inspect_fixture();
        let legacy = parse(value.clone()).unwrap();
        assert!(legacy.data_contract.is_none());
        assert!(
            serde_json::to_value(legacy.inspect(app.path()).unwrap()).unwrap()["dataContract"]
                .is_null()
        );
        value["dataContract"] = json!({"id":"org.example.notes-data","version":1});
        let declared = parse(value.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(declared.inspect(app.path()).unwrap()).unwrap()["dataContract"],
            value["dataContract"]
        );
        for invalid in [
            json!(null),
            json!({}),
            json!({"id":"data"}),
            json!({"id":"data","version":"1"}),
            json!({"id":"data","version":1.5}),
            json!({"id":"data","version":0}),
            json!({"id":"data","version":-1}),
            json!({"id":"data","version":2147483648_u64}),
            json!({"id":"data","version":1,"migrate":true}),
            json!({"id":"","version":1}),
            json!({"id":".hidden","version":1}),
            json!({"id":"data/schema","version":1}),
            json!({"id":"人工数据","version":1}),
            json!({"id":"a".repeat(129),"version":1}),
        ] {
            value["dataContract"] = invalid;
            assert!(parse(value.clone()).is_err(), "{}", value["dataContract"]);
        }
        value["dataContract"] = json!({"id":"A_1.data-contract","version":2147483647});
        assert!(parse(value.clone()).is_ok());
        let bytes = serde_json::to_string(&value).unwrap().replace(
            "\"id\":\"A_1.data-contract\"",
            "\"id\":\"A_1.data-contract\",\"id\":\"duplicate\"",
        );
        assert!(TrialManifest::parse(bytes.as_bytes()).is_err());
        value["runtime"] = json!("static-web");
        value["formatVersion"] = json!(1);
        value.as_object_mut().unwrap().remove("backend");
        assert!(
            parse(value).is_err(),
            "static v1 does not acquire a Node data contract"
        );
    }

    fn inspect_fixture() -> (tempfile::TempDir, Value) {
        let app = tempfile::tempdir().unwrap();
        for directory in ["public", "server", "docs"] {
            fs::create_dir(app.path().join(directory)).unwrap();
        }
        for path in [
            "public/index.html",
            "server/managed.mjs",
            "docs/usage.md",
            "docs/development.md",
        ] {
            fs::write(app.path().join(path), "artificial file, never executed").unwrap();
        }
        let mut value = manifest();
        value["documentation"] = json!({
            "readme":"docs/usage.md", "development":"docs/development.md"
        });
        (app, value)
    }

    #[test]
    fn runtime_types_cannot_silently_disagree_or_fall_back() {
        assert_eq!(parse(manifest()).unwrap().runtime.label(), "Node 服务项目");
        for key in ["backend", "runtime"] {
            let mut value = manifest();
            value.as_object_mut().unwrap().remove(key);
            assert!(parse(value).is_err());
        }
        let mut value = manifest();
        value["runtime"] = json!("static-web");
        value["formatVersion"] = json!(1);
        assert!(parse(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("backend");
        assert_eq!(
            parse(value.clone()).unwrap().runtime,
            TrialRuntime::StaticWeb
        );
        value["backend"] = Value::Null;
        assert!(parse(value).is_err());
    }

    #[test]
    fn strict_json_rejects_duplicate_unknown_and_over_budget_declarations() {
        let original = serde_json::to_string(&manifest()).unwrap();
        let duplicate = original.replacen(
            "\"formatVersion\":2",
            "\"formatVersion\":2,\"formatVersion\":2",
            1,
        );
        assert_ne!(duplicate, original);
        assert!(TrialManifest::parse(duplicate.as_bytes()).is_err());
        let mut value = manifest();
        value["needsNode"] = json!(true);
        assert!(parse(value).is_err());
        assert!(TrialManifest::parse(&vec![b' '; MAX_MANIFEST_BYTES + 1]).is_err());
    }

    #[test]
    fn declarations_cannot_override_runtime_or_ship_secret_defaults() {
        for name in [
            "NODE_OPTIONS",
            "NODE_EXTRA_CA_CERTS",
            "APP_PORT",
            "PATH",
            "HOME",
            "VITE_KEY",
            "ai_key",
        ] {
            assert!(!valid_business_variable(name));
        }
        let mut value = manifest();
        value["configuration"] = json!({"version":1,"fields":[field()]});
        assert!(parse(value.clone()).is_ok());
        value["configuration"]["fields"][0]["default"] = json!("test-only-cannot-be-default");
        assert!(parse(value).is_err());
    }

    #[test]
    fn every_field_needs_readable_acquisition_steps_and_consistent_endpoint() {
        let mut value = manifest();
        let mut secret = field();
        secret["endpointField"] = json!("AI_BASE_URL");
        value["configuration"] = json!({"version":1,"fields":[secret]});
        assert!(parse(value.clone()).is_err());
        let mut endpoint = field();
        endpoint["name"] = json!("AI_BASE_URL");
        endpoint["type"] = json!("url");
        endpoint["secret"] = json!(false);
        value["configuration"]["fields"]
            .as_array_mut()
            .unwrap()
            .push(endpoint);
        assert!(parse(value.clone()).is_ok());
        value["configuration"]["fields"][0]["howToGet"] = json!([]);
        assert!(parse(value).is_err());
    }

    #[test]
    fn trial_defaults_preserve_false_and_zero_but_never_borrow_missing_secrets() {
        let mut boolean = field();
        boolean["name"] = json!("FEATURE_ENABLED");
        boolean["type"] = json!("boolean");
        boolean["secret"] = json!(false);
        boolean["default"] = json!(false);
        let mut integer = boolean.clone();
        integer["name"] = json!("RETRY_COUNT");
        integer["type"] = json!("integer");
        integer["default"] = json!(0);
        let mut value = manifest();
        value["configuration"] = json!({"version":1,"fields":[boolean,integer,field()]});
        let parsed = parse(value.clone()).unwrap();
        let defaults = parsed.business_defaults().unwrap();
        assert_eq!(
            defaults.get("FEATURE_ENABLED").map(String::as_str),
            Some("false")
        );
        assert_eq!(defaults.get("RETRY_COUNT").map(String::as_str), Some("0"));
        assert!(!defaults.contains_key("AI_API_KEY"));
        value["configuration"]["fields"][2]["required"] = json!(true);
        assert!(parse(value).unwrap().business_defaults().is_err());
    }

    #[test]
    fn resolving_entry_refuses_escape_and_symbolic_links() {
        let app = tempfile::tempdir().unwrap();
        fs::create_dir(app.path().join("server")).unwrap();
        fs::write(
            app.path().join("server/managed.mjs"),
            "// artificial fixture",
        )
        .unwrap();
        let valid = parse(manifest()).unwrap();
        assert!(valid.resolve_backend(app.path()).unwrap().0.is_file());
        let mut value = manifest();
        value["backend"]["entry"] = json!("../outside.mjs");
        assert!(parse(value).is_err());
        #[cfg(unix)]
        {
            fs::remove_file(app.path().join("server/managed.mjs")).unwrap();
            std::os::unix::fs::symlink("missing", app.path().join("server/managed.mjs")).unwrap();
            assert!(valid.resolve_backend(app.path()).is_err());
        }
    }

    #[test]
    fn inspection_previews_missing_required_values_without_reading_user_values() {
        let (app, mut value) = inspect_fixture();
        let mut secret = field();
        secret["required"] = json!(true);
        let mut endpoint = field();
        endpoint["name"] = json!("AI_BASE_URL");
        endpoint["type"] = json!("url");
        endpoint["secret"] = json!(false);
        endpoint["required"] = json!(true);
        endpoint["default"] = json!("https://api.example.invalid/v1");
        secret["endpointField"] = json!("AI_BASE_URL");
        value["configuration"] = json!({"version":1,"fields":[endpoint, secret]});
        fs::write(
            app.path().join(".env"),
            "AI_API_KEY=artificial-env-value-must-not-appear",
        )
        .unwrap();
        fs::create_dir(app.path().join(".data")).unwrap();
        fs::write(
            app.path().join(".data/configuration.json"),
            r#"{"AI_API_KEY":"artificial-saved-value-must-not-appear"}"#,
        )
        .unwrap();
        let manifest = parse(value).unwrap();
        assert!(manifest.business_defaults().is_err());
        let inspection = manifest.inspect(app.path()).unwrap();
        let preview = serde_json::to_value(inspection).unwrap();
        assert_eq!(preview["inspectionKind"], "read-only-trial");
        assert_eq!(preview["runtime"], "node-service");
        assert_eq!(preview["checkedFiles"].as_array().unwrap().len(), 4);
        assert_eq!(preview["documentationDeclared"], true);
        assert_eq!(
            preview["configuration"]["missingRequired"],
            json!(["AI_API_KEY"])
        );
        assert_eq!(preview["configuration"]["status"], "needs-configuration");
        let fields = preview["configuration"]["fields"].as_array().unwrap();
        assert_eq!(fields[0]["default"], "https://api.example.invalid/v1");
        assert_eq!(fields[1]["secret"], true);
        assert!(fields[1].get("default").is_none());
        assert!(fields[1].get("value").is_none());
        assert!(fields[1].get("savedValue").is_none());
        let serialized = serde_json::to_string(&preview).unwrap();
        assert!(!serialized.contains("artificial-env-value"));
        assert!(!serialized.contains("artificial-saved-value"));
    }

    #[test]
    fn inspection_checks_each_declared_entry_and_both_documents() {
        for path in [
            "public/index.html",
            "server/managed.mjs",
            "docs/usage.md",
            "docs/development.md",
        ] {
            let (app, value) = inspect_fixture();
            let manifest = parse(value).unwrap();
            fs::remove_file(app.path().join(path)).unwrap();
            assert!(manifest.inspect(app.path()).is_err());
            fs::create_dir(app.path().join(path)).unwrap();
            assert!(manifest.inspect(app.path()).is_err());
        }
    }

    #[test]
    fn inspection_keeps_optional_documentation_and_default_values_distinct() {
        let (app, mut value) = inspect_fixture();
        value.as_object_mut().unwrap().remove("documentation");
        let mut field = field();
        field["name"] = json!("FEATURE_ENABLED");
        field["type"] = json!("boolean");
        field["secret"] = json!(false);
        field["required"] = json!(true);
        field["default"] = json!(false);
        value["configuration"] = json!({"version":1,"fields":[field]});
        let manifest = parse(value).unwrap();
        let preview = serde_json::to_value(manifest.inspect(app.path()).unwrap()).unwrap();
        assert_eq!(preview["documentationDeclared"], false);
        assert_eq!(preview["checkedFiles"].as_array().unwrap().len(), 2);
        assert_eq!(preview["configuration"]["missingRequired"], json!([]));
        assert_eq!(preview["configuration"]["fields"][0]["default"], false);
        assert_eq!(
            preview["configuration"]["fields"][0]["valueStatus"],
            "author-default"
        );
    }

    #[test]
    #[cfg(unix)]
    fn inspection_rejects_linked_entry_files_and_document_directories() {
        for path in ["public/index.html", "server/managed.mjs", "docs/usage.md"] {
            let (app, value) = inspect_fixture();
            let manifest = parse(value).unwrap();
            fs::remove_file(app.path().join(path)).unwrap();
            std::os::unix::fs::symlink("missing-artificial-target", app.path().join(path)).unwrap();
            assert!(manifest.inspect(app.path()).is_err());
        }
        let (app, value) = inspect_fixture();
        let manifest = parse(value).unwrap();
        fs::rename(app.path().join("docs"), app.path().join("hidden-docs")).unwrap();
        std::os::unix::fs::symlink("hidden-docs", app.path().join("docs")).unwrap();
        assert!(manifest.inspect(app.path()).is_err());
        // Unrelated app-private paths are deliberately neither followed nor read.
        fs::remove_file(app.path().join("docs")).unwrap();
        fs::rename(app.path().join("hidden-docs"), app.path().join("docs")).unwrap();
        std::os::unix::fs::symlink("missing-env", app.path().join(".env")).unwrap();
        assert!(manifest.inspect(app.path()).is_ok());
    }
}
