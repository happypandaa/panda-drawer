//! Shared portable project records and digest rule 1. No filesystem access.
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    path::{Component, Path, PathBuf},
};

pub const DIGEST_RULE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectManifest {
    pub manifest_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(
        default,
        deserialize_with = "display_icon",
        skip_serializing_if = "Option::is_none"
    )]
    pub icon: Option<String>,
    pub sdk_api_version: u32,
    pub source_dir: String,
    pub output_dir: String,
    pub entry: String,
    pub database: String,
    pub attachments_dir: String,
    pub build: Value,
    pub docs: Value,
    #[serde(default)]
    pub runtime_requirements: Value,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

fn display_icon<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    // Keep the existing display-metadata policy: malformed icons do not disable
    // a project; the management metadata reader reports their problem.
    let value = Value::deserialize(deserializer)?;
    Ok(value.as_str().map(str::to_owned))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateProjectInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub project_root: PathBuf,
    pub document_epoch: String,
    pub state: String,
    pub issues: Vec<String>,
    pub manifest: Option<ProjectManifest>,
    pub build_info: Value,
}

/// Built while packaging the platform, never a successful user build task.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StarterDistribution {
    pub schema_version: u32,
    pub origin: String,
    pub template_version: String,
    pub sdk_version: String,
    pub source_digest: String,
    pub artifact_digest: String,
    pub entry_relative: String,
    pub built_at: u64,
    pub node_version: String,
    pub npm_version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StarterProvenance {
    pub schema_version: u32,
    pub origin: String,
    pub build_id: String,
    pub installed_at: u64,
    pub digest_rule_version: u32,
    pub input_digest: String,
    pub distribution: StarterDistribution,
}

/// The existing input projection and serialization order remain unchanged.
pub fn build_config(manifest: &ProjectManifest) -> Value {
    json!({"manifestVersion":manifest.manifest_version,"sdkApiVersion":manifest.sdk_api_version,"sourceDir":manifest.source_dir,"outputDir":manifest.output_dir,"entry":manifest.entry,"build":manifest.build})
}

pub fn input_hasher(manifest: &ProjectManifest) -> Result<Sha256, serde_json::Error> {
    let mut digest = Sha256::new();
    digest.update(b"local-projects-input-v1\0");
    digest.update(serde_json::to_vec(&build_config(manifest))?);
    Ok(digest)
}

/// Relative host paths become slash-separated UTF-8 logical paths. Filesystem
/// safety, ordinary-file checks and stable reads remain the caller's job.
pub fn logical_path(relative: &Path) -> Result<String, &'static str> {
    let mut names = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => names.push(name.to_str().ok_or("Non UTF-8 path")?),
            _ => return Err("Expected a relative path without dot segments"),
        }
    }
    if names.is_empty() {
        return Err("Expected a nonempty relative file path");
    }
    Ok(names.join("/"))
}

/// Compare directory components as UTF-8 bytes, matching a sorted depth-first
/// traversal. Sorting the entire joined path would change "a/x" vs "a-.txt".
pub fn compare_logical_paths(left: &str, right: &str) -> Ordering {
    left.split('/')
        .map(str::as_bytes)
        .cmp(right.split('/').map(str::as_bytes))
}

/// Hash exactly one file's logical label and declared byte length. The caller
/// streams the actual bytes into the same hasher and verifies the read remained
/// stable. No native separators, timestamps or filesystem identities are hashed.
pub fn update_file_header(digest: &mut Sha256, logical_path: &str, byte_length: u64) {
    digest.update((logical_path.len() as u64).to_le_bytes());
    digest.update(logical_path.as_bytes());
    digest.update(byte_length.to_le_bytes());
}

#[derive(Clone, Copy)]
pub struct TreeFile<'a> {
    pub logical_path: &'a str,
    pub bytes: &'a [u8],
}

