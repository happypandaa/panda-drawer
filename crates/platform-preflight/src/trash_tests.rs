//! Synthetic retained-record bytes. No payload, baseline, directory or current
//! project state is inspected, and no classification grants restore/delete rights.
use super::lifecycle_tests::{maintenance_control, ready_control, synthetic_error};
use super::*;
use serde_json::{json, Value};

const TRASH: &str = "trash_0123456789abcdef0123456789abcdef";
const PROJECT: &str = "prj_0123456789abcdef0123456789abcdef";
const DELETE_TASK: &str = "task_0123456789abcdef0123456789abcdef";
const LAST_TASK: &str = "task_abcdef0123456789abcdef0123456789";

fn record(state: &str) -> Value {
    json!({
        "schemaVersion":1, "trashId":TRASH, "projectId":PROJECT,
        "name":"合成回收记录", "deletedAt":1_800_000_000_000_u64, "state":state,
        "control":if state == "purged" {ready_control()} else {maintenance_control()},
        "rootIdentity":[1,200], "deleteTaskId":DELETE_TASK, "lastTaskId":LAST_TASK, "issues":[]
    })
}

fn context() -> EvidenceContext<'static> {
    EvidenceContext {
        kind: EvidenceKind::TrashRecord,
        owner_id: TRASH,
        expected_environment: None,
    }
}

fn check_bytes(bytes: &[u8], status: EvidenceStatus) {
    assert_eq!(
        inspect_record(context(), EvidenceInput::Bytes(bytes)),
        Assessment {
            kind: EvidenceKind::TrashRecord,
            status
        }
    );
}

fn check(value: &Value, status: EvidenceStatus) {
    check_bytes(&serde_json::to_vec(value).unwrap(), status);
}

#[test]
fn trashed_bytes_retain_evidence_without_proving_the_payload_has_moved() {
    // record.json is saved before the directory move by the actual writer.
    // This same record therefore cannot prove that restore is available.
    check(&record("trashed"), EvidenceStatus::RetainedTrash);
}

#[test]
fn purging_and_partial_purge_stay_distinct_from_completed_history() {
    check(&record("purging"), EvidenceStatus::UnresolvedOperation);
    // Empty issues are allowed; an interrupted writer need not have saved an
    // error payload before it saved or recovered the incomplete state.
    check(
        &record("purgeIncomplete"),
        EvidenceStatus::IncompleteOperation,
    );
    let mut partial = record("purgeIncomplete");
    partial["issues"] = json!([synthetic_error()]);
    check(&partial, EvidenceStatus::IncompleteOperation);
}

#[test]
fn restored_cancelled_and_purged_records_are_history_not_current_ownership() {
    for state in ["restored", "cancelled", "purged"] {
        check(&record(state), EvidenceStatus::HistoricalResolved);
    }
}

#[test]
fn saved_controls_are_historical_snapshots_not_current_project_modes() {
    for (state, status) in [
        ("trashed", EvidenceStatus::RetainedTrash),
        ("purging", EvidenceStatus::UnresolvedOperation),
        ("purgeIncomplete", EvidenceStatus::IncompleteOperation),
        ("restored", EvidenceStatus::HistoricalResolved),
        ("cancelled", EvidenceStatus::HistoricalResolved),
    ] {
        for mut control in [ready_control(), maintenance_control()] {
            let mut value = record(state);
            value["control"] = control.clone();
            check(&value, status);
            control = maintenance_control();
            control["recoveryRequired"] = json!(true);
            value["control"] = control;
            check(&value, status);
        }
    }
}

