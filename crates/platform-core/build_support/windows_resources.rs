//! Windows-specific immutable resource protocol. No macOS signing fields are
//! invented, and an adjacent manifest is never a runtime trust root.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

pub const MAX_MANIFEST_BYTES: usize = 8 * 1024 * 1024;
const MAX_FILES: usize = 10_001;
const MAX_PAYLOAD_BYTES: u64 = 256 * 1024 * 1024;
pub(super) const MAX_FILE_BYTES: u64 = 160 * 1024 * 1024;

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn relative(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 2048
        || value.contains('\\')
        || value.chars().any(char::is_control)
        || value.split('/').count() > 64
    {
        return false;
    }
    value.split('/').all(|part| {
        let base = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        !part.is_empty()
            && part.len() <= 255
            && part != "."
            && part != ".."
            && !part.ends_with([' ', '.'])
            && !part.contains(['<', '>', ':', '"', '|', '?', '*'])
            && !["CON", "PRN", "AUX", "NUL"].contains(&base.as_str())
            && !(base.len() == 4
                && (base.starts_with("COM") || base.starts_with("LPT"))
                && matches!(base.as_bytes()[3], b'1'..=b'9'))
    })
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
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Payload {
    pub root: String,
    pub tool_version: String,
    pub npm_version: Option<String>,
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
    fn metadata(self) -> &'static str {
        match self {
            Self::NodeRuntime => "runtime.json",
            Self::DevelopmentNode => "development-node.json",
            Self::DevelopmentGit => "development-git.json",
        }
    }
}

pub struct TrustInputs<'a> {
    pub profile: &'a [u8],
    pub target: &'a str,
    pub node_runtime: &'a [u8],
    pub development_node: &'a [u8],
    pub development_git: &'a [u8],
}
pub struct TrustedContext {
    pub profile: ProfileBinding,
    pub source_anchors: SourceAnchors,
    runtime: Value,
    node: Value,
    git: Value,
    runtime_metadata: Vec<u8>,
    node_metadata: Vec<u8>,
    git_metadata: Vec<u8>,
}
impl TrustedContext {
    pub fn new(inputs: TrustInputs<'_>) -> Result<Self, String> {
        let p = super::profile::ProductionProfile::parse(inputs.profile, inputs.target)?;
        if inputs.target != "x86_64-pc-windows-msvc" {
            return Err("Windows resource target mismatch".into());
        }
        let parse = |bytes: &[u8]| -> Result<Value, String> {
            let value: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            if value["schemaVersion"] != 1 || value["platform"] != "win32" || value["arch"] != "x64"
            {
                return Err("Windows source declaration target mismatch".into());
            }
            Ok(value)
        };
        let runtime = parse(inputs.node_runtime)?;
        let node = parse(inputs.development_node)?;
        let git = parse(inputs.development_git)?;
        let mut metadata = runtime.clone();
        for key in ["archiveName", "archiveRoot"] {
            metadata
                .as_object_mut()
                .ok_or("Invalid runtime source")?
                .remove(key);
        }
        metadata["source"]
            .as_object_mut()
            .ok_or("Invalid runtime source")?
            .remove("checksumUrls");
        let mut runtime_metadata =
            serde_json::to_vec_pretty(&metadata).map_err(|e| e.to_string())?;
        runtime_metadata.push(b'\n');
        Ok(Self {
            profile: ProfileBinding {
                sha256: digest(inputs.profile),
                version: p.version,
                identifier: p.identifier,
                environment: p.environment,
                target: p.target,
                minimum_system_version: p.minimum_system_version,
            },
            source_anchors: SourceAnchors {
                node_runtime: digest(inputs.node_runtime),
                development_node: digest(inputs.development_node),
                development_git: digest(inputs.development_git),
            },
            runtime,
            node,
            git,
            runtime_metadata,
            node_metadata: inputs.development_node.to_vec(),
            git_metadata: inputs.development_git.to_vec(),
        })
    }
    fn source(&self, kind: PayloadKind) -> &Value {
        match kind {
            PayloadKind::NodeRuntime => &self.runtime,
            PayloadKind::DevelopmentNode => &self.node,
            PayloadKind::DevelopmentGit => &self.git,
        }
    }
    fn metadata(&self, kind: PayloadKind) -> &[u8] {
        match kind {
            PayloadKind::NodeRuntime => &self.runtime_metadata,
            PayloadKind::DevelopmentNode => &self.node_metadata,
            PayloadKind::DevelopmentGit => &self.git_metadata,
        }
    }
}

