//! Compile-time identity. There is no runtime profile or data-root selector.
//!
//! Production metadata is inspectable before signed tool resources exist. This
//! first foundation deliberately cannot admit production runtime operations.

use serde_json::{json, Value};

include!(concat!(env!("OUT_DIR"), "/build_metadata.rs"));

// Reject build-script/feature disagreement instead of allowing production
// identity constants to run under a development admission policy.
const _: () = assert!(PRODUCTION_PROFILE == cfg!(feature = "production-profile"));
pub const PRODUCTION_RESOURCE_ERROR: &str = "PRODUCTION_RESOURCE_ANCHORS_UNCONFIGURED";
pub const PRODUCTION_PROOF_ERROR: &str = "PRODUCTION_DISTRIBUTION_PROOF_UNCONFIGURED";
pub const PRODUCTION_PROOF_MESSAGE: &str = "此构建已绑定资源摘要，但真实签名前后来源链、Developer ID 和分发验证尚未完成；不会读取或初始化生产数据、发现记录或凭据。";
pub const PRODUCTION_RESOURCE_MESSAGE: &str = "正式版签后资源校验锚尚未配置；此构建仅供离线检查元数据，未读取或初始化数据目录、发现记录或本机凭据，不能启动平台。";

/// No path, environment, discovery, credentials, network or resource reads.
pub fn info() -> Value {
    json!({
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
        "resourceAnchors": if crate::production_resources::COMPILED_RESOURCE_ANCHORS { "compiled-signed-tools-v1" } else if PRODUCTION_PROFILE { "unconfigured" } else { "development-source-v1" },
        "resourceManifestSha256": if crate::production_resources::COMPILED_RESOURCE_ANCHORS { Some(crate::production_resources::COMPILED_RESOURCE_MANIFEST_SHA256) } else { None },
        "signatureEvidenceVerified": false,
        "sourceTransformationVerified": false,
        "releaseReady": false,
        "runtimeAdmission": crate::platform_support::runtime_support().runtime_available,
        "runtimeErrorCode": crate::platform_support::runtime_support().error_code,
    })
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
            assert_eq!(APP_VERSION, "0.1.0");
            assert_eq!(APP_IDENTIFIER, "com.happypandaa.pandadrawer");
            assert_eq!(ENVIRONMENT, "production");
            assert_eq!(DISPLAY_NAME, "熊猫小抽屉");
            assert_eq!(DISTRIBUTION, "production-candidate");
            assert_eq!(
                metadata["resourceAnchors"],
                if crate::production_resources::COMPILED_RESOURCE_ANCHORS {
                    "compiled-signed-tools-v1"
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
