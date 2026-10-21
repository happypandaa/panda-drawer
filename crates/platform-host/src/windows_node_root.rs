//! Admission of the independent Windows Node slots under an already admitted
//! fixed root. Unknown directories are never accepted merely by their name.
use crate::{
    WindowsObjectIdentity, WorkspaceDirectory, WorkspaceError, APP_IDENTIFIER, ENVIRONMENT,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

type Result<T> = std::result::Result<T, WorkspaceError>;
fn invalid(message: &str) -> WorkspaceError {
    WorkspaceError {
        code: "NODE_ROOT_UNRECOGNIZED",
        message: message.into(),
    }
}
fn read<T: serde::de::DeserializeOwned>(directory: &WorkspaceDirectory, name: &str) -> Result<T> {
    serde_json::from_slice(&directory.read_bounded(name, 65536)?.bytes)
        .map_err(|_| invalid("Node控制记录损坏或格式未知，已保留。"))
}
fn write(directory: &WorkspaceDirectory, name: &str, record: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(record).map_err(|_| invalid("Node控制记录无法编码。"))?;
    directory.write_new(name, &bytes).map(|_| ())
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NodeRoot {
    schema_version: u32,
    identifier: String,
    environment: String,
    data_root_identity: WindowsObjectIdentity,
    node_root_identity: WindowsObjectIdentity,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Slot {
    schema_version: u32,
    id: String,
    slot_identity: WindowsObjectIdentity,
    registration_identity: WindowsObjectIdentity,
    directories: BTreeMap<String, WindowsObjectIdentity>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Registration {
    version: u32,
    id: String,
    source_path: std::path::PathBuf,
    manifest_digest: String,
}
const PROOF: &str = ".windows-node-slot.json";
const MARKER: &str = ".node-apps-root.json";
fn slot_directories(slot: &WorkspaceDirectory) -> Result<BTreeMap<String, WindowsObjectIdentity>> {
    let mut identities = BTreeMap::new();
    for name in ["configuration", "data", "cache", "runs"] {
        let directory = slot.open_directory(name)?;
        identities.insert(name.to_owned(), directory.identity().clone());
        if name == "configuration" {
            identities.insert(
                "configuration/credentials".into(),
                directory.open_directory("credentials")?.identity().clone(),
            );
        }
    }
    Ok(identities)
}
pub fn seal_slot(root: &WorkspaceDirectory, id: &str) -> Result<()> {
    let parsed = Uuid::parse_str(id).map_err(|_| invalid("Node槽标识无效。"))?;
    if parsed.is_nil() || parsed.to_string() != id {
        return Err(invalid("Node槽标识无效。"));
    }
    let slot = root.open_directory(id)?;
    let registration = slot.read_bounded("registration.json", 65536)?;
    write(
        &slot,
        PROOF,
        &Slot {
            schema_version: 1,
            id: id.into(),
            slot_identity: slot.identity().clone(),
            registration_identity: registration.identity,
            directories: slot_directories(&slot)?,
        },
    )?;
    validate_slot(root, id)
}
fn validate_slot(root: &WorkspaceDirectory, id: &str) -> Result<()> {
    let slot = root.open_directory(id)?;
    let proof: Slot = read(&slot, PROOF)?;
    if proof.schema_version != 1
        || proof.id != id
        || &proof.slot_identity != slot.identity()
        || proof.directories != slot_directories(&slot)?
    {
        return Err(invalid("Node槽目录身份或格式变化，未接管替换对象。"));
    }
    for entry in slot.list()? {
        if !matches!(
            (entry.name.as_str(), entry.is_directory),
            ("configuration", true)
                | ("data", true)
                | ("cache", true)
                | ("runs", true)
                | ("registration.json", false)
                | (".windows-node-slot.json", false)
                | ("active.json", false)
        ) {
            return Err(invalid("Node槽含未知或未完成现场，已保留。"));
        }
    }
    let bytes = slot.read_bounded("registration.json", 65536)?;
    let registration: Registration =
        serde_json::from_slice(&bytes.bytes).map_err(|_| invalid("Node登记格式无效，已保留。"))?;
    if bytes.identity != proof.registration_identity
        || registration.version != 1
        || registration.id != id
        || !registration.source_path.is_absolute()
        || registration.manifest_digest.len() != 64
        || !registration
            .manifest_digest
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid("Node登记身份或内容不符，已保留。"));
    }
    Ok(())
}
/// `create` is used only by explicit registration. A missing root during list
/// remains missing; existing entries always require a complete typed proof.
pub fn node_apps_directory(
    data: &WorkspaceDirectory,
    create: bool,
) -> Result<Option<WorkspaceDirectory>> {
    data.verify_binding()?;
    let exists = data
        .list()?
        .iter()
        .any(|entry| entry.name == "node-apps" && entry.is_directory);
    if !exists {
        if !create {
            return Ok(None);
        }
        let directory = data.create_directory("node-apps")?;
        write(
            &directory,
            MARKER,
            &NodeRoot {
                schema_version: 1,
                identifier: APP_IDENTIFIER.into(),
                environment: ENVIRONMENT.into(),
                data_root_identity: data.identity().clone(),
                node_root_identity: directory.identity().clone(),
            },
        )?;
    }
    let directory = data.open_directory("node-apps")?;
    let record: NodeRoot = read(&directory, MARKER)?;
    if record.schema_version != 1
        || record.identifier != APP_IDENTIFIER
        || record.environment != ENVIRONMENT
        || &record.data_root_identity != data.identity()
        || &record.node_root_identity != directory.identity()
    {
        return Err(invalid("Node根身份或格式不匹配，未采用已有目录。"));
    }
    for entry in directory.list()? {
        if entry.name == MARKER && !entry.is_directory {
            continue;
        }
        if !entry.is_directory
            || !Uuid::parse_str(&entry.name)
                .is_ok_and(|id| !id.is_nil() && id.to_string() == entry.name)
        {
            return Err(invalid("Node根包含未知条目，已保留。"));
        }
        validate_slot(&directory, &entry.name)?;
    }
    Ok(Some(directory))
}
