//! Classification of caller-supplied historical record bytes.
//!
//! This crate performs no I/O. Its results neither admit a runtime nor verify
//! paths, file identities, backup archives, or the current project state.
#![forbid(unsafe_code)]

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::fmt;

mod lifecycle;
mod migration;
mod trash;

pub const MAX_RECORD_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceKind {
    MaintenanceControl,
    RestoreJournal,
    MigrationJournal,
    LifecycleJournal,
    TrashRecord,
    CleanupEvidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceInput<'a> {
    Missing,
    Unavailable,
    Bytes(&'a [u8]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnvironmentIdentity<'a> {
    pub identifier: &'a str,
    pub environment: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvidenceContext<'a> {
    pub kind: EvidenceKind,
    pub owner_id: &'a str,
    /// Trusted host configuration, never values copied from the input record.
    /// Required for migration journals; not used by the other record kinds.
    pub expected_environment: Option<EnvironmentIdentity<'a>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceStatus {
    NoEvidence,
    Unavailable,
    UnsupportedKind,
    TooLarge,
    InvalidRecord,
    UnrecognizedRecord,
    IdentityMismatch,
    InactiveControl,
    MaintenanceActive,
    RecoveryRequired,
    UnresolvedOperation,
    /// A purge can be resolved as a task while retained contents remain partial.
    IncompleteOperation,
    /// A saved trash record does not establish the payload's current location.
    RetainedTrash,
    HistoricalResolved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assessment {
    pub kind: EvidenceKind,
    pub status: EvidenceStatus,
}

/// `owner_id` is a project ID for maintenance and a task ID for restoration or
/// migration/lifecycle. For trash records it is the trash ID. Migration
/// additionally requires the trusted host environment.
/// No result, including absent evidence or a resolved history record, grants
/// permission to read a workspace or start its runtime.
pub fn inspect_record(context: EvidenceContext<'_>, input: EvidenceInput<'_>) -> Assessment {
    let invalid_environment = context.kind == EvidenceKind::MigrationJournal
        && !context.expected_environment.is_some_and(|expected| {
            [expected.identifier, expected.environment]
                .iter()
                .all(|value| !value.is_empty() && !value.contains('\0'))
        });
    let invalid_owner = !valid_id(context.owner_id)
        || (context.kind == EvidenceKind::TrashRecord && !trash_id(context.owner_id));
    let status = if invalid_owner || invalid_environment {
        EvidenceStatus::IdentityMismatch
    } else if !matches!(
        context.kind,
        EvidenceKind::MaintenanceControl
            | EvidenceKind::RestoreJournal
            | EvidenceKind::MigrationJournal
            | EvidenceKind::LifecycleJournal
            | EvidenceKind::TrashRecord
    ) {
        EvidenceStatus::UnsupportedKind
    } else {
        match input {
            EvidenceInput::Missing => EvidenceStatus::NoEvidence,
            EvidenceInput::Unavailable => EvidenceStatus::Unavailable,
            EvidenceInput::Bytes(bytes) if bytes.len() > MAX_RECORD_BYTES => {
                EvidenceStatus::TooLarge
            }
            EvidenceInput::Bytes(bytes) => match serde_json::from_slice::<UniqueValue>(bytes) {
                Err(_) => EvidenceStatus::InvalidRecord,
                Ok(value) => match context.kind {
                    EvidenceKind::MaintenanceControl => control(&value.0)
                        .map(|record| record.status)
                        .unwrap_or(EvidenceStatus::UnrecognizedRecord),
                    EvidenceKind::RestoreJournal => restore(&value.0, context.owner_id),
                    EvidenceKind::MigrationJournal => migration::classify(
                        &value.0,
                        context.owner_id,
                        context.expected_environment.unwrap(),
                    ),
                    EvidenceKind::LifecycleJournal => {
                        lifecycle::classify(&value.0, context.owner_id)
                    }
                    EvidenceKind::TrashRecord => trash::classify(&value.0, context.owner_id),
                    _ => EvidenceStatus::UnsupportedKind,
                },
            },
        }
    };
    Assessment {
        kind: context.kind,
        status,
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

fn backup_id(value: &str) -> bool {
    value.len() == 39
        && value.starts_with("backup_")
        && value.as_bytes()[7..].iter().all(u8::is_ascii_hexdigit)
}

fn trash_id(value: &str) -> bool {
    value.starts_with("trash_") && valid_id(value)
}

fn uuid_text(text: &str) -> bool {
    text.len() == 36
        && text.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn exact_fields(value: &Value, fields: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == fields.len() && fields.iter().all(|field| object.contains_key(*field))
    })
}

fn nullable_string(value: &Value) -> bool {
    value.is_null() || value.is_string()
}

fn optional_error(value: &Value) -> bool {
    value.is_null()
        || (exact_fields(value, &["code", "message", "details", "retryable"])
            && value["code"].is_string()
            && value["message"].is_string()
            && value["retryable"].is_boolean())
}

const CONTROL_FIELDS: &[&str] = &[
    "mode",
    "maintenanceId",
    "reason",
    "startedAt",
    "baselineBackupId",
    "protectedBackupIds",
    "readyForChanges",
    "recoveryRequired",
    "baselineInputDigest",
    "pendingBuild",
    "pendingBuildReason",
    "hadArtifact",
    "lastError",
    "lastTaskId",
];

struct Control<'a> {
    mode: &'a str,
    baseline: Option<&'a str>,
    ready_for_changes: bool,
    recovery_required: bool,
    status: EvidenceStatus,
}

fn control(value: &Value) -> Option<Control<'_>> {
    if !exact_fields(value, CONTROL_FIELDS)
        || ![
            "maintenanceId",
            "reason",
            "baselineBackupId",
            "baselineInputDigest",
            "pendingBuildReason",
            "lastTaskId",
        ]
        .iter()
        .all(|field| nullable_string(&value[*field]))
        || !(value["startedAt"].is_null() || value["startedAt"].as_u64().is_some())
        || ![
            "readyForChanges",
            "recoveryRequired",
            "pendingBuild",
            "hadArtifact",
        ]
        .iter()
        .all(|field| value[*field].is_boolean())
        || !optional_error(&value["lastError"])
    {
        return None;
    }
    let mode = value["mode"].as_str()?;
    if !matches!(mode, "ready" | "maintenance") {
        return None;
    }
    let maintenance = value["maintenanceId"].as_str();
    let baseline = value["baselineBackupId"].as_str();
    let protected = value["protectedBackupIds"].as_array()?;
    if maintenance.is_some_and(|id| !valid_id(id))
        || baseline.is_some_and(|id| !backup_id(id))
        || value["lastTaskId"].as_str().is_some_and(|id| !valid_id(id))
        || !protected
            .iter()
            .all(|id| id.as_str().is_some_and(backup_id))
    {
        return None;
    }
    let ready_for_changes = value["readyForChanges"].as_bool()?;
    let recovery_required = value["recoveryRequired"].as_bool()?;
    let status = if recovery_required {
        EvidenceStatus::RecoveryRequired
    } else if mode == "ready" {
        if maintenance.is_some() || baseline.is_some() || ready_for_changes || !protected.is_empty()
        {
            return None;
        }
        EvidenceStatus::InactiveControl
    } else {
        maintenance?;
        if ready_for_changes
            && !baseline.is_some_and(|id| protected.iter().any(|entry| entry.as_str() == Some(id)))
        {
            return None;
        }
        EvidenceStatus::MaintenanceActive
    };
    Some(Control {
        mode,
        baseline,
        ready_for_changes,
        recovery_required,
        status,
    })
}

const RESTORE_FIELDS: &[&str] = &[
    "taskId",
    "projectId",
    "backupId",
    "originalIdentity",
    "candidateIdentity",
    "committed",
    "resolved",
    "finalControl",
    "previousControl",
    "previousProvenance",
];

fn identity(value: &Value) -> bool {
    if let Some(parts) = value.as_array() {
        return parts.len() == 2 && parts.iter().all(|part| part.as_u64().is_some());
    }
    // Historical tuples, legacy objects and the current UUID-bearing objects
    // describe record formats only. No shape establishes that a file, inode or
    // volume exists, authorizes a repair, or grants runtime admission.
    if value["device"].as_u64().is_none() || value["inode"].as_u64().is_none() {
        return false;
    }
    if exact_fields(value, &["device", "inode"]) {
        return true;
    }
    exact_fields(value, &["device", "inode", "volumeUuid"])
        && value["volumeUuid"].as_str().is_some_and(volume_uuid)
}

fn volume_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
        && bytes
            .iter()
            .any(|byte| byte.is_ascii_hexdigit() && *byte != b'0')
}

fn restore(value: &Value, owner: &str) -> EvidenceStatus {
    use EvidenceStatus::*;
    if !exact_fields(value, RESTORE_FIELDS)
        || !["taskId", "projectId", "backupId"]
            .iter()
            .all(|field| value[*field].is_string())
        || !identity(&value["originalIdentity"])
        || !identity(&value["candidateIdentity"])
        || !value["committed"].is_boolean()
        || !value["resolved"].is_boolean()
    {
        return UnrecognizedRecord;
    }
    let task = value["taskId"].as_str().unwrap();
    let project = value["projectId"].as_str().unwrap();
    if !valid_id(task) || !valid_id(project) || task != owner {
        return IdentityMismatch;
    }
    let backup = value["backupId"].as_str().unwrap();
    if !backup_id(backup) || control(&value["previousControl"]).is_none() {
        return UnrecognizedRecord;
    }
    let committed = value["committed"].as_bool().unwrap();
    if value["finalControl"].is_null() {
        if committed {
            return UnrecognizedRecord;
        }
    } else {
        // The existing writer publishes finalControl and committed=true in the
        // same atomic journal replacement; rollback never supplies finalControl.
        if !committed {
            return UnrecognizedRecord;
        }
        let Some(final_control) = control(&value["finalControl"]) else {
            return UnrecognizedRecord;
        };
        if final_control.mode != "maintenance"
            || final_control.recovery_required
            || !final_control.ready_for_changes
            || final_control.baseline != Some(backup)
        {
            return UnrecognizedRecord;
        }
    }
    // Previous control and provenance are historical snapshots. They are never
    // applied to current state or used to decide that a workspace is healthy.
    if value["resolved"].as_bool().unwrap() {
        HistoricalResolved
    } else {
        UnresolvedOperation
    }
}

// serde_json::Value normally keeps the last duplicate object key. Preserve no
// such ambiguity: reject duplicates at every depth, including opaque metadata.
struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| UniqueValue(Value::Number(number)))
                    .ok_or_else(|| E::custom("invalid JSON number"))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<UniqueValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some(key) = object.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom("duplicate JSON object key"));
                    }
                    values.insert(key, object.next_value::<UniqueValue>()?.0);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod migration_tests;

#[cfg(test)]
mod lifecycle_tests;

#[cfg(test)]
mod trash_tests;