#[test]
fn purged_records_need_the_exact_reset_control_and_no_saved_issues() {
    let mut value = record("purged");
    value["control"] = maintenance_control();
    check(&value, EvidenceStatus::UnrecognizedRecord);
    value = record("purged");
    value["issues"] = json!([synthetic_error()]);
    check(&value, EvidenceStatus::UnrecognizedRecord);
    for (field, leftover) in [
        ("reason", json!("Historical inactive metadata")),
        ("startedAt", json!(1)),
        ("pendingBuild", json!(true)),
        ("pendingBuildReason", json!("Previous pending work")),
        ("hadArtifact", json!(true)),
        ("baselineInputDigest", json!("old digest")),
        ("lastTaskId", json!(LAST_TASK)),
        ("lastError", synthetic_error()),
    ] {
        value = record("purged");
        value["control"][field] = leftover;
        check(&value, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn later_task_ids_and_legacy_valid_trash_ids_are_not_forced_to_uuid_equality() {
    let value = record("trashed");
    assert_ne!(value["deleteTaskId"], value["lastTaskId"]);
    check(&value, EvidenceStatus::RetainedTrash);
    let mut value = record("trashed");
    value["trashId"] = json!("trash_legacy-id");
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut owner = context();
    owner.owner_id = "trash_legacy-id";
    assert_eq!(
        inspect_record(owner, EvidenceInput::Bytes(&bytes)).status,
        EvidenceStatus::RetainedTrash
    );
}

#[test]
fn all_current_record_fields_are_required_and_future_fields_stay_unrecognized() {
    let full = record("trashed");
    assert_eq!(full.as_object().unwrap().len(), 11);
    for field in full.as_object().unwrap().keys() {
        let mut value = full.clone();
        value.as_object_mut().unwrap().remove(field);
        check(&value, EvidenceStatus::UnrecognizedRecord);
    }
    for (field, value) in [
        ("schemaVersion", json!(0)),
        ("schemaVersion", json!(2)),
        ("schemaVersion", json!("1")),
        ("futureField", json!(true)),
    ] {
        let mut changed = full.clone();
        changed[field] = value;
        check(&changed, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn record_owners_and_other_ids_cannot_be_paths_or_claim_another_record() {
    for (field, value) in [
        ("trashId", "trash_other"),
        ("trashId", "not-trash"),
        ("projectId", "../project"),
        ("deleteTaskId", "task/path"),
        ("lastTaskId", "UPPERCASE"),
    ] {
        let mut changed = record("trashed");
        changed[field] = json!(value);
        check(&changed, EvidenceStatus::IdentityMismatch);
    }
    let bytes = serde_json::to_vec(&record("trashed")).unwrap();
    for owner in ["", "task_valid-but-wrong-kind", "../trash", "trash/escape"] {
        let mut changed = context();
        changed.owner_id = owner;
        assert_eq!(
            inspect_record(changed, EvidenceInput::Bytes(&bytes)).status,
            EvidenceStatus::IdentityMismatch
        );
    }
}

#[test]
fn record_types_control_defaults_and_error_shapes_are_never_coerced() {
    for (field, value) in [
        ("trashId", json!(1)),
        ("projectId", Value::Null),
        ("name", json!([])),
        ("deletedAt", json!(-1)),
        ("state", json!(true)),
        ("control", json!({})),
        ("rootIdentity", json!({"device":1,"inode":200})),
        ("rootIdentity", json!([1, -1])),
        ("deleteTaskId", json!(0)),
        ("lastTaskId", json!(false)),
        ("issues", Value::Null),
        ("issues", json!([null])),
        (
            "issues",
            json!([{"code":"X","message":"missing details and retryable"}]),
        ),
    ] {
        let mut changed = record("trashed");
        changed[field] = value;
        check(&changed, EvidenceStatus::UnrecognizedRecord);
    }
    let mut changed = record("trashed");
    changed["control"]
        .as_object_mut()
        .unwrap()
        .remove("recoveryRequired");
    check(&changed, EvidenceStatus::UnrecognizedRecord);
}

#[test]
fn ui_fallback_and_unknown_states_are_not_persisted_writer_states() {
    for state in ["needsAttention", "future-state", "ready", "deleted"] {
        check(&record(state), EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn malformed_and_nested_duplicate_record_json_is_rejected() {
    for bytes in [b"".as_slice(), b"{".as_slice(), b"\xff".as_slice()] {
        check_bytes(bytes, EvidenceStatus::InvalidRecord);
    }
    let serialized = serde_json::to_string(&record("trashed")).unwrap();
    let duplicate = format!("{{\"\\u0073tate\":\"purged\",{}", &serialized[1..]);
    check_bytes(duplicate.as_bytes(), EvidenceStatus::InvalidRecord);
    let duplicate = serialized.replacen("\"control\":{", "\"control\":{\"mode\":\"ready\",", 1);
    assert_ne!(duplicate, serialized);
    check_bytes(duplicate.as_bytes(), EvidenceStatus::InvalidRecord);
    let mut value = record("purgeIncomplete");
    value["issues"] = json!([synthetic_error()]);
    let serialized = serde_json::to_string(&value).unwrap();
    let duplicate = serialized.replacen(
        "\"details\":{",
        "\"details\":{\"opaque\":1,\"opaque\":2,",
        1,
    );
    assert_ne!(duplicate, serialized);
    check_bytes(duplicate.as_bytes(), EvidenceStatus::InvalidRecord);
}

#[test]
fn absent_unavailable_or_oversized_records_never_prove_retained_contents() {
    assert_eq!(
        inspect_record(context(), EvidenceInput::Missing).status,
        EvidenceStatus::NoEvidence
    );
    assert_eq!(
        inspect_record(context(), EvidenceInput::Unavailable).status,
        EvidenceStatus::Unavailable
    );
    let mut bytes = serde_json::to_vec(&record("purged")).unwrap();
    bytes.resize(MAX_RECORD_BYTES, b' ');
    check_bytes(&bytes, EvidenceStatus::HistoricalResolved);
    bytes.push(b' ');
    check_bytes(&bytes, EvidenceStatus::TooLarge);
}

#[test]
fn names_control_reasons_and_issue_details_do_not_escape_the_assessment() {
    const MARKER: &str = "synthetic-private-trash-text";
    let mut value = record("purgeIncomplete");
    value["name"] = json!(MARKER);
    value["control"]["reason"] = json!(MARKER);
    let mut error = synthetic_error();
    error["details"] = json!({"payload":[MARKER]});
    value["issues"] = json!([error]);
    let bytes = serde_json::to_vec(&value).unwrap();
    let assessment = inspect_record(context(), EvidenceInput::Bytes(&bytes));
    assert_eq!(assessment.status, EvidenceStatus::IncompleteOperation);
    assert!(!format!("{assessment:?}").contains(MARKER));
}
