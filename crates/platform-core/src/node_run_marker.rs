//! Durable two-phase Node run identities. Old/incomplete same-boot records never
//! authorize a signal. A marker from an earlier system boot can be retired.
use super::*;
use crate::node_process_identity::{self, Observation, RuntimeProcessIdentity};
use std::time::SystemTime;

pub(super) fn boot_time_or_unknown() -> SystemTime {
    // Missing boot evidence must not stop the entire manager from opening.
    // Epoch disables every previous-boot shortcut; incomplete records stay gated.
    node_process_identity::boot_time().unwrap_or(SystemTime::UNIX_EPOCH)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RunMarker {
    version: u32,
    app_id: String,
    host_pid: u32,
    run_directory: PathBuf,
    phase: String,
    process: Option<RuntimeProcessIdentity>,
}

impl RunMarker {
    pub(super) fn launching(id: &str, directory: PathBuf) -> Self {
        Self {
            version: 2,
            app_id: id.into(),
            host_pid: std::process::id(),
            run_directory: directory,
            phase: "launching".into(),
            process: None,
        }
    }
    pub(super) fn bind(&mut self, process: RuntimeProcessIdentity) {
        self.phase = "running".into();
        self.process = Some(process);
    }
}

pub(super) enum RunState {
    Absent,
    Stopped,
    Orphaned(RuntimeProcessIdentity),
    Unconfirmed,
}

pub(super) struct Inspection {
    pub state: RunState,
    owner: Option<StartIntentOwnership>,
}

pub(super) fn unknown() -> ApiError {
    ApiError::new(
        "NODE_APP_RUN_RECOVERY_REQUIRED",
        "无法确认上次运行是否已经停止。请重启电脑后再停止或删除；平台不会按旧 PID 猜测终止进程。",
    )
    .outcome("not_deleted")
}

pub(super) fn inspect(slot: &Path, id: &str, boot: SystemTime) -> Result<Inspection> {
    if cleanup_pending(slot, boot)? {
        return Ok(Inspection {
            state: RunState::Unconfirmed,
            owner: None,
        });
    }
    let path = slot.join("active.json");
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Inspection {
                state: RunState::Absent,
                owner: None,
            })
        }
        Err(error) => return Err(error.into()),
        Ok(_) => (),
    }
    let owner = StartIntentOwnership::open_existing(&path)?;
    if owner.file.metadata()?.created().map_err(|_| unknown())? < boot {
        return Ok(Inspection {
            state: RunState::Stopped,
            owner: Some(owner),
        });
    }
    let marker = serde_json::from_slice::<RunMarker>(&owner.contents).ok();
    let state = match marker {
        Some(marker)
            if marker.version == 2
                && marker.app_id == id
                && marker.phase == "running"
                && marker.run_directory.parent() == Some(slot.join("runs").as_path()) =>
        {
            match marker.process {
                Some(process) => match node_process_identity::observe(&process)? {
                    Observation::Stopped => RunState::Stopped,
                    Observation::Matching => RunState::Orphaned(process),
                    Observation::Unconfirmed => RunState::Unconfirmed,
                },
                None => RunState::Unconfirmed,
            }
        }
        _ => RunState::Unconfirmed,
    };
    Ok(Inspection {
        state,
        owner: Some(owner),
    })
}

pub(super) fn cleanup_pending(slot: &Path, boot: SystemTime) -> Result<bool> {
    match fs::symlink_metadata(slot) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
        Ok(_) => ordinary_path(slot, true)?,
    }
    for entry in fs::read_dir(slot)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(START_INTENT_CLEANUP_PREFIX)
        {
            // Old cleanup evidence remains on disk, but a system reboot proves
            // no worker from that cleanup is still running.
            if fs::symlink_metadata(entry.path())?
                .created()
                .is_ok_and(|created| created < boot)
            {
                continue;
            }
            return Ok(true);
        }
    }
    Ok(false)
}

impl Inspection {
    pub(super) fn retire(&self, slot: &Path) -> Result<()> {
        if let Some(owner) = &self.owner {
            owner.remove_if_owned(&slot.join("active.json"))?;
        }
        Ok(())
    }
    pub(super) fn view(&self) -> Option<Value> {
        match self.state {
            RunState::Absent => None,
            RunState::Stopped => Some(
                json!({"state":"orphaned","canStop":true,"message":"遗留进程组已停止，可清理运行记录后继续。"}),
            ),
            RunState::Orphaned(_) => Some(
                json!({"state":"orphaned","canStop":true,"message":"发现身份已核实的遗留运行，可停止该应用后继续。"}),
            ),
            RunState::Unconfirmed => {
                Some(json!({"state":"unconfirmed","canStop":false,"message":unknown().message}))
            }
        }
    }
    pub(super) async fn stop_and_retire(self, slot: &Path) -> Result<()> {
        match &self.state {
            RunState::Absent => return Ok(()),
            RunState::Stopped => (),
            RunState::Orphaned(identity) => {
                node_process_identity::stop(identity).await?;
            }
            RunState::Unconfirmed => return Err(unknown()),
        }
        self.retire(slot)
    }
}

impl NodeApps {
    /// Called once before publishing manager commands. This touches only proven
    /// obsolete active markers, never application data, configuration or code.
    pub async fn reconcile_run_markers(&self) -> Result<Value> {
        self.reconcile_run_markers_at(boot_time_or_unknown()).await
    }
    async fn reconcile_run_markers_at(&self, boot: SystemTime) -> Result<Value> {
        let mut inner = self.inner.lock().await;
        if !self.root.try_exists()? {
            return Ok(json!({"retired":0,"orphaned":0,"unconfirmed":0}));
        }
        ordinary_path(&self.root, true)?;
        let (mut retired, mut orphaned, mut unconfirmed) = (0, 0, 0);
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if Uuid::parse_str(&id).is_err() || inner.running.contains_key(&id) {
                continue;
            }
            let slot = entry.path();
            let result = (|| -> Result<Inspection> {
                ordinary_path(&slot, true)?;
                inspect(&slot, &id, boot)
            })();
            match result {
                Ok(value) => match value.state {
                    RunState::Absent => (),
                    RunState::Stopped => match value.retire(&slot) {
                        Ok(()) => {
                            retired += 1;
                            inner.errors.remove(&id);
                        }
                        Err(_) => {
                            unconfirmed += 1;
                            inner.errors.insert(id, unknown().message);
                        }
                    },
                    RunState::Orphaned(_) => {
                        orphaned += 1;
                    }
                    RunState::Unconfirmed => {
                        unconfirmed += 1;
                        inner.errors.insert(id, unknown().message);
                    }
                },
                Err(_) => {
                    unconfirmed += 1;
                    inner.errors.insert(id, unknown().message);
                }
            }
        }
        Ok(json!({"retired":retired,"orphaned":orphaned,"unconfirmed":unconfirmed}))
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "node_run_marker_orphan_tests.rs"]
mod orphan_tests;
#[cfg(all(test, target_os = "macos"))]
#[path = "node_run_marker_tests.rs"]
mod tests;