pub fn payload(manifest: &Manifest, kind: PayloadKind) -> &Payload {
    match kind {
        PayloadKind::NodeRuntime => &manifest.payloads.node_runtime,
        PayloadKind::DevelopmentNode => &manifest.payloads.development_node,
        PayloadKind::DevelopmentGit => &manifest.payloads.development_git,
    }
}

fn tree<'a>(entries: impl Iterator<Item = &'a Entry>) -> Result<(usize, u64, String), String> {
    let (mut count, mut total, mut hash) = (0usize, 0u64, Sha256::new());
    for e in entries {
        count += 1;
        total = total
            .checked_add(e.size)
            .ok_or("Windows resource size overflow")?;
        hash.update(format!("{}\0{}\0{}\n", e.path, e.size, e.sha256).as_bytes());
    }
    Ok((count, total, format!("{:x}", hash.finalize())))
}

fn validate_payload(
    p: &Payload,
    kind: PayloadKind,
    trusted: &TrustedContext,
) -> Result<(), String> {
    let source = trusted.source(kind);
    let version_key = match kind {
        PayloadKind::NodeRuntime => "version",
        PayloadKind::DevelopmentNode => "nodeVersion",
        PayloadKind::DevelopmentGit => "gitVersion",
    };
    let expected_version = source[version_key]
        .as_str()
        .ok_or("Missing fixed Windows tool version")?;
    let npm = (kind == PayloadKind::DevelopmentNode)
        .then(|| source["npmVersion"].as_str())
        .flatten();
    if p.root != kind.root()
        || p.tool_version != expected_version
        || p.npm_version.as_deref() != npm
        || p.entries.is_empty()
        || p.entries.len() > MAX_FILES
        || p.total_bytes > MAX_PAYLOAD_BYTES
    {
        return Err("Windows payload identity/count/size mismatch".into());
    }
    let mut names = BTreeSet::new();
    let mut previous: Option<&str> = None;
    for entry in &p.entries {
        if !relative(&entry.path)
            || !valid_digest(&entry.sha256)
            || entry.size > MAX_FILE_BYTES
            || previous.is_some_and(|last| last.as_bytes() >= entry.path.as_bytes())
            || !names.insert(entry.path.to_lowercase())
        {
            return Err("Unsafe, unsorted or duplicate Windows payload member".into());
        }
        previous = Some(&entry.path);
    }
    for name in &names {
        let mut path = Path::new(name).parent();
        while let Some(parent) = path.filter(|p| !p.as_os_str().is_empty()) {
            if names.contains(parent.to_string_lossy().as_ref()) {
                return Err("Windows payload file/directory collision".into());
            }
            path = parent.parent();
        }
    }
    let (count, total, hash) = tree(p.entries.iter())?;
    if p.file_count != count || p.total_bytes != total || p.sha256 != hash {
        return Err("Windows payload tree digest mismatch".into());
    }
    let metadata = p
        .entries
        .iter()
        .find(|e| e.path == kind.metadata())
        .ok_or("Windows payload metadata missing")?;
    let expected_metadata = trusted.metadata(kind);
    if metadata.size != expected_metadata.len() as u64
        || metadata.sha256 != digest(expected_metadata)
    {
        return Err("Windows payload metadata is not the compiled source declaration".into());
    }
    let official = p.entries.iter().filter(|e| e.path != kind.metadata());
    if kind == PayloadKind::NodeRuntime {
        if official.clone().count() != 2 {
            return Err("Windows runtime member set differs".into());
        }
        for (name, field) in [("node.exe", "binary"), ("LICENSE", "license")] {
            let e = p
                .entries
                .iter()
                .find(|e| e.path == name)
                .ok_or("Windows runtime member missing")?;
            if Some(e.sha256.as_str()) != source[field]["sha256"].as_str()
                || (name == "node.exe" && Some(e.size) != source[field]["size"].as_u64())
            {
                return Err("Windows runtime bytes differ from fixed release".into());
            }
        }
    } else {
        let (count, bytes, hash) = tree(official)?;
        if source["tree"]
            != json!({"rule":"sorted-utf8-path-nul-size-nul-sha256-lf-v1", "fileCount":count, "totalBytes":bytes, "sha256":hash})
        {
            return Err("Windows complete tool tree differs from fixed official release".into());
        }
    }
    Ok(())
}

