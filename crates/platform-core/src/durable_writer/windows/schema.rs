use super::{io, Kind, WriterOwner};
use crate::file_identity::PersistedIdentity;
use platform_native::windows::{boot_session, files};
use serde::{Deserialize, Serialize};
use std::{fs::File, path::Path};
use uuid::Uuid;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BootEvidence {
    pub source: String,
    pub boot_id: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Marker {
    pub version: u32,
    pub platform: String,
    pub kind: String,
    pub owner_id: String,
    pub token: String,
    pub component: String,
    pub file_identity: PersistedIdentity,
    pub boot_evidence: BootEvidence,
}

pub(super) fn kind(owner: &WriterOwner) -> &'static str {
    match owner._kind {
        Kind::Node => "node",
        Kind::Web => "web",
        Kind::Platform => "platform",
    }
}

pub(super) fn preparing_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".preparing-"))
        .is_some_and(|token| Uuid::parse_str(token).is_ok())
}

fn published_name(path: &Path, token: &str) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == format!("{token}.json")
        || name
            .strip_prefix(".retiring-")
            .and_then(|name| name.strip_suffix(".json"))
            .is_some_and(|token| Uuid::parse_str(token).is_ok())
}

pub(super) fn validate_published(
    owner: &WriterOwner,
    path: &Path,
    file: &File,
    bytes: &[u8],
    current_boot: u32,
) -> io::Result<()> {
    // No defaults for boot evidence: a pre-W5 record has no proof and remains
    // unknown, without migration or inference from its PID, age or file times.
    let marker: Marker = serde_json::from_slice(bytes).map_err(|error| io::blocked(path, error))?;
    if marker.version != 2
        || marker.platform != "windows"
        || marker.kind != kind(owner)
        || marker.owner_id != owner._id
        || !Uuid::parse_str(&marker.token).is_ok_and(|token| !token.is_nil())
        || !published_name(path, &marker.token)
        || marker.component.is_empty()
        || marker.component.len() > 256
        || marker.component.chars().any(char::is_control)
        || marker.boot_evidence.source != boot_session::SOURCE
        || !marker
            .file_identity
            .matches_open_runtime(file)
            .map_err(|error| io::blocked(path, error))?
    {
        return Err(io::blocked(
            path,
            "Writer marker proof does not match its owner and object",
        ));
    }
    // Zero is valid; inequality is the same session policy as Mac UUIDs. A
    // larger counter is not an additional requirement, and no other source's
    // counter can be compared with this process-telemetry BootId.
    if marker.boot_evidence.boot_id == current_boot {
        return Err(io::blocked(
            path,
            "A writer from the current boot remains unconfirmed",
        ));
    }
    let actual = files::identity(file).map_err(|error| io::blocked(path, error))?;
    if actual.links != 1 || actual.directory {
        return Err(io::blocked(
            path,
            "Writer marker is no longer an independent file",
        ));
    }
    Ok(())
}
