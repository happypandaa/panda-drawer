//! Build-time metadata only. This schema cannot enable a production runtime.

use serde::Deserialize;
use sha2::{Digest, Sha256};

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
        // An explicit allowlist binds this first foundation to the confirmed
        // release. Extending it requires a reviewed source change, never an env
        // variable, arbitrary config path, data-root setting or ready flag.
        if profile.schema_version != 1
            || profile.profile != "production"
            || profile.version != "0.1.0"
            || profile.identifier != "com.happypandaa.pandadrawer"
            || profile.environment != "production"
            || profile.display_name != "熊猫小抽屉"
            || profile.distribution != "production-candidate"
            || profile.release_repository != "https://github.com/happypandaa/panda-drawer-releases"
            || profile.target != "aarch64-apple-darwin"
            || profile.target != target
            || profile.minimum_system_version != "15.0"
            || profile.resource_anchors != "unconfigured"
        {
            return Err("Production profile does not match the confirmed metadata-only foundation; signed resource anchors are not implemented".into());
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

    #[test]
    fn confirmed_profile_has_one_fixed_identity_and_target() {
        let profile = ProductionProfile::parse(PROFILE, "aarch64-apple-darwin").unwrap();
        assert_eq!(profile.version, "0.1.0");
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
            ("version", serde_json::json!("0.2.0")),
            (
                "identifier",
                serde_json::json!("dev.localprojects.platform.m0"),
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
