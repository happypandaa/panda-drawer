//! Windows marker observation. A previous host's Job is not recoverable from
//! its numeric root PID. Live owned runs are stopped through their Job owner.
use crate::ApiError;
use platform_native::windows::process::{self, ProcessIdentity, ProcessObservation};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, time::SystemTime};

pub(crate) const RUN_IDENTITY_ENV: &str = "PANDA_DRAWER_RUN_IDENTITY";
const WINDOWS_UNIX_EPOCH_100NS: u64 = 116_444_736_000_000_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RuntimeProcessIdentity {
    pub pid: u32,
    /// Existing caller's root label; Windows never treats it as a process group.
    pub pgid: u32,
    pub started_at_sec: u64,
    pub started_at_usec: u64,
    pub executable: PathBuf,
    pub nonce: String,
    // Required: old experimental records and microsecond-only identities do not
    // authorize Windows recovery. Retain every bit supplied by GetProcessTimes.
    pub windows_creation_100ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Observation {
    #[allow(dead_code)] // Shared caller variant: Windows cannot adopt a lost Job.
    Matching,
    Stopped,
    Unconfirmed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StopReceipt {
    pub forced: bool,
}

fn unconfirmed() -> ApiError {
    ApiError::new(
        "NODE_PROCESS_IDENTITY_UNCONFIRMED",
        "无法完整核实遗留应用的运行身份，请重启电脑后重试。",
    )
    .outcome("not_deleted")
}

fn unix_parts(creation_100ns: u64) -> Result<(u64, u64), ApiError> {
    let unix = creation_100ns
        .checked_sub(WINDOWS_UNIX_EPOCH_100NS)
        .ok_or_else(unconfirmed)?;
    Ok((unix / 10_000_000, (unix % 10_000_000) / 10))
}

fn validate(value: &RuntimeProcessIdentity) -> Result<(), ApiError> {
    let parts = unix_parts(value.windows_creation_100ns)?;
    if value.pid <= 1
        || value.pgid != value.pid
        || parts.0 == 0
        || parts != (value.started_at_sec, value.started_at_usec)
        || !value.executable.is_absolute()
        || !(16..=128).contains(&value.nonce.len())
        || !value
            .nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(unconfirmed());
    }
    Ok(())
}

pub(crate) fn boot_time() -> Result<SystemTime, ApiError> {
    process::boot_time().map_err(|_| {
        ApiError::new(
            "NODE_BOOT_TIME_UNCONFIRMED",
            "无法核对本次系统开机时间，运行记录保持不变。",
        )
    })
}

/// The caller must retain its direct child's actual owner across this read.
/// Reading an arbitrary PID is never a way to acquire a Job or stop authority.
pub(crate) fn capture_owned(pid: u32, nonce: &str) -> Result<RuntimeProcessIdentity, ApiError> {
    let native = process::capture_live(pid)
        .map_err(|_| unconfirmed())?
        .ok_or_else(unconfirmed)?;
    let (started_at_sec, started_at_usec) = unix_parts(native.creation_100ns)?;
    let value = RuntimeProcessIdentity {
        pid,
        pgid: pid,
        started_at_sec,
        started_at_usec,
        executable: native.executable,
        nonce: nonce.into(),
        windows_creation_100ns: native.creation_100ns,
    };
    validate(&value)?;
    Ok(value)
}

pub(crate) fn observe(value: &RuntimeProcessIdentity) -> Result<Observation, ApiError> {
    validate(value)?;
    let native = ProcessIdentity {
        pid: value.pid,
        creation_100ns: value.windows_creation_100ns,
        executable: value.executable.clone(),
    };
    match process::observation(&native).map_err(|_| unconfirmed())? {
        ProcessObservation::Stopped => Ok(Observation::Stopped),
        // A live root (matching or reused) never proves a prior host's whole
        // Job has drained. Never signal it or pretend to adopt its descendants.
        ProcessObservation::Matching | ProcessObservation::Different => {
            Ok(Observation::Unconfirmed)
        }
    }
}

pub(crate) async fn stop(value: &RuntimeProcessIdentity) -> Result<StopReceipt, ApiError> {
    if observe(value)? == Observation::Stopped {
        return Ok(StopReceipt { forced: false });
    }
    Err(unconfirmed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_native::windows::process::{OwnedProcess, Spawn};
    use std::time::Duration;

    fn child() -> (tempfile::TempDir, OwnedProcess) {
        let temp = tempfile::Builder::new()
            .prefix("panda-disposable-process-identity-中文 space-")
            .tempdir()
            .unwrap();
        let process = OwnedProcess::spawn(Spawn {
            program: std::env::current_exe().unwrap(),
            arguments: [
                "--ignored",
                "--exact",
                "node_process_identity::windows::tests::fixture_child",
                "--nocapture",
            ]
            .into_iter()
            .map(Into::into)
            .collect(),
            current_directory: temp.path().canonicalize().unwrap(),
            environment: std::env::vars_os().collect(),
        })
        .unwrap();
        (temp, process)
    }

    #[test]
    #[ignore = "entry point for a disposable identity-observation child"]
    fn fixture_child() {
        std::thread::sleep(Duration::from_secs(60));
    }

    #[tokio::test]
    async fn native_capture_keeps_full_precision_and_never_adopts_or_kills_a_live_pid() {
        let (_temp, child) = child();
        let identity = capture_owned(child.pid, "disposable-identity-nonce").unwrap();
        assert_eq!(
            identity.windows_creation_100ns,
            child.identity().unwrap().creation_100ns
        );
        assert_eq!(observe(&identity).unwrap(), Observation::Unconfirmed);
        assert!(stop(&identity).await.is_err());
        assert!(child.exit_code().unwrap().is_none());
        let mut different = identity.clone();
        different.windows_creation_100ns += 1;
        (different.started_at_sec, different.started_at_usec) =
            unix_parts(different.windows_creation_100ns).unwrap();
        assert_eq!(observe(&different).unwrap(), Observation::Unconfirmed);
        assert!(stop(&different).await.is_err());
        assert!(child.exit_code().unwrap().is_none());
        child.terminate(0).unwrap();
        assert_eq!(child.wait(Duration::from_secs(5)).unwrap(), Some(0));
        assert!(child.wait_empty(Duration::from_secs(5)).unwrap());
        assert_eq!(observe(&identity).unwrap(), Observation::Stopped);
        assert!(!stop(&identity).await.unwrap().forced);
    }

    #[test]
    fn persisted_record_requires_full_windows_time_and_round_trips_without_loss() {
        let (_temp, child) = child();
        let identity = capture_owned(child.pid, "disposable-identity-nonce").unwrap();
        let bytes = serde_json::to_vec(&identity).unwrap();
        assert_eq!(
            serde_json::from_slice::<RuntimeProcessIdentity>(&bytes).unwrap(),
            identity
        );
        let mut legacy = serde_json::to_value(&identity).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("windowsCreation100ns");
        assert!(serde_json::from_value::<RuntimeProcessIdentity>(legacy).is_err());
        assert!(capture_owned(child.pid, "short").is_err());
    }

    #[test]
    fn boot_estimate_is_past_and_invalid_numeric_processes_are_rejected() {
        let boot = boot_time().unwrap();
        assert!(boot < SystemTime::now());
        assert!(process::capture_live(0).is_err());
        assert!(process::capture_live(1).is_err());
    }
}
