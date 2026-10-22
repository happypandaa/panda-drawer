//! Explicit deletion of one platform-owned Node slot. External author sources,
//! installation candidates and shared source records are never deletion targets.
use super::*;
use crate::file_identity::PersistedIdentity;
use std::time::{SystemTime, UNIX_EPOCH};

const PREVIEW_LIFETIME_MS: u64 = 10 * 60 * 1000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteNodeAppInput {
    pub preview_id: String,
}

pub(super) struct DeletePreview {
    app_id: String,
    expires_at: u64,
    root_identity: PersistedIdentity,
    slot_identity: PersistedIdentity,
    registration_digest: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteReceipt {
    version: u32,
    deletion_id: String,
    app_id: String,
    original_path: PathBuf,
    slot_identity: PersistedIdentity,
    registration_digest: String,
    phase: String,
    system_trash_path: Option<PathBuf>,
    error: Option<ApiError>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn deletion_error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}

fn capture(path: &Path) -> Result<PersistedIdentity> {
    PersistedIdentity::capture(path)
        .map_err(|error| deletion_error("NODE_DELETE_PATH_UNSAFE", &error.to_string()))
}

fn matches(identity: &PersistedIdentity, path: &Path) -> bool {
    identity.matches_path(path).unwrap_or(false)
}

impl NodeApps {
    fn deletion_registration(&self, id: &str) -> Result<(Registration, String)> {
        if Uuid::parse_str(id).is_err() {
            return Err(deletion_error("NODE_DELETE_INVALID", "应用实例标识无效。"));
        }
        let slot = self.root.join(id);
        ordinary_path(&slot, true)?;
        let bytes = read_small(&slot.join("registration.json"))?;
        let value: Value = serde_json::from_slice(&bytes)?;
        let record = match value["version"].as_u64() {
            Some(1) => Registration::from(serde_json::from_slice::<ExternalRegistration>(&bytes)?),
            Some(2) => {
                let record: ManagedNodeRegistration = serde_json::from_slice(&bytes)?;
                Registration {
                    id: record.id.clone(),
                    source_path: node_app_install::managed_program_path(&self.root, &record)?,
                    manifest_digest: record.manifest_digest.clone(),
                    managed: Some(record),
                }
            }
            _ => {
                return Err(deletion_error(
                    "NODE_DELETE_INVALID",
                    "应用登记版本无效，请保留现场。",
                ))
            }
        };
        if record.id != id {
            return Err(deletion_error(
                "NODE_DELETE_INVALID",
                "应用登记身份与目录不一致，请保留现场。",
            ));
        }
        Ok((record, format!("{:x}", Sha256::digest(bytes))))
    }

    pub(super) fn require_no_pending_delete(&self, id: &str) -> Result<()> {
        let root = self.root.join(".deletions");
        crate::projects::check_no_symlinks(&root, true)?;
        if !root.try_exists()? {
            return Ok(());
        }
        ordinary_path(&root, true)?;
        for entry in fs::read_dir(&root)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.starts_with("delete_") || !name.ends_with(".json") {
                continue;
            }
            let receipt: DeleteReceipt =
                serde_json::from_slice(&read_small(&path)?).map_err(|_| {
                    deletion_error(
                        "NODE_DELETE_RECOVERY_REQUIRED",
                        "删除记录无法核对，请保留现场。",
                    )
                })?;
            if receipt.version != 1 || receipt.deletion_id != name.trim_end_matches(".json") {
                return Err(deletion_error(
                    "NODE_DELETE_RECOVERY_REQUIRED",
                    "删除记录身份无效，请保留现场。",
                ));
            }
            if receipt.app_id == id
                && !matches!(receipt.phase.as_str(), "system_trashed" | "not_moved")
            {
                return Err(deletion_error(
                    "NODE_DELETE_RECOVERY_REQUIRED",
                    "此应用的上次删除结果尚未确认，请保留删除记录与应用目录，不要重复操作。",
                ));
            }
        }
        Ok(())
    }

    fn deletion_blockers(&self, id: &str, inner: &Inner) -> Result<Vec<String>> {
        let mut reasons = Vec::new();
        if inner.shutting_down {
            reasons.push("平台正在退出，暂时不能删除应用。".into());
        }
        if inner.running.contains_key(id) {
            reasons.push("请先处理草稿并正常关闭应用，等待后台服务完成收尾后再删除。".into());
        }
        let marker = self.root.join(id).join("active.json");
        crate::projects::check_no_symlinks(&marker, true)?;
        if marker.try_exists()? {
            reasons
                .push("应用运行记录尚未完成正常收尾，不能确认后台进程已退出；请保留现场。".into());
        }
        if let Err(error) = self.require_no_pending_delete(id) {
            reasons.push(error.message);
        }
        Ok(reasons)
    }

