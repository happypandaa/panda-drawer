//! Compile-time identity. There is no runtime profile or data-root selector.
//!
//! Production metadata is inspectable without loading the installed resources.

use serde_json::{json, Value};

include!(concat!(env!("OUT_DIR"), "/build_metadata.rs"));

// Reject build-script/feature disagreement instead of allowing production
// identity constants to run under a development admission policy.
const _: () = assert!(PRODUCTION_PROFILE == cfg!(feature = "production-profile"));
const _: () = assert!(LOCAL_UPDATE_TEST_PROFILE == cfg!(feature = "local-update-test"));
pub const PRODUCTION_RESOURCE_ERROR: &str = "PRODUCTION_RESOURCE_ANCHORS_UNCONFIGURED";
pub const PRODUCTION_PROOF_ERROR: &str = "PRODUCTION_DISTRIBUTION_PROOF_UNCONFIGURED";
#[cfg(not(windows))]
pub const PRODUCTION_PROOF_MESSAGE: &str =
    "正式安装包的 Developer ID 签名、公证和内置工具资源尚未通过本机校验。";
#[cfg(windows)]
pub const PRODUCTION_PROOF_MESSAGE: &str =
    "正式 Windows 安装目录及内置 Node/Git 完整性尚未通过本机校验。";
pub const PRODUCTION_RESOURCE_MESSAGE: &str = "正式版签后资源校验锚尚未配置；此构建仅供离线检查元数据，未读取或初始化数据目录、发现记录或本机凭据，不能启动平台。";

/// No path, environment, discovery, credentials, network or resource reads.
pub fn info() -> Value {
    let support = crate::platform_support::runtime_support();
    let mut metadata = json!({
        "schemaVersion": 1,
        "profile": BUILD_PROFILE,
        "version": APP_VERSION,
        "identifier": APP_IDENTIFIER,
        "environment": ENVIRONMENT,
        "displayName": DISPLAY_NAME,
        "distribution": DISTRIBUTION,
        "target": BUILD_TARGET,
        "minimumSystemVersion": MINIMUM_SYSTEM_VERSION,
        "profileSha256": if PROFILE_DIGEST.is_empty() { None } else { Some(PROFILE_DIGEST) },
        "releaseRepository": if RELEASE_REPOSITORY.is_empty() { None } else { Some(RELEASE_REPOSITORY) },
        "cargoPackageVersion": env!("CARGO_PKG_VERSION"),
        "resourceAnchors": if crate::production_resources::COMPILED_RESOURCE_ANCHORS { if cfg!(windows) { "compiled-windows-tools-v1" } else { "compiled-signed-tools-v1" } } else if PRODUCTION_PROFILE { "unconfigured" } else { "development-source-v1" },
        "resourceManifestSha256": if crate::production_resources::COMPILED_RESOURCE_ANCHORS { Some(crate::production_resources::COMPILED_RESOURCE_MANIFEST_SHA256) } else { None },
        "signatureEvidenceVerified": PRODUCTION_PROFILE && cfg!(target_os = "macos") && support.runtime_available,
        "sourceTransformationVerified": false,
        "releaseReady": false,
        "runtimeAdmission": support.runtime_available,
        "runtimeErrorCode": support.error_code,
    });
    #[cfg(windows)]
    {
        metadata["installationIntegrityVerified"] =
            json!(PRODUCTION_PROFILE && support.runtime_available);
    }
    if !UPDATE_TEST_RELEASE_TAG.is_empty() {
        metadata["updateTestReleaseTag"] = json!(UPDATE_TEST_RELEASE_TAG);
    }
    metadata
}

/// Validate the desktop's actual package metadata before data-root I/O.
pub fn validate_desktop_identity(identifier: &str, version: &str) -> Result<(), &'static str> {
    if identifier != APP_IDENTIFIER || version != APP_VERSION {
        return Err("Desktop package identity/version and compiled platform metadata do not match");
    }
    Ok(())
}

