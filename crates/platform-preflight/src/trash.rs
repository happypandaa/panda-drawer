//! Recognize saved trash-record bytes without locating or inspecting payloads.
use super::{control, exact_fields, identity, optional_error, trash_id, valid_id, EvidenceStatus};
use serde_json::Value;

const FIELDS: &[&str] = &[
    "schemaVersion",
    "trashId",
    "projectId",
    "name",
    "deletedAt",
    "state",
    "control",
    "rootIdentity",
    "deleteTaskId",
    "lastTaskId",
    "issues",
];

pub(super) fn classify(value: &Value, owner: &str) -> EvidenceStatus {
    use EvidenceStatus::*;
    if !exact_fields(value, FIELDS)
        || value["schemaVersion"].as_u64() != Some(1)
        || ![
            "trashId",
            "projectId",
            "name",
            "state",
            "deleteTaskId",
            "lastTaskId",
        ]
        .iter()
        .all(|field| value[*field].is_string())
        || value["deletedAt"].as_u64().is_none()
        || !identity(&value["rootIdentity"])
    {
        return UnrecognizedRecord;
    }

    let trash = value["trashId"].as_str().unwrap();
    if !trash_id(trash)
        || trash != owner
        || !["projectId", "deleteTaskId", "lastTaskId"]
            .iter()
            .all(|field| valid_id(value[*field].as_str().unwrap()))
    {
        return IdentityMismatch;
    }

    let Some(issues) = value["issues"].as_array() else {
        return UnrecognizedRecord;
    };
    if !issues
        .iter()
        .all(|issue| !issue.is_null() && optional_error(issue))
    {
        return UnrecognizedRecord;
    }
    let Some(saved_control) = control(&value["control"]) else {
        return UnrecognizedRecord;
    };

    // The writer saves "trashed" before moving the payload, and later updates
    // lastTaskId for restore/purge attempts. Neither field proves file location
    // or that the delete and latest task are the same operation.
    match value["state"].as_str().unwrap() {
        "trashed" => RetainedTrash,
        "purging" => UnresolvedOperation,
        "purgeIncomplete" => IncompleteOperation,
        "restored" | "cancelled" => HistoricalResolved,
        "purged" => {
            // The only successful-purge writer resets every control field to
            // its default and clears issues. An inactive control with leftover
            // metadata is not that known shape, nor is it current permission.
            let reset = saved_control.status == InactiveControl
                && [
                    "reason",
                    "startedAt",
                    "baselineInputDigest",
                    "pendingBuildReason",
                    "lastError",
                    "lastTaskId",
                ]
                .iter()
                .all(|field| value["control"][*field].is_null())
                && ["pendingBuild", "hadArtifact"]
                    .iter()
                    .all(|field| value["control"][*field].as_bool() == Some(false));
            if reset && issues.is_empty() {
                HistoricalResolved
            } else {
                UnrecognizedRecord
            }
        }
        // The reader understands needsAttention, but the current writer does
        // not persist it. Keep that and future states outside this decoder.
        _ => UnrecognizedRecord,
    }
}
