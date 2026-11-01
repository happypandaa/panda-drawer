//! Read only current panda.json direct-source instance registrations.
use crate::{projects::check_no_symlinks, ApiError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, ApiError>;
const MAX_REGISTRATION_BYTES: u64 = 1024 * 1024;
pub(crate) const DIRECT_NODE_REGISTRATION_VERSION: u32 = 4;
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
    pub address: crate::FixedAppAddress,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<DirectNodeProvenance>,
}

#[derive(Clone, Debug)]
pub(crate) enum StoredNodeRegistration {
    Direct(DirectNodeRegistration),
}

pub(crate) fn decode_registration(
    value: Value,
    expected_id: &str,
    node_root: &Path,
) -> Result<StoredNodeRegistration> {
    if !uuid(expected_id)
        || value["id"] != expected_id
        || !node_root.is_absolute()
        || value["version"].as_u64() != Some(u64::from(DIRECT_NODE_REGISTRATION_VERSION))
    {
        return Err(invalid());
    }
    let direct: DirectNodeRegistration = serde_json::from_value(value).map_err(|_| invalid())?;
    if !absolute_source(&direct.source_path)
        || !hex(&direct.manifest_digest, 64)
        || direct.address.validate().is_err()
    {
        return Err(invalid());
    }
    Ok(StoredNodeRegistration::Direct(direct))
}

fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    check_no_symlinks(path, false)?;
    let before = platform_native::tool_files::Facts::capture(path)?;
    if !before.is_file() || before.links() != 1 || before.len() > MAX_REGISTRATION_BYTES {
        return Err(invalid());
    }
    let file = platform_native::filesystem::open_read(
        path,
        platform_native::filesystem::ReadOptions::NoLinksNonblocking,
    )?;
    let opened = platform_native::tool_files::Facts::from_file(&file)?;
    if !opened.same_object(&before) || !opened.is_file() {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(MAX_REGISTRATION_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let after = platform_native::tool_files::Facts::capture(path)?;
    if bytes.len() as u64 > MAX_REGISTRATION_BYTES || !before.same_modified_file(&after) {
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

    fn disposable_path(name: &str) -> PathBuf {
        #[cfg(unix)]
        let root = PathBuf::from("/disposable");
        #[cfg(windows)]
        let root = std::env::temp_dir().join("panda-disposable-registration-decoding");
        root.join(name)
    }
    #[test]
    fn direct_records_accept_optional_git_provenance() {
        let id = Uuid::new_v4().to_string();
        let mut value = json!({"version":DIRECT_NODE_REGISTRATION_VERSION,"id":id,"sourcePath":disposable_path("source"),"manifestDigest":"a".repeat(64),"address":{"port":32001}});
        assert!(matches!(
            decode_registration(value.clone(), &id, disposable_path("nodes").as_path()).unwrap(),
            StoredNodeRegistration::Direct(_)
        ));
        value["provenance"] =
            json!({"sourceUrl":"https://github.com/example/disposable","sourceBranch":null});
        assert!(matches!(
            decode_registration(value, &id, disposable_path("nodes").as_path()).unwrap(),
            StoredNodeRegistration::Direct(_)
        ));
    }
    #[test]
    fn unsupported_registration_versions_are_not_interpreted() {
        let id = Uuid::new_v4().to_string();
        for version in [0, 1, 2, 3, 5] {
            let value = json!({"version":version,"id":id,"sourcePath":disposable_path("source"),"manifestDigest":"a".repeat(64),"address":{"port":32001}});
            assert!(decode_registration(value, &id, disposable_path("nodes").as_path()).is_err());
        }
    }
    #[test]
    fn instance_uuid_and_path_identity_are_distinct_from_author_app_identity() {
        let id = Uuid::new_v4().to_string();
        let value = json!({"version":DIRECT_NODE_REGISTRATION_VERSION,"id":id,"sourcePath":disposable_path("source"),"manifestDigest":"a".repeat(64),"address":{"port":32001}});
        assert!(decode_registration(
            value,
            &Uuid::new_v4().to_string(),
            disposable_path("nodes").as_path()
        )
        .is_err());
        assert_eq!(
            source_binding_path(
                &json!({"sourceRoot":disposable_path("source"),"manifestId":"changed-author-id"})
            )
            .unwrap(),
            disposable_path("source")
        );
    }

    #[test]
    fn registration_digest_reads_only_bounded_independent_files_and_never_creates_missing_files() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-registration-read-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let slot = root.join("slot");
        fs::create_dir(&slot).unwrap();
        assert!(registration_digest(&slot).unwrap().is_none());
        assert_eq!(fs::read_dir(&slot).unwrap().count(), 0);
        let path = slot.join("registration.json");
        let bytes = b"{\"disposable\":true}";
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            registration_digest(&slot).unwrap(),
            Some(format!("{:x}", Sha256::digest(bytes)))
        );
        fs::hard_link(&path, slot.join("second-name")).unwrap();
        assert!(registration_digest(&slot).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::remove_file(slot.join("second-name")).unwrap();
        fs::write(&path, vec![b'x'; MAX_REGISTRATION_BYTES as usize + 1]).unwrap();
        assert!(registration_digest(&slot).is_err());
        fs::write(&path, bytes).unwrap();
        #[cfg(windows)]
        {
            let junction = root.join("disposable-junction");
            assert!(std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&junction)
                .arg(&slot)
                .output()
                .unwrap()
                .status
                .success());
            assert_eq!(
                registration_digest(&junction).unwrap_err().code,
                "UNSAFE_PATH"
            );
            fs::remove_dir(junction).unwrap();
        }
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}
