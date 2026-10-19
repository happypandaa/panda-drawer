//! Recognize lifecycle journal bytes without inspecting any project or trash.
use super::{
    control, exact_fields, identity, nullable_string, optional_error, trash_id, uuid_text,
    valid_id, EvidenceStatus,
};
use serde_json::Value;

const FIELDS: &[&str] = &[
    "schemaVersion",
    "taskId",
    "kind",
    "projectId",
    "sourceProjectId",
    "trashId",
    "targetEpoch",
    "phase",
    "directoryIdentity",
    "control",
    "destination",
    "publishedSha256",
    "committed",
    "resolved",
    "result",
    "error",
];

fn opaque_path(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| !text.is_empty() && !text.contains('\0'))
}

fn phase_known(kind: &str, phase: &str) -> bool {
    phase == "intent"
        || match kind {
            "export" => matches!(
                phase,
                "snapshot-freeze" | "archive-verify" | "export-publish"
            ),
            "copy" => matches!(
                phase,
                "snapshot-freeze" | "candidate-initialize" | "install-intent"
            ),
            "import" => matches!(
                phase,
                "archive-copy" | "archive-validate" | "candidate-initialize" | "install-intent"
            ),
            "delete" => matches!(phase, "trash-freeze" | "trash-move-intent"),
            "trashRestore" => matches!(phase, "trash-restore-check" | "trash-restore-intent"),
            "trashPurge" => phase == "purge-intent",
            _ => false,
        }
}

fn positive_outcome(kind: &str, outcome: &str) -> bool {
    matches!(
        (kind, outcome),
        ("export", "exported")
            | ("copy", "copied" | "registered_maintenance_not_ready")
            | ("import", "imported" | "registered_maintenance_not_ready")
            | ("delete", "trashed")
            | ("trashRestore", "restored")
            | ("trashPurge", "purged")
    )
}

fn negative_outcome(kind: &str, outcome: &str) -> bool {
    matches!(
        (kind, outcome),
        ("export", "not_published")
            | ("copy" | "import", "not_registered")
            | ("delete", "not_deleted")
            | ("trashRestore", "not_restored")
            | ("trashPurge", "not_purged" | "purge_incomplete")
    )
}

