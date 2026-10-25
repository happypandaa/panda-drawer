//! Permanent removal is limited to named platform-owned paths. External source
//! directories and retained run/installation/recovery evidence are never targets.
use super::*;
use crate::{builds::atomic_json, file_identity::PersistedIdentity};
use std::os::unix::fs::MetadataExt;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteNodeAppInput {
    pub expected_revision: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    version: u32,
    app_id: String,
    expected_revision: String,
    ownership: String,
    root_identity: PersistedIdentity,
    phase: String,
}

fn receipt_path(root: &Path, id: &str) -> PathBuf {
    root.join(".deletions").join(format!("{id}.json"))
}

fn receipt(root: &Path, id: &str) -> Result<Option<Receipt>> {
    if Uuid::parse_str(id).is_err() {
        return Err(fail("应用标识无效。"));
    }
    let path = receipt_path(root, id);
    crate::projects::check_no_symlinks(&path, true)?;
    if !path.try_exists()? {
        return Ok(None);
    }
    let value: Receipt = serde_json::from_slice(&read_small(&path)?)
        .map_err(|_| fail("删除记录无法核对，请保留现场。"))?;
    if value.version != 1
        || value.app_id != id
        || !matches!(value.ownership.as_str(), "managed" | "external")
        || !matches!(
            value.phase.as_str(),
            "delete-intent" | "delete-incomplete" | "deleted"
        )
    {
        return Err(fail("删除记录身份不符，请保留现场。"));
    }
    Ok(Some(value))
}

pub(super) fn completed(root: &Path, id: &str) -> Result<bool> {
    Ok(receipt(root, id)?.is_some_and(|value| value.phase == "deleted"))
}

pub(super) fn require_available(root: &Path, id: &str) -> Result<()> {
    if receipt(root, id)?.is_some() {
        return Err(ApiError::new(
            "NODE_APP_DELETION_PENDING",
            "此项目已进入永久删除流程，不能启动或重新创建配置；请核对删除结果，未完成时明确重试。",
        ));
    }
    Ok(())
}

fn revision(slot: &Path) -> Result<String> {
    let identity = PersistedIdentity::capture(slot).map_err(|error| fail(error.to_string()))?;
    let mut digest = Sha256::new();
    digest.update(read_small(&slot.join("registration.json"))?);
    // A copied registration at a replaced directory is not the reviewed target.
    // Stable volumes may receive a new device number after a reboot.
    digest.update(serde_json::to_vec(&json!({"inode":identity.inode,
        "volume":identity.volume_uuid,"device":if identity.volume_uuid.is_none(){Some(identity.device)}else{None}}))?);
    Ok(format!("{:x}", digest.finalize()))
}

fn targets(slot: &Path, managed: bool) -> Vec<PathBuf> {
    let mut result = vec![
        slot.join("configuration"),
        slot.join("data"),
        slot.join("cache"),
    ];
    if managed {
        result.push(slot.join("generations"));
    }
    result
}

fn check_root(slot: &Path, expected: &PersistedIdentity) -> Result<()> {
    ordinary_path(slot, true)?;
    if !expected
        .matches_path(slot)
        .map_err(|error| fail(error.to_string()))?
    {
        return Err(ApiError::new(
            "NODE_APP_DELETE_CHANGED",
            "项目目录身份已变化，未继续删除。",
        ));
    }
    Ok(())
}

impl NodeApps {
    /// Preview reads only registration and owned directory identity, so an
    /// unavailable external source or broken business database remains removable.
    pub async fn delete_preview(&self, id: &str) -> Result<Value> {
        let _inner = self.inner.lock().await;
        self.require_no_development_operation(id)?;
        self.require_no_source_restore(id)?;
        let record = self.registration(id)?;
        let slot = self.root.join(id);
        ordinary_path(&slot, true)?;
        let expected_revision = revision(&slot)?;
        let managed = record.managed.is_some();
        if let Some(previous) = receipt(&self.root, id)? {
            if previous.expected_revision != expected_revision {
                return Err(ApiError::new(
                    "NODE_APP_DELETE_CHANGED",
                    "登记已变化，请保留删除现场。",
                ));
            }
            check_root(&slot, &previous.root_identity)?;
        }
        let name = self
            .manifest(&record)
            .ok()
            .map(|value| value.app.name)
            .unwrap_or_else(|| {
                record
                    .source_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("项目")
                    .into()
            });
        let mut preserved = vec![
            slot.join("runs"),
            self.root.join(".installations"),
            self.root.join(".deletions"),
        ];
        if !managed {
            preserved.push(record.source_path.clone());
        }
        let mut delete_paths = targets(&slot, managed);
        delete_paths.push(slot.join("registration.json"));
        Ok(
            json!({"appId":id,"name":name,"ownership":if managed {"managed"} else {"external"},
            "platformRoot":slot,"sourcePath":record.source_path,"deletePaths":delete_paths,
            "preservedPaths":preserved,"expectedRevision":expected_revision}),
        )
    }

