//! Optional development areas under the admitted workspace. These proofs never
//! adopt a directory merely because it has a familiar name.
use crate::{
    WindowsObjectIdentity, WorkspaceDirectory, WorkspaceError, APP_IDENTIFIER, ENVIRONMENT,
};
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, WorkspaceError>;
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Proof {
    schema_version: u32,
    identifier: String,
    environment: String,
    kind: String,
    data_root_identity: WindowsObjectIdentity,
    directory_identity: WindowsObjectIdentity,
}
fn invalid() -> WorkspaceError {
    WorkspaceError {
        code: "DEVELOPMENT_ROOT_UNRECOGNIZED",
        message: "开发目录的格式或身份不能确认，已保留现场。".into(),
    }
}
pub fn area(
    data: &WorkspaceDirectory,
    kind: &str,
    create: bool,
) -> Result<Option<WorkspaceDirectory>> {
    if !matches!(kind, "development-tools" | "sources" | "tasks") {
        return Err(invalid());
    }
    data.verify_binding()?;
    let parent = data
        .open_directory("workspace")?
        .open_directory(".platform")?;
    let exists = parent.list()?.iter().any(|entry| entry.name == kind);
    if !exists {
        if !create {
            return Ok(None);
        }
        let directory = parent.create_directory(kind)?;
        let proof = Proof {
            schema_version: 1,
            identifier: APP_IDENTIFIER.into(),
            environment: ENVIRONMENT.into(),
            kind: kind.into(),
            data_root_identity: data.identity().clone(),
            directory_identity: directory.identity().clone(),
        };
        directory.write_new(
            ".windows-development.json",
            &serde_json::to_vec(&proof).map_err(|_| invalid())?,
        )?;
    }
    let directory = parent.open_directory(kind)?;
    let proof: Proof = serde_json::from_slice(
        &directory
            .read_bounded(".windows-development.json", 65536)?
            .bytes,
    )
    .map_err(|_| invalid())?;
    if proof.schema_version != 1
        || proof.identifier != APP_IDENTIFIER
        || proof.environment != ENVIRONMENT
        || proof.kind != kind
        || proof.data_root_identity != *data.identity()
        || proof.directory_identity != *directory.identity()
    {
        return Err(invalid());
    }
    // Payloads have their own typed records and are validated before use. No
    // incomplete source or tool setting is treated as an executable project.
    for entry in directory.list()? {
        if entry.name == ".windows-development.json" && !entry.is_directory {
            continue;
        }
        let valid = if kind == "development-tools" {
            !entry.is_directory
                && entry
                    .name
                    .strip_prefix("selection-")
                    .and_then(|s| s.strip_suffix(".json"))
                    .is_some_and(|s| s.len() == 8 && s.bytes().all(|c| c.is_ascii_digit()))
        } else {
            entry.is_directory
                && entry
                    .name
                    .strip_prefix(if kind == "sources" { "src_" } else { "task_" })
                    .is_some_and(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        };
        if !valid {
            return Err(invalid());
        }
    }
    directory.verify_binding()?;
    Ok(Some(directory))
}
