//! An explicit program-pointer rollback. Business data, configuration, author
//! source and credentials are never restored or rewritten by this operation.
use super::*;
use crate::{builds::atomic_json, file_identity::PersistedIdentity, ProjectStore};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RollbackNodeProgramInput {
    pub expected_revision: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviousProgram {
    version: u32,
    app_id: String,
    expected_generation: String,
    previous_registration: ManagedNodeRegistration,
    previous_registration_digest: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RollbackReceipt {
    version: u32,
    app_id: String,
    expected_revision: String,
    root_identity: PersistedIdentity,
    source_registration: ManagedNodeRegistration,
    source_registration_digest: String,
    target_registration: ManagedNodeRegistration,
    target_registration_digest: String,
}

struct Plan {
    revision: String,
    current: ManagedNodeRegistration,
    target: ManagedNodeRegistration,
    root_identity: PersistedIdentity,
    current_registration_digest: String,
    app_name: String,
    current_version: String,
    previous_version: String,
}

fn problem(message: &str) -> ApiError {
    ApiError::new("NODE_PROGRAM_ROLLBACK_UNAVAILABLE", message)
}
fn changed() -> ApiError {
    ApiError::new(
        "NODE_PROGRAM_ROLLBACK_CHANGED",
        "应用、旧程序或配置已变化，请重新核对退回范围。",
    )
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn encoded_digest(value: &impl Serialize) -> Result<String> {
    Ok(digest(&serde_json::to_vec_pretty(value)?))
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Only new managed-to-managed updates create this pointer. An immutable copy
/// keeps the preceding evidence when a later update replaces the latest pointer.
pub(crate) fn record_previous_program(
    slot: &Path,
    expected_generation: &str,
    previous: ManagedNodeRegistration,
) -> Result<()> {
    ordinary_path(slot, true)?;
    if previous.version != 2
        || previous.kind != "managed"
        || slot.file_name().and_then(|value| value.to_str()) != Some(previous.id.as_str())
        || previous.generation == expected_generation
    {
        return Err(changed());
    }
    let node_root = slot.parent().ok_or_else(changed)?;
    node_app_install::managed_program_path(node_root, &previous)?;
    let record = PreviousProgram {
        version: 1,
        app_id: previous.id.clone(),
        expected_generation: expected_generation.into(),
        previous_registration_digest: encoded_digest(&previous)?,
        previous_registration: previous,
    };
    let history = slot.join("program-history");
    private_dir(&history)?;
    let latest_path = slot.join("previous-program.json");
    crate::projects::check_no_symlinks(&latest_path, true)?;
    if latest_path.try_exists()? {
        let saved_bytes = read_small(&latest_path)?;
        let saved: PreviousProgram = serde_json::from_slice(&saved_bytes)?;
        if saved.version != 1
            || saved.app_id != record.app_id
            || saved.previous_registration.id != record.app_id
            || saved.previous_registration_digest != encoded_digest(&saved.previous_registration)?
        {
            return Err(changed());
        }
        let mut saved_target = saved.previous_registration.clone();
        saved_target.generation = saved.expected_generation.clone();
        node_app_install::managed_program_path(node_root, &saved_target)?;
        // An unexpected file at the new metadata path is never silently
        // adopted or erased to make an update proceed.
        if read_small(&history.join(format!("{}.json", saved.expected_generation)))? != saved_bytes
        {
            return Err(changed());
        }
    }
    // Generation names are validated before journal publication; validate the
    // prospective generation through the same path resolver as a registration.
    let mut next = record.previous_registration.clone();
    next.generation = expected_generation.into();
    node_app_install::managed_program_path(node_root, &next)?;
    let history_path = history.join(format!("{expected_generation}.json"));
    crate::projects::check_no_symlinks(&history_path, true)?;
    if history_path.try_exists()? {
        if read_small(&history_path)? != serde_json::to_vec_pretty(&record)? {
            return Err(changed());
        }
    } else {
        write_new_json(&history_path, &record)?;
    }
    atomic_json(&latest_path, &record)
}

impl NodeApps {
    fn rollback_plan(&self, id: &str, inner: &Inner) -> Result<Option<Plan>> {
        self.require_install_stopped(id, inner)?;
        let installer = node_app_install::NodeAppInstaller::new(self.data_root.clone())?;
        if installer.installation_states()?.iter().any(|state| {
            !state.is_terminal()
                && state.state.as_deref() != Some("prepared")
                && state.app_id.as_deref().is_none_or(|app| app == id)
        }) {
            return Err(problem("这个应用还有需要核对的更新记录，请先处理原任务。"));
        }
        let registration = self.registration(id)?;
        let Some(current) = registration.managed else {
            return Ok(None);
        };
        let slot = self.root.join(id);
        ordinary_path(&slot, true)?;
        let previous_path = slot.join("previous-program.json");
        crate::projects::check_no_symlinks(&previous_path, true)?;
        if !previous_path.try_exists()? {
            return Ok(None);
        }
        let previous_bytes = read_small(&previous_path)?;
        let previous: PreviousProgram = serde_json::from_slice(&previous_bytes)?;
        if previous.version != 1
            || previous.app_id != id
            || previous.previous_registration.id != id
            || previous.previous_registration.version != 2
            || previous.previous_registration.kind != "managed"
            || previous.previous_registration_digest
                != encoded_digest(&previous.previous_registration)?
        {
            return Err(problem("上一个程序版本的登记无法核对，不能退回。"));
        }
        if previous.expected_generation != current.generation {
            return Ok(None);
        }
        if previous.previous_registration.generation == current.generation {
            return Err(problem("当前程序与旧程序版本相同，没有可退回的版本。"));
        }
        // A broken current entry may be the reason the user is here. Only its
        // declared manifest must still match the saved registration. The old
        // target, which will actually run, is checked as a complete program.
        let current_path = node_app_install::managed_program_path(&self.root, &current)?;
        let current_manifest_bytes = read_small(&current_path.join("local-app.json"))?;
        if digest(&current_manifest_bytes) != current.manifest_digest {
            return Err(problem(
                "当前应用说明已变化，无法核对退回后的配置和数据声明。",
            ));
        }
        let current_manifest = TrialManifest::parse(&current_manifest_bytes)
            .map_err(|_| problem("当前应用说明无效，不能猜测兼容性。"))?;
        if current_manifest.runtime != TrialRuntime::NodeService {
            return Err(problem("当前应用类型不支持程序版本退回。"));
        }
        let previous_path =
            node_app_install::verify_managed_program(&self.root, &previous.previous_registration)?;
        let previous_manifest = TrialManifest::load(&previous_path)
            .map_err(|_| problem("旧程序的应用说明无法读取。"))?;
        previous_manifest
            .inspect(&previous_path)
            .map_err(|_| problem("旧程序缺少必要运行文件。"))?;
        let configuration = self
            .store(id)?
            .view(&current_manifest)
            .map_err(|_| problem("当前配置状态无法核对。"))?;
        let current_registration_digest =
            node_app_install::registration_digest(&slot)?.ok_or_else(changed)?;
        let snapshot = InstallTargetSnapshot {
            app_id: id.into(),
            registration_digest: Some(current_registration_digest.clone()),
            generation: Some(current.generation.clone()),
            manifest: Some(serde_json::from_slice(&current_manifest_bytes)?),
            configuration_revision: configuration.revision,
            external_registration: None,
            development_source: current.development_source.clone(),
        };
        node_app_install::compatible(
            &snapshot,
            &serde_json::from_slice(&read_small(&previous_path.join("local-app.json"))?)?,
        )?;
        let root_identity =
            PersistedIdentity::capture(&slot).map_err(|_| problem("应用目录身份无法核对。"))?;
        let mut revision = Sha256::new();
        revision.update(current_registration_digest.as_bytes());
        revision.update(&previous_bytes);
        revision.update(serde_json::to_vec(&json!({
            "inode":root_identity.inode,"volume":root_identity.volume_uuid,
            "device":if root_identity.volume_uuid.is_none(){Some(root_identity.device)}else{None},
            "configurationRevision":configuration.revision
        }))?);
        let mut target = previous.previous_registration;
        // Source selection is a development concern. Rolling the executable
        // back must not silently rebind it to an older author directory.
        target.development_source = current.development_source.clone();
        Ok(Some(Plan {
            revision: format!("{:x}", revision.finalize()),
            current,
            target,
            root_identity,
            current_registration_digest,
            app_name: current_manifest.app.name,
            current_version: current_manifest.app.version,
            previous_version: previous_manifest.app.version,
        }))
    }

    /// A workspace lease excludes both migration and historical generation
    /// cleanup. The install commit lock excludes removal after verification.
    pub async fn rollback_preview(&self, id: &str, store: Arc<ProjectStore>) -> Result<Value> {
        if Uuid::parse_str(id).is_err() {
            return Err(changed());
        }
        let _workspace = store
            .reserve_transient_operation(&format!("node-rollback:{id}"), "program-rollback")?;
        let inner = self.inner.lock().await;
        let installer = node_app_install::NodeAppInstaller::new(self.data_root.clone())?;
        let _commit = installer.commit_lock()?;
        let Some(plan) = self.rollback_plan(id, &inner)? else {
            return Ok(json!({"appId":id,"canRollback":false}));
        };
        Ok(json!({"appId":id,"canRollback":true,"name":plan.app_name,
            "currentVersion":plan.current_version,"previousVersion":plan.previous_version,
            "currentGeneration":plan.current.generation,"previousGeneration":plan.target.generation,
            "expectedRevision":plan.revision,"dataWillRollback":false,"configurationWillRollback":false}))
    }

    /// Only the registration pointer changes. The immutable receipt and actual
    /// pointer make a lost response retryable without toggling versions twice.
    pub async fn rollback_apply(
        &self,
        id: &str,
        input: RollbackNodeProgramInput,
        store: Arc<ProjectStore>,
    ) -> Result<Value> {
        if Uuid::parse_str(id).is_err() || !valid_digest(&input.expected_revision) {
            return Err(changed());
        }
        let _workspace = store
            .reserve_transient_operation(&format!("node-rollback:{id}"), "program-rollback")?;
        let mut inner = self.inner.lock().await;
        self.require_install_stopped(id, &inner)?;
        let installer = node_app_install::NodeAppInstaller::new(self.data_root.clone())?;
        let _commit = installer.commit_lock()?;
        let slot = self.root.join(id);
        ordinary_path(&slot, true)?;
        let receipts = slot.join("program-rollbacks");
        let receipt_path = receipts.join(format!("{}.json", input.expected_revision));
        crate::projects::check_no_symlinks(&receipt_path, true)?;
        if receipt_path.try_exists()? {
            let receipt: RollbackReceipt = serde_json::from_slice(&read_small(&receipt_path)?)?;
            if receipt.version != 1
                || receipt.app_id != id
                || receipt.expected_revision != input.expected_revision
                || receipt.source_registration.id != id
                || receipt.target_registration.id != id
                || !receipt
                    .root_identity
                    .matches_path(&slot)
                    .map_err(|_| changed())?
                || receipt.target_registration_digest
                    != encoded_digest(&receipt.target_registration)?
            {
                return Err(changed());
            }
            if node_app_install::registration_digest(&slot)?.as_deref()
                == Some(receipt.target_registration_digest.as_str())
            {
                node_app_install::verify_managed_program(&self.root, &receipt.target_registration)?;
                inner.errors.remove(id);
                return Ok(
                    json!({"appId":id,"operationOutcome":"committed","generation":receipt.target_registration.generation,
                    "alreadyApplied":true,"dataChanged":false,"configurationChanged":false,"started":false}),
                );
            }
            if node_app_install::registration_digest(&slot)?.as_deref()
                != Some(receipt.source_registration_digest.as_str())
            {
                return Err(changed().outcome("unknown"));
            }
        }
        let plan = self.rollback_plan(id, &inner)?.ok_or_else(changed)?;
        if plan.revision != input.expected_revision {
            return Err(changed());
        }
        let receipt = RollbackReceipt {
            version: 1,
            app_id: id.into(),
            expected_revision: plan.revision,
            root_identity: plan.root_identity,
            source_registration: plan.current,
            source_registration_digest: plan.current_registration_digest,
            target_registration_digest: encoded_digest(&plan.target)?,
            target_registration: plan.target,
        };
        private_dir(&receipts)?;
        if !receipt_path.try_exists()? {
            write_new_json(&receipt_path, &receipt)?;
        } else if read_small(&receipt_path)? != serde_json::to_vec_pretty(&receipt)? {
            return Err(changed());
        }
        // Recheck the actual current pointer immediately before the atomic
        // switch. Program/config checks above happened under the same leases.
        if node_app_install::registration_digest(&slot)?.as_deref()
            != Some(receipt.source_registration_digest.as_str())
            || !receipt
                .root_identity
                .matches_path(&slot)
                .map_err(|_| changed())?
        {
            return Err(changed());
        }
        let operation = atomic_json(
            &slot.join("registration.json"),
            &receipt.target_registration,
        );
        let actual = node_app_install::registration_digest(&slot);
        match actual {
            Ok(Some(actual)) if actual == receipt.target_registration_digest => {
                if let Err(error) = operation {
                    return Err(error
                        .outcome("committed")
                        .detail("generation", json!(receipt.target_registration.generation)));
                }
                inner.errors.remove(id);
                Ok(
                    json!({"appId":id,"operationOutcome":"committed","generation":receipt.target_registration.generation,
                    "alreadyApplied":false,"dataChanged":false,"configurationChanged":false,"started":false}),
                )
            }
            Ok(Some(actual)) if actual == receipt.source_registration_digest => Err(operation
                .err()
                .unwrap_or_else(changed)
                .outcome("not-committed")),
            _ => Err(operation.err().unwrap_or_else(changed).outcome("unknown")),
        }
    }
}
