//! Application-runtime adapter over the shared native Job implementation.
//! Protocol behavior and the established TRIAL errors remain local to Node.
use super::{ExitObservation, TrialResult};
use platform_host::windows_process as native;
use std::{collections::BTreeMap, ffi::OsString, path::Path, process::ExitStatus, sync::Arc};

fn trial_error(mut error: crate::ApiError) -> crate::ApiError {
    if let Some(code) = error.code.strip_prefix("JOB_") {
        error.code = format!("TRIAL_{code}");
    }
    error
}
pub(super) fn windows_directory() -> TrialResult<OsString> {
    native::windows_directory().map_err(trial_error)
}
pub(super) struct ProcessOwnership {
    inner: Arc<native::JobOwnership>,
    #[cfg(test)]
    process: std::os::windows::io::OwnedHandle,
}
impl ProcessOwnership {
    pub(super) fn observe(&self) -> TrialResult<Option<ExitObservation>> {
        self.inner
            .observe()
            .map(|value| value.map(|exit| ExitObservation { code: exit.code }))
            .map_err(trial_error)
    }
    pub(super) fn signal(&self, _signal: i32) -> TrialResult<()> {
        // Preserve the runtime's refusal to signal after ownership observation
        // becomes uncertain, while the generic executor owns its own policy.
        self.inner.observe().map_err(trial_error)?;
        self.inner.terminate().map_err(trial_error)
    }
    pub(super) fn has_descendants(&self) -> TrialResult<bool> {
        self.inner.has_descendants().map_err(trial_error)
    }
    pub(super) fn disarm_before_reap(&self) {
        self.inner.disarm_before_reap();
    }
    #[cfg(test)]
    fn active(&self) -> TrialResult<u32> {
        self.inner.active_processes().map_err(trial_error)
    }
}
pub(super) struct Child {
    inner: native::JobChild,
    ownership: Arc<ProcessOwnership>,
    pub(super) stdin: Option<tokio::fs::File>,
    pub(super) stdout: Option<tokio::fs::File>,
    pub(super) stderr: Option<tokio::fs::File>,
}
impl Child {
    fn from_native(mut inner: native::JobChild) -> TrialResult<Self> {
        let ownership = inner.ownership();
        Ok(Self {
            ownership: Arc::new(ProcessOwnership {
                #[cfg(test)]
                process: ownership.leader_handle().map_err(|_| {
                    super::fail(
                        "TRIAL_PROCESS_FAILED",
                        "Could not retain the owned process handle",
                    )
                })?,
                inner: ownership,
            }),
            stdin: inner.stdin.take(),
            stdout: inner.stdout.take(),
            stderr: inner.stderr.take(),
            inner,
        })
    }
    pub(super) fn spawn(
        executable: &Path,
        args: &[OsString],
        cwd: &Path,
        values: &BTreeMap<OsString, OsString>,
    ) -> TrialResult<Self> {
        Self::from_native(
            native::JobChild::spawn(executable, args, cwd, values).map_err(trial_error)?,
        )
    }
    pub(super) fn id(&self) -> Option<u32> {
        self.inner.id()
    }
    pub(super) fn ownership(&self) -> Arc<ProcessOwnership> {
        self.ownership.clone()
    }
    pub(super) async fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.inner.wait().await
    }
    pub(super) async fn wait_for_tree_exit(&self) {
        self.inner.wait_for_tree_exit().await;
    }
    pub(super) fn wait_for_tree_exit_blocking(&self) {
        self.inner.wait_for_tree_exit_blocking();
    }
    #[cfg(test)]
    fn spawn_inner(
        executable: &Path,
        args: &[OsString],
        cwd: &Path,
        values: &BTreeMap<OsString, OsString>,
        reject_assignment: bool,
    ) -> TrialResult<Self> {
        if reject_assignment {
            Self::from_native(
                native::testing::spawn_rejecting_assignment(executable, args, cwd, values)
                    .map_err(trial_error)?,
            )
        } else {
            Self::spawn(executable, args, cwd, values)
        }
    }
}
#[cfg(test)]
use native::testing::{environment_block as environment, quote_argument as quote};
#[cfg(test)]
#[path = "windows_process_tests.rs"]
pub(super) mod tests;
