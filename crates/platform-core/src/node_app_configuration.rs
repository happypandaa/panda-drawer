//! Configuration for the separately registered, local Node application slots.
//! Explicitly requested local-file credentials use immutable private files.
//! Configuration JSON contains only ordinary values and opaque entry IDs;
//! credentials are never serialized back to the frontend or placed in logs.

use crate::panda_manifest::{
    valid_business_variable, ConfigurationField, ConfigurationType, PandaManifest,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

const MAX_RECORD_BYTES: u64 = 256 * 1024;
const MAX_SECRET_BYTES: usize = 8192;

#[derive(Debug)]
pub struct ConfigurationError(&'static str);
impl fmt::Display for ConfigurationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ConfigurationError {}
type Result<T> = std::result::Result<T, ConfigurationError>;
fn invalid(message: &'static str) -> ConfigurationError {
    ConfigurationError(message)
}

/// Implementations must add a new entry or fail, never replace an existing one.
/// `discard` is called only with IDs successfully inserted by this save attempt.
/// The configured provider is authoritative: no implicit OS-store fallback.
pub trait SecretVault: Send + Sync {
    fn insert(&self, instance: Uuid, entry: Uuid, value: &str) -> Result<()>;
    fn get(&self, instance: Uuid, entry: Uuid) -> Result<String>;
    fn discard(&self, instance: Uuid, entry: Uuid) -> Result<()>;
    /// Checks only the specific entry's presence and metadata, without echoing
    /// or reading the credential. Missing legacy provider entries stay missing.
    fn contains(&self, instance: Uuid, entry: Uuid) -> Result<bool>;
    fn retire(&self, instance: Uuid, entry: Uuid) -> Result<()> {
        self.discard(instance, entry)
    }
    fn retire_instance(&self, _instance: Uuid) -> Result<()> {
        Err(invalid("此密钥存储不支持实例清理。"))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveConfigurationRequest {
    pub revision: u64,
    /// NodeApps checks this against the declaration used by the editor. The
    /// lower-level store also serves callers with an already pinned manifest.
    #[serde(default)]
    pub manifest_digest: Option<String>,
    /// A complete ordinary-value snapshot; omitted fields use author defaults.
    pub values: BTreeMap<String, Value>,
    #[serde(default)]
    pub secrets: BTreeMap<String, SecretAction>,
}

// Deliberately no Debug or Serialize: replacement credentials must not become
// command diagnostics or returned JSON by accident.
pub enum SecretAction {
    Keep,
    Replace {
        value: String,
    },
    Unbind,
    /// Explicitly permit the old credential at the newly selected endpoint.
    Rebind,
}

impl<'de> Deserialize<'de> for SecretAction {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        // Serde's internally tagged unit variants otherwise silently accept a
        // value property on `keep`. Parse a strict record and enforce its shape.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ActionRecord {
            action: String,
            #[serde(default, deserialize_with = "present_value")]
            value: Option<String>,
        }
        fn present_value<'de, D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> std::result::Result<Option<String>, D::Error> {
            String::deserialize(deserializer).map(Some)
        }
        let record = ActionRecord::deserialize(deserializer)?;
        match (record.action.as_str(), record.value) {
            ("keep", None) => Ok(Self::Keep),
            ("replace", Some(value)) => Ok(Self::Replace { value }),
            ("unbind", None) => Ok(Self::Unbind),
            ("rebind", None) => Ok(Self::Rebind),
            _ => Err(serde::de::Error::custom("密钥操作格式无效。")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SecretStatus {
    Set,
    Missing,
    EndpointChanged,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigurationView {
    pub revision: u64,
    pub values: BTreeMap<String, Value>,
    /// Only explicit user overrides; the editor must not materialize defaults.
    pub saved_values: BTreeMap<String, Value>,
    pub secrets: BTreeMap<String, SecretStatus>,
    pub missing_required: Vec<String>,
    pub invalid_fields: Vec<String>,
    pub can_start: bool,
}

/// Only the process launcher receives this value, never a frontend response.
pub struct LaunchConfigurationSnapshot {
    pub revision: u64,
    pub environment: BTreeMap<String, String>,
    pub redaction_values: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredBinding {
    entry: String,
    endpoint: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredConfiguration {
    version: u32,
    instance_id: String,
    revision: u64,
    values: BTreeMap<String, Value>,
    secrets: BTreeMap<String, StoredBinding>,
}

#[derive(Clone, Copy)]
enum ConfigurationUse {
    Node,
    Web,
}
pub struct NodeConfigurationStore {
    directory: PathBuf,
    instance: Uuid,
    vault: Arc<dyn SecretVault>,
    usage: ConfigurationUse,
}
struct NoSecrets;
impl SecretVault for NoSecrets {
    fn insert(&self, _instance: Uuid, _entry: Uuid, _value: &str) -> Result<()> {
        Err(invalid("网页配置不支持密钥。"))
    }
    fn get(&self, _instance: Uuid, _entry: Uuid) -> Result<String> {
        Err(invalid("网页配置不支持密钥。"))
    }
    fn discard(&self, _instance: Uuid, _entry: Uuid) -> Result<()> {
        Err(invalid("网页配置不支持密钥。"))
    }
    fn contains(&self, _instance: Uuid, _entry: Uuid) -> Result<bool> {
        Err(invalid("网页配置不支持密钥。"))
    }
}

impl NodeConfigurationStore {
    /// The caller owns this stable, per-instance configuration directory. It is
    /// deliberately separate from workspaces, backups and application sources.
    pub fn new(directory: PathBuf, instance: Uuid, vault: Arc<dyn SecretVault>) -> Result<Self> {
        if instance.is_nil() {
            return Err(invalid("应用配置实例标识无效。"));
        }
        checked_directory(&directory)?;
        Ok(Self {
            directory,
            instance,
            vault,
            usage: ConfigurationUse::Node,
        })
    }

    pub(crate) fn new_web(directory: PathBuf, instance: Uuid) -> Result<Self> {
        let mut store = Self::new(directory, instance, Arc::new(NoSecrets))?;
        store.usage = ConfigurationUse::Web;
        Ok(store)
    }
    fn fields<'a>(&self, manifest: &'a PandaManifest) -> Result<Vec<&'a ConfigurationField>> {
        let fields = checked_fields(manifest)?;
        if matches!(self.usage, ConfigurationUse::Web) && fields.iter().any(|field| field.secret) {
            return Err(invalid("网页配置不支持密钥。"));
        }
        Ok(fields)
    }
    pub fn view(&self, manifest: &PandaManifest) -> Result<ConfigurationView> {
        let _lock = self.lock()?;
        let record = self.read()?;
        self.view_record(manifest, &record)
    }

    pub fn save(
        &self,
        manifest: &PandaManifest,
        request: SaveConfigurationRequest,
    ) -> Result<ConfigurationView> {
        let _lock = self.lock()?;
        let previous = self.read()?;
        if previous.revision != request.revision {
            return Err(invalid("配置已被其他窗口更新，请重新读取后再保存。"));
        }
        let fields = self.fields(manifest)?;
        let (previous, _) = project_record(&fields, &previous)?;
        validate_ordinary(&fields, &request.values)?;
        for name in request.secrets.keys() {
            if !fields
                .iter()
                .any(|field| field.name == *name && field.secret)
            {
                return Err(invalid("提交了未声明的密钥配置。"));
            }
        }
        let values = effective_values(&fields, &request.values)?;
        // Validate the whole transaction before the first credential write.
        for field in &fields {
            if !field.secret {
                continue;
            }
            let binding = previous.secrets.get(&field.name);
            let action = request
                .secrets
                .get(&field.name)
                .unwrap_or(&SecretAction::Keep);
            match action {
                SecretAction::Keep
                    if binding
                        .is_some_and(|binding| binding.endpoint != endpoint(field, &values)) =>
                {
                    return Err(invalid(
                        "服务地址已更改，请明确替换密钥、确认重新绑定或移除密钥。",
                    ))
                }
                SecretAction::Replace { value } => validate_secret(value)?,
                SecretAction::Rebind if binding.is_none() => {
                    return Err(invalid("没有可重新绑定的密钥，请填写新密钥。"));
                }
                _ => {}
            }
            if field.endpoint_field.is_some()
                && endpoint(field, &values).is_none()
                && matches!(action, SecretAction::Replace { .. } | SecretAction::Rebind)
            {
                return Err(invalid("保存此密钥前请先填写它关联的服务地址。"));
            }
        }
        let revision = previous
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("配置版本已到达上限。"))?;
        let mut next = StoredConfiguration {
            version: 1,
            instance_id: self.instance.to_string(),
            revision,
            values: request.values,
            secrets: previous.secrets,
        };
        let mut inserted = Vec::new();
        let attempt = (|| -> Result<ConfigurationView> {
            for field in &fields {
                if !field.secret {
                    continue;
                }
                match request
                    .secrets
                    .get(&field.name)
                    .unwrap_or(&SecretAction::Keep)
                {
                    SecretAction::Keep => {}
                    SecretAction::Unbind => {
                        next.secrets.remove(&field.name);
                    }
                    action => {
                        let credential = match action {
                            SecretAction::Replace { value } => value.clone(),
                            SecretAction::Rebind => {
                                let binding = next
                                    .secrets
                                    .get(&field.name)
                                    .ok_or_else(|| invalid("没有可重新绑定的密钥。"))?;
                                self.vault.get(self.instance, entry_uuid(binding)?)?
                            }
                            _ => unreachable!(),
                        };
                        validate_secret(&credential)?;
                        let entry = Uuid::new_v4();
                        self.vault.insert(self.instance, entry, &credential)?;
                        inserted.push(entry);
                        next.secrets.insert(
                            field.name.clone(),
                            StoredBinding {
                                entry: entry.to_string(),
                                endpoint: endpoint(field, &values),
                            },
                        );
                    }
                }
            }
            let view = self.view_record(manifest, &next)?;
            self.commit(&next)?;
            Ok(view)
        })();
        if let Err(error) = attempt {
            // A failed cleanup can leave only new, unreferenced credential files.
            // Never remove an entry referenced by the previous committed record.
            for entry in inserted {
                let _ = self.vault.discard(self.instance, entry);
            }
            return Err(error);
        }
        // Old immutable entries intentionally remain unreferenced. Destructive
        // credential garbage collection is outside this initial slot workflow.
        attempt
    }

    /// Retire stored credentials only after a backend-free reload has succeeded.
    /// The immutable credential files remain owned by this private application
    /// slot and are removed together with the slot during application deletion.
    pub fn remove_all_secrets(&self) -> Result<Vec<String>> {
        let _lock = self.lock()?;
        let mut record = self.read()?;
        let names = record.secrets.keys().cloned().collect::<Vec<_>>();
        self.vault.retire_instance(self.instance)?;
        record.secrets.clear();
        if !names.is_empty() {
            record.revision = record
                .revision
                .checked_add(1)
                .ok_or_else(|| invalid("配置版本已到达上限。"))?;
            self.commit(&record)?;
        }
        Ok(names)
    }

    pub fn launch_snapshot(&self, manifest: &PandaManifest) -> Result<LaunchConfigurationSnapshot> {
        if !matches!(self.usage, ConfigurationUse::Node) {
            return Err(invalid("网页配置不能生成Node环境变量或密钥快照。"));
        }
        let _lock = self.lock()?;
        let record = self.read()?;
        let view = self.view_record(manifest, &record)?;
        if view.secrets.iter().any(|(name, status)| {
            *status == SecretStatus::Missing && record.secrets.contains_key(name)
        }) {
            return Err(invalid("本地密钥文件缺失，请重新填写并保存密钥后再启动。"));
        }
        if !view.can_start {
            return Err(invalid("应用配置尚未完整，或密钥需要重新绑定服务地址。"));
        }
        let fields = self.fields(manifest)?;
        let mut environment = BTreeMap::new();
        let mut redaction_values = Vec::new();
        for (name, value) in &view.values {
            environment.insert(name.clone(), environment_value(value)?);
        }
        for field in fields {
            if !field.secret {
                continue;
            }
            if let Some(binding) = record.secrets.get(&field.name) {
                let credential = self.vault.get(self.instance, entry_uuid(binding)?)?;
                validate_secret(&credential)?;
                redaction_values.push(credential.clone());
                environment.insert(field.name.clone(), credential);
            }
        }
        if environment
            .iter()
            .map(|(name, value)| name.len() + value.len())
            .sum::<usize>()
            > 32768
        {
            return Err(invalid("启动配置总大小超过当前 Node 运行时限制。"));
        }
        Ok(LaunchConfigurationSnapshot {
            revision: record.revision,
            environment,
            redaction_values,
        })
    }

    fn view_record(
        &self,
        manifest: &PandaManifest,
        record: &StoredConfiguration,
    ) -> Result<ConfigurationView> {
        let fields = self.fields(manifest)?;
        let (record, invalid_fields) = project_record(&fields, record)?;
        let values = effective_values(&fields, &record.values)?;
        let mut secrets = BTreeMap::new();
        let mut missing_required = Vec::new();
        let mut binding_changed = false;
        let mut binding_missing = false;
        for field in fields {
            if field.secret {
                let status = match record.secrets.get(&field.name) {
                    Some(binding)
                        if !self.vault.contains(self.instance, entry_uuid(binding)?)? =>
                    {
                        binding_missing = true;
                        SecretStatus::Missing
                    }
                    Some(binding) if binding.endpoint != endpoint(field, &values) => {
                        binding_changed = true;
                        SecretStatus::EndpointChanged
                    }
                    Some(_) => SecretStatus::Set,
                    None => SecretStatus::Missing,
                };
                if field.required && status != SecretStatus::Set {
                    missing_required.push(field.name.clone());
                }
                secrets.insert(field.name.clone(), status);
            } else if field.required && !values.contains_key(&field.name) {
                missing_required.push(field.name.clone());
            }
        }
        Ok(ConfigurationView {
            revision: record.revision,
            values,
            saved_values: record.values,
            secrets,
            can_start: missing_required.is_empty()
                && invalid_fields.is_empty()
                && !binding_changed
                && !binding_missing,
            missing_required,
            invalid_fields,
        })
    }
}

fn checked_fields(manifest: &PandaManifest) -> Result<Vec<&ConfigurationField>> {
    // The manifest's typed declarations are public. Revalidate by round-tripping
    // just the declaration through its existing strict reader is unnecessary:
    // enforce each launch-relevant rule locally, including mutated Rust values.
    let Some(configuration) = &manifest.configuration else {
        return Ok(Vec::new());
    };
    if configuration.version != 1 || configuration.fields.len() > 32 {
        return Err(invalid("应用配置声明版本或字段数量无效。"));
    }
    let mut names = HashSet::new();
    for field in &configuration.fields {
        if !valid_business_variable(&field.name)
            || !names.insert(&field.name)
            || (field.secret
                && (field.r#type != ConfigurationType::String || field.default.is_some()))
        {
            return Err(invalid("应用配置字段或密钥默认值无效。"));
        }
        if let Some(default) = &field.default {
            validate_value(field, default)?;
        }
        if field.r#type == ConfigurationType::Select
            && field.options.as_ref().is_none_or(Vec::is_empty)
        {
            return Err(invalid("应用配置选项无效。"));
        }
    }
    for field in &configuration.fields {
        if let Some(endpoint) = &field.endpoint_field {
            if !field.secret
                || !configuration.fields.iter().any(|candidate| {
                    candidate.name == *endpoint
                        && candidate.r#type == ConfigurationType::Url
                        && !candidate.secret
                })
            {
                return Err(invalid("密钥关联的服务地址声明无效。"));
            }
        }
    }
    Ok(configuration.fields.iter().collect())
}

fn validate_ordinary(
    fields: &[&ConfigurationField],
    values: &BTreeMap<String, Value>,
) -> Result<()> {
    for (name, value) in values {
        let field = fields
            .iter()
            .find(|field| field.name == *name && !field.secret)
            .ok_or_else(|| invalid("提交了未声明的普通配置或密钥明文。"))?;
        validate_value(field, value)?;
    }
    Ok(())
}

fn effective_values(
    fields: &[&ConfigurationField],
    values: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>> {
    validate_ordinary(fields, values)?;
    let mut effective = BTreeMap::new();
    for field in fields {
        if !field.secret {
            if let Some(value) = values.get(&field.name).or(field.default.as_ref()) {
                validate_value(field, value)?;
                effective.insert(field.name.clone(), value.clone());
            }
        }
    }
    Ok(effective)
}

/// Read a saved record through the current declaration. Retired fields never
/// reach the editor or process environment. An explicit save, rather than a
/// detail read, commits removal of retired or incompatible bindings.
fn project_record(
    fields: &[&ConfigurationField],
    record: &StoredConfiguration,
) -> Result<(StoredConfiguration, Vec<String>)> {
    let mut values = BTreeMap::new();
    let mut secrets = BTreeMap::new();
    let mut invalid_fields = Vec::new();
    for field in fields {
        if field.secret {
            if record.values.contains_key(&field.name) {
                invalid_fields.push(field.name.clone());
            }
            if let Some(binding) = record.secrets.get(&field.name) {
                entry_uuid(binding)?;
                if binding
                    .endpoint
                    .as_ref()
                    .is_some_and(|value| !valid_url(value))
                {
                    return Err(invalid("已保存的密钥地址绑定无效。"));
                }
                secrets.insert(field.name.clone(), binding.clone());
            }
        } else {
            // Never read a former credential into an ordinary field, including
            // when the new declaration supplies a default with the same name.
            if record.secrets.contains_key(&field.name) {
                invalid_fields.push(field.name.clone());
            }
            if let Some(value) = record.values.get(&field.name) {
                if validate_value(field, value).is_ok() {
                    values.insert(field.name.clone(), value.clone());
                } else if !invalid_fields.contains(&field.name) {
                    invalid_fields.push(field.name.clone());
                }
            }
        }
    }
    Ok((
        StoredConfiguration {
            version: record.version,
            instance_id: record.instance_id.clone(),
            revision: record.revision,
            values,
            secrets,
        },
        invalid_fields,
    ))
}

fn valid_url(value: &str) -> bool {
    value.len() <= 4000
        && value.trim() == value
        && !value.chars().any(char::is_control)
        && url::Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        })
}

fn validate_value(field: &ConfigurationField, value: &Value) -> Result<()> {
    let valid = match field.r#type {
        ConfigurationType::String => value.as_str().is_some_and(|value| {
            !value.trim().is_empty() && value.len() <= 4000 && !value.chars().any(char::is_control)
        }),
        ConfigurationType::Url => value.as_str().is_some_and(valid_url),
        ConfigurationType::Integer => value.as_i64().is_some(),
        ConfigurationType::Boolean => value.is_boolean(),
        ConfigurationType::Select => value.as_str().is_some_and(|value| {
            field
                .options
                .as_ref()
                .is_some_and(|options| options.iter().any(|option| option.value == value))
        }),
    };
    if !valid {
        return Err(invalid("配置值与声明类型不匹配，或服务地址无效。"));
    }
    Ok(())
}

fn validate_secret(value: &str) -> Result<()> {
    if value.trim().is_empty()
        || value.len() > MAX_SECRET_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(invalid("密钥为空、过长或含有不可用字符。"));
    }
    Ok(())
}

fn endpoint(field: &ConfigurationField, values: &BTreeMap<String, Value>) -> Option<String> {
    field
        .endpoint_field
        .as_ref()
        .and_then(|name| values.get(name))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn entry_uuid(binding: &StoredBinding) -> Result<Uuid> {
    Uuid::parse_str(&binding.entry)
        .ok()
        .filter(|id| !id.is_nil() && id.to_string() == binding.entry)
        .ok_or_else(|| invalid("已保存的密钥条目标识无效。"))
}

fn environment_value(value: &Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        _ => Err(invalid("配置值无法转换为启动参数。")),
    }
}

#[cfg(unix)]
include!("node_app_configuration/unix.rs");
#[cfg(windows)]
include!("node_app_configuration/windows.rs");

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeVault {
        values: Mutex<BTreeMap<(Uuid, Uuid), String>>,
        insertions: AtomicUsize,
        fail_insert_at: AtomicUsize,
        unavailable: AtomicBool,
        #[cfg(unix)]
        make_directory_read_only: Mutex<Option<PathBuf>>,
    }

    impl SecretVault for FakeVault {
        fn contains(&self, instance: Uuid, entry: Uuid) -> Result<bool> {
            Ok(self.values.lock().unwrap().contains_key(&(instance, entry)))
        }
        fn insert(&self, instance: Uuid, entry: Uuid, value: &str) -> Result<()> {
            let count = self.insertions.fetch_add(1, Ordering::SeqCst) + 1;
            if self.unavailable.load(Ordering::SeqCst)
                || self.fail_insert_at.load(Ordering::SeqCst) == count
            {
                return Err(invalid("模拟本地密钥存储不可用。"));
            }
            let mut values = self.values.lock().unwrap();
            if values.contains_key(&(instance, entry)) {
                return Err(invalid("重复条目。"));
            }
            values.insert((instance, entry), value.to_owned());
            #[cfg(unix)]
            if let Some(path) = self.make_directory_read_only.lock().unwrap().take() {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o500)).unwrap();
            }
            Ok(())
        }
        fn get(&self, instance: Uuid, entry: Uuid) -> Result<String> {
            if self.unavailable.load(Ordering::SeqCst) {
                return Err(invalid("模拟本地密钥存储不可用。"));
            }
            self.values
                .lock()
                .unwrap()
                .get(&(instance, entry))
                .cloned()
                .ok_or_else(|| invalid("模拟本地密钥存储条目缺失。"))
        }
        fn discard(&self, instance: Uuid, entry: Uuid) -> Result<()> {
            self.values.lock().unwrap().remove(&(instance, entry));
            Ok(())
        }
    }

    fn manifest() -> PandaManifest {
        PandaManifest::parse(&serde_json::to_vec(&json!({
            "manifestVersion":2, "app":{"name":"可丢弃配置测试","version":"1"},
            "web":{"root":"public","entry":"index.html"},
            "backend":{"root":"server","entry":"managed.mjs","nodeMajor":24,"protocolVersion":1},
            "configuration":{"version":1,"fields":[
                {"name":"BASE_URL","label":"服务地址","type":"url","required":true,"secret":false,
                    "description":"测试地址","howToGet":["仅用于测试"],"default":"https://example.invalid/v1"},
                {"name":"API_KEY","label":"密钥","type":"string","required":true,"secret":true,
                    "description":"测试密钥","howToGet":["人工测试凭据"],"endpointField":"BASE_URL"},
                {"name":"EXTRA_KEY","label":"可选密钥","type":"string","required":false,"secret":true,
                    "description":"测试密钥","howToGet":["人工测试凭据"]},
                {"name":"COUNT","label":"数量","type":"integer","required":false,"secret":false,
                    "description":"测试零值","howToGet":["使用默认"],"default":0},
                {"name":"ENABLED","label":"开关","type":"boolean","required":false,"secret":false,
                    "description":"测试否值","howToGet":["使用默认"],"default":false},
                {"name":"MODEL","label":"模型","type":"select","required":false,"secret":false,
                    "description":"测试选项","howToGet":["使用默认"],"default":"one",
                    "options":[{"value":"one","label":"一"},{"value":"two","label":"二"}]}
            ]}
        })).unwrap()).unwrap()
    }

    fn store() -> (tempfile::TempDir, Arc<FakeVault>, NodeConfigurationStore) {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().canonicalize().unwrap();
        let vault = Arc::new(FakeVault::default());
        let store = NodeConfigurationStore::new(directory, Uuid::new_v4(), vault.clone()).unwrap();
        (temporary, vault, store)
    }

    fn request(revision: u64, key: Option<&str>) -> SaveConfigurationRequest {
        SaveConfigurationRequest {
            revision,
            manifest_digest: None,
            values: BTreeMap::new(),
            secrets: key
                .into_iter()
                .map(|value| {
                    (
                        "API_KEY".into(),
                        SecretAction::Replace {
                            value: value.into(),
                        },
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn defaults_status_and_missing_do_not_read_credential() {
        let (_temporary, vault, store) = store();
        vault.unavailable.store(true, Ordering::SeqCst);
        let view = store.view(&manifest()).unwrap();
        assert_eq!(view.revision, 0);
        assert_eq!(view.values["COUNT"], json!(0));
        assert_eq!(view.values["ENABLED"], json!(false));
        assert_eq!(view.secrets["API_KEY"], SecretStatus::Missing);
        assert_eq!(view.missing_required, ["API_KEY"]);
        assert!(!view.can_start);
        assert!(store.launch_snapshot(&manifest()).is_err());
    }

    #[test]
    fn editor_values_keep_defaults_unset_and_preserve_explicit_zero() {
        let (_temporary, _vault, store) = store();
        let original = manifest();
        let view = store.save(&original, request(0, None)).unwrap();
        assert_eq!(view.values["COUNT"], json!(0));
        assert!(view.saved_values.is_empty());
        let mut changed = manifest();
        changed.configuration.as_mut().unwrap().fields[3].default = Some(json!(5));
        assert_eq!(store.view(&changed).unwrap().values["COUNT"], json!(5));
        let mut explicit = request(1, None);
        explicit.values.insert("COUNT".into(), json!(0));
        let view = store.save(&changed, explicit).unwrap();
        assert_eq!(view.values["COUNT"], json!(0));
        assert_eq!(view.saved_values["COUNT"], json!(0));
    }

    #[test]
    fn saved_values_never_contain_secret_and_snapshot_retains_zero_false() {
        let (temporary, _vault, store) = store();
        let credential = "disposable-secret-never-written";
        let view = store
            .save(&manifest(), request(0, Some(credential)))
            .unwrap();
        assert_eq!(view.revision, 1);
        assert!(view.can_start);
        let rendered = serde_json::to_string(&view).unwrap();
        assert!(!rendered.contains(credential));
        let disk = fs::read_to_string(temporary.path().join("configuration.json")).unwrap();
        assert!(!disk.contains(credential));
        let snapshot = store.launch_snapshot(&manifest()).unwrap();
        assert_eq!(snapshot.revision, 1);
        assert_eq!(snapshot.environment["API_KEY"], credential);
        assert_eq!(snapshot.environment["COUNT"], "0");
        assert_eq!(snapshot.environment["ENABLED"], "false");
        assert!(!snapshot.environment.contains_key("EXTRA_KEY"));
    }

    #[test]
    fn revision_conflict_preserves_original_and_does_not_touch_credential() {
        let (temporary, vault, store) = store();
        store
            .save(&manifest(), request(0, Some("first-dummy")))
            .unwrap();
        let before = fs::read(temporary.path().join("configuration.json")).unwrap();
        assert!(store
            .save(&manifest(), request(0, Some("second-dummy")))
            .is_err());
        assert_eq!(
            before,
            fs::read(temporary.path().join("configuration.json")).unwrap()
        );
        assert_eq!(vault.insertions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn endpoint_change_requires_explicit_rebind_or_replacement() {
        let (_temporary, vault, store) = store();
        store
            .save(&manifest(), request(0, Some("disposable-key")))
            .unwrap();
        let mut update = request(1, None);
        update
            .values
            .insert("BASE_URL".into(), json!("https://other.invalid/api"));
        assert!(store.save(&manifest(), update).is_err());
        assert_eq!(store.view(&manifest()).unwrap().revision, 1);
        let mut update = request(1, None);
        update
            .values
            .insert("BASE_URL".into(), json!("https://other.invalid/api"));
        update
            .secrets
            .insert("API_KEY".into(), SecretAction::Rebind);
        store.save(&manifest(), update).unwrap();
        let snapshot = store.launch_snapshot(&manifest()).unwrap();
        assert_eq!(
            snapshot.environment["BASE_URL"],
            "https://other.invalid/api"
        );
        assert_eq!(snapshot.environment["API_KEY"], "disposable-key");
        assert_eq!(vault.insertions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn author_endpoint_change_cannot_silently_send_old_key() {
        let (_temporary, _vault, store) = store();
        store
            .save(&manifest(), request(0, Some("disposable-key")))
            .unwrap();
        let mut changed = manifest();
        changed.configuration.as_mut().unwrap().fields[0].default =
            Some(json!("https://changed.invalid"));
        let view = store.view(&changed).unwrap();
        assert_eq!(view.secrets["API_KEY"], SecretStatus::EndpointChanged);
        assert!(!view.can_start);
        assert!(store.launch_snapshot(&changed).is_err());
        assert!(store.save(&changed, request(1, None)).is_err());
    }

    #[test]
    fn unbind_is_explicit_and_never_launches_old_key() {
        let (_temporary, vault, store) = store();
        store
            .save(&manifest(), request(0, Some("disposable-key")))
            .unwrap();
        let mut update = request(1, None);
        update
            .secrets
            .insert("API_KEY".into(), SecretAction::Unbind);
        let view = store.save(&manifest(), update).unwrap();
        assert_eq!(view.secrets["API_KEY"], SecretStatus::Missing);
        assert!(store.launch_snapshot(&manifest()).is_err());
        // This first implementation retains old immutable, unreferenced entries.
        assert_eq!(vault.values.lock().unwrap().len(), 1);
    }

    #[test]
    fn unknown_fields_secret_plaintext_and_invalid_types_are_rejected_before_write() {
        let (_temporary, vault, store) = store();
        for (name, value) in [
            ("UNDECLARED", json!("x")),
            ("NODE_OPTIONS", json!("x")),
            ("API_KEY", json!("secret-in-ordinary-map")),
            ("COUNT", json!("0")),
            ("ENABLED", json!(0)),
            ("MODEL", json!("not-an-option")),
            ("BASE_URL", json!("http://example.invalid")),
            ("BASE_URL", json!("https://user:password@example.invalid")),
            ("BASE_URL", json!("https://example.invalid?key=dummy")),
            ("BASE_URL", json!("https://example.invalid/#fragment")),
        ] {
            let mut update = request(0, Some("never-inserted"));
            update.values.insert(name.into(), value);
            assert!(store.save(&manifest(), update).is_err());
        }
        assert_eq!(vault.insertions.load(Ordering::SeqCst), 0);
        assert_eq!(store.view(&manifest()).unwrap().revision, 0);
    }

    #[test]
    fn secret_actions_only_accept_declared_secrets_and_valid_values() {
        let (_temporary, vault, store) = store();
        for value in ["", " ", "dummy\nline", "dummy\0line"] {
            assert!(store.save(&manifest(), request(0, Some(value))).is_err());
        }
        for name in ["BASE_URL", "NODE_OPTIONS", "UNKNOWN_KEY"] {
            let mut update = request(0, None);
            update.secrets.insert(
                name.into(),
                SecretAction::Replace {
                    value: "dummy".into(),
                },
            );
            assert!(store.save(&manifest(), update).is_err());
        }
        let mut update = request(0, None);
        update
            .secrets
            .insert("API_KEY".into(), SecretAction::Rebind);
        assert!(store.save(&manifest(), update).is_err());
        assert_eq!(vault.insertions.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn credential_failure_rolls_back_whole_save_without_bypassing_provider() {
        let (temporary, vault, store) = store();
        store
            .save(&manifest(), request(0, Some("original-dummy")))
            .unwrap();
        let original = fs::read(temporary.path().join("configuration.json")).unwrap();
        vault.fail_insert_at.store(3, Ordering::SeqCst);
        let mut update = request(1, Some("replacement-not-committed"));
        update.secrets.insert(
            "EXTRA_KEY".into(),
            SecretAction::Replace {
                value: "second-not-committed".into(),
            },
        );
        assert!(store.save(&manifest(), update).is_err());
        assert_eq!(
            original,
            fs::read(temporary.path().join("configuration.json")).unwrap()
        );
        assert_eq!(vault.values.lock().unwrap().len(), 1);
        assert_eq!(
            store.launch_snapshot(&manifest()).unwrap().environment["API_KEY"],
            "original-dummy"
        );
        vault.unavailable.store(true, Ordering::SeqCst);
        assert!(store.launch_snapshot(&manifest()).is_err());
        let disk = fs::read_to_string(temporary.path().join("configuration.json")).unwrap();
        for secret in [
            "original-dummy",
            "replacement-not-committed",
            "second-not-committed",
        ] {
            assert!(!disk.contains(secret));
        }
    }

    #[cfg(unix)]
    #[test]
    fn failed_atomic_record_commit_cleans_only_new_credential_entries() {
        use std::os::unix::fs::PermissionsExt;
        let (temporary, vault, store) = store();
        store
            .save(&manifest(), request(0, Some("original-dummy")))
            .unwrap();
        let before = fs::read(temporary.path().join("configuration.json")).unwrap();
        *vault.make_directory_read_only.lock().unwrap() = Some(temporary.path().to_owned());
        let attempted = store.save(&manifest(), request(1, Some("replacement-not-committed")));
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(attempted.is_err());
        assert_eq!(vault.insertions.load(Ordering::SeqCst), 2);
        assert_eq!(vault.values.lock().unwrap().len(), 1);
        assert_eq!(
            before,
            fs::read(temporary.path().join("configuration.json")).unwrap()
        );
        assert_eq!(
            store.launch_snapshot(&manifest()).unwrap().environment["API_KEY"],
            "original-dummy"
        );
    }

    #[test]
    fn concurrent_saves_with_same_revision_have_exactly_one_winner() {
        let (_temporary, vault, store) = store();
        store
            .save(&manifest(), request(0, Some("original-dummy")))
            .unwrap();
        let store = Arc::new(store);
        let first = store.clone();
        let second = store.clone();
        let first =
            std::thread::spawn(move || first.save(&manifest(), request(1, Some("first-dummy"))));
        let second =
            std::thread::spawn(move || second.save(&manifest(), request(1, Some("second-dummy"))));
        let outcomes = [
            first.join().unwrap().is_ok(),
            second.join().unwrap().is_ok(),
        ];
        assert_eq!(outcomes.into_iter().filter(|success| *success).count(), 1);
        assert_eq!(store.view(&manifest()).unwrap().revision, 2);
        assert_eq!(vault.insertions.load(Ordering::SeqCst), 2);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_and_hardlinked_records_are_rejected() {
        use std::os::unix::fs::symlink;
        let (temporary, _vault, store) = store();
        let other = tempfile::tempdir().unwrap();
        fs::write(other.path().join("foreign"), b"{}").unwrap();
        symlink(
            other.path().join("foreign"),
            temporary.path().join("configuration.json"),
        )
        .unwrap();
        assert!(store.view(&manifest()).is_err());
        fs::remove_file(temporary.path().join("configuration.json")).unwrap();
        fs::hard_link(
            other.path().join("foreign"),
            temporary.path().join("configuration.json"),
        )
        .unwrap();
        assert!(store.view(&manifest()).is_err());
    }

    #[test]
    fn changed_declaration_cannot_promote_secrets_to_plaintext() {
        let (_temporary, _vault, store) = store();
        store
            .save(&manifest(), request(0, Some("disposable-key")))
            .unwrap();
        let mut changed = manifest();
        let field = &mut changed.configuration.as_mut().unwrap().fields[1];
        field.secret = false;
        field.endpoint_field = None;
        let view = store.view(&changed).unwrap();
        assert_eq!(view.invalid_fields, ["API_KEY"]);
        assert!(!view.can_start);
        assert!(!view.saved_values.contains_key("API_KEY"));
        assert!(!serde_json::to_string(&view)
            .unwrap()
            .contains("disposable-key"));
        assert!(store.launch_snapshot(&changed).is_err());
        let mut update = request(1, None);
        update
            .values
            .insert("API_KEY".into(), json!("new ordinary value"));
        store.save(&changed, update).unwrap();
        let snapshot = store.launch_snapshot(&changed).unwrap();
        assert_eq!(snapshot.environment["API_KEY"], "new ordinary value");
        assert!(snapshot.redaction_values.is_empty());
    }

    #[test]
    fn removed_fields_are_hidden_and_never_injected_without_mutating_on_read() {
        let (temporary, _vault, store) = store();
        let mut original = request(0, Some("retired-secret-value"));
        original.values.insert("MODEL".into(), json!("two"));
        store.save(&manifest(), original).unwrap();
        let before = fs::read(temporary.path().join("configuration.json")).unwrap();
        let mut changed = manifest();
        changed
            .configuration
            .as_mut()
            .unwrap()
            .fields
            .retain(|field| field.name != "API_KEY" && field.name != "MODEL");
        let view = store.view(&changed).unwrap();
        assert!(view.can_start);
        assert!(!view.saved_values.contains_key("MODEL"));
        assert!(!view.secrets.contains_key("API_KEY"));
        let snapshot = store.launch_snapshot(&changed).unwrap();
        assert!(!snapshot.environment.contains_key("API_KEY"));
        assert!(!snapshot.environment.contains_key("MODEL"));
        assert_eq!(
            fs::read(temporary.path().join("configuration.json")).unwrap(),
            before
        );
        store.save(&changed, request(1, None)).unwrap();
        let record = store.read().unwrap();
        assert!(!record.values.contains_key("MODEL"));
        assert!(!record.secrets.contains_key("API_KEY"));
    }

    #[test]
    fn incompatible_saved_value_requires_explicit_save_without_coercion() {
        let (_temporary, _vault, store) = store();
        let mut original = request(0, Some("artificial-key"));
        original.values.insert("COUNT".into(), json!(7));
        store.save(&manifest(), original).unwrap();
        let mut changed = manifest();
        let field = &mut changed.configuration.as_mut().unwrap().fields[3];
        field.r#type = ConfigurationType::String;
        field.default = None;
        let view = store.view(&changed).unwrap();
        assert_eq!(view.invalid_fields, ["COUNT"]);
        assert!(!view.saved_values.contains_key("COUNT"));
        assert!(!view.can_start);
        assert!(store.launch_snapshot(&changed).is_err());
        let mut update = request(1, None);
        update.values.insert("COUNT".into(), json!("seven"));
        store.save(&changed, update).unwrap();
        assert_eq!(
            store.launch_snapshot(&changed).unwrap().environment["COUNT"],
            "seven"
        );
    }

    #[test]
    fn ordinary_values_do_not_become_credentials_when_declaration_changes() {
        let (_temporary, _vault, store) = store();
        let mut original = request(0, Some("artificial-key"));
        original.values.insert("COUNT".into(), json!(7));
        store.save(&manifest(), original).unwrap();
        let mut changed = manifest();
        let field = &mut changed.configuration.as_mut().unwrap().fields[3];
        field.r#type = ConfigurationType::String;
        field.secret = true;
        field.required = true;
        field.default = None;
        let view = store.view(&changed).unwrap();
        assert_eq!(view.invalid_fields, ["COUNT"]);
        assert_eq!(view.secrets["COUNT"], SecretStatus::Missing);
        assert!(!view.saved_values.contains_key("COUNT"));
        assert!(store.launch_snapshot(&changed).is_err());
        let mut update = request(1, None);
        update.secrets.insert(
            "COUNT".into(),
            SecretAction::Replace {
                value: "explicit-new-key".into(),
            },
        );
        store.save(&changed, update).unwrap();
        let snapshot = store.launch_snapshot(&changed).unwrap();
        assert_eq!(snapshot.environment["COUNT"], "explicit-new-key");
        assert!(snapshot
            .redaction_values
            .contains(&"explicit-new-key".to_owned()));
    }

    #[test]
    fn request_is_strict_and_has_no_secret_echo_path() {
        assert!(serde_json::from_value::<SaveConfigurationRequest>(json!({
            "revision":0,"values":{},"secrets":{"API_KEY":{"action":"keep","value":"extra"}}
        }))
        .is_err());
        assert!(serde_json::from_value::<SaveConfigurationRequest>(json!({
            "revision":0,"values":{},"unknown":"extra"
        }))
        .is_err());
    }

    mod file_tests {
        include!("node_app_configuration_file_tests.rs");
    }
    #[cfg(windows)]
    mod windows_file_tests {
        include!("node_app_configuration/windows_tests.rs");
    }
}
