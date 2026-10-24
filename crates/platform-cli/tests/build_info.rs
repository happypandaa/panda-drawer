use serde_json::Value;
use std::process::Command;

fn cli(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_local-projects-dev"))
        .args(arguments)
        // These unsupported values must never select another identity/root or
        // turn this foundation into an admitted production runtime.
        .env("PANDA_DRAWER_IDENTIFIER", "attacker.application")
        .env("PANDA_DRAWER_VERSION", "99.99.99")
        .env("PANDA_DRAWER_ENVIRONMENT", "other")
        .env(
            "PANDA_DRAWER_DATA_ROOT",
            "/nonexistent/disposable-ignored-root",
        )
        .env("PANDA_DRAWER_RELEASE_READY", "true")
        .output()
        .unwrap()
}

#[test]
fn build_info_and_version_are_offline_and_share_compiled_metadata() {
    let output = cli(&["build-info", "--json"]);
    assert!(output.status.success(), "{:?}", output);
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["ok"], true);
    assert_eq!(response["data"], platform_core::build_profile::info());
    assert_eq!(response["data"]["releaseReady"], false);
    assert!(response["data"].get("dataRoot").is_none());
    assert!(response["data"].get("workspaceRoot").is_none());
    let version = cli(&["--version"]);
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        format!("local-projects-dev {}", platform_core::APP_VERSION)
    );
    let help = cli(&["--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    if platform_core::build_profile::PRODUCTION_PROFILE {
        assert!(help.contains(
            if platform_core::production_resources::COMPILED_RESOURCE_ANCHORS {
                "signature and distribution proof is unavailable"
            } else {
                "Signed resource anchors are not configured"
            }
        ));
        assert!(!help.contains("connects to a production environment"));
    } else {
        assert!(
            help.contains("never starts the desktop app or connects to a production environment")
        );
    }
}

#[test]
#[cfg(feature = "production-profile")]
fn every_production_command_family_is_blocked_before_command_input_or_discovery() {
    let absent = "/nonexistent/disposable-production-profile-no-input.json";
    for arguments in [
        vec!["service", "status", "--json"],
        vec!["env", "doctor", "--json"],
        vec!["env", "git", "doctor", "--json"],
        vec!["env", "node", "clear", "--json"],
        vec!["source", "inspect", "--path", absent, "--json"],
        vec!["project", "create", "--input", absent, "--json"],
        vec![
            "db",
            "execute",
            "--project",
            "project_a",
            "--input",
            absent,
            "--json",
        ],
        vec!["task", "get", "--task", "task_a", "--json"],
        vec!["backup", "list", "--project", "project_a", "--json"],
        vec!["maintenance", "status", "--project", "project_a", "--json"],
        vec!["trash", "list", "--json"],
        vec!["storage", "info", "--project", "project_a", "--json"],
        vec!["workspace", "identity-repair-plan", "--json"],
        vec!["workspace", "identity-repair", "--input", absent, "--json"],
    ] {
        let output = cli(&arguments);
        assert_eq!(output.status.code(), Some(1), "{arguments:?}: {output:?}");
        let response: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|_| panic!("{arguments:?}: {output:?}"));
        assert_eq!(response["ok"], false);
        assert_eq!(
            response["error"]["code"],
            platform_core::platform_support::runtime_support()
                .error_code
                .unwrap(),
            "{arguments:?}"
        );
        assert_eq!(
            response["error"]["details"]["operationOutcome"],
            "not_started"
        );
        assert_eq!(response["error"]["retryable"], false);
    }
    assert_eq!(
        platform_core::build_profile::info()["releaseRepository"],
        serde_json::json!("https://github.com/happypandaa/panda-drawer-releases")
    );
}