    /// The native caller also holds the window-operation lock. Deletion never
    /// continues on startup; a retained intent blocks execution until reviewed.
    pub async fn delete(&self, id: &str, input: DeleteNodeAppInput) -> Result<Value> {
        let mut inner = self.inner.lock().await;
        self.require_no_development_operation(id)?;
        self.require_no_source_restore(id)?;
        if inner.shutting_down {
            return Err(fail("平台正在退出，不能删除项目。"));
        }
        let record = self.registration(id)?;
        let slot = self.root.join(id);
        ordinary_path(&slot, true)?;
        let current_revision = revision(&slot)?;
        if current_revision != input.expected_revision {
            return Err(ApiError::new(
                "NODE_APP_DELETE_CHANGED",
                "项目登记已变化，请重新核对删除范围。",
            ));
        }
        self.stop_locked(id, &mut inner).await?;
        if inner.running.contains_key(id)
            || slot.join("active.json").try_exists()?
            || has_start_intent_cleanup(&slot)?
        {
            return Err(fail("应用尚未完成安全关闭，未删除文件；请保留运行现场。"));
        }
        let managed = record.managed.is_some();
        let mut saved = match receipt(&self.root, id)? {
            Some(value) => value,
            None => Receipt {
                version: 1,
                app_id: id.into(),
                expected_revision: current_revision.clone(),
                ownership: if managed {
                    "managed".into()
                } else {
                    "external".into()
                },
                root_identity: PersistedIdentity::capture(&slot)
                    .map_err(|error| fail(error.to_string()))?,
                phase: "delete-intent".into(),
            },
        };
        if saved.expected_revision != current_revision || (saved.ownership == "managed") != managed
        {
            return Err(ApiError::new(
                "NODE_APP_DELETE_CHANGED",
                "项目登记与删除记录不符，未继续删除。",
            ));
        }
        check_root(&slot, &saved.root_identity)?;
        private_dir(&self.root.join(".deletions"))?;
        let path = receipt_path(&self.root, id);
        saved.phase = "delete-intent".into();
        atomic_json(&path, &saved)?;
        let owned = targets(&slot, managed);
        let remove_slot = slot.clone();
        let identity = saved.root_identity.clone();
        let progress_root = self.root.join(".deletions");
        let removal_id = id.to_owned();
        let removal = tokio::task::spawn_blocking(move || -> Result<()> {
            for target in owned {
                check_root(&remove_slot, &identity)?;
                crate::projects::check_no_symlinks(&target, true)?;
                match fs::symlink_metadata(&target) {
                    Ok(metadata) if metadata.is_dir() => {
                        // The receipt survives reboot/remount; only live device
                        // numbers can establish this traversal's mount boundary.
                        if metadata.dev() != fs::symlink_metadata(&remove_slot)?.dev() {
                            return Err(fail("平台数据路径跨越挂载点，未继续删除。"));
                        }
                        let target_identity = PersistedIdentity::capture(&target)
                            .map_err(|error| fail(error.to_string()))?;
                        let progress =
                            progress_root.join(format!("{removal_id}-{}.ndjson", Uuid::new_v4()));
                        crate::history_cleanup::remove_owned_directory(
                            &target,
                            &target_identity,
                            &progress,
                        )?;
                    }
                    Ok(_) => return Err(fail("平台数据路径不是普通目录，已停止删除并保留现场。")),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            fs::File::open(&remove_slot)?.sync_all()?;
            Ok(())
        })
        .await
        .unwrap_or_else(|error| Err(fail(error.to_string())));
        let finalization = removal.and_then(|_| {
            check_root(&slot, &saved.root_identity)?;
            if revision(&slot)? != current_revision {
                return Err(ApiError::new(
                    "NODE_APP_DELETE_CHANGED",
                    "删除期间登记发生变化，请保留现场核对。",
                ));
            }
            // The durable retirement excludes this ID even if final metadata
            // cleanup is interrupted. It retains no program/data recycle copy.
            saved.phase = "deleted".into();
            atomic_json(&path, &saved)
        });
        if let Err(mut error) = finalization {
            saved.phase = "delete-incomplete".into();
            let _ = atomic_json(&path, &saved);
            error.message = format!(
                "项目删除未完成，部分文件可能已永久删除。请保留现场，重新核对范围后明确重试；平台不会自动继续删除。原因：{}",
                error.message
            );
            return Err(error
                .outcome("delete_incomplete")
                .detail("platformRoot", json!(slot))
                .detail("deletionRecord", json!(path)));
        }
        let cleanup = fs::remove_file(slot.join("registration.json"));
        let sync = fs::File::open(&slot).and_then(|directory| directory.sync_all());
        inner.errors.remove(id);
        Ok(
            json!({"appId":id,"operationOutcome":"deleted","ownership":saved.ownership,
            "sourcePreserved":true,"evidenceRetained":true,"metadataCleanupPending":cleanup.is_err() || sync.is_err()}),
        )
    }
}