    /// A token binds the exact slot and registration, without reading business
    /// data or requiring an available external source/configuration.
    pub async fn delete_preview(&self, id: &str) -> Result<Value> {
        self.delete_preview_supported(id, crate::system_trash::ensure_supported())
            .await
    }

    async fn delete_preview_supported(&self, id: &str, supported: Result<()>) -> Result<Value> {
        let mut inner = self.inner.lock().await;
        let (record, registration_digest) = self.deletion_registration(id)?;
        let slot = self.root.join(id);
        let root_identity = capture(&self.root)?;
        let slot_identity = capture(&slot)?;
        // The name is presentation only. Deletion does not verify/hash all
        // program files or require an executable/healthy author directory.
        let name = read_small(&record.source_path.join("local-app.json"))
            .ok()
            .and_then(|bytes| TrialManifest::parse(&bytes).ok())
            .map(|manifest| manifest.app.name)
            .unwrap_or_else(|| {
                record
                    .source_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(id)
                    .to_owned()
            });
        let mut blockers = self.deletion_blockers(id, &inner)?;
        if let Err(error) = supported {
            blockers.push(error.message);
        }
        let created_at = now_ms();
        let expires_at = created_at.saturating_add(PREVIEW_LIFETIME_MS);
        let preview_id = format!("delete_preview_{}", Uuid::new_v4().simple());
        inner
            .deletion_previews
            .retain(|_, preview| preview.expires_at > created_at && preview.app_id != id);
        // Do not retain an unbounded collection of stale confirmation tokens.
        if inner.deletion_previews.len() >= 32 {
            inner.deletion_previews.clear();
        }
        inner.deletion_previews.insert(
            preview_id.clone(),
            DeletePreview {
                app_id: id.into(),
                expires_at,
                root_identity,
                slot_identity,
                registration_digest,
            },
        );
        Ok(json!({
            "appId":id,"name":name,"path":slot,"previewId":preview_id,
            "createdAt":created_at,"expiresAt":expires_at,"canDelete":blockers.is_empty(),"blockedReasons":blockers,
            "externalSourcePath":record.managed.is_none().then_some(record.source_path),
            "git":{"state":"unverified","remoteSync":"unverified"},
            "platformRestoreAvailable":false,
            "scope":["已安装程序及依赖","本机数据库与附件","配置及本机密钥","缓存与运行记录"],
            "preserved":["外部作者源码","独立源码来源副本","安装候选与历史操作记录"]
        }))
    }

    pub async fn delete(&self, id: &str, input: DeleteNodeAppInput) -> Result<Value> {
        crate::system_trash::ensure_supported()?;
        self.delete_with(id, input, crate::system_trash::move_directory_with_identity)
            .await
    }