pub fn parse(bytes: &[u8], trusted: &TrustedContext) -> Result<Manifest, String> {
    if bytes.is_empty() || bytes.len() > MAX_MANIFEST_BYTES {
        return Err("Windows resource manifest exceeds budget".into());
    }
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if manifest.schema_version != 1
        || manifest.digest_rule_version != 1
        || manifest.profile != trusted.profile
        || manifest.source_anchors != trusted.source_anchors
    {
        return Err(
            "Windows resource manifest is not bound to the fixed production profile/sources".into(),
        );
    }
    for kind in [
        PayloadKind::NodeRuntime,
        PayloadKind::DevelopmentNode,
        PayloadKind::DevelopmentGit,
    ] {
        validate_payload(payload(&manifest, kind), kind, trusted)?;
    }
    Ok(manifest)
}

pub fn runtime_node_digest(manifest: &Manifest) -> Result<&str, String> {
    manifest
        .payloads
        .node_runtime
        .entries
        .iter()
        .find(|e| e.path == "node.exe")
        .map(|e| e.sha256.as_str())
        .ok_or_else(|| "Windows runtime Node is missing".into())
}

#[cfg(windows)]
#[path = "windows_resources_io.rs"]
mod native_io;
#[cfg(windows)]
pub use native_io::{
    pin_verified_resources, read_manifest, verify_payload, verify_resources, VerifiedResources,
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_paths_reject_aliases_traversal_and_reserved_names() {
        for path in [
            "node.exe",
            "node_modules/npm/bin/npm-cli.js",
            "ucrt64/share/git-core/templates/hooks/pre-commit.sample",
        ] {
            assert!(relative(path));
        }
        for path in [
            "../node.exe",
            "a/../node.exe",
            "a\\node.exe",
            "a//b",
            "C:/a",
            "NUL.txt",
            "a/LPT1",
            "a/child.",
            "a/child ",
            "a/b:stream",
        ] {
            assert!(!relative(path), "{path}");
        }
    }
    #[test]
    fn tree_digest_binds_paths_sizes_and_each_file_digest() {
        let original = Entry {
            path: "node.exe".into(),
            size: 3,
            sha256: digest(b"abc"),
        };
        let expected = tree(std::iter::once(&original)).unwrap();
        for altered in [
            Entry {
                path: "other.exe".into(),
                ..original.clone()
            },
            Entry {
                size: 4,
                ..original.clone()
            },
            Entry {
                sha256: digest(b"abd"),
                ..original.clone()
            },
        ] {
            assert_ne!(expected, tree(std::iter::once(&altered)).unwrap());
        }
    }

    fn trusted() -> TrustedContext {
        TrustedContext::new(TrustInputs {
            profile: include_bytes!("../../../build-profiles/production.windows-x64.json"),
            target: "x86_64-pc-windows-msvc",
            node_runtime: include_bytes!("../../../scripts/node-runtime-release.windows-x64.json"),
            development_node: include_bytes!(
                "../../../scripts/development-node-release.windows-x64.json"
            ),
            development_git: include_bytes!(
                "../../../scripts/development-git-release.windows-x64.json"
            ),
        })
        .unwrap()
    }

    #[test]
    fn runtime_manifest_cannot_replace_fixed_source_bytes_or_metadata() {
        let context = trusted();
        // Format-only entries. These are not executable fixture bytes and no
        // disk verification or execution is implied by this schema test.
        let mut entries = vec![
            Entry {
                path: "LICENSE".into(),
                size: 0,
                sha256: context.runtime["license"]["sha256"]
                    .as_str()
                    .unwrap()
                    .into(),
            },
            Entry {
                path: "node.exe".into(),
                size: context.runtime["binary"]["size"].as_u64().unwrap(),
                sha256: context.runtime["binary"]["sha256"].as_str().unwrap().into(),
            },
            Entry {
                path: "runtime.json".into(),
                size: context.runtime_metadata.len() as u64,
                sha256: digest(&context.runtime_metadata),
            },
        ];
        let (file_count, total_bytes, sha256) = tree(entries.iter()).unwrap();
        let good = Payload {
            root: "node-runtime".into(),
            tool_version: context.runtime["version"].as_str().unwrap().into(),
            npm_version: None,
            file_count,
            total_bytes,
            sha256,
            entries: entries.clone(),
        };
        validate_payload(&good, PayloadKind::NodeRuntime, &context).unwrap();
        for name in ["node.exe", "runtime.json"] {
            let mut changed = good.clone();
            changed
                .entries
                .iter_mut()
                .find(|e| e.path == name)
                .unwrap()
                .sha256 = digest(b"operator replacement");
            let (count, total, hash) = tree(changed.entries.iter()).unwrap();
            changed.file_count = count;
            changed.total_bytes = total;
            changed.sha256 = hash;
            assert!(validate_payload(&changed, PayloadKind::NodeRuntime, &context).is_err());
        }
        entries.push(Entry {
            path: "unknown.dll".into(),
            size: 0,
            sha256: digest(b""),
        });
        let mut changed = good;
        changed.entries = entries;
        let (count, total, hash) = tree(changed.entries.iter()).unwrap();
        changed.file_count = count;
        changed.total_bytes = total;
        changed.sha256 = hash;
        assert!(validate_payload(&changed, PayloadKind::NodeRuntime, &context).is_err());
    }

    #[test]
    fn manifest_profile_and_source_bindings_reject_adjacent_overrides() {
        let context = trusted();
        let placeholder = Payload {
            root: "node-runtime".into(),
            tool_version: "schema-only".into(),
            npm_version: None,
            file_count: 0,
            total_bytes: 0,
            sha256: digest(b""),
            entries: Vec::new(),
        };
        let original = Manifest {
            schema_version: 1,
            digest_rule_version: 1,
            profile: context.profile.clone(),
            source_anchors: context.source_anchors.clone(),
            payloads: Payloads {
                node_runtime: placeholder.clone(),
                development_node: placeholder.clone(),
                development_git: placeholder,
            },
        };
        let mut changed = original.clone();
        changed.profile.identifier = "dev.happypandaa.pandadrawer".into();
        assert!(parse(&serde_json::to_vec(&changed).unwrap(), &context)
            .unwrap_err()
            .contains("profile/sources"));
        let mut changed = original;
        changed.source_anchors.node_runtime = digest(b"operator selected source");
        assert!(parse(&serde_json::to_vec(&changed).unwrap(), &context)
            .unwrap_err()
            .contains("profile/sources"));
        assert!(parse(&vec![b' '; MAX_MANIFEST_BYTES + 1], &context).is_err());
    }
}
