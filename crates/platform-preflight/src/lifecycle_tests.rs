//! Current lifecycle writer fixtures, entirely synthetic and in memory.
use super::*;
use serde_json::{json, Value};

const TASK: &str = "task_0123456789abcdef0123456789abcdef";
const PROJECT: &str = "prj_0123456789abcdef0123456789abcdef";
const SOURCE: &str = "prj_abcdef0123456789abcdef0123456789";
const TRASH: &str = "trash_0123456789abcdef0123456789abcdef";
const EPOCH: &str = "01234567-89ab-4cde-8fab-0123456789ab";
const KINDS: [&str; 6] = [
    "export",
    "copy",
    "import",
    "delete",
    "trashRestore",
    "trashPurge",
];

pub(super) fn ready_control() -> Value {
    json!({
        "mode":"ready", "maintenanceId":null, "reason":null, "startedAt":null,
        "baselineBackupId":null, "protectedBackupIds":[], "readyForChanges":false,
        "recoveryRequired":false, "baselineInputDigest":null, "pendingBuild":false,
        "pendingBuildReason":null, "hadArtifact":false, "lastError":null, "lastTaskId":null
    })
}

pub(super) fn maintenance_control() -> Value {
    let mut control = ready_control();
    control["mode"] = json!("maintenance");
    control["maintenanceId"] = json!("maintenance_synthetic");
    control["reason"] = json!("Synthetic historical maintenance");
    control["startedAt"] = json!(1_800_000_000_000_u64);
    control["pendingBuild"] = json!(true);
    control["pendingBuildReason"] = json!("Retain known source work");
    control
}

pub(super) fn synthetic_error() -> Value {
    json!({"code":"SYNTHETIC_FAILURE", "message":"Preserve retained evidence",
        "details":{"operationOutcome":"needs_attention"}, "retryable":false})
}

fn initial(kind: &str) -> Value {
    let trash_operation = matches!(kind, "trashRestore" | "trashPurge");
    json!({
        "schemaVersion":1, "taskId":TASK, "kind":kind, "projectId":PROJECT,
        "sourceProjectId":match kind { "export" | "delete" => json!(PROJECT), "copy" | "import" => json!(SOURCE), _ => Value::Null },
        "trashId":if matches!(kind, "delete" | "trashRestore" | "trashPurge") {json!(TRASH)} else {Value::Null},
        "targetEpoch":EPOCH, "phase":"intent",
        "directoryIdentity":if trash_operation {json!([1, 200])} else {Value::Null},
        "control":if trash_operation {maintenance_control()} else {Value::Null},
        "destination":if kind == "export" {json!("/synthetic/export.zip")} else {Value::Null},
        "publishedSha256":null, "committed":false, "resolved":false, "result":{}, "error":null
    })
}

fn prepared(kind: &str) -> Value {
    let mut journal = initial(kind);
    journal["phase"] = json!(match kind {
        "export" => "export-publish",
        "copy" | "import" => "install-intent",
        "delete" => "trash-move-intent",
        "trashRestore" => "trash-restore-intent",
        _ => "purge-intent",
    });
    if kind == "export" {
        journal["publishedSha256"] = json!("a".repeat(64));
        journal["result"] = json!({"projectId":PROJECT,"path":"/synthetic/export.zip",
            "backupId":"backup_0123456789abcdef0123456789abcdef", "createdAt":1,
            "snapshotState":"ready", "pendingBuild":{"required":false,"reason":null},
            "verified":true,"diagnostics":{},"operationOutcome":"exported"});
    } else {
        journal["directoryIdentity"] = json!([1, 200]);
        if matches!(kind, "copy" | "import" | "delete") {
            journal["control"] = ready_control();
        }
        journal["result"] = match kind {
            "copy" | "import" => json!({"projectId":PROJECT,"targetProjectId":PROJECT,
                "sourceProjectId":SOURCE,"documentEpoch":EPOCH,"mode":"ready",
                "maintenanceId":null,"readyForChanges":false,"baselineBackupId":null,
                "pendingBuild":{"required":false,"reason":null},"diagnostics":{},
                "operationOutcome":if kind == "copy" {"copied"} else {"imported"}}),
            "delete" => json!({"projectId":PROJECT,"trashId":TRASH,"operationOutcome":"trashed"}),
            "trashRestore" => json!({"projectId":PROJECT,"trashId":TRASH,"documentEpoch":EPOCH,
                "mode":"maintenance","maintenanceId":"maintenance_synthetic",
                "readyForChanges":false,"issues":[],"operationOutcome":"restored"}),
            _ => json!({}),
        };
    }
    journal
}

