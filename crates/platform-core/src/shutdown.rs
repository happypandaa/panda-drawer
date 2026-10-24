//! Shutdown diagnostics are not updater-install permissions.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShutdownIssue {
    pub component: String,
    pub code: String,
    pub message: String,
    pub resource_closed: bool,
    pub retryable: bool,
}
impl ShutdownIssue {
    pub(crate) fn new(
        component: &str,
        code: &str,
        message: impl Into<String>,
        resource_closed: bool,
        retryable: bool,
    ) -> Self {
        Self {
            component: component.into(),
            code: code.into(),
            message: message.into(),
            resource_closed,
            retryable,
        }
    }
    pub(crate) fn join(component: &str, error: impl std::fmt::Display) -> Self {
        Self::new(
            component,
            "SHUTDOWN_JOIN_FAILED",
            error.to_string(),
            false,
            false,
        )
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ShutdownStatus {
    Pending,
    Blocked,
    Complete,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShutdownReport {
    pub status: ShutdownStatus,
    pub phase: String,
    pub attempt: u64,
    pub resource_closed: bool,
    pub issues: Vec<ShutdownIssue>,
}
impl Default for ShutdownReport {
    fn default() -> Self {
        Self {
            status: ShutdownStatus::Pending,
            phase: "running".into(),
            attempt: 0,
            resource_closed: false,
            issues: Vec::new(),
        }
    }
}
impl ShutdownReport {
    pub fn is_complete(&self) -> bool {
        self.status == ShutdownStatus::Complete && self.resource_closed && self.issues.is_empty()
    }
    pub fn into_result(self) -> anyhow::Result<()> {
        if self.is_complete() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "SERVICE_SHUTDOWN_{:?}: phase={}, resourceClosed={}, issues={:?}",
                self.status,
                self.phase,
                self.resource_closed,
                self.issues
            ))
        }
    }
}
/// Collect every runner failure; a missing handle on retry never erases it.
pub(crate) async fn join_all(
    component: &str,
    runners: Vec<tokio::task::JoinHandle<()>>,
) -> Vec<ShutdownIssue> {
    let mut issues = Vec::new();
    for runner in runners {
        if let Err(error) = runner.await {
            issues.push(ShutdownIssue::join(component, error));
        }
    }
    issues
}