pub(super) fn classify(value: &Value, owner: &str) -> EvidenceStatus {
    use EvidenceStatus::*;
    if !exact_fields(value, FIELDS)
        || value["schemaVersion"].as_u64() != Some(1)
        || !["taskId", "kind", "projectId", "targetEpoch", "phase"]
            .iter()
            .all(|field| value[*field].is_string())
        || ![
            "sourceProjectId",
            "trashId",
            "destination",
            "publishedSha256",
        ]
        .iter()
        .all(|field| nullable_string(&value[*field]))
        || !["committed", "resolved"]
            .iter()
            .all(|field| value[*field].is_boolean())
        || !value["result"].is_object()
        || !optional_error(&value["error"])
        || !(value["directoryIdentity"].is_null() || identity(&value["directoryIdentity"]))
        || !(value["control"].is_null() || control(&value["control"]).is_some())
    {
        return UnrecognizedRecord;
    }
    let task = value["taskId"].as_str().unwrap();
    let project = value["projectId"].as_str().unwrap();
    let source = value["sourceProjectId"].as_str();
    let trash = value["trashId"].as_str();
    let epoch = value["targetEpoch"].as_str().unwrap();
    if task != owner
        || !valid_id(task)
        || !valid_id(project)
        || source.is_some_and(|id| !valid_id(id))
        || trash.is_some_and(|id| !trash_id(id))
    {
        return IdentityMismatch;
    }
    let kind = value["kind"].as_str().unwrap();
    let phase = value["phase"].as_str().unwrap();
    if !matches!(
        kind,
        "export" | "copy" | "import" | "delete" | "trashRestore" | "trashPurge"
    ) || !phase_known(kind, phase)
        || !uuid_text(epoch)
    {
        return UnrecognizedRecord;
    }
    let source_shape = match kind {
        "export" | "delete" => source == Some(project),
        "copy" => source.is_some_and(|id| id != project),
        "import" => source.is_some(),
        _ => source.is_none(),
    };
    if !source_shape
        || (matches!(kind, "delete" | "trashRestore" | "trashPurge") != trash.is_some())
    {
        return UnrecognizedRecord;
    }
    // Old Mac paths are opaque text here. In particular, a copy/delete request
    // can persist an unused destinationPath; it is not a filesystem capability.
    let destination_ok = match kind {
        "export" => opaque_path(&value["destination"]),
        "copy" | "delete" => value["destination"].is_null() || value["destination"].is_string(),
        _ => value["destination"].is_null(),
    };
    let prepared = match kind {
        "copy" | "import" => phase == "install-intent",
        "delete" => phase == "trash-move-intent",
        "trashRestore" | "trashPurge" => true,
        _ => false,
    };
    if !destination_ok
        || prepared == value["directoryIdentity"].is_null()
        || prepared == value["control"].is_null()
    {
        return UnrecognizedRecord;
    }
    let digest_ok = if kind == "export" && phase == "export-publish" {
        value["publishedSha256"].as_str().is_some_and(|digest| {
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    } else {
        value["publishedSha256"].is_null()
    };
    if !digest_ok {
        return UnrecognizedRecord;
    }

    let result = value["result"].as_object().unwrap();
    // These fields bind a result to its journal. Diagnostics and the other
    // operation-specific result data remain opaque, recursively duplicate-free.
    for (field, expected) in [
        ("projectId", Some(project)),
        ("targetProjectId", Some(project)),
        ("sourceProjectId", source),
        ("trashId", trash),
        ("documentEpoch", Some(epoch)),
    ] {
        if let Some(actual) = result.get(field) {
            if !(actual.is_string() || actual.is_null()) {
                return UnrecognizedRecord;
            }
            if actual.as_str() != expected {
                return IdentityMismatch;
            }
        }
    }
    let committed = value["committed"].as_bool().unwrap();
    let resolved = value["resolved"].as_bool().unwrap();
    // These operations can only commit after the digest or prepared directory
    // identity was saved in their final intent. Trash recovery can also confirm
    // completion from an earlier phase using the initial saved identity.
    if committed
        && match kind {
            "export" => phase != "export-publish",
            "copy" | "import" => phase != "install-intent",
            "delete" => phase != "trash-move-intent",
            _ => false,
        }
    {
        return UnrecognizedRecord;
    }
    let outcome = result.get("operationOutcome").and_then(Value::as_str);
    if result.contains_key("operationOutcome") && outcome.is_none() {
        return UnrecognizedRecord;
    }
    if let Some(outcome) = outcome {
        if !(positive_outcome(kind, outcome) || negative_outcome(kind, outcome)) {
            return UnrecognizedRecord;
        }
    }
    // The existing trash-restore reconciler may confirm an already moved root
    // using the initial identity, without filling in the still-empty result.
    // Recognize that narrow history shape without inventing result metadata.
    let recovered_empty_restore = kind == "trashRestore"
        && matches!(phase, "intent" | "trash-restore-check")
        && committed
        && result.is_empty();
    let matching_result = result.get("projectId").and_then(Value::as_str) == Some(project)
        && outcome.is_some_and(|outcome| {
            if committed {
                positive_outcome(kind, outcome)
            } else {
                negative_outcome(kind, outcome)
            }
        });
    if (committed || resolved) && !recovered_empty_restore && !matching_result {
        return UnrecognizedRecord;
    }
    // A resolved purge failure still records retained, potentially partial
    // contents. Never erase that distinction merely because the task was closed.
    if outcome == Some("purge_incomplete") {
        IncompleteOperation
    } else if resolved {
        HistoricalResolved
    } else {
        // All four flag pairs can be persisted around failed completion saves.
        // Neither a committed flag nor saved control proves current health.
        UnresolvedOperation
    }
}