fn positive(kind: &str) -> Value {
    let mut journal = prepared(kind);
    journal["committed"] = json!(true);
    journal["resolved"] = json!(true);
    if kind == "trashPurge" {
        journal["control"] = ready_control();
        journal["result"] = json!({"projectId":PROJECT,"trashId":TRASH,
            "operationOutcome":"purged","historicalBackupsRetained":true,
            "restoreAvailable":false});
    }
    journal
}

fn negative(kind: &str) -> Value {
    let mut journal = initial(kind);
    journal["resolved"] = json!(true);
    journal["result"] = json!({"projectId":PROJECT,"trashId":journal["trashId"].clone(),
    "operationOutcome":match kind {
        "export" => "not_published", "copy" | "import" => "not_registered",
        "delete" => "not_deleted", "trashRestore" => "not_restored", _ => "not_purged"
    }});
    journal
}

fn context() -> EvidenceContext<'static> {
    EvidenceContext {
        kind: EvidenceKind::LifecycleJournal,
        owner_id: TASK,
        expected_environment: None,
    }
}

fn check_bytes(bytes: &[u8], status: EvidenceStatus) {
    assert_eq!(
        inspect_record(context(), EvidenceInput::Bytes(bytes)),
        Assessment {
            kind: EvidenceKind::LifecycleJournal,
            status
        }
    );
}

fn check(journal: &Value, status: EvidenceStatus) {
    check_bytes(&serde_json::to_vec(journal).unwrap(), status);
}

#[test]
fn every_current_kind_has_a_real_pending_intent_and_historical_positive_result() {
    for kind in KINDS {
        check(&initial(kind), EvidenceStatus::UnresolvedOperation);
        check(&prepared(kind), EvidenceStatus::UnresolvedOperation);
        check(&positive(kind), EvidenceStatus::HistoricalResolved);
    }
}

#[test]
fn current_early_phases_keep_their_unprepared_fields() {
    for (kind, phases) in [
        ("export", vec!["snapshot-freeze", "archive-verify"]),
        ("copy", vec!["snapshot-freeze", "candidate-initialize"]),
        (
            "import",
            vec!["archive-copy", "archive-validate", "candidate-initialize"],
        ),
        ("delete", vec!["trash-freeze"]),
        ("trashRestore", vec!["trash-restore-check"]),
    ] {
        for phase in phases {
            let mut journal = initial(kind);
            journal["phase"] = json!(phase);
            check(&journal, EvidenceStatus::UnresolvedOperation);
        }
    }
}

#[test]
fn all_flag_pairs_include_resolved_noncommit_without_claiming_success() {
    for kind in KINDS {
        check(&prepared(kind), EvidenceStatus::UnresolvedOperation);
        let mut lost_completion = positive(kind);
        lost_completion["resolved"] = json!(false);
        check(&lost_completion, EvidenceStatus::UnresolvedOperation);
        check(&positive(kind), EvidenceStatus::HistoricalResolved);
        check(&negative(kind), EvidenceStatus::HistoricalResolved);
    }
}

#[test]
fn failed_baseline_can_be_committed_while_management_changes_remain_blocked() {
    for kind in ["copy", "import"] {
        let mut journal = positive(kind);
        journal["control"] = maintenance_control();
        journal["error"] = synthetic_error();
        journal["result"]["operationOutcome"] = json!("registered_maintenance_not_ready");
        journal["result"]["mode"] = json!("maintenance");
        journal["result"]["maintenanceId"] = json!("maintenance_synthetic");
        journal["result"]["readyForChanges"] = json!(false);
        check(&journal, EvidenceStatus::HistoricalResolved);
    }
}

