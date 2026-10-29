//! Read current direct registrations and retired managed registrations.
//! Retired decoding never verifies or executes installed program contents.
use crate::{projects::check_no_symlinks, ApiError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, ApiError>;
const MAX_REGISTRATION_BYTES: u64 = 1024 * 1024;
pub(crate) const RETIRED_NODE_MESSAGE: &str =
    "这个应用使用旧的安装方式，已不再支持。请删除后重新添加。";
fn invalid() -> ApiError {
    ApiError::new("NODE_REGISTRATION_INVALID", "Node 项目登记无效。")
}
fn absolute_source(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
}
fn uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}
fn hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn identifier(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|value| hex(value, 32))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DirectNodeProvenance {
    pub source_url: String,
    pub source_branch: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DirectNodeRegistration {
    pub version: u32,
    pub id: String,
    pub source_path: PathBuf,
    pub manifest_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<DirectNodeProvenance>,
}

#[derive(Clone, Debug)]
pub(crate) struct RetiredNodeRegistration {
    pub id: String,
    pub generation: String,
    pub manifest_digest: String,
    pub source_id: String,
    pub source_digest: String,
    pub program_path: PathBuf,
    pub development_source: Option<Value>,
}
#[derive(Clone, Debug)]
pub(crate) enum StoredNodeRegistration {
    Direct(DirectNodeRegistration),
    Retired(RetiredNodeRegistration),
}

pub(crate) fn decode_registration(
    value: Value,
    expected_id: &str,
    node_root: &Path,
) -> Result<StoredNodeRegistration> {
    if !uuid(expected_id) || value["id"] != expected_id || !node_root.is_absolute() {
        return Err(invalid());
    }
    match value["version"].as_u64() {
        Some(1) => {
            let direct: DirectNodeRegistration =
                serde_json::from_value(value).map_err(|_| invalid())?;
            if !absolute_source(&direct.source_path) || !hex(&direct.manifest_digest, 64) {
                return Err(invalid());
            }
            Ok(StoredNodeRegistration::Direct(direct))
        }
        Some(2) => {
            // This version can only be displayed or deleted. Its stable slot
            // UUID and ownership index define deletion, not obsolete program
            // metadata. Never let damaged display fields hide every project.
            let generation = value["generation"]
                .as_str()
                .filter(|value| identifier(value, "gen_"))
                .unwrap_or_default()
                .to_owned();
            let manifest_digest = value["manifestDigest"]
                .as_str()
                .filter(|value| hex(value, 64))
                .unwrap_or_default()
                .to_owned();
            let source_id = value["provenance"]["sourceId"]
                .as_str()
                .filter(|value| identifier(value, "src_"))
                .unwrap_or_default()
                .to_owned();
            let source_digest = value["provenance"]["sourceDigest"]
                .as_str()
                .filter(|value| hex(value, 64))
                .unwrap_or_default()
                .to_owned();
            let slot = node_root.join(expected_id);
            let program_path = if generation.is_empty() {
                slot
            } else {
                slot.join("generations").join(&generation)
            };
            Ok(StoredNodeRegistration::Retired(RetiredNodeRegistration {
                id: expected_id.into(),
                generation,
                manifest_digest,
                source_id,
                source_digest,
                program_path,
                development_source: value
                    .get("developmentSource")
                    .filter(|value| !value.is_null())
                    .cloned(),
            }))
        }
        _ => Err(invalid()),
    }
}

fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    check_no_symlinks(path, false)?;
    let before = fs::symlink_metadata(path)?;
    if !before.is_file() || before.nlink() != 1 || before.len() > MAX_REGISTRATION_BYTES {
        return Err(invalid());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let opened = file.metadata()?;
    if opened.dev() != before.dev() || opened.ino() != before.ino() || !opened.is_file() {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(MAX_REGISTRATION_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let after = fs::symlink_metadata(path)?;
    if bytes.len() as u64 > MAX_REGISTRATION_BYTES
        || before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
    {
        return Err(invalid());
    }
    Ok(bytes)
}

pub(crate) fn registration_digest(slot: &Path) -> Result<Option<String>> {
    let path = slot.join("registration.json");
    check_no_symlinks(&path, true)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        Ok(_) => Ok(Some(format!("{:x}", Sha256::digest(read_bytes(&path)?)))),
    }
}

/// Used by destructive cleanup to preserve author directories. Author app.id and
/// old declaration digests are deliberately not source-ownership identities.
pub(crate) fn source_binding_path(value: &Value) -> Result<PathBuf> {
    let path = value["sourceRoot"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(invalid)?;
    if !absolute_source(&path) {
        return Err(invalid());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn direct_records_accept_optional_git_provenance() {
        let id = Uuid::new_v4().to_string();
        let mut value = json!({"version":1,"id":id,"sourcePath":"/disposable/source","manifestDigest":"a".repeat(64)});
        assert!(matches!(
            decode_registration(value.clone(), &id, Path::new("/disposable/nodes")).unwrap(),
            StoredNodeRegistration::Direct(_)
        ));
        value["provenance"] =
            json!({"sourceUrl":"https://github.com/example/disposable","sourceBranch":null});
        assert!(matches!(
            decode_registration(value, &id, Path::new("/disposable/nodes")).unwrap(),
            StoredNodeRegistration::Direct(_)
        ));
    }
    #[test]
    fn retired_record_resolves_only_fixed_path_without_reading_program() {
        let id = Uuid::new_v4().to_string();
        let mut value = json!({"version":2,"id":id,"kind":"managed","generation":format!("gen_{}", "1".repeat(32)),"manifestDigest":"a".repeat(64),"provenance":{"sourceId":format!("src_{}", "2".repeat(32)),"sourceDigest":"b".repeat(64)}});
        let StoredNodeRegistration::Retired(record) =
            decode_registration(value.clone(), &id, Path::new("/disposable/nodes")).unwrap()
        else {
            panic!("expected retired record");
        };
        assert_eq!(
            record.program_path,
            Path::new("/disposable/nodes")
                .join(&id)
                .join("generations")
                .join(format!("gen_{}", "1".repeat(32)))
        );
        for generation in [
            json!("../../user-source"),
            json!("/user/source"),
            Value::Null,
        ] {
            value["generation"] = generation;
            let StoredNodeRegistration::Retired(record) =
                decode_registration(value.clone(), &id, Path::new("/disposable/nodes")).unwrap()
            else {
                panic!("expected retired record");
            };
            assert_eq!(
                record.program_path,
                Path::new("/disposable/nodes").join(&id)
            );
            assert!(record.generation.is_empty());
        }
        assert!(decode_registration(
            value.clone(),
            &Uuid::new_v4().to_string(),
            Path::new("/disposable/nodes")
        )
        .is_err());
        assert!(decode_registration(value, &id, Path::new("relative-node-root")).is_err());
    }
    #[test]
    fn instance_uuid_and_path_identity_are_distinct_from_author_app_identity() {
        let id = Uuid::new_v4().to_string();
        let value = json!({"version":1,"id":id,"sourcePath":"/disposable/source","manifestDigest":"a".repeat(64)});
        assert!(decode_registration(
            value,
            &Uuid::new_v4().to_string(),
            Path::new("/disposable/nodes")
        )
        .is_err());
        assert_eq!(
            source_binding_path(
                &json!({"sourceRoot":"/disposable/source","manifestId":"changed-author-id"})
            )
            .unwrap(),
            Path::new("/disposable/source")
        );
    }
}
