//! Immutable compile anchors. Adjacent package metadata is never a trust input.
//! Digest/signature-field checks do not prove real signing or a source-to-signed
//! transaction. Even configured anchors cannot enable production runtime yet.

use crate::build_profile::profile_schema as profile;
#[allow(dead_code)]
#[path = "../build_support/signed_resources.rs"]
mod protocol;

use crate::ApiError;
use serde_json::{json, Value};
use std::path::Path;

pub use protocol::PayloadKind;

include!(concat!(env!("OUT_DIR"), "/compiled_signed_tool_config.rs"));
const MANIFEST: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/compiled_signed_tool_manifest.json"
));
const _: () = assert!(COMPILED_RESOURCE_ANCHORS == cfg!(feature = "production-resource-anchors"));

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new("PRODUCTION_RESOURCE_ANCHOR_INVALID", message).outcome("not_started")
}

fn compiled() -> Result<protocol::Manifest, ApiError> {
    if !COMPILED_RESOURCE_ANCHORS || MANIFEST.is_empty() {
        return Err(ApiError::new(
            crate::build_profile::PRODUCTION_RESOURCE_ERROR,
            "此程序未编译正式签后资源锚。",
        )
        .outcome("not_started"));
    }
    if protocol::digest(MANIFEST) != COMPILED_RESOURCE_MANIFEST_SHA256 {
        return Err(invalid("程序内的正式资源锚摘要不一致。"));
    }
    let trusted = protocol::TrustedContext::new(protocol::TrustInputs {
        profile: include_bytes!("../../../build-profiles/production.json"),
        target: crate::build_profile::BUILD_TARGET,
        signing_policy: include_bytes!("../../../build-profiles/macos-signing-policy.json"),
        node_runtime: include_bytes!("../../../scripts/node-runtime-release.json"),
        development_node: include_bytes!("../../../scripts/development-node-release.json"),
        development_git: include_bytes!("../../../scripts/development-git-release.json"),
    })
    .map_err(invalid)?;
    protocol::parse(MANIFEST, &trusted).map_err(invalid)
}

/// Explicit installed/staged resources only; no discovery or user data reads.
/// Success means byte/path bindings match, never permission to start runtime.
pub fn verify_installed_resources(root: &Path) -> Result<Value, ApiError> {
    let manifest = compiled()?;
    protocol::verify_resources(root, &manifest).map_err(invalid)?;
    Ok(json!({"manifestSha256": COMPILED_RESOURCE_MANIFEST_SHA256,
        "digestBindingsVerified": true, "signatureEvidenceVerified": false,
        "sourceTransformationVerified": false, "releaseReady": false}))
}

pub(crate) fn verify_tool_payload(root: &Path, kind: PayloadKind) -> Result<(), ApiError> {
    let manifest = compiled()?;
    protocol::verify_payload(root, protocol::payload(&manifest, kind)).map_err(invalid)
}

pub(crate) fn runtime_node_digest() -> Result<String, ApiError> {
    let manifest = compiled()?;
    protocol::runtime_node_digest(&manifest)
        .map(str::to_owned)
        .map_err(invalid)
}

#[cfg(all(test, unix))]
#[path = "production_resources_tests.rs"]
mod tests;