#[test]
fn purge_incomplete_keeps_its_distinction_even_when_the_journal_is_resolved() {
    for resolved in [false, true] {
        let mut journal = prepared("trashPurge");
        journal["resolved"] = json!(resolved);
        journal["error"] = synthetic_error();
        journal["result"] = json!({"projectId":PROJECT,"trashId":TRASH,
            "operationOutcome":"purge_incomplete","restoreAvailable":false,
            "remainingPath":"/synthetic/trash/payload"});
        check(&journal, EvidenceStatus::IncompleteOperation);
        journal["committed"] = json!(true);
        check(&journal, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn cleanup_child_can_resolve_a_partial_purge_from_its_initial_intent() {
    let mut journal = initial("trashPurge");
    journal["resolved"] = json!(true);
    journal["error"] = synthetic_error();
    journal["result"] = json!({"projectId":PROJECT,"trashId":TRASH,
        "operationOutcome":"purge_incomplete","restoreAvailable":false,
        "remainingPath":"/synthetic/trash/payload","parentTaskId":"task_cleanup_parent"});
    check(&journal, EvidenceStatus::IncompleteOperation);
}

#[test]
fn saved_negative_outcomes_with_neither_completion_flag_remain_unresolved() {
    for kind in KINDS {
        let mut journal = negative(kind);
        journal["resolved"] = json!(false);
        check(&journal, EvidenceStatus::UnresolvedOperation);
    }
}

#[test]
fn reconciled_early_trash_restore_may_keep_its_completely_empty_result() {
    for phase in ["intent", "trash-restore-check"] {
        for (resolved, status) in [
            (false, EvidenceStatus::UnresolvedOperation),
            (true, EvidenceStatus::HistoricalResolved),
        ] {
            let mut journal = initial("trashRestore");
            journal["phase"] = json!(phase);
            journal["committed"] = json!(true);
            journal["resolved"] = json!(resolved);
            check(&journal, status);
            // The exception recognizes precisely the untouched initial result.
            journal["result"] = json!({"opaque":"partial result"});
            check(&journal, EvidenceStatus::UnrecognizedRecord);
        }
    }
    let mut uncommitted = initial("trashRestore");
    uncommitted["resolved"] = json!(true);
    check(&uncommitted, EvidenceStatus::UnrecognizedRecord);
    let mut prepared = prepared("trashRestore");
    prepared["committed"] = json!(true);
    prepared["result"] = json!({});
    check(&prepared, EvidenceStatus::UnrecognizedRecord);
}

#[test]
fn reconciled_purged_record_can_complete_a_new_initial_purge_journal() {
    for (resolved, status) in [
        (false, EvidenceStatus::UnresolvedOperation),
        (true, EvidenceStatus::HistoricalResolved),
    ] {
        let mut journal = positive("trashPurge");
        journal["phase"] = json!("intent");
        journal["resolved"] = json!(resolved);
        check(&journal, status);
    }
}

#[test]
fn committed_export_install_and_delete_need_their_saved_final_intent() {
    for (kind, phase) in [
        ("export", "archive-verify"),
        ("copy", "candidate-initialize"),
        ("import", "candidate-initialize"),
        ("delete", "trash-freeze"),
    ] {
        for resolved in [false, true] {
            let mut journal = initial(kind);
            journal["phase"] = json!(phase);
            journal["committed"] = json!(true);
            journal["resolved"] = json!(resolved);
            journal["result"] = positive(kind)["result"].clone();
            check(&journal, EvidenceStatus::UnrecognizedRecord);
        }
    }
}

#[test]
fn original_import_and_unused_copy_delete_destinations_are_representable() {
    let mut original = positive("import");
    original["sourceProjectId"] = json!(PROJECT);
    original["result"]["sourceProjectId"] = json!(PROJECT);
    check(&original, EvidenceStatus::HistoricalResolved);
    for kind in ["copy", "delete"] {
        for destination in [r"D:\synthetic\unused-destination", "", "unused\0opaque"] {
            let mut journal = initial(kind);
            journal["destination"] = json!(destination);
            check(&journal, EvidenceStatus::UnresolvedOperation);
        }
    }
}

#[test]
fn source_and_trash_roles_must_match_the_recorded_operation() {
    for (kind, field, value) in [
        ("export", "sourceProjectId", Value::Null),
        ("export", "sourceProjectId", json!(SOURCE)),
        ("export", "trashId", json!(TRASH)),
        ("copy", "sourceProjectId", Value::Null),
        ("copy", "sourceProjectId", json!(PROJECT)),
        ("copy", "trashId", json!(TRASH)),
        ("import", "sourceProjectId", Value::Null),
        ("import", "trashId", json!(TRASH)),
        ("delete", "sourceProjectId", json!(SOURCE)),
        ("delete", "trashId", Value::Null),
        ("trashRestore", "sourceProjectId", json!(PROJECT)),
        ("trashRestore", "trashId", Value::Null),
        ("trashPurge", "sourceProjectId", json!(PROJECT)),
        ("trashPurge", "trashId", Value::Null),
    ] {
        let mut journal = initial(kind);
        journal[field] = value;
        check(&journal, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn current_full_field_set_does_not_inherit_nullable_serde_defaults() {
    let complete = positive("copy");
    assert_eq!(complete.as_object().unwrap().len(), 16);
    for field in complete.as_object().unwrap().keys() {
        let mut journal = complete.clone();
        journal.as_object_mut().unwrap().remove(field);
        check(&journal, EvidenceStatus::UnrecognizedRecord);
    }
    for (field, value) in [
        ("schemaVersion", json!(2)),
        ("schemaVersion", json!("1")),
        ("futureField", json!(true)),
    ] {
        let mut journal = complete.clone();
        journal[field] = value;
        check(&journal, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn journal_and_exposed_result_identities_cannot_claim_other_owners() {
    for (field, value) in [
        ("taskId", "task_another"),
        ("projectId", "../project"),
        ("sourceProjectId", "../source"),
        ("trashId", "not_a_trash_id"),
    ] {
        let mut journal = positive("delete");
        journal[field] = json!(value);
        check(&journal, EvidenceStatus::IdentityMismatch);
    }
    let bytes = serde_json::to_vec(&positive("copy")).unwrap();
    let mut other = context();
    other.owner_id = "task_other";
    assert_eq!(
        inspect_record(other, EvidenceInput::Bytes(&bytes)).status,
        EvidenceStatus::IdentityMismatch
    );
    for field in [
        "projectId",
        "targetProjectId",
        "sourceProjectId",
        "documentEpoch",
    ] {
        let mut journal = positive("copy");
        journal["result"][field] = json!("another_identity");
        check(&journal, EvidenceStatus::IdentityMismatch);
    }
    let mut journal = positive("delete");
    journal["result"]["trashId"] = json!("trash_other");
    check(&journal, EvidenceStatus::IdentityMismatch);
}

#[test]
fn unknown_kinds_phases_and_outcomes_do_not_look_like_historical_completion() {
    for (field, value) in [
        ("kind", "restore"),
        ("kind", "future-kind"),
        ("phase", "completed"),
        ("phase", "purge-intent"),
    ] {
        let mut journal = positive("copy");
        journal[field] = json!(value);
        check(&journal, EvidenceStatus::UnrecognizedRecord);
    }
    for outcome in ["future-outcome", "not_started", "purged", "not_registered"] {
        let mut journal = positive("copy");
        journal["result"]["operationOutcome"] = json!(outcome);
        check(&journal, EvidenceStatus::UnrecognizedRecord);
    }
    let mut negative_with_positive_outcome = negative("delete");
    negative_with_positive_outcome["result"]["operationOutcome"] = json!("trashed");
    check(
        &negative_with_positive_outcome,
        EvidenceStatus::UnrecognizedRecord,
    );
}

#[test]
fn prepared_records_need_recognizable_control_and_persisted_identity() {
    for field in ["control", "directoryIdentity"] {
        let mut journal = positive("copy");
        journal[field] = Value::Null;
        check(&journal, EvidenceStatus::UnrecognizedRecord);
    }
    for identity in [
        json!([1]),
        json!([1, -1]),
        json!({"device":1}),
        json!({"device":"1","inode":200}),
        json!({"device":1,"inode":false}),
        json!({"device":1,"inode":200,"futureField":true}),
        json!({"device":1,"inode":200,"volumeUuid":null}),
        json!({"device":1,"inode":200,"volumeUuid":1}),
        json!({"device":1,"inode":200,"volumeUuid":"not-a-uuid"}),
        json!({"device":1,"inode":200,"volumeUuid":"00000000-0000-0000-0000-000000000000"}),
        json!({"device":1,"inode":200,"volumeUuid":"01234567-89ab-4cde-8fab-0123456789ab","futureField":true}),
    ] {
        let mut journal = positive("delete");
        journal["directoryIdentity"] = identity;
        check(&journal, EvidenceStatus::UnrecognizedRecord);
    }
    for control in [json!({}), json!(false)] {
        let mut journal = positive("copy");
        journal["control"] = control;
        check(&journal, EvidenceStatus::UnrecognizedRecord);
    }
    let mut journal = initial("copy");
    journal["control"] = ready_control();
    check(&journal, EvidenceStatus::UnrecognizedRecord);
}

#[test]
fn stable_and_legacy_directory_identity_shapes_preserve_only_historical_classification() {
    for kind in ["copy", "import", "delete", "trashRestore", "trashPurge"] {
        for identity in [
            json!([1, 200]),
            json!({"device":1,"inode":200}),
            json!({"device":1,"inode":200,"volumeUuid":"01234567-89ab-4cde-8fab-0123456789ab"}),
        ] {
            let mut journal = positive(kind);
            journal["directoryIdentity"] = identity;
            check(&journal, EvidenceStatus::HistoricalResolved);
            journal["resolved"] = json!(false);
            check(&journal, EvidenceStatus::UnresolvedOperation);
        }
    }
}

#[test]
fn wrong_types_or_terminal_results_missing_required_fields_are_unrecognized() {
    for (field, value) in [
        ("committed", json!("true")),
        ("resolved", Value::Null),
        ("targetEpoch", json!(0)),
        ("targetEpoch", json!("not-a-uuid")),
        ("destination", json!([])),
        ("error", json!({})),
        ("result", Value::Null),
    ] {
        let mut journal = positive("export");
        journal[field] = value;
        check(&journal, EvidenceStatus::UnrecognizedRecord);
    }
    for field in ["projectId", "operationOutcome"] {
        let mut journal = positive("copy");
        journal["result"].as_object_mut().unwrap().remove(field);
        check(&journal, EvidenceStatus::UnrecognizedRecord);
    }
    for digest in [Value::Null, json!("short"), json!("g".repeat(64))] {
        let mut journal = positive("export");
        journal["publishedSha256"] = digest;
        check(&journal, EvidenceStatus::UnrecognizedRecord);
    }
    for destination in [Value::Null, json!(""), json!("/synthetic/with\0nul")] {
        let mut journal = initial("export");
        journal["destination"] = destination;
        check(&journal, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn malformed_or_duplicate_json_is_never_reduced_to_its_last_value() {
    for bytes in [b"".as_slice(), b"{".as_slice(), b"\xff".as_slice()] {
        check_bytes(bytes, EvidenceStatus::InvalidRecord);
    }
    let serialized = serde_json::to_string(&positive("copy")).unwrap();
    let duplicate = format!("{{\"\\u0072esolved\":false,{}", &serialized[1..]);
    check_bytes(duplicate.as_bytes(), EvidenceStatus::InvalidRecord);
    for (needle, replacement) in [
        ("\"control\":{", "\"control\":{\"mode\":\"maintenance\","),
        ("\"result\":{", "\"result\":{\"opaque\":{\"k\":1,\"k\":2},"),
    ] {
        let duplicate = serialized.replacen(needle, replacement, 1);
        assert_ne!(duplicate, serialized);
        check_bytes(duplicate.as_bytes(), EvidenceStatus::InvalidRecord);
    }
}

#[test]
fn missing_unavailable_and_size_limits_do_not_certify_any_operation() {
    assert_eq!(
        inspect_record(context(), EvidenceInput::Missing).status,
        EvidenceStatus::NoEvidence
    );
    assert_eq!(
        inspect_record(context(), EvidenceInput::Unavailable).status,
        EvidenceStatus::Unavailable
    );
    let mut bytes = serde_json::to_vec(&positive("copy")).unwrap();
    bytes.resize(MAX_RECORD_BYTES, b' ');
    check_bytes(&bytes, EvidenceStatus::HistoricalResolved);
    bytes.push(b' ');
    check_bytes(&bytes, EvidenceStatus::TooLarge);
}

#[test]
fn historical_control_and_private_diagnostics_are_not_applied_or_echoed() {
    const MARKER: &str = "synthetic-private-lifecycle-text";
    let mut journal = positive("copy");
    let mut control = maintenance_control();
    control["recoveryRequired"] = json!(true);
    control["reason"] = json!(MARKER);
    journal["control"] = control;
    journal["error"] = synthetic_error();
    journal["error"]["details"] = json!({"payload":MARKER});
    journal["result"]["opaque"] = json!(MARKER);
    let bytes = serde_json::to_vec(&journal).unwrap();
    let assessment = inspect_record(context(), EvidenceInput::Bytes(&bytes));
    assert_eq!(assessment.status, EvidenceStatus::HistoricalResolved);
    assert!(!format!("{assessment:?}").contains(MARKER));
}