#[path = "../build_support/profile.rs"]
pub(crate) mod profile_schema;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_version_and_desktop_package_must_match() {
        assert!(validate_desktop_identity(APP_IDENTIFIER, APP_VERSION).is_ok());
        assert!(validate_desktop_identity("other.application", APP_VERSION).is_err());
        assert!(validate_desktop_identity(APP_IDENTIFIER, "99.99.99").is_err());
        assert_eq!(info()["version"], APP_VERSION);
        assert_eq!(info()["releaseReady"], false);
    }

    #[test]
    fn compiled_profile_preserves_dev_or_binds_confirmed_production() {
        let metadata = info();
        if PRODUCTION_PROFILE {
            assert_eq!(APP_VERSION, profile_schema::RELEASE_VERSION_FILE.trim_end());
            assert_eq!(APP_IDENTIFIER, "com.happypandaa.pandadrawer");
            assert_eq!(ENVIRONMENT, "production");
            assert_eq!(DISPLAY_NAME, "熊猫小抽屉");
            assert_eq!(DISTRIBUTION, "production-candidate");
            assert_eq!(
                metadata["resourceAnchors"],
                if crate::production_resources::COMPILED_RESOURCE_ANCHORS {
                    if cfg!(windows) {
                        "compiled-windows-tools-v1"
                    } else {
                        "compiled-signed-tools-v1"
                    }
                } else {
                    "unconfigured"
                }
            );
            assert_eq!(metadata["runtimeAdmission"], false);
            assert_eq!(
                metadata["runtimeErrorCode"],
                crate::platform_support::runtime_support()
                    .error_code
                    .unwrap()
            );
            assert_eq!(PROFILE_DIGEST.len(), 64);
        } else if LOCAL_UPDATE_TEST_PROFILE {
            assert!(matches!(APP_VERSION, "0.1.0" | "0.1.1"));
            assert_eq!(ENVIRONMENT, "local-update-test");
            if UPDATE_TEST_RELEASE_TAG.is_empty() {
                assert_eq!(APP_IDENTIFIER, "test.happypandaa.pandadrawer.update");
                assert_eq!(DISTRIBUTION, "isolated-local-update-test");
            } else {
                assert_eq!(metadata["updateTestReleaseTag"], UPDATE_TEST_RELEASE_TAG);
                if UPDATE_TEST_RELEASE_TAG.starts_with("auto-update-dual-test-") {
                    assert_eq!(APP_IDENTIFIER, "test.happypandaa.pandadrawer.dual-update");
                    assert_eq!(DISPLAY_NAME, "熊猫小抽屉 · 双线路更新测试版");
                    assert_eq!(DISTRIBUTION, "isolated-dual-update-test");
                    assert_eq!(
                        metadata["releaseRepository"],
                        "https://github.com/happypandaa/panda-drawer"
                    );
                } else if UPDATE_TEST_RELEASE_TAG.starts_with("auto-update-oss-test-") {
                    assert_eq!(APP_IDENTIFIER, "test.happypandaa.pandadrawer.oss-update");
                    assert_eq!(DISPLAY_NAME, "熊猫小抽屉 · 国内更新测试版");
                    assert_eq!(DISTRIBUTION, "isolated-oss-update-test");
                    assert!(metadata["releaseRepository"].is_null());
                } else {
                    assert_eq!(APP_IDENTIFIER, "test.happypandaa.pandadrawer.online-update");
                    assert_eq!(DISTRIBUTION, "isolated-online-update-test");
                }
            }
            assert!(PROFILE_DIGEST.is_empty());
        } else {
            assert_eq!(APP_VERSION, "0.0.0");
            assert_eq!(APP_IDENTIFIER, "dev.happypandaa.pandadrawer");
            assert_eq!(ENVIRONMENT, "development");
            assert_eq!(DISPLAY_NAME, "熊猫小抽屉 · 开发版");
            assert_eq!(DISTRIBUTION, "internal-development");
            assert!(PROFILE_DIGEST.is_empty());
            assert_eq!(metadata["resourceAnchors"], "development-source-v1");
        }
    }
}
