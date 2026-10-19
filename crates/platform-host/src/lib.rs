//! Read-only host observations, independent of the business runtime.
//!
//! A report never grants access to project data or permission to initialize,
//! recover, migrate, clean up, or start the existing platform runtime.
use serde::{Deserialize, Serialize};

#[cfg(windows)]
mod webview_policy;
#[cfg(windows)]
pub use windows::WebviewCache;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub mod windows_projects;
#[cfg(windows)]
pub mod windows_runtime;
#[cfg(windows)]
mod windows_workspace;
#[cfg(windows)]
pub use windows_runtime::open_project_browser;
#[cfg(windows)]
pub use windows_workspace::{
    fixed_workspace_data_root, WindowsObjectIdentity, WindowsWorkspace, WorkspaceDirectory,
    WorkspaceEntry, WorkspaceError, WorkspaceFile, WorkspaceLock,
};

// Native probes briefly hold strict ancestor handles. Their fixtures share
// ancestry with the workspace tests, so serialize only actual filesystem tests.
#[cfg(all(test, windows))]
pub(crate) static WINDOWS_FILESYSTEM_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub mod platform_support;

pub const APP_IDENTIFIER: &str = "dev.localprojects.platform.m0";
pub const DISPLAY_NAME: &str = "Local Projects Dev";
pub const ENVIRONMENT: &str = "development";
pub const API_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostReport {
    pub schema_version: u32,
    pub identifier: String,
    pub environment: String,
    pub platform: String,
    pub architecture: String,
    pub stage: String,
    pub runtime_available: bool,
    pub probe: ProbeReport,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReport {
    pub data_root: Option<String>,
    /// missing, retained, blocked, or unavailable; none is runtime admission.
    pub status: String,
    pub checks: Vec<HostCheck>,
    pub evidence: Vec<EvidenceCount>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCheck {
    pub code: String,
    pub status: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceCount {
    pub kind: String,
    pub count: u32,
}

fn report(probe: ProbeReport) -> HostReport {
    HostReport {
        schema_version: 1,
        identifier: APP_IDENTIFIER.into(),
        environment: ENVIRONMENT.into(),
        platform: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        stage: "readOnly".into(),
        runtime_available: false,
        probe,
    }
}

/// Observe the current host without constructing any platform manager.
/// A missing root is an observation, not authorization to create one.
pub fn inspect_host() -> HostReport {
    #[cfg(windows)]
    let probe = windows::inspect();
    #[cfg(not(windows))]
    let probe = ProbeReport {
        data_root: None,
        status: "unavailable".into(),
        checks: vec![HostCheck {
            code: "HOST_PROBE_UNAVAILABLE".into(),
            status: "unavailable".into(),
            message: "The Windows host observation is unavailable on this platform.".into(),
        }],
        evidence: vec![],
    };
    report(probe)
}

/// Allocate a private, disposable browser cache without creating the platform
/// data root or adopting an existing directory. Business data never uses it.
#[cfg(windows)]
pub fn create_webview_cache() -> Result<WebviewCache, String> {
    webview_policy::check()?;
    windows::create_webview_cache()
}

#[cfg(windows)]
pub fn check_webview_overrides() -> Result<(), String> {
    webview_policy::check()
}

#[cfg(windows)]
pub fn show_startup_error(message: &str) {
    webview_policy::show_startup_error(message);
}

impl From<platform_support::RuntimeSupportError> for platform_project_format::ApiError {
    fn from(value: platform_support::RuntimeSupportError) -> Self {
        Self::new(value.code, value.message)
            .detail("platform", value.platform.as_str())
            .detail("capability", "runtime")
            .outcome("not_started")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn synthetic_probe(status: &str) -> ProbeReport {
        ProbeReport {
            data_root: Some("synthetic-retained-root".into()),
            status: status.into(),
            checks: vec![HostCheck {
                code: "SYNTHETIC_OBSERVATION".into(),
                status: status.into(),
                message: "Synthetic metadata; no filesystem was inspected.".into(),
            }],
            evidence: vec![EvidenceCount {
                kind: "retainedEntry".into(),
                count: 2,
            }],
        }
    }

    #[test]
    fn every_probe_state_keeps_the_business_runtime_closed() {
        for status in ["missing", "retained", "blocked", "unavailable"] {
            let report = report(synthetic_probe(status));
            assert!(!report.runtime_available);
            assert_eq!(report.stage, "readOnly");
            assert_eq!(report.schema_version, 1);
            assert_eq!(report.identifier, APP_IDENTIFIER);
            assert_eq!(report.environment, ENVIRONMENT);
            assert_eq!(report.platform, std::env::consts::OS);
            assert_eq!(report.architecture, std::env::consts::ARCH);
            assert_eq!(report.probe.status, status);
        }
    }

    #[test]
    fn serialized_report_has_a_stable_camel_case_contract_without_admission_fields() {
        let value = serde_json::to_value(report(synthetic_probe("retained"))).unwrap();
        assert_eq!(value["schemaVersion"], json!(1));
        assert_eq!(value["runtimeAvailable"], json!(false));
        assert_eq!(value["stage"], json!("readOnly"));
        assert_eq!(value["probe"]["dataRoot"], json!("synthetic-retained-root"));
        assert_eq!(
            value["probe"]["checks"][0]["code"],
            json!("SYNTHETIC_OBSERVATION")
        );
        assert_eq!(
            value["probe"]["evidence"][0],
            json!({"kind":"retainedEntry","count":2})
        );
        for field in [
            "schema_version",
            "runtime_available",
            "ready",
            "allow",
            "permit",
        ] {
            assert!(value.get(field).is_none(), "unexpected field: {field}");
        }
        let decoded: HostReport = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    }

    #[test]
    fn absent_root_observation_serializes_as_null_without_an_empty_path() {
        let mut probe = synthetic_probe("missing");
        probe.data_root = None;
        probe.evidence.clear();
        let value = serde_json::to_value(report(probe)).unwrap();
        assert!(value["probe"]["dataRoot"].is_null());
        assert_eq!(value["probe"]["evidence"], json!([]));
        assert_eq!(value["runtimeAvailable"], json!(false));
    }

    #[cfg(not(windows))]
    #[test]
    fn another_platform_does_not_claim_to_have_inspected_windows() {
        let report = inspect_host();
        assert_ne!(report.platform, "windows");
        assert_eq!(report.probe.status, "unavailable");
        assert!(report.probe.data_root.is_none());
        assert!(report.probe.evidence.is_empty());
        assert_eq!(report.probe.checks[0].code, "HOST_PROBE_UNAVAILABLE");
        assert!(!report.runtime_available);
    }
}
