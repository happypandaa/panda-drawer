//! Configuration for the separately registered, local Node application slots.
//! Explicitly requested local-file credentials use immutable private files.
//! Configuration JSON contains only ordinary values and opaque entry IDs;
//! credentials are never serialized back to the frontend or placed in logs.

use crate::node_app_manifest::{
    valid_business_variable, ConfigurationField, ConfigurationType, TrialManifest,
};
#[cfg(not(windows))]
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
#[cfg(any(test, not(windows)))]
use std::fs;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(not(windows))]
use std::{
    fs::OpenOptions,
    io::{Read, Write},
};
use uuid::Uuid;

const MAX_RECORD_BYTES: u64 = 256 * 1024;
const MAX_SECRET_BYTES: usize = 8192;

#[derive(Debug)]
pub struct ConfigurationError(&'static str, bool);
impl fmt::Display for ConfigurationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ConfigurationError {}
impl ConfigurationError {
    pub(crate) fn into_api_error(self) -> crate::ApiError {
        if self.1 {
            crate::ApiError::new("NODE_CONFIGURATION_COMMIT_UNKNOWN", self.0).outcome("unknown")
        } else {
            crate::ApiError::new("NODE_APP_ERROR", self.0)
        }
    }
}
type Result<T> = std::result::Result<T, ConfigurationError>;
fn invalid(message: &'static str) -> ConfigurationError {
    ConfigurationError(message, false)
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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveConfigurationRequest {
    pub revision: u64,
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
    pub can_start: bool,
}

/// Only the process launcher receives this value, never a frontend response.
pub struct LaunchConfigurationSnapshot {
    pub revision: u64,
    pub environment: BTreeMap<String, String>,
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

pub struct NodeConfigurationStore {
    #[cfg_attr(windows, allow(dead_code))]
    directory: PathBuf,
    instance: Uuid,
    vault: Arc<dyn SecretVault>,
    #[cfg(windows)]
    windows: platform_host::WindowsSourceDirectory,
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
            #[cfg(windows)]
            windows: platform_host::WindowsSourceDirectory::open_private(&directory)
                .map_err(|_| invalid("应用配置目录权限无效。"))?,
            directory,
            instance,
            vault,
        })
    }

    pub fn view(&self, manifest: &TrialManifest) -> Result<ConfigurationView> {
        let _lock = self.lock()?;
        let record = self.read()?;
        self.view_record(manifest, &record)
    }

    pub fn save(
        &self,
        manifest: &TrialManifest,
        request: SaveConfigurationRequest,
    ) -> Result<ConfigurationView> {
        let _lock = self.lock()?;
        let previous = self.read()?;
        if previous.revision != request.revision {
            return Err(invalid("配置已被其他窗口更新，请重新读取后再保存。"));
        }
        let fields = checked_fields(manifest)?;
        validate_record_fields(&fields, &previous)?;
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
            // An immutable revision can already have reached the native rename
            // commit point when its post-verification fails. Preserve every new
            // credential on an uncertain Windows commit; it may now be bound.
            #[cfg(windows)]
            if error.1 {
                return Err(error);
            }
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

    pub fn launch_snapshot(&self, manifest: &TrialManifest) -> Result<LaunchConfigurationSnapshot> {
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
        let fields = checked_fields(manifest)?;
        let mut environment = BTreeMap::new();
        for (name, value) in &view.values {
            environment.insert(name.clone(), environment_value(value)?);
        }
        for field in fields {
            if let Some(binding) = record.secrets.get(&field.name) {
                let credential = self.vault.get(self.instance, entry_uuid(binding)?)?;
                validate_secret(&credential)?;
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
        })
    }

    fn view_record(
        &self,
        manifest: &TrialManifest,
        record: &StoredConfiguration,
    ) -> Result<ConfigurationView> {
        let fields = checked_fields(manifest)?;
        validate_record_fields(&fields, record)?;
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
            saved_values: record.values.clone(),
            secrets,
            can_start: missing_required.is_empty() && !binding_changed && !binding_missing,
            missing_required,
        })
    }

    fn lock(&self) -> Result<File> {
        #[cfg(windows)]
        {
            self.windows
                .lock_private("configuration.lock")
                .map_err(|_| invalid("无法锁定应用私有配置。"))
        }
        #[cfg(not(windows))]
        {
            checked_directory(&self.directory)?;
            let mut options = safe_options();
            options.read(true).write(true).create(true);
            let file = options
                .open(self.directory.join("configuration.lock"))
                .map_err(|_| invalid("无法锁定应用配置。"))?;
            checked_file(&file)?;
            file.lock_exclusive()
                .map_err(|_| invalid("无法锁定应用配置。"))?;
            Ok(file)
        }
    }

    #[cfg(not(windows))]
    fn read(&self) -> Result<StoredConfiguration> {
        let mut options = safe_options();
        options.read(true);
        let file = match options.open(self.directory.join("configuration.json")) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(StoredConfiguration {
                    version: 1,
                    instance_id: self.instance.to_string(),
                    revision: 0,
                    values: BTreeMap::new(),
                    secrets: BTreeMap::new(),
                })
            }
            Err(_) => return Err(invalid("无法读取应用配置。")),
        };
        checked_file(&file)?;
        if file
            .metadata()
            .map_err(|_| invalid("无法读取应用配置。"))?
            .len()
            > MAX_RECORD_BYTES
        {
            return Err(invalid("应用配置文件超过大小限制。"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_RECORD_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid("无法读取应用配置。"))?;
        if bytes.len() > MAX_RECORD_BYTES as usize {
            return Err(invalid("应用配置文件超过大小限制。"));
        }
        let record: StoredConfiguration =
            serde_json::from_slice(&bytes).map_err(|_| invalid("应用配置记录格式无效。"))?;
        if record.version != 1 || record.instance_id != self.instance.to_string() {
            return Err(invalid("应用配置与当前实例不匹配。"));
        }
        Ok(record)
    }

    #[cfg(not(windows))]
    fn commit(&self, record: &StoredConfiguration) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(record).map_err(|_| invalid("无法编码应用配置。"))?;
        if bytes.len() > MAX_RECORD_BYTES as usize {
            return Err(invalid("应用配置超过大小限制。"));
        }
        let temporary = self
            .directory
            .join(format!(".configuration-{}.tmp", Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = safe_options();
            options.write(true).create_new(true);
            let mut file = options
                .open(&temporary)
                .map_err(|_| invalid("无法准备应用配置保存。"))?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| invalid("无法写入应用配置，原配置已保留。"))?;
            fs::rename(&temporary, self.directory.join("configuration.json"))
                .map_err(|_| invalid("无法提交应用配置，原配置已保留。"))?;
            // The successful rename is the commit point. A directory fsync is a
            // best-effort power-loss durability improvement, not a second commit.
            if let Ok(directory) = File::open(&self.directory) {
                let _ = directory.sync_all();
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    #[cfg(windows)]
    fn read(&self) -> Result<StoredConfiguration> {
        let entries = self
            .windows
            .list()
            .map_err(|_| invalid("无法检查应用配置现场。"))?;
        for entry in &entries {
            if !matches!(
                (entry.name.as_str(), entry.is_directory),
                ("configuration.lock", false) | ("credentials", true) | ("revisions", true)
            ) {
                return Err(invalid("配置目录包含未知或未完成的现场，已保留。"));
            }
        }
        let empty = || StoredConfiguration {
            version: 1,
            instance_id: self.instance.to_string(),
            revision: 0,
            values: BTreeMap::new(),
            secrets: BTreeMap::new(),
        };
        if !entries.iter().any(|entry| entry.name == "revisions") {
            return Ok(empty());
        }
        let revisions = self
            .windows
            .open_directory("revisions")
            .map_err(|_| invalid("配置修订目录不可读取。"))?;
        let mut records = BTreeMap::new();
        for entry in revisions
            .list()
            .map_err(|_| invalid("配置修订目录不可读取。"))?
        {
            // A bounded, unreferenced preparation is retained evidence, never a
            // committed revision or permission to discard the previous value.
            if !entry.is_directory
                && entry
                    .name
                    .strip_prefix(".configuration-")
                    .and_then(|v| v.strip_suffix(".tmp"))
                    .is_some_and(|id| Uuid::parse_str(id).is_ok())
            {
                continue;
            }
            let number = entry
                .name
                .strip_prefix("revision-")
                .and_then(|value| value.strip_suffix(".json"))
                .filter(|value| value.len() == 20 && value.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| invalid("发现未知配置修订，已保留。"))?;
            if entry.is_directory {
                return Err(invalid("配置修订不能是目录。"));
            }
            let bytes = revisions
                .read_bounded(&entry.name, MAX_RECORD_BYTES as usize)
                .map_err(|_| invalid("配置修订读取失败，已保留。"))?
                .bytes;
            let record: StoredConfiguration =
                serde_json::from_slice(&bytes).map_err(|_| invalid("配置修订格式无效。"))?;
            if record.version != 1
                || record.instance_id != self.instance.to_string()
                || record.revision != number
            {
                return Err(invalid("配置修订与当前应用实例不符。"));
            }
            records.insert(number, record);
        }
        for (index, revision) in records.keys().enumerate() {
            if *revision != index as u64 + 1 {
                return Err(invalid("应用配置修订不连续，已保留现场。"));
            }
        }
        Ok(records
            .pop_last()
            .map(|(_, record)| record)
            .unwrap_or_else(empty))
    }

    #[cfg(windows)]
    fn commit(&self, record: &StoredConfiguration) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(record).map_err(|_| invalid("无法编码应用配置。"))?;
        if bytes.len() > MAX_RECORD_BYTES as usize {
            return Err(invalid("应用配置超过大小限制。"));
        }
        let revisions = match self.windows.open_directory("revisions") {
            Ok(directory) => directory,
            Err(error) if error.code == "WORKSPACE_NAME_MISSING" => self
                .windows
                .create_private_directory("revisions")
                .map_err(|_| invalid("无法创建私有配置修订目录。"))?,
            Err(_) => return Err(invalid("配置修订目录无效，已保留。")),
        };
        let temporary = format!(".configuration-{}.tmp", Uuid::new_v4());
        let identity = revisions
            .write_new(&temporary, &bytes)
            .map_err(|_| invalid("无法准备配置修订，原配置已保留。"))?;
        if let Err(error) = revisions.publish_noreplace_checked(
            &temporary,
            &format!("revision-{:020}.json", record.revision),
            &identity,
        ) {
            if error.code == "NODE_PUBLICATION_COMMITTED_UNVERIFIED" {
                return Err(ConfigurationError(
                    "配置提交结果需要核对；已保留全部配置修订与新密钥，请重新读取后处理。",
                    true,
                ));
            }
            let _ = revisions.remove_file_checked(&temporary, &identity);
            return Err(invalid("无法提交配置修订，原配置已保留。"));
        }
        Ok(())
    }
}

fn checked_fields(manifest: &TrialManifest) -> Result<Vec<&ConfigurationField>> {
    // The manifest's typed declarations are public. Revalidate by round-tripping
    // just the declaration through its existing strict reader is unnecessary:
    // enforce each launch-relevant rule locally, including mutated Rust values.
    if manifest.runtime != crate::node_app_manifest::TrialRuntime::NodeService {
        return Err(invalid("此配置入口仅支持 Node 服务应用。"));
    }
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

fn validate_record_fields(
    fields: &[&ConfigurationField],
    record: &StoredConfiguration,
) -> Result<()> {
    validate_ordinary(fields, &record.values)?;
    for (name, binding) in &record.secrets {
        if !fields
            .iter()
            .any(|field| field.name == *name && field.secret)
        {
            return Err(invalid("已保存的密钥绑定与应用声明不匹配。"));
        }
        entry_uuid(binding)?;
        if binding
            .endpoint
            .as_ref()
            .is_some_and(|value| !valid_url(value))
        {
            return Err(invalid("已保存的密钥地址绑定无效。"));
        }
    }
    Ok(())
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

#[cfg(windows)]
fn checked_directory(path: &Path) -> Result<()> {
    platform_host::WindowsSourceDirectory::open_private(path)
        .map(|_| ())
        .map_err(|_| invalid("应用配置目录必须为当前用户私有普通目录。"))
}
#[cfg(not(windows))]
fn checked_directory(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(invalid("应用配置目录必须为绝对普通目录。"));
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        let metadata =
            fs::symlink_metadata(&current).map_err(|_| invalid("应用配置目录不可读取。"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid("应用配置目录不得经过链接或特殊文件。"));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::metadata(path).map_err(|_| invalid("应用配置目录不可读取。"))?;
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(invalid("应用配置目录不属于当前用户。"));
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| invalid("无法保护应用配置目录。"))?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn safe_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options
}

#[cfg(not(windows))]
fn checked_file(file: &File) -> Result<()> {
    let metadata = file
        .metadata()
        .map_err(|_| invalid("应用配置文件不可读取。"))?;
    if !metadata.is_file() {
        return Err(invalid("应用配置不得使用特殊文件。"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.nlink() != 1 || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(invalid("应用配置不得使用硬链接或其他用户的文件。"));
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| invalid("无法保护应用配置文件。"))?;
    }
    Ok(())
}

/// Local plaintext credential storage, selected explicitly by the user. This
/// implementation never calls an OS credential service and never falls back to
/// a different provider. Each instance gets its own private directory.
pub struct LocalFileVault {
    #[cfg_attr(windows, allow(dead_code))]
    directory: PathBuf,
    instance: Uuid,
    created: std::sync::Mutex<BTreeMap<Uuid, CredentialIdentity>>,
    #[cfg(windows)]
    windows: platform_host::WindowsSourceDirectory,
}
#[cfg(windows)]
type CredentialIdentity = platform_host::WindowsObjectIdentity;
#[cfg(not(windows))]
type CredentialIdentity = (u64, u64);

impl LocalFileVault {
    #[cfg(not(windows))]
    pub fn new(directory: PathBuf, instance: Uuid) -> Result<Self> {
        if !cfg!(unix) {
            return Err(invalid("当前平台尚未验证本地密钥文件权限。"));
        }
        if instance.is_nil() || !directory.is_absolute() {
            return Err(invalid("本地密钥目录或应用实例无效。"));
        }
        let parent = directory
            .parent()
            .ok_or_else(|| invalid("本地密钥目录无效。"))?;
        checked_directory(parent)?;
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(invalid("无法创建本地密钥目录。")),
        }
        checked_credential_directory(&directory)?;
        Ok(Self {
            directory,
            instance,
            created: std::sync::Mutex::new(BTreeMap::new()),
        })
    }

    #[cfg(windows)]
    pub fn new(directory: PathBuf, instance: Uuid) -> Result<Self> {
        if instance.is_nil() {
            return Err(invalid("本地密钥应用实例无效。"));
        }
        let windows = crate::windows_fs::create_private_runtime_dir(&directory)
            .map_err(|_| invalid("本地密钥目录必须为当前用户私有普通目录。"))?;
        Ok(Self {
            directory,
            instance,
            created: std::sync::Mutex::new(BTreeMap::new()),
            windows,
        })
    }
    #[cfg(windows)]
    pub fn open_existing(directory: PathBuf, instance: Uuid) -> Result<Self> {
        if instance.is_nil() {
            return Err(invalid("本地密钥应用实例无效。"));
        }
        let windows = platform_host::WindowsSourceDirectory::open_private(&directory)
            .map_err(|_| invalid("本地密钥目录缺失或不再是当前用户私有目录，已保留现场。"))?;
        Ok(Self {
            directory,
            instance,
            created: std::sync::Mutex::new(BTreeMap::new()),
            windows,
        })
    }

    #[cfg(not(windows))]
    fn entry_path(&self, instance: Uuid, entry: Uuid) -> Result<PathBuf> {
        if instance != self.instance || entry.is_nil() {
            return Err(invalid("本地密钥条目不属于此应用实例。"));
        }
        checked_credential_directory(&self.directory)?;
        Ok(self.directory.join(entry.to_string()))
    }

    #[cfg(not(windows))]
    fn open_entry(&self, instance: Uuid, entry: Uuid) -> Result<Option<File>> {
        let path = self.entry_path(instance, entry)?;
        let mut options = safe_options();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Inspecting a malicious special file must not block on a FIFO.
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
        }
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(invalid("无法读取本地密钥文件，请重新填写密钥。")),
        };
        checked_credential_file(&file)?;
        Ok(Some(file))
    }
}

#[cfg(not(windows))]
fn checked_credential_directory(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(invalid("本地密钥目录必须为绝对路径。"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        if !matches!(
            component,
            std::path::Component::RootDir | std::path::Component::Normal(_)
        ) {
            return Err(invalid("本地密钥目录必须为规范路径。"));
        }
        current.push(component);
        let metadata =
            fs::symlink_metadata(&current).map_err(|_| invalid("无法检查本地密钥目录。"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid("本地密钥目录不能经过链接或特殊文件。"));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(path).map_err(|_| invalid("无法检查本地密钥目录。"))?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o7777 != 0o700 {
            return Err(invalid("本地密钥目录必须属于当前用户且权限为 0700。"));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        Err(invalid("当前平台尚未验证本地密钥目录权限。"))
    }
}

#[cfg(not(windows))]
fn checked_credential_file(file: &File) -> Result<(u64, u64)> {
    let metadata = file
        .metadata()
        .map_err(|_| invalid("无法检查本地密钥文件。"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SECRET_BYTES as u64 {
        return Err(invalid("本地密钥文件类型或大小无效，请重新填写密钥。"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o7777 != 0o600
        {
            return Err(invalid(
                "本地密钥文件必须属于当前用户、权限为 0600，且不能使用链接。",
            ));
        }
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        Err(invalid("当前平台尚未验证本地密钥文件权限。"))
    }
}

#[cfg(not(windows))]
impl SecretVault for LocalFileVault {
    fn insert(&self, instance: Uuid, entry: Uuid, value: &str) -> Result<()> {
        validate_secret(value)?;
        let path = self.entry_path(instance, entry)?;
        let mut options = safe_options();
        options.write(true).create_new(true);
        let mut file = options
            .open(&path)
            .map_err(|_| invalid("无法新建本地密钥文件，原配置已保留。"))?;
        let written = file
            .write_all(value.as_bytes())
            .and_then(|_| file.sync_all());
        if written.is_err() {
            drop(file);
            // create_new succeeded: this path was created by this operation.
            let _ = fs::remove_file(&path);
            return Err(invalid("无法写入本地密钥文件，原配置已保留。"));
        }
        let identity = match checked_credential_file(&file) {
            Ok(identity) => identity,
            Err(error) => {
                drop(file);
                let _ = fs::remove_file(&path);
                return Err(error);
            }
        };
        if File::open(&self.directory)
            .and_then(|directory| directory.sync_all())
            .is_err()
        {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(invalid("无法完成本地密钥文件保存，原配置已保留。"));
        }
        self.created
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(entry, identity);
        Ok(())
    }

    fn get(&self, instance: Uuid, entry: Uuid) -> Result<String> {
        let file = self
            .open_entry(instance, entry)?
            .ok_or_else(|| invalid("本地密钥文件缺失，请重新填写并保存密钥。"))?;
        let mut bytes = Vec::new();
        file.take((MAX_SECRET_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid("无法读取本地密钥文件，请重新填写密钥。"))?;
        if bytes.len() > MAX_SECRET_BYTES {
            return Err(invalid("本地密钥文件超过大小限制。"));
        }
        let value = String::from_utf8(bytes)
            .map_err(|_| invalid("本地密钥文件格式无效，请重新填写密钥。"))?;
        validate_secret(&value)?;
        Ok(value)
    }

    fn discard(&self, instance: Uuid, entry: Uuid) -> Result<()> {
        let mut created = self.created.lock().unwrap_or_else(|p| p.into_inner());
        let expected = created
            .get(&entry)
            .ok_or_else(|| invalid("只能清理本次新建的密钥文件。"))?;
        if let Some(file) = self.open_entry(instance, entry)? {
            if checked_credential_file(&file)? != *expected {
                return Err(invalid("密钥文件已变化，已保留现场。"));
            }
            fs::remove_file(self.entry_path(instance, entry)?)
                .map_err(|_| invalid("无法清理本次未提交的密钥文件。"))?;
        }
        created.remove(&entry);
        if let Ok(directory) = File::open(&self.directory) {
            let _ = directory.sync_all();
        }
        Ok(())
    }

    fn contains(&self, instance: Uuid, entry: Uuid) -> Result<bool> {
        self.open_entry(instance, entry).map(|file| file.is_some())
    }
}

#[cfg(windows)]
impl LocalFileVault {
    fn windows_entry(
        &self,
        instance: Uuid,
        entry: Uuid,
    ) -> Result<Option<platform_host::WorkspaceFile>> {
        if instance != self.instance || entry.is_nil() {
            return Err(invalid("密钥条目不属于此应用实例。"));
        }
        match self
            .windows
            .read_bounded(&entry.to_string(), MAX_SECRET_BYTES)
        {
            Ok(file) if !file.bytes.is_empty() => Ok(Some(file)),
            Ok(_) => Err(invalid("本地密钥文件无效。")),
            Err(error) if error.code == "WORKSPACE_NAME_MISSING" => Ok(None),
            Err(_) => Err(invalid("本地密钥文件权限、身份或内容无效。")),
        }
    }
}
#[cfg(windows)]
impl SecretVault for LocalFileVault {
    fn insert(&self, instance: Uuid, entry: Uuid, value: &str) -> Result<()> {
        if instance != self.instance || entry.is_nil() {
            return Err(invalid("密钥条目不属于此应用实例。"));
        }
        validate_secret(value)?;
        let identity = self
            .windows
            .write_new(&entry.to_string(), value.as_bytes())
            .map_err(|_| invalid("无法新建本地私有密钥文件。"))?;
        self.created
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(entry, identity);
        Ok(())
    }
    fn get(&self, instance: Uuid, entry: Uuid) -> Result<String> {
        let file = self
            .windows_entry(instance, entry)?
            .ok_or_else(|| invalid("本地密钥文件缺失，请重新填写。"))?;
        let value = String::from_utf8(file.bytes).map_err(|_| invalid("本地密钥文件格式无效。"))?;
        validate_secret(&value)?;
        Ok(value)
    }
    fn discard(&self, instance: Uuid, entry: Uuid) -> Result<()> {
        if instance != self.instance || entry.is_nil() {
            return Err(invalid("密钥条目不属于此应用实例。"));
        }
        let mut created = self.created.lock().unwrap_or_else(|p| p.into_inner());
        let expected = created
            .get(&entry)
            .ok_or_else(|| invalid("只能清理本次新建的密钥文件。"))?;
        if self.windows_entry(instance, entry)?.is_some() {
            self.windows
                .remove_file_checked(&entry.to_string(), expected)
                .map_err(|_| invalid("密钥文件已变化或被占用，已保留。"))?;
        }
        created.remove(&entry);
        Ok(())
    }
    fn contains(&self, instance: Uuid, entry: Uuid) -> Result<bool> {
        self.windows_entry(instance, entry)
            .map(|file| file.is_some())
    }
}

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

    fn manifest() -> TrialManifest {
        TrialManifest::parse(&serde_json::to_vec(&json!({
            "packageFormat":"local-web-app", "formatVersion":2, "runtime":"node-service",
            "app":{"id":"dev.disposable.config","name":"可丢弃配置测试","version":"1"},
            "web":{"root":"public","entry":"index.html","routing":"files"},
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

    #[cfg(not(windows))]
    type TestDirectory = tempfile::TempDir;
    #[cfg(windows)]
    struct TestDirectory(PathBuf);
    #[cfg(windows)]
    impl TestDirectory {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    fn test_directory() -> TestDirectory {
        #[cfg(windows)]
        {
            let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join(".test-artifacts");
            fs::create_dir_all(&parent).unwrap();
            let path = parent.join(format!("windows-node-disposable-config-{}", Uuid::new_v4()));
            platform_host::disposable_node_directory(&path).unwrap();
            TestDirectory(path)
        }
        #[cfg(not(windows))]
        {
            tempfile::tempdir().unwrap()
        }
    }
    fn configuration_path(directory: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            let mut entries = fs::read_dir(directory.join("revisions"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("revision-")
                })
                .collect::<Vec<_>>();
            entries.sort();
            entries.pop().unwrap()
        }
        #[cfg(not(windows))]
        {
            directory.join("configuration.json")
        }
    }
    fn store() -> (TestDirectory, Arc<FakeVault>, NodeConfigurationStore) {
        let temporary = test_directory();
        let directory = temporary.path().canonicalize().unwrap();
        let vault = Arc::new(FakeVault::default());
        let store = NodeConfigurationStore::new(directory, Uuid::new_v4(), vault.clone()).unwrap();
        (temporary, vault, store)
    }

    fn request(revision: u64, key: Option<&str>) -> SaveConfigurationRequest {
        SaveConfigurationRequest {
            revision,
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
        let disk = fs::read_to_string(configuration_path(temporary.path())).unwrap();
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
        let before = fs::read(configuration_path(temporary.path())).unwrap();
        assert!(store
            .save(&manifest(), request(0, Some("second-dummy")))
            .is_err());
        assert_eq!(
            before,
            fs::read(configuration_path(temporary.path())).unwrap()
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
        let original = fs::read(configuration_path(temporary.path())).unwrap();
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
            fs::read(configuration_path(temporary.path())).unwrap()
        );
        assert_eq!(vault.values.lock().unwrap().len(), 1);
        assert_eq!(
            store.launch_snapshot(&manifest()).unwrap().environment["API_KEY"],
            "original-dummy"
        );
        vault.unavailable.store(true, Ordering::SeqCst);
        assert!(store.launch_snapshot(&manifest()).is_err());
        let disk = fs::read_to_string(configuration_path(temporary.path())).unwrap();
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
        let before = fs::read(configuration_path(temporary.path())).unwrap();
        *vault.make_directory_read_only.lock().unwrap() = Some(temporary.path().to_owned());
        let attempted = store.save(&manifest(), request(1, Some("replacement-not-committed")));
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(attempted.is_err());
        assert_eq!(vault.insertions.load(Ordering::SeqCst), 2);
        assert_eq!(vault.values.lock().unwrap().len(), 1);
        assert_eq!(
            before,
            fs::read(configuration_path(temporary.path())).unwrap()
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
            configuration_path(temporary.path()),
        )
        .unwrap();
        assert!(store.view(&manifest()).is_err());
        fs::remove_file(configuration_path(temporary.path())).unwrap();
        fs::hard_link(
            other.path().join("foreign"),
            configuration_path(temporary.path()),
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
        assert!(store.view(&changed).is_err());
        assert!(store.launch_snapshot(&changed).is_err());
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
}
