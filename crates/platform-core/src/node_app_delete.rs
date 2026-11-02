//! Permanent removal is limited to named platform-owned paths. External source
//! directories are preserved; platform-owned source and every instance file are targets.
use super::*;
use crate::file_identity::PersistedIdentity;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteNodeAppInput {
    pub expected_revision: String,
    /// The user chose to delete although the platform could not confirm the
    /// app stopped (for example after a crash). Deletion still stops what it
    /// owns, then removes platform data without waiting on stale markers.
    #[serde(default)]
    pub force: bool,
}

pub(super) fn revision(slot: &Path) -> Result<String> {
    let identity = PersistedIdentity::capture(slot).map_err(|error| fail(error.to_string()))?;
    let mut digest = Sha256::new();
    digest.update(read_small(&slot.join("registration.json"))?);
    // A copied registration at a replaced directory is not the reviewed target.
    // Stable volumes may receive a new device number after a reboot.
    digest.update(serde_json::to_vec(&identity.deletion_revision_value())?);
    Ok(format!("{:x}", digest.finalize()))
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
        let record = self.deletion_source(id)?;
        let slot = self.root.join(id);
        ordinary_path(&slot, true)?;
        let expected_revision = revision(&slot)?;
        let blocker = run_recovery_blocker(&slot, inner.running.contains_key(id))?
            .map(|error| json!({"code":error.code,"message":error.message}));
        // An unreadable ownership index only narrows deletion: the folder is
        // then treated as the user's and preserved.
        let managed = self
            .project_ownership()
            .owns_source_directory(id, &record.source_path)
            .unwrap_or(false);
        // The name is display metadata, not permission to execute this program.
        // `manifest()` verifies every installed file, which can make opening the
        // confirmation scan all of node_modules while holding the Node lock.
        let name = read_small(&record.source_path.join("panda.json"))
            .ok()
            .filter(|bytes| {
                record
                    .manifest_digest
                    .as_ref()
                    .is_none_or(|digest| &format!("{:x}", Sha256::digest(bytes)) == digest)
            })
            .and_then(|bytes| PandaManifest::parse(&bytes).ok())
            .filter(|manifest| manifest.runtime == PandaRuntime::Node)
            .map(|value| value.app.name)
            .unwrap_or_else(|| {
                record
                    .source_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("项目")
                    .into()
            });
        let mut preserved = vec![];
        if !managed {
            preserved.push(record.source_path.clone());
        }
        let mut delete_paths = vec![slot.clone()];
        delete_paths.extend(self.project_ownership().owned_paths(id).unwrap_or_default());
        Ok(
            json!({"appId":id,"name":name,"ownership":if managed {"managed"} else {"external"},
            "platformRoot":slot,"sourcePath":record.source_path,"deletePaths":delete_paths,
            "preservedPaths":preserved,"expectedRevision":expected_revision,"blocker":blocker}),
        )
    }

    /// Await the final result for callers that do not consume the task API.
    /// Native task submission uses submit_deletion_with instead.
    pub async fn delete(&self, id: &str, input: DeleteNodeAppInput) -> Result<Value> {
        let task = self.submit_deletion(id, input).await?;
        let task_id = task["taskId"]
            .as_str()
            .ok_or_else(|| fail("删除任务身份缺失。"))?;
        if task["phase"] == "failed" && task["canRetry"] == true {
            self.retry_deletion(task_id, id, task["journalRevision"].as_u64().unwrap_or(0))
                .await?;
        }
        self.wait_deletion_result(task_id, id).await
    }
}
