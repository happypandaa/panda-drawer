//! One typed application configuration store. Only launchers receive secrets.
use crate::{
    node_app_configuration::{
        ConfigurationView, LocalFileVault, NodeConfigurationStore, SaveConfigurationRequest,
    },
    panda_manifest::{ConfigurationField, Localization},
    ApiError, ProjectHandle, ProjectStore,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebConfigurationState {
    pub revision: u64,
    pub values: BTreeMap<String, Value>,
    pub secret_status: BTreeMap<String, Value>,
    pub missing_required: Vec<String>,
    pub invalid_fields: Vec<String>,
    pub can_start: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebConfigurationDetail {
    pub id: String,
    pub has_backend: bool,
    pub name: String,
    pub localization: Option<Localization>,
    pub manifest_digest: String,
    pub fields: Vec<ConfigurationField>,
    pub configuration: WebConfigurationState,
}
fn error(e: impl std::fmt::Display) -> ApiError {
    ApiError::new("WEB_CONFIGURATION_INVALID", e.to_string())
}
pub(crate) fn store(project: &ProjectHandle) -> Result<NodeConfigurationStore, ApiError> {
    project.check()?;
    let instance = Uuid::parse_str(&project.id)
        .ok()
        .filter(|id| id.to_string() == project.id)
        .ok_or_else(|| error("网页配置实例标识无效。"))?;
    let directory = project.private_dir.join("configuration");
    // Configuration lives in the private instance, outside the author directory.
    crate::project_file_io::ensure_directory(&directory).map_err(error)?;
    let vault =
        LocalFileVault::new(project.private_dir.join("secrets"), instance).map_err(error)?;
    NodeConfigurationStore::new(directory, instance, Arc::new(vault)).map_err(error)
}
fn view(project: &ProjectHandle) -> Result<ConfigurationView, ApiError> {
    store(project)?.view(&project.manifest).map_err(error)
}
fn detail(project: &ProjectHandle, view: ConfigurationView) -> WebConfigurationDetail {
    WebConfigurationDetail {
        id: project.id.clone(),
        has_backend: project.manifest.has_backend(),
        name: project.manifest.app.name.clone(),
        localization: project.manifest.localization.clone(),
        manifest_digest: project.manifest.source_digest.clone(),
        fields: project
            .manifest
            .configuration
            .as_ref()
            .map(|decl| decl.fields.clone())
            .unwrap_or_default(),
        configuration: WebConfigurationState {
            revision: view.revision,
            values: view.saved_values,
            secret_status: view
                .secrets
                .into_iter()
                .map(|(name, status)| (name, json!(status)))
                .collect(),
            missing_required: view.missing_required,
            invalid_fields: view.invalid_fields,
            can_start: view.can_start,
        },
    }
}
impl ProjectStore {
    pub fn web_configuration(&self, id: &str) -> Result<WebConfigurationDetail, ApiError> {
        let project = self.project(id)?;
        let _io = project
            .operation_gate
            .clone()
            .try_read_owned()
            .map_err(|_| ApiError::new("PROJECT_BUSY", "项目正在处理其他操作。"))?;
        project.validate_epoch(None)?;
        Ok(detail(&project, view(&project)?))
    }
    pub fn save_web_configuration(
        &self,
        id: &str,
        input: SaveConfigurationRequest,
    ) -> Result<WebConfigurationDetail, ApiError> {
        let project = self.project(id)?;
        let _io = project
            .operation_gate
            .clone()
            .try_read_owned()
            .map_err(|_| ApiError::new("PROJECT_BUSY", "项目正在处理其他操作。"))?;
        project.validate_epoch(None)?;
        if !project.manifest.has_backend() && !input.secrets.is_empty() {
            return Err(error("网页配置不支持密钥。"));
        }
        if input.manifest_digest.as_deref() != Some(project.manifest.source_digest.as_str()) {
            return Err(ApiError::new(
                "WEB_MANIFEST_CHANGED",
                "应用说明已变化，请重新读取配置后再保存。",
            ));
        }
        let saved = store(&project)?
            .save(&project.manifest, input)
            .map_err(error)?;
        Ok(detail(&project, saved))
    }
    pub fn web_configuration_values(
        &self,
        project: &ProjectHandle,
    ) -> Result<BTreeMap<String, Value>, ApiError> {
        Ok(view(project)?.values)
    }
    pub fn require_web_configuration_ready(&self, project: &ProjectHandle) -> Result<(), ApiError> {
        let current = view(project)?;
        if !current.can_start {
            return Err(
                ApiError::new("WEB_CONFIGURATION_REQUIRED", "请先完成应用配置后再打开。")
                    .detail("missingRequired", json!(current.missing_required))
                    .detail("invalidFields", json!(current.invalid_fields)),
            );
        }
        Ok(())
    }
    pub(crate) fn web_configuration_can_start(
        &self,
        project: &ProjectHandle,
    ) -> Result<bool, ApiError> {
        Ok(view(project)?.can_start)
    }
}
