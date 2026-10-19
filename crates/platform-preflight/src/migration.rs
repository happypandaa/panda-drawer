//! Recognize the current journal writer's bytes, without opening either root.
use super::{exact_fields, optional_error, valid_id, EnvironmentIdentity, EvidenceStatus};
use serde_json::Value;

const FIELDS: &[&str] = &[
    "schemaVersion",
    "identifier",
    "environment",
    "taskId",
    "sourcePath",
    "destinationPath",
    "sourceIdentity",
    "targetEmptyIdentity",
    "destinationIdentity",
    "stagePath",
    "phase",
    "published",
    "committed",
    "resolved",
    "epochs",
    "epochsApplied",
    "blockedProjects",
    "fileCount",
    "totalBytes",
    "startedAt",
    "retainedSourceWarning",
];

fn directory_identity(value: &Value) -> bool {
    // These are the existing Mac journal fields, not Windows object identities.
    exact_fields(value, &["device", "inode"])
        && value["device"].as_u64().is_some()
        && value["inode"].as_u64().is_some()
}

fn epoch(value: &Value) -> bool {
    value.as_str().is_some_and(super::uuid_text)
}

pub(super) fn classify(
    value: &Value,
    owner: &str,
    expected: EnvironmentIdentity<'_>,
) -> EvidenceStatus {
    use EvidenceStatus::*;
    if !exact_fields(value, FIELDS)
        || value["schemaVersion"].as_u64() != Some(1)
        || !["identifier", "environment", "taskId", "phase"]
            .iter()
            .all(|field| value[*field].is_string())
        || !["sourcePath", "destinationPath", "stagePath"]
            .iter()
            .all(|field| {
                value[*field]
                    .as_str()
                    .is_some_and(|text| !text.is_empty() && !text.contains('\0'))
            })
        || !["published", "committed", "resolved", "epochsApplied"]
            .iter()
            .all(|field| value[*field].is_boolean())
        || !["fileCount", "totalBytes", "startedAt"]
            .iter()
            .all(|field| value[*field].as_u64().is_some())
        || !(value["retainedSourceWarning"].is_null() || value["retainedSourceWarning"].is_string())
        || !directory_identity(&value["sourceIdentity"])
        || !directory_identity(&value["targetEmptyIdentity"])
        || !(value["destinationIdentity"].is_null()
            || directory_identity(&value["destinationIdentity"]))
    {
        return UnrecognizedRecord;
    }

    let task = value["taskId"].as_str().unwrap();
    if !valid_id(task)
        || task != owner
        || value["identifier"].as_str() != Some(expected.identifier)
        || value["environment"].as_str() != Some(expected.environment)
    {
        return IdentityMismatch;
    }

    let (Some(epochs), Some(blocked)) = (
        value["epochs"].as_object(),
        value["blockedProjects"].as_object(),
    ) else {
        return UnrecognizedRecord;
    };
    if !epochs
        .iter()
        .all(|(project, version)| valid_id(project) && epoch(version))
        || !blocked.iter().all(|(project, error)| {
            valid_id(project)
                && epochs.contains_key(project)
                && !error.is_null()
                && optional_error(error)
        })
    {
        return UnrecognizedRecord;
    }

    let published = value["published"].as_bool().unwrap();
    let committed = value["committed"].as_bool().unwrap();
    let resolved = value["resolved"].as_bool().unwrap();
    let applied = value["epochsApplied"].as_bool().unwrap();
    let has_destination = !value["destinationIdentity"].is_null();

    // journal.phase is not the task's display phase. Destination identity is
    // persisted for the staging tree before publication. A committing journal
    // with committed=false may already have committed settings: these bytes
    // cannot select the current workspace or authorize falling back to source.
    let coherent = match value["phase"].as_str().unwrap() {
        "queued" | "copying" => {
            !published && !committed && !resolved && !applied && !has_destination
        }
        "publishing" => !published && !committed && !resolved && !applied && has_destination,
        "committing" => published && !resolved && has_destination && (!applied || committed),
        "completed" => published && committed && resolved && applied && has_destination,
        _ => false,
    };
    if !coherent {
        return UnrecognizedRecord;
    }
    // Paths, errors, warnings and epochs remain historical data. Nothing is
    // followed, applied, repaired or returned as a current workspace permission.
    if resolved {
        HistoricalResolved
    } else {
        UnresolvedOperation
    }
}
