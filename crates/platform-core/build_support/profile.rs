//! Build-time metadata only. This schema cannot enable a production runtime.

use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const RELEASE_VERSION_FILE: &str =
    include_str!("../../../build-profiles/production-version.txt");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductionProfile {
    pub schema_version: u32,
    pub profile: String,
    pub version: String,
    pub identifier: String,
    pub environment: String,
    pub display_name: String,
    pub distribution: String,
    pub release_repository: String,
    pub target: String,
    pub minimum_system_version: String,
    pub resource_anchors: String,
}

impl ProductionProfile {
    pub fn parse(bytes: &[u8], target: &str) -> Result<Self, String> {
        let profile: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        // An explicit allowlist binds each target profile to the confirmed
        // release. Extending it requires a reviewed source change, never an env
        // variable, arbitrary config path, data-root setting or ready flag.
        let supported_target = match target {
            "aarch64-apple-darwin" => profile.minimum_system_version == "15.0",
            "x86_64-pc-windows-msvc" => profile.minimum_system_version == "10.0.19045",
            _ => false,
        };
        if profile.schema_version != 1
            || profile.profile != "production"
            || profile.version != RELEASE_VERSION_FILE.trim_end()
            || profile.identifier != "com.happypandaa.pandadrawer"
            || profile.environment != "production"
            || profile.display_name != "熊猫小抽屉"
            || profile.distribution != "production-candidate"
            || profile.release_repository != "https://github.com/happypandaa/panda-drawer"
            || profile.target != target
            || !supported_target
            || profile.resource_anchors != "unconfigured"
        {
            return Err("Production profile does not match the fixed identity, release version and supported target".into());
        }
        Ok(profile)
    }
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: &[u8] = include_bytes!("../../../build-profiles/production.json");
    const WINDOWS_PROFILE: &[u8] =
        include_bytes!("../../../build-profiles/production.windows-x64.json");

    #[test]
    fn windows_production_profile_is_fixed_and_cannot_be_used_for_another_target() {
        let profile = ProductionProfile::parse(WINDOWS_PROFILE, "x86_64-pc-windows-msvc").unwrap();
        assert_eq!(profile.identifier, "com.happypandaa.pandadrawer");
        assert_eq!(profile.minimum_system_version, "10.0.19045");
        assert!(ProductionProfile::parse(WINDOWS_PROFILE, "aarch64-apple-darwin").is_err());
        assert!(ProductionProfile::parse(WINDOWS_PROFILE, "aarch64-pc-windows-msvc").is_err());
        let mut modified: serde_json::Value = serde_json::from_slice(WINDOWS_PROFILE).unwrap();
        modified["minimumSystemVersion"] = serde_json::json!("10");
        assert!(ProductionProfile::parse(
            &serde_json::to_vec(&modified).unwrap(),
            "x86_64-pc-windows-msvc"
        )
        .is_err());
    }

    #[test]
    fn confirmed_profile_has_one_fixed_identity_and_target() {
        let profile = ProductionProfile::parse(PROFILE, "aarch64-apple-darwin").unwrap();
        assert_eq!(profile.version, RELEASE_VERSION_FILE.trim_end());
        assert_eq!(profile.identifier, "com.happypandaa.pandadrawer");
        assert_eq!(digest(PROFILE).len(), 64);
        assert!(ProductionProfile::parse(PROFILE, "x86_64-pc-windows-msvc").is_err());
    }

    #[test]
    fn profile_rejects_each_identity_version_anchor_and_schema_override() {
        let original: serde_json::Value = serde_json::from_slice(PROFILE).unwrap();
        for (key, replacement) in [
            ("schemaVersion", serde_json::json!(2)),
            ("profile", serde_json::json!("development")),
            ("version", serde_json::json!("0.0.0")),
            (
                "identifier",
                serde_json::json!("dev.happypandaa.pandadrawer"),
            ),
            ("environment", serde_json::json!("development")),
            ("displayName", serde_json::json!("Other")),
            ("distribution", serde_json::json!("production")),
            (
                "releaseRepository",
                serde_json::json!("https://example.org/releases"),
            ),
            ("target", serde_json::json!("x86_64-apple-darwin")),
            ("minimumSystemVersion", serde_json::json!("14.0")),
            ("resourceAnchors", serde_json::json!("ready")),
            ("releaseReady", serde_json::json!(true)),
            ("dataRoot", serde_json::json!("/arbitrary/root")),
        ] {
            let mut modified = original.clone();
            modified[key] = replacement;
            assert!(
                ProductionProfile::parse(
                    &serde_json::to_vec(&modified).unwrap(),
                    "aarch64-apple-darwin"
                )
                .is_err(),
                "accepted {key}"
            );
        }
        let mut missing = original;
        missing.as_object_mut().unwrap().remove("resourceAnchors");
        assert!(ProductionProfile::parse(
            &serde_json::to_vec(&missing).unwrap(),
            "aarch64-apple-darwin"
        )
        .is_err());
        assert_ne!(digest(PROFILE), digest(b"different profile"));
    }
}
