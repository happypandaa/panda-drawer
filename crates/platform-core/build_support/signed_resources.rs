//! Digest/path protocol shared by build-time and installed-resource checks.
//! Signature fields are bindings, not proof of codesign or a source-to-signed
//! transformation. Production runtime remains blocked until that transaction
//! is implemented and verified separately.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
};

pub const MAX_MANIFEST_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 30_000;
const MAX_TREE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ALL_BYTES: u64 = 640 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 180 * 1024 * 1024;
const SIGNING_PROFILE: &str = "macos-developer-id-hardened-runtime-v1";

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn relative(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value.split('/').count() <= 65
        && value.split('/').all(|part| {
            !part.is_empty()
                && part.len() <= 255
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._@+".contains(&b))
        })
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SigningPolicy {
    pub schema_version: u32,
    pub signing_profile: String,
    pub expected_team_id: Option<String>,
    pub node_entitlements_sha256: Option<String>,
    pub git_entitlements_sha256: Option<String>,
}

impl SigningPolicy {
    fn configured(bytes: &[u8]) -> Result<Self, String> {
        let policy: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let team = policy.expected_team_id.as_deref().unwrap_or_default();
        if policy.schema_version != 1
            || policy.signing_profile != SIGNING_PROFILE
            || team.len() != 10
            || !team
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            || !policy
                .node_entitlements_sha256
                .as_deref()
                .is_some_and(valid_digest)
            || !policy
                .git_entitlements_sha256
                .as_deref()
                .is_some_and(valid_digest)
        {
            return Err("Fixed signing policy is unconfigured or unsupported; no Team ID or entitlement digest may be inferred".into());
        }
        Ok(policy)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileBinding {
    pub sha256: String,
    pub version: String,
    pub identifier: String,
    pub environment: String,
    pub target: String,
    pub minimum_system_version: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceAnchors {
    pub node_runtime: String,
    pub development_node: String,
    pub development_git: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignatureBinding {
    pub team_id: String,
    pub identifier: String,
    pub hardened_runtime: bool,
    pub entitlements_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceFile {
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Entry {
    Directory {
        path: String,
        mode: u32,
    },
    File {
        path: String,
        mode: u32,
        size: u64,
        sha256: String,
        signature: Option<SignatureBinding>,
        source: Option<SourceFile>,
    },
}

impl Entry {
    fn path(&self) -> &str {
        match self {
            Self::Directory { path, .. } | Self::File { path, .. } => path,
        }
    }
    fn mode(&self) -> u32 {
        match self {
            Self::Directory { mode, .. } | Self::File { mode, .. } => *mode,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Payload {
    pub root: String,
    pub root_mode: u32,
    pub tool_version: String,
    pub npm_version: Option<String>,
    pub entry_count: usize,
    pub file_count: usize,
    pub total_bytes: u64,
    pub sha256: String,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Payloads {
    pub node_runtime: Payload,
    pub development_node: Payload,
    pub development_git: Payload,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub digest_rule_version: u32,
    pub profile: ProfileBinding,
    pub signing_profile: String,
    pub signing_policy_sha256: String,
    pub expected_team_id: String,
    pub source_anchors: SourceAnchors,
    pub payloads: Payloads,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadKind {
    NodeRuntime,
    DevelopmentNode,
    DevelopmentGit,
}

impl PayloadKind {
    pub fn root(self) -> &'static str {
        match self {
            Self::NodeRuntime => "node-runtime",
            Self::DevelopmentNode => "development-node",
            Self::DevelopmentGit => "development-git",
        }
    }
}

pub struct TrustInputs<'a> {
    pub profile: &'a [u8],
    pub target: &'a str,
    pub signing_policy: &'a [u8],
    pub node_runtime: &'a [u8],
    pub development_node: &'a [u8],
    pub development_git: &'a [u8],
}

pub struct TrustedContext {
    pub profile: ProfileBinding,
    pub policy: SigningPolicy,
    pub signing_policy_sha256: String,
    pub source_anchors: SourceAnchors,
    runtime_source: Value,
    node_source: Value,
    git_source: Value,
}

impl TrustedContext {
    pub fn new(inputs: TrustInputs<'_>) -> Result<Self, String> {
        let p = super::profile::ProductionProfile::parse(inputs.profile, inputs.target)?;
        let runtime_source: Value =
            serde_json::from_slice(inputs.node_runtime).map_err(|e| e.to_string())?;
        let node_source: Value =
            serde_json::from_slice(inputs.development_node).map_err(|e| e.to_string())?;
        let git_source: Value =
            serde_json::from_slice(inputs.development_git).map_err(|e| e.to_string())?;
        for value in [&runtime_source, &node_source, &git_source] {
            if value["schemaVersion"] != 1
                || value["platform"] != "darwin"
                || value["arch"] != "arm64"
            {
                return Err("Unsupported trusted source anchor schema/target".into());
            }
        }
        Ok(Self {
            profile: ProfileBinding {
                sha256: super::profile::digest(inputs.profile),
                version: p.version,
                identifier: p.identifier,
                environment: p.environment,
                target: p.target,
                minimum_system_version: p.minimum_system_version,
            },
            policy: SigningPolicy::configured(inputs.signing_policy)?,
            signing_policy_sha256: digest(inputs.signing_policy),
            source_anchors: SourceAnchors {
                node_runtime: digest(inputs.node_runtime),
                development_node: digest(inputs.development_node),
                development_git: digest(inputs.development_git),
            },
            runtime_source,
            node_source,
            git_source,
        })
    }

    fn source(&self, kind: PayloadKind) -> &Value {
        match kind {
            PayloadKind::NodeRuntime => &self.runtime_source,
            PayloadKind::DevelopmentNode => &self.node_source,
            PayloadKind::DevelopmentGit => &self.git_source,
        }
    }

    fn entitlements(&self, kind: PayloadKind) -> &str {
        match kind {
            PayloadKind::DevelopmentGit => self.policy.git_entitlements_sha256.as_deref().unwrap(),
            _ => self.policy.node_entitlements_sha256.as_deref().unwrap(),
        }
    }

    pub fn expected_identifier(&self, kind: PayloadKind, path: &str) -> String {
        format!(
            "{}.tools.{}.{}",
            self.profile.identifier,
            kind.root(),
            digest(path.as_bytes())
        )
    }
}

fn payloads(manifest: &Manifest) -> [(PayloadKind, &Payload); 3] {
    [
        (PayloadKind::NodeRuntime, &manifest.payloads.node_runtime),
        (
            PayloadKind::DevelopmentNode,
            &manifest.payloads.development_node,
        ),
        (
            PayloadKind::DevelopmentGit,
            &manifest.payloads.development_git,
        ),
    ]
}

pub fn tree_digest(payload: &Payload) -> Result<String, String> {
    let mut h = Sha256::new();
    h.update(format!(
        "signed-tools-tree-v1\0{}\0{}\n",
        payload.root, payload.root_mode
    ));
    for entry in &payload.entries {
        h.update(serde_json::to_vec(entry).map_err(|e| e.to_string())?);
        h.update(b"\n");
    }
    Ok(format!("{:x}", h.finalize()))
}

fn source_version(source: &Value, kind: PayloadKind) -> Option<&str> {
    source[match kind {
        PayloadKind::NodeRuntime => "version",
        PayloadKind::DevelopmentNode => "nodeVersion",
        PayloadKind::DevelopmentGit => "gitVersion",
    }]
    .as_str()
}

fn source_file(source: &Value, key: &str) -> Result<SourceFile, String> {
    Ok(SourceFile {
        size: source[key]["size"]
            .as_u64()
            .ok_or("Missing trusted source size")?,
        sha256: source[key]["sha256"]
            .as_str()
            .filter(|x| valid_digest(x))
            .ok_or("Missing trusted source digest")?
            .into(),
    })
}

/// Exact projection produced by the unchanged package-node-runtime.mjs.
pub fn runtime_metadata_bytes(source: &Value) -> Result<Vec<u8>, String> {
    #[derive(Serialize)]
    struct Source<'a> {
        url: &'a str,
        sha256: &'a str,
    }
    #[derive(Serialize)]
    struct Binary<'a> {
        path: &'a str,
        sha256: &'a str,
        size: u64,
    }
    #[derive(Serialize)]
    struct License<'a> {
        path: &'a str,
        sha256: &'a str,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Runtime<'a> {
        schema_version: u32,
        version: &'a str,
        platform: &'a str,
        arch: &'a str,
        source: Source<'a>,
        binary: Binary<'a>,
        license: License<'a>,
    }
    let metadata = Runtime {
        schema_version: 1,
        version: source["version"]
            .as_str()
            .ok_or("Missing runtime source version")?,
        platform: source["platform"]
            .as_str()
            .ok_or("Missing runtime source platform")?,
        arch: source["arch"]
            .as_str()
            .ok_or("Missing runtime source architecture")?,
        source: Source {
            url: source["source"]["url"]
                .as_str()
                .ok_or("Missing runtime source URL")?,
            sha256: source["source"]["sha256"]
                .as_str()
                .filter(|s| valid_digest(s))
                .ok_or("Missing runtime archive digest")?,
        },
        binary: Binary {
            path: "bin/node",
            sha256: source["binary"]["sha256"]
                .as_str()
                .filter(|s| valid_digest(s))
                .ok_or("Missing runtime binary digest")?,
            size: source["binary"]["size"]
                .as_u64()
                .ok_or("Missing runtime binary size")?,
        },
        license: License {
            path: "LICENSE",
            sha256: source["license"]["sha256"]
                .as_str()
                .filter(|s| valid_digest(s))
                .ok_or("Missing runtime license digest")?,
        },
    };
    let mut bytes = serde_json::to_vec_pretty(&metadata).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn validate_payload(
    kind: PayloadKind,
    payload: &Payload,
    trusted: &TrustedContext,
) -> Result<(), String> {
    let source = trusted.source(kind);
    if payload.root != kind.root()
        || ![0o555, 0o755].contains(&payload.root_mode)
        || Some(payload.tool_version.as_str()) != source_version(source, kind)
        || payload.npm_version.as_deref()
            != if kind == PayloadKind::DevelopmentNode {
                source["npmVersion"].as_str()
            } else {
                None
            }
        || payload.entries.is_empty()
        || payload.entries.len() > MAX_ENTRIES
        || payload.entry_count != payload.entries.len()
        || payload.total_bytes > MAX_TREE_BYTES
        || !valid_digest(&payload.sha256)
        || tree_digest(payload)? != payload.sha256
    {
        return Err("Payload identity, version, digest or budget does not match".into());
    }
    let executables: BTreeSet<_> = source["executables"]
        .as_array()
        .map(|x| x.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut paths: BTreeMap<&str, &Entry> = BTreeMap::new();
    let mut nonempty_directories = BTreeSet::new();
    let mut folded = BTreeSet::new();
    let mut previous = "";
    let mut total = 0u64;
    let mut count = 0usize;
    let mut source_hash = Sha256::new();
    let mut source_total = 0u64;
    let mut source_count = 0usize;
    for entry in &payload.entries {
        let path = entry.path();
        if !relative(path) || path <= previous || !folded.insert(path.to_ascii_lowercase()) {
            return Err(
                "Payload paths must be sorted unique printable ASCII without case collisions"
                    .into(),
            );
        }
        previous = path;
        if let Some((parent, _)) = path.rsplit_once('/') {
            if !matches!(paths.get(parent), Some(Entry::Directory { .. })) {
                return Err("Missing directory parent or file/directory conflict".into());
            }
        }
        paths.insert(path, entry);
        match entry {
            Entry::Directory { mode, .. } => {
                if path.split('/').count() > 64 || ![0o555, 0o755].contains(mode) {
                    return Err("Invalid directory depth or permissions".into());
                }
            }
            Entry::File {
                mode,
                size,
                sha256,
                signature,
                source: original,
                ..
            } => {
                let executable = if kind == PayloadKind::NodeRuntime {
                    path == "bin/node"
                } else {
                    executables.contains(path)
                };
                let allowed = if executable {
                    [0o555, 0o755]
                } else {
                    [0o444, 0o644]
                };
                if !allowed.contains(mode) || *size > MAX_FILE_BYTES || !valid_digest(sha256) {
                    return Err("Invalid file mode, size or digest".into());
                }
                total = total
                    .checked_add(*size)
                    .filter(|n| *n <= MAX_TREE_BYTES)
                    .ok_or("Payload size budget exceeded")?;
                count += 1;
                let mut ancestor = path;
                while let Some((parent, _)) = ancestor.rsplit_once('/') {
                    nonempty_directories.insert(parent);
                    ancestor = parent;
                }
                match (signature, original) {
                    (Some(sig), Some(old)) => {
                        if !executable
                            || !sig.hardened_runtime
                            || sig.team_id != trusted.policy.expected_team_id.as_deref().unwrap()
                            || sig.identifier != trusted.expected_identifier(kind, path)
                            || sig.entitlements_sha256 != trusted.entitlements(kind)
                            || old.size > MAX_FILE_BYTES
                            || !valid_digest(&old.sha256)
                        {
                            return Err(
                                "Signature/source bindings do not match fixed policy".into()
                            );
                        }
                    }
                    (None, None) => {}
                    _ => return Err("A signature requires exactly one source binding".into()),
                }
                if kind == PayloadKind::NodeRuntime {
                    match path {
                        "bin/node"
                            if original.as_ref() == Some(&source_file(source, "binary")?)
                                && signature.is_some() => {}
                        "LICENSE"
                            if signature.is_none()
                                && source["license"]["sha256"].as_str()
                                    == Some(sha256.as_str()) => {}
                        "runtime.json" if signature.is_none() && *size <= 64 * 1024 => {
                            let expected = runtime_metadata_bytes(source)?;
                            if *size != expected.len() as u64 || *sha256 != digest(&expected) {
                                return Err(
                                    "Runtime metadata differs from the pinned source projection"
                                        .into(),
                                );
                            }
                        }
                        _ => return Err("Unknown runtime Node file or source binding".into()),
                    }
                } else {
                    let metadata = if kind == PayloadKind::DevelopmentNode {
                        "development-node.json"
                    } else {
                        "development-git.json"
                    };
                    if path == metadata {
                        let expected = if kind == PayloadKind::DevelopmentNode {
                            &trusted.source_anchors.development_node
                        } else {
                            &trusted.source_anchors.development_git
                        };
                        if signature.is_some() || sha256 != expected || *size > 64 * 1024 {
                            return Err("Source metadata must remain byte-identical".into());
                        }
                    } else {
                        let old = original.as_ref();
                        let old_size = old.map_or(*size, |x| x.size);
                        let old_sha = old.map_or(sha256.as_str(), |x| x.sha256.as_str());
                        source_hash.update(format!("{path}\0{old_size}\0{old_sha}\n"));
                        source_total = source_total
                            .checked_add(old_size)
                            .ok_or("Source size overflow")?;
                        source_count += 1;
                    }
                }
            }
        }
    }
    if paths.iter().any(|(path, entry)| {
        matches!(entry, Entry::Directory { .. }) && !nonempty_directories.contains(path)
    }) {
        return Err("Empty directories are not permitted in a tool payload".into());
    }
    if total != payload.total_bytes || count != payload.file_count {
        return Err("Payload counts/bytes do not match".into());
    }
    let required: &[&str] = match kind {
        PayloadKind::NodeRuntime => &["bin/node", "LICENSE", "runtime.json"],
        PayloadKind::DevelopmentNode => &[
            "bin/node",
            "bin/npm",
            "bin/npx",
            "lib/node_modules/npm/bin/npm-cli.js",
            "development-node.json",
        ],
        PayloadKind::DevelopmentGit => &[
            "bin/git",
            "libexec/git-core",
            "share/git-core/templates",
            "development-git.json",
        ],
    };
    for path in required {
        let directory = kind == PayloadKind::DevelopmentGit
            && ["libexec/git-core", "share/git-core/templates"].contains(path);
        match paths.get(path) {
            Some(Entry::Directory { .. }) if directory => {}
            Some(Entry::File { .. }) if !directory => {}
            _ => return Err("Required tool entry missing or has the wrong type".into()),
        }
    }
    if kind != PayloadKind::NodeRuntime
        && (source["tree"]["rule"] != "sorted-utf8-path-nul-size-nul-sha256-lf-v1"
            || source["tree"]["fileCount"].as_u64() != Some(source_count as u64)
            || source["tree"]["totalBytes"].as_u64() != Some(source_total)
            || source["tree"]["sha256"].as_str()
                != Some(format!("{:x}", source_hash.finalize()).as_str()))
    {
        return Err("Recorded source claims and untouched files do not match old source anchor; this alone is not transformation proof".into());
    }
    Ok(())
}

pub fn parse(bytes: &[u8], trusted: &TrustedContext) -> Result<Manifest, String> {
    if bytes.is_empty() || bytes.len() > MAX_MANIFEST_BYTES {
        return Err("Signed manifest exceeds size budget or is missing".into());
    }
    let m: Manifest = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if m.schema_version != 1
        || m.digest_rule_version != 1
        || m.profile != trusted.profile
        || m.signing_profile != SIGNING_PROFILE
        || m.signing_profile != trusted.policy.signing_profile
        || m.signing_policy_sha256 != trusted.signing_policy_sha256
        || Some(m.expected_team_id.as_str()) != trusted.policy.expected_team_id.as_deref()
        || m.source_anchors != trusted.source_anchors
    {
        return Err("Signed manifest schema/profile/target/signing/source binding mismatch".into());
    }
    let mut total = 0u64;
    for (kind, payload) in payloads(&m) {
        validate_payload(kind, payload, trusted)?;
        total = total
            .checked_add(payload.total_bytes)
            .filter(|n| *n <= MAX_ALL_BYTES)
            .ok_or("Aggregate tool budget exceeded")?;
    }
    Ok(m)
}

/// Validate before emitting any Cargo rerun directive containing this path.
pub fn validate_input_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .to_str()
            .is_none_or(|value| !input_text_allowed(value, cfg!(windows)))
        || path
            .components()
            .any(|x| matches!(x, Component::CurDir | Component::ParentDir))
    {
        return Err("Resource input needs an absolute clean path".into());
    }
    Ok(())
}

fn input_text_allowed(value: &str, windows: bool) -> bool {
    (windows || !value.contains('\\')) && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod portable_path_tests {
    use super::*;

    #[test]
    fn native_windows_separators_are_not_confused_with_cargo_control_characters() {
        let path = r"C:\Users\example\development project";
        assert!(input_text_allowed(path, true));
        assert!(!input_text_allowed(path, false));
        for text in [
            "C:\\repo\nCargo:override",
            "C:\\repo\rnext",
            "C:\\repo\0next",
        ] {
            assert!(!input_text_allowed(text, true));
        }
    }

    #[test]
    fn actual_native_manifest_directory_passes_portable_checks() {
        assert!(validate_input_path(Path::new(env!("CARGO_MANIFEST_DIR"))).is_ok());
        assert!(validate_input_path(Path::new("relative/path")).is_err());
    }
}

#[cfg(unix)]
#[path = "signed_resources_unix.rs"]
mod unix_io;
#[cfg(unix)]
pub fn read_manifest(path: &Path) -> Result<Vec<u8>, String> {
    unix_io::read_manifest(path)
}
#[cfg(unix)]
pub fn verify_payload(root: &Path, payload: &Payload) -> Result<(), String> {
    unix_io::verify_payload(root, payload)
}
#[cfg(unix)]
pub fn verify_resources(root: &Path, manifest: &Manifest) -> Result<(), String> {
    unix_io::verify_resources(root, manifest)
}

#[cfg(not(unix))]
pub fn read_manifest(_path: &Path) -> Result<Vec<u8>, String> {
    Err("Production resource verification is unavailable on non-Unix hosts".into())
}
#[cfg(not(unix))]
pub fn verify_payload(_root: &Path, _payload: &Payload) -> Result<(), String> {
    Err("Production resource verification is unavailable on non-Unix hosts".into())
}
#[cfg(not(unix))]
pub fn verify_resources(_root: &Path, _manifest: &Manifest) -> Result<(), String> {
    Err("Production resource verification is unavailable on non-Unix hosts".into())
}

pub fn payload(manifest: &Manifest, kind: PayloadKind) -> &Payload {
    match kind {
        PayloadKind::NodeRuntime => &manifest.payloads.node_runtime,
        PayloadKind::DevelopmentNode => &manifest.payloads.development_node,
        PayloadKind::DevelopmentGit => &manifest.payloads.development_git,
    }
}

pub fn runtime_node_digest(manifest: &Manifest) -> Result<&str, String> {
    manifest
        .payloads
        .node_runtime
        .entries
        .iter()
        .find_map(|e| match e {
            Entry::File { path, sha256, .. } if path == "bin/node" => Some(sha256.as_str()),
            _ => None,
        })
        .ok_or_else(|| "Runtime Node digest missing".into())
}
