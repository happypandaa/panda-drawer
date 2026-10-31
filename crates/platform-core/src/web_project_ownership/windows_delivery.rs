//! Windows delivery decisions are immutable and bound by the existing owner
//! index before any external rename. An old user file is never a project-owned
//! cleanup target until the approved replacement has actually been verified.
use super::*;
use platform_native::windows::files as native;
use serde_json::json;
use std::sync::{Mutex, OnceLock};

#[path = "windows_delivery/io.rs"]
mod io;
#[path = "windows_delivery/transaction.rs"]
mod transaction;
use io::*;

static DELIVERY_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
const CONTAINER: &str = "native-deliveries";
const INTENT: &str = "intent.json";
const INTENT_PREPARING: &str = ".intent-preparing";
const COMMITTED: &str = "committed.json";
const DECISION_PREPARING: &str = ".decision-preparing";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Binding {
    owner_identity: PersistedIdentity,
    directory_identity: PersistedIdentity,
    plan: Plan,
    controls: Option<Controls>,
    completed: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Plan {
    pub(crate) task: String,
    pub(crate) stage: PathBuf,
    pub(crate) destination: PathBuf,
    pub(crate) parent_identity: PersistedIdentity,
    pub(crate) payload: PersistedIdentity,
    pub(crate) selected: Option<PersistedIdentity>,
}
impl Plan {
    fn held(&self) -> PathBuf {
        self.stage
            .with_file_name(format!(".export-{}.held", self.task))
    }
    fn validate(&self) -> Result<()> {
        task_id(&self.task)?;
        validate_export_stage(&self.task, &self.stage)?;
        absolute(&self.destination)?;
        if self.stage.parent() != self.destination.parent()
            || self.stage == self.destination
            || self.held() == self.destination
        {
            return Err(failure("原生文件交付路径关系无效。"));
        }
        for id in std::iter::once(&self.parent_identity)
            .chain(std::iter::once(&self.payload))
            .chain(self.selected.iter())
        {
            full_identity(id)?;
            if id.device != self.parent_identity.device {
                return Err(failure("原生文件交付材料不在已绑定的目标卷。"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Controls {
    intent_identity: PersistedIdentity,
    intent_sha256: String,
    decision_identity: PersistedIdentity,
    decision_sha256: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Intent<'a> {
    version: u32,
    app_id: &'a str,
    directory_identity: &'a PersistedIdentity,
    plan: &'a Plan,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Decision<'a> {
    version: u32,
    app_id: &'a str,
    task: &'a str,
    intent_identity: &'a PersistedIdentity,
    intent_sha256: &'a str,
}

fn failure(message: impl Into<String>) -> ApiError {
    ApiError::new("DOWNLOAD_FAILED", message).outcome("unknown")
}
fn cause(issue: impl std::fmt::Display) -> ApiError {
    failure(issue.to_string())
}
fn full_identity(value: &PersistedIdentity) -> Result<()> {
    if value.windows_file_id.is_none() || value.volume_uuid.is_some() {
        return Err(failure("原生文件交付缺少完整 Windows 身份。"));
    }
    Ok(())
}
fn directory(owner: &WebProjectOwnership, id: &str, task: &str) -> Result<PathBuf> {
    task_id(task)?;
    Ok(owner.directory(id)?.join(CONTAINER).join(task))
}
fn intent_bytes(id: &str, binding: &Binding) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&Intent {
        version: 1,
        app_id: id,
        directory_identity: &binding.directory_identity,
        plan: &binding.plan,
    })?)
}
fn decision_bytes(id: &str, binding: &Binding) -> Result<Vec<u8>> {
    let controls = binding
        .controls
        .as_ref()
        .ok_or_else(|| failure("交付决定材料尚未绑定。"))?;
    Ok(serde_json::to_vec(&Decision {
        version: 1,
        app_id: id,
        task: &binding.plan.task,
        intent_identity: &controls.intent_identity,
        intent_sha256: &controls.intent_sha256,
    })?)
}
fn digest(value: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value))
}

pub(super) fn validate_binding(task: &str, binding: &Binding) -> Result<()> {
    binding.plan.validate()?;
    full_identity(&binding.directory_identity)?;
    full_identity(&binding.owner_identity)?;
    if binding.plan.task != task {
        return Err(failure("交付记录与任务不符。"));
    }
    if let Some(controls) = &binding.controls {
        full_identity(&controls.intent_identity)?;
        full_identity(&controls.decision_identity)?;
        for hash in [&controls.intent_sha256, &controls.decision_sha256] {
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(failure("交付控制材料摘要无效。"));
            }
        }
    }
    Ok(())
}

pub(crate) fn publish(owner: &WebProjectOwnership, id: &str, plan: Plan) -> Result<()> {
    let _lock = DELIVERY_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    transaction::publish(owner, id, plan, |_| Ok(()))
}
pub(super) fn recover_project(owner: &WebProjectOwnership, id: &str) -> Result<()> {
    let _lock = DELIVERY_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let bindings = owner.read(id)?.native_deliveries;
    for (task, binding) in bindings {
        transaction::recover(owner, id, &task, &binding)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "windows_delivery/tests.rs"]
mod tests;