    async fn delete_with(
        &self,
        id: &str,
        input: DeleteNodeAppInput,
        move_directory: impl FnOnce(&Path, &PersistedIdentity) -> Result<PathBuf> + Send + 'static,
    ) -> Result<Value> {
        let mut inner = self.inner.lock().await;
        let preview = inner
            .deletion_previews
            .remove(&input.preview_id)
            .ok_or_else(|| {
                deletion_error(
                    "NODE_DELETE_PREVIEW_CHANGED",
                    "删除确认已失效，请重新核对应用及删除范围。",
                )
            })?;
        if preview.app_id != id || preview.expires_at <= now_ms() {
            return Err(deletion_error(
                "NODE_DELETE_PREVIEW_CHANGED",
                "删除确认已失效，请重新核对应用及删除范围。",
            ));
        }
        let (_, registration_digest) = self.deletion_registration(id)?;
        let slot = self.root.join(id);
        if !matches(&preview.root_identity, &self.root)
            || !matches(&preview.slot_identity, &slot)
            || preview.registration_digest != registration_digest
        {
            return Err(deletion_error(
                "NODE_DELETE_PREVIEW_CHANGED",
                "应用目录或登记已变化，请重新核对；本次尚未删除。",
            ));
        }
        let blockers = self.deletion_blockers(id, &inner)?;
        if !blockers.is_empty() {
            return Err(deletion_error("NODE_DELETE_BUSY", &blockers.join("\n")));
        }
        let receipts = self.root.join(".deletions");
        private_dir(&receipts)?;
        let deletion_id = format!("delete_{}", Uuid::new_v4().simple());
        let receipt_path = receipts.join(format!("{deletion_id}.json"));
        let mut receipt = DeleteReceipt {
            version: 1,
            deletion_id,
            app_id: id.into(),
            original_path: slot.clone(),
            slot_identity: preview.slot_identity,
            registration_digest,
            phase: "intent".into(),
            system_trash_path: None,
            error: None,
        };
        write_new_json(&receipt_path, &receipt)?;
        // Retain the NodeApps mutation lock while the native move runs. A
        // cancelled caller cannot turn the durable intent into a second start.
        let original = slot.clone();
        let expected = receipt.slot_identity.clone();
        let result = tokio::task::spawn_blocking(move || move_directory(&original, &expected))
            .await
            .map_err(|_| {
                deletion_error(
                    "NODE_DELETE_UNKNOWN",
                    "系统删除操作未正常返回，请保留现场并核对系统废纸篓。",
                )
            })
            .and_then(|result| result);
        match result {
            Ok(destination) => {
                // A successful native result must identify this exact moved
                // directory, and the old path must no longer name an object.
                let source_missing = fs::symlink_metadata(&slot)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
                let destination_check = crate::system_trash::moved_directory_matches(
                    &receipt.slot_identity,
                    &destination,
                );
                if !source_missing || !destination_check.as_ref().is_ok_and(|matches| *matches) {
                    receipt.phase = "needs_attention".into();
                    receipt.system_trash_path = Some(destination);
                    let mut error = deletion_error(
                        "NODE_DELETE_UNKNOWN",
                        "系统返回的删除结果无法核对，请保留记录及现有文件；不会自动重复删除。",
                    );
                    if let Err(cause) = destination_check {
                        error = error.detail("verificationError", json!(cause));
                    }
                    receipt.error = Some(error.clone());
                    let _ = crate::builds::atomic_json(&receipt_path, &receipt);
                    return Err(error
                        .detail("receiptPath", json!(receipt_path))
                        .detail("originalPath", json!(slot))
                        .detail("systemTrashPath", json!(receipt.system_trash_path))
                        .outcome("unknown"));
                }
                receipt.phase = "system_trashed".into();
                receipt.system_trash_path = Some(destination.clone());
                if let Err(error) = crate::builds::atomic_json(&receipt_path, &receipt) {
                    return Err(deletion_error(
                        "NODE_DELETE_RECEIPT_FAILED",
                        "应用已移入系统废纸篓，但删除回执未完整保存；请核对废纸篓，不要重复删除。",
                    )
                    .detail("systemTrashPath", json!(destination))
                    .detail("originalPath", json!(slot))
                    .detail("receiptPath", json!(receipt_path))
                    .detail("receiptError", json!(error))
                    .outcome("committed"));
                }
                inner.errors.remove(id);
                inner
                    .deletion_previews
                    .retain(|_, preview| preview.app_id != id);
                Ok(
                    json!({"appId":id,"operationOutcome":"system_trashed","systemTrashPath":destination,
                    "originalPath":slot,"receiptPath":receipt_path,"platformRestoreAvailable":false}),
                )
            }
            Err(error) => {
                let unconfirmed = error.details["operationOutcome"] == "unknown";
                let known_no_move = error.code == "SYSTEM_TRASH_IDENTITY_CHANGED"
                    && error.details["operationOutcome"] == "not_started";
                let unchanged = !unconfirmed && matches(&receipt.slot_identity, &slot);
                receipt.phase = if unchanged {
                    "not_moved"
                } else {
                    "needs_attention"
                }
                .into();
                receipt.system_trash_path =
                    error.details["systemTrashPath"].as_str().map(PathBuf::from);
                receipt.error = Some(error.clone());
                let saved = crate::builds::atomic_json(&receipt_path, &receipt);
                Err(error
                    .detail("receiptPath", json!(receipt_path))
                    .detail("receiptSaved", saved.is_ok())
                    .detail("originalPath", json!(slot))
                    .outcome(if unchanged || known_no_move {
                        "not_started"
                    } else {
                        "unknown"
                    }))
            }
        }
    }
}

#[cfg(test)]
#[path = "node_app_delete_tests.rs"]
mod tests;
