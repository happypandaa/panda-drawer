//! Typed, non-secret browser configuration reuses the existing configuration engine.
use crate::{
    node_app_configuration::{ConfigurationView, NodeConfigurationStore, SaveConfigurationRequest},
    panda_manifest::{ConfigurationField, PandaRuntime},
    ApiError, ProjectHandle, ProjectStore,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
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
    pub runtime: PandaRuntime,
    pub name: String,
    pub manifest_digest: String,
    pub fields: Vec<ConfigurationField>,
    pub configuration: WebConfigurationState,
}
fn error(e: impl std::fmt::Display) -> ApiError {
    ApiError::new("WEB_CONFIGURATION_INVALID", e.to_string())
}
fn store(project: &ProjectHandle) -> Result<NodeConfigurationStore, ApiError> {
    project.check()?;
    if project.manifest.runtime != PandaRuntime::Web {
        return Err(ApiError::new(
            "APP_RUNTIME_CHANGED",
            "应用类型变了，请删除后重新添加。",
        ));
    }
    let instance = project
        .id
        .strip_prefix("prj_")
        .and_then(|id| Uuid::parse_str(id).ok())
        .filter(|id| format!("prj_{}", id.simple()) == project.id)
        .ok_or_else(|| error("网页配置实例标识无效。"))?;
    let directory = project.private_dir.join("configuration");
    // Node registration creates this private leaf up front. Web registration is
    // lazy, so the first read must prepare the same control directory before
    // entering the shared store; it never writes into the author directory.
    crate::project_file_io::ensure_directory(&directory).map_err(error)?;
    NodeConfigurationStore::new_web(directory, instance).map_err(error)
}
fn view(project: &ProjectHandle) -> Result<ConfigurationView, ApiError> {
    store(project)?.view(&project.manifest).map_err(error)
}
fn detail(project: &ProjectHandle, view: ConfigurationView) -> WebConfigurationDetail {
    WebConfigurationDetail {
        id: project.id.clone(),
        runtime: PandaRuntime::Web,
        name: project.manifest.app.name.clone(),
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
            secret_status: BTreeMap::new(),
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
        if !input.secrets.is_empty() {
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
