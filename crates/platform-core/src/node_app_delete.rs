//! Permanent removal is limited to named platform-owned paths. External source
//! directories and retained run/installation/recovery evidence are never targets.
use super::*;
use crate::file_identity::PersistedIdentity;
use std::os::unix::fs::MetadataExt;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteNodeAppInput {
    pub expected_revision: String,
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

fn has_active_marker(slot: &Path) -> Result<bool> {
    // Any object at the marker name is evidence, including a dangling link.
    // Presence checks never follow it or infer ownership from its contents.
    match fs::symlink_metadata(slot.join("active.json")) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn run_recovery_blocker(slot: &Path, owns_runtime: bool) -> Result<Option<ApiError>> {
    let cleanup_pending = has_start_intent_cleanup(slot)?;
    if cleanup_pending {
        return Ok(Some(run_marker::unknown()));
    }
    if has_active_marker(slot)? && !owns_runtime {
        let id = slot
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(run_marker::unknown)?;
        let inspected = run_marker::inspect(slot, id, run_marker::boot_time_or_unknown());
        if !matches!(
            inspected,
            Ok(run_marker::Inspection {
                state: run_marker::RunState::Stopped | run_marker::RunState::Orphaned(_),
                ..
            })
        ) {
            return Ok(Some(run_marker::unknown()));
        }
    }
    Ok(None)
}

impl NodeApps {
    /// Preview reads registration, owned directory identity and optional bounded
    /// display metadata. It never verifies the complete installed program or
    /// opens private configuration, so damaged applications remain removable.
    pub async fn delete_preview(&self, id: &str) -> Result<Value> {
        let inner = self.inner.lock().await;
        self.require_no_development_operation(id)?;
        self.require_no_source_restore(id)?;
        let record = self.registration(id)?;
        let slot = self.root.join(id);
        ordinary_path(&slot, true)?;
        let expected_revision = revision(&slot)?;
        let blocker = run_recovery_blocker(&slot, inner.running.contains_key(id))?
            .map(|error| json!({"code":error.code,"message":error.message}));
        let managed = record.managed.is_some();
        // The name is display metadata, not permission to execute this program.
        // `manifest()` verifies every installed file, which can make opening the
        // confirmation scan all of node_modules while holding the Node lock.
        let name = read_small(&record.source_path.join("local-app.json"))
            .ok()
            .filter(|bytes| format!("{:x}", Sha256::digest(bytes)) == record.manifest_digest)
            .and_then(|bytes| TrialManifest::parse(&bytes).ok())
            .filter(|manifest| manifest.runtime == TrialRuntime::NodeService)
            .map(|value| value.app.name)
            .unwrap_or_else(|| {
                record
                    .source_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("项目")
                    .into()
            });
        let mut preserved = vec![slot.join("runs"), self.root.join(".installations")];
        if !managed {
            preserved.push(record.source_path.clone());
        }
        let mut delete_paths = targets(&slot, managed);
        delete_paths.push(slot.join("registration.json"));
        Ok(
            json!({"appId":id,"name":name,"ownership":if managed {"managed"} else {"external"},
            "platformRoot":slot,"sourcePath":record.source_path,"deletePaths":delete_paths,
            "preservedPaths":preserved,"expectedRevision":expected_revision,"blocker":blocker}),
        )
    }

    /// The blocking owner keeps the application lock until removal and final
    /// registration cleanup have finished, even when the caller disappears.
    pub async fn delete(&self, id: &str, input: DeleteNodeAppInput) -> Result<Value> {
        let mut inner = self.inner.clone().lock_owned().await;
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
        if let Some(error) = run_recovery_blocker(&slot, inner.running.contains_key(id))? {
            return Err(error);
        }
        if inner.running.contains_key(id) || has_active_marker(&slot)? {
            return Err(
                fail("应用尚未完成安全关闭，未删除文件；请保留运行现场。").outcome("not_deleted")
            );
        }
        let managed = record.managed.is_some();
        let root_identity =
            PersistedIdentity::capture(&slot).map_err(|error| fail(error.to_string()))?;
        check_root(&slot, &root_identity)?;
        let owned = targets(&slot, managed);
        let remove_slot = slot.clone();
        let identity = root_identity;
        let removal_id = id.to_owned();
        let ownership = if managed { "managed" } else { "external" };
        tokio::task::spawn_blocking(move || -> Result<Value> {
            let removal = (|| -> Result<bool> {
                let mut checked = Vec::new();
                for target in owned {
                    check_root(&remove_slot, &identity)?;
                    crate::projects::check_no_symlinks(&target, true)?;
                    match fs::symlink_metadata(&target) {
                        Ok(metadata) if metadata.is_dir() => {
                            if metadata.dev() != fs::symlink_metadata(&remove_slot)?.dev() {
                                return Err(fail("平台数据路径跨越挂载点，未继续删除。"));
                            }
                            let target_identity = PersistedIdentity::capture(&target)
                                .map_err(|error| fail(error.to_string()))?;
                            checked.push((target, target_identity));
                        }
                        Ok(_) => return Err(fail("平台数据路径不是普通目录，未开始删除。")),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                check_root(&remove_slot, &identity)?;
                if revision(&remove_slot)? != current_revision {
                    return Err(ApiError::new("NODE_APP_DELETE_CHANGED", "删除前登记发生变化，请重新核对。"));
                }
                let registration_path = remove_slot.join("registration.json");
                let mut current: Value = serde_json::from_slice(&read_small(&registration_path)?)?;
                if current["removalPending"] != true {
                    current["removalPending"] = true.into();
                    crate::builds::atomic_json(&registration_path, &current)?;
                }
                let pending_revision = revision(&remove_slot)?;
                for (target, target_identity) in checked {
                    check_root(&remove_slot, &identity)?;
                    crate::history_cleanup::remove_owned_directory(&target, &target_identity)?;
                }
                check_root(&remove_slot, &identity)?;
                if revision(&remove_slot)? != pending_revision {
                    return Err(ApiError::new("NODE_APP_DELETE_CHANGED", "删除期间登记发生变化，请重新核对剩余内容。"));
                }
                fs::remove_file(registration_path)?;
                Ok(fs::File::open(&remove_slot).and_then(|directory| directory.sync_all()).is_ok())
            })();
            let durable = match removal {
                Ok(durable) => durable,
                Err(mut error) => {
                    error.message = format!("项目删除未完成，部分文件可能已永久删除。请重新核对剩余内容后明确重试；平台不会自动继续删除。原因：{}", error.message);
                    inner.errors.insert(removal_id.clone(), error.message.clone());
                    return Err(error.outcome("delete_incomplete").detail("platformRoot", json!(remove_slot)));
                }
            };
            inner.errors.remove(&removal_id);
            Ok(json!({"appId":removal_id,"operationOutcome":"deleted","ownership":ownership,
                "sourcePreserved":true,"evidenceRetained":true,"metadataCleanupPending":!durable,
                "durabilityConfirmed":durable,
                "warning":if durable { Value::Null } else { json!("文件及登记已移除，但目录落盘尚未确认；请刷新或重开核对结果。") }}))
        })
        .await
        .unwrap_or_else(|error| Err(fail(error.to_string())))
    }
}