fn update_tree(digest: &mut Sha256, files: &[TreeFile<'_>]) {
    let mut files = files.to_vec();
    files.sort_by(|left, right| compare_logical_paths(left.logical_path, right.logical_path));
    for file in files {
        update_file_header(digest, file.logical_path, file.bytes.len() as u64);
        digest.update(file.bytes);
    }
}

/// Pure byte counterpart of the streaming production scanner. Input entries
/// must already represent unique ordinary files in a validated logical tree.
pub fn tree_digest(files: &[TreeFile<'_>]) -> String {
    let mut digest = Sha256::new();
    update_tree(&mut digest, files);
    format!("{:x}", digest.finalize())
}

pub fn input_digest(
    manifest: &ProjectManifest,
    files: &[TreeFile<'_>],
) -> Result<String, serde_json::Error> {
    let mut digest = input_hasher(manifest)?;
    update_tree(&mut digest, files);
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> ProjectManifest {
        serde_json::from_value(json!({
            "manifestVersion":1,"id":"prj_fixture","name":"Fixture","sdkApiVersion":1,
            "sourceDir":"app","outputDir":"dist","entry":"dist/index.html",
            "database":"data/main.sqlite","attachmentsDir":"data/attachments",
            "build":{"script":"build"},"docs":{"readme":"README.md"}
        }))
        .unwrap()
    }

    #[test]
    fn portable_manifest_preserves_defaults_display_icon_and_extra_fields() {
        let mut value = serde_json::to_value(manifest()).unwrap();
        value["icon"] = json!({"bad":"display metadata"});
        value["futureField"] = json!({"retained":true});
        let parsed: ProjectManifest = serde_json::from_value(value).unwrap();
        assert!(parsed.icon.is_none());
        assert_eq!(parsed.description, "");
        assert_eq!(parsed.extra["futureField"], json!({"retained":true}));
        let encoded = serde_json::to_value(parsed).unwrap();
        assert!(encoded.get("icon").is_none());
        assert_eq!(encoded["futureField"], json!({"retained":true}));
    }

    #[test]
    fn creation_input_keeps_unknown_field_rejection_and_optional_description() {
        let parsed: CreateProjectInput = serde_json::from_value(json!({"name":"Fixture"})).unwrap();
        assert!(parsed.description.is_none());
        assert!(serde_json::from_value::<CreateProjectInput>(
            json!({"name":"Fixture","overwrite":true})
        )
        .is_err());
    }

    #[test]
    fn digest_order_is_directory_depth_first_and_utf8_not_utf16() {
        let mut labels = vec!["a😀/z.txt", "a\u{e000}/x.txt", "a-.txt", "a/z.txt"];
        labels.sort_by(|left, right| compare_logical_paths(left, right));
        assert_eq!(
            labels,
            ["a/z.txt", "a-.txt", "a\u{e000}/x.txt", "a😀/z.txt"]
        );
    }

    #[test]
    fn tree_digest_is_independent_of_input_order_and_matches_legacy_mac_bytes() {
        let files = [
            TreeFile {
                logical_path: "a/z.txt",
                bytes: b"alpha",
            },
            TreeFile {
                logical_path: "a-.txt",
                bytes: b"beta",
            },
            TreeFile {
                logical_path: "a\u{e000}/x.txt",
                bytes: b"bmp",
            },
            TreeFile {
                logical_path: "a😀/z.txt",
                bytes: b"unicode",
            },
        ];
        let mut legacy = Sha256::new();
        for file in files {
            legacy.update((file.logical_path.len() as u64).to_le_bytes());
            legacy.update(file.logical_path.as_bytes());
            legacy.update((file.bytes.len() as u64).to_le_bytes());
            legacy.update(file.bytes);
        }
        let mut reversed = files;
        reversed.reverse();
        assert_eq!(tree_digest(&reversed), format!("{:x}", legacy.finalize()));
        assert_eq!(
            tree_digest(&[]),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn input_digest_keeps_the_original_projection_and_prefix() {
        let original = manifest();
        let mut renamed = original.clone();
        renamed.id = "prj_other".into();
        renamed.name = "Renamed".into();
        renamed.database = "data/other.sqlite".into();
        renamed.description = "Changed".into();
        assert_eq!(
            input_digest(&original, &[]).unwrap(),
            input_digest(&renamed, &[]).unwrap()
        );
        renamed.build["script"] = json!("other");
        assert_ne!(
            input_digest(&original, &[]).unwrap(),
            input_digest(&renamed, &[]).unwrap()
        );
        let mut legacy = Sha256::new();
        legacy.update(b"local-projects-input-v1\0");
        legacy.update(serde_json::to_vec(&build_config(&original)).unwrap());
        assert_eq!(
            input_digest(&original, &[]).unwrap(),
            format!("{:x}", legacy.finalize())
        );
    }

    #[test]
    fn logical_host_paths_never_hash_windows_directory_separators() {
        let nested = Path::new("app").join("src").join("App.tsx");
        assert_eq!(logical_path(&nested), Ok("app/src/App.tsx".into()));
        assert!(logical_path(Path::new("")).is_err());
        assert!(logical_path(Path::new("../outside")).is_err());
    }

    #[test]
    fn starter_records_keep_schema_one_and_do_not_acquire_task_fields() {
        let distribution = json!({
            "schemaVersion":1,"origin":"bundledTemplate","templateVersion":"fixture",
            "sdkVersion":"0.1.0","sourceDigest":"source","artifactDigest":"artifact",
            "entryRelative":"index.html","builtAt":123,"nodeVersion":"24","npmVersion":"11"
        });
        let mut value = json!({
            "schemaVersion":1,"origin":"bundledTemplate","buildId":"starter_fixture",
            "installedAt":456,"digestRuleVersion":1,"inputDigest":"inputs",
            "distribution":distribution
        });
        let parsed: StarterProvenance = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), value);
        value["taskId"] = json!("task_invented");
        assert!(serde_json::from_value::<StarterProvenance>(value).is_err());
    }
}

pub mod error;
pub use error::ApiError;
pub mod http_protocol;
pub mod runtime_notice;
pub mod runtime_policy;
