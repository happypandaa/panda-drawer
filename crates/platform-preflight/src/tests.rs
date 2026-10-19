//! Synthetic protocol fixtures only: no filesystem, platform managers or runtime.
use super::*;
use serde_json::{json, Value};

const PROJECT: &str = "prj_0123456789abcdef0123456789abcdef";
const TASK: &str = "task_0123456789abcdef0123456789abcdef";
const BACKUP: &str = "backup_0123456789abcdef0123456789abcdef";
const MAINTENANCE: &str = "maintenance_0123456789abcdef0123456789abcdef";

// Match the complete, currently serialized ControlState shape. In particular,
// these records have no schemaVersion and nullable keys are still present.
fn ready_control() -> Value {
    json!({
        "mode": "ready",
        "maintenanceId": null,
        "reason": null,
        "startedAt": null,
        "baselineBackupId": null,
        "protectedBackupIds": [],
        "readyForChanges": false,
        "recoveryRequired": false,
        "baselineInputDigest": null,
        "pendingBuild": false,
        "pendingBuildReason": null,
        "hadArtifact": false,
        "lastError": null,
        "lastTaskId": null
    })
}

fn maintenance_control(baseline_ready: bool) -> Value {
    let mut control = ready_control();
    control["mode"] = json!("maintenance");
    control["maintenanceId"] = json!(MAINTENANCE);
    control["reason"] = json!("合成维护记录，不接触磁盘");
    control["startedAt"] = json!(1_800_000_000_000_u64);
    control["lastTaskId"] = json!(TASK);
    if baseline_ready {
        control["readyForChanges"] = json!(true);
        control["baselineBackupId"] = json!(BACKUP);
        control["protectedBackupIds"] = json!([BACKUP]);
        control["baselineInputDigest"] = json!("a".repeat(64));
        control["hadArtifact"] = json!(true);
    }
    control
}

fn restore_journal(committed: bool, resolved: bool) -> Value {
    json!({
        "taskId": TASK,
        "projectId": PROJECT,
        "backupId": BACKUP,
        "originalIdentity": [1, 200],
        "candidateIdentity": [1, 201],
        "committed": committed,
        "resolved": resolved,
        "finalControl": if committed { maintenance_control(true) } else { Value::Null },
        "previousControl": maintenance_control(true),
        "previousProvenance": null
    })
}

fn assess(kind: EvidenceKind, owner_id: &str, input: EvidenceInput<'_>) -> Assessment {
    inspect_record(
        EvidenceContext {
            kind,
            owner_id,
            expected_environment: None,
        },
        input,
    )
}

fn check_bytes(kind: EvidenceKind, owner_id: &str, bytes: &[u8], expected: EvidenceStatus) {
    assert_eq!(
        assess(kind, owner_id, EvidenceInput::Bytes(bytes)),
        Assessment {
            kind,
            status: expected
        }
    );
}

fn check_control(control: &Value, expected: EvidenceStatus) {
    check_bytes(
        EvidenceKind::MaintenanceControl,
        PROJECT,
        &serde_json::to_vec(control).unwrap(),
        expected,
    );
}

fn check_restore(record: &Value, expected: EvidenceStatus) {
    check_bytes(
        EvidenceKind::RestoreJournal,
        TASK,
        &serde_json::to_vec(record).unwrap(),
        expected,
    );
}

#[test]
fn absent_and_unavailable_evidence_never_become_a_ready_control() {
    for kind in [
        EvidenceKind::MaintenanceControl,
        EvidenceKind::RestoreJournal,
    ] {
        let owner = if kind == EvidenceKind::MaintenanceControl {
            PROJECT
        } else {
            TASK
        };
        assert_eq!(
            assess(kind, owner, EvidenceInput::Missing),
            Assessment {
                kind,
                status: EvidenceStatus::NoEvidence
            }
        );
        assert_eq!(
            assess(kind, owner, EvidenceInput::Unavailable),
            Assessment {
                kind,
                status: EvidenceStatus::Unavailable
            }
        );
    }
}

#[test]
fn unsupported_evidence_kinds_are_explicit_even_when_given_a_control_record() {
    let bytes = serde_json::to_vec(&ready_control()).unwrap();
    let kind = EvidenceKind::CleanupEvidence;
    check_bytes(kind, TASK, &bytes, EvidenceStatus::UnsupportedKind);
    check_bytes(kind, TASK, &[0xff], EvidenceStatus::UnsupportedKind);
}

#[test]
fn complete_ready_record_is_only_inactive_control_evidence() {
    // This status describes one record; it does not grant runtime admission or
    // certify the workspace, backup files, identities or other journals.
    check_control(&ready_control(), EvidenceStatus::InactiveControl);
}

#[test]
fn ending_maintenance_can_leave_historical_reason_time_and_build_metadata() {
    let mut ended = ready_control();
    ended["reason"] = json!("The maintenance session has ended");
    ended["startedAt"] = json!(1_800_000_000_000_u64);
    ended["baselineInputDigest"] = json!("b".repeat(64));
    ended["pendingBuild"] = json!(true);
    ended["pendingBuildReason"] = json!("No published webpage is available yet");
    ended["hadArtifact"] = json!(true);
    ended["lastTaskId"] = json!(TASK);
    check_control(&ended, EvidenceStatus::InactiveControl);
}

#[test]
fn maintenance_stays_active_before_and_after_a_baseline_is_declared() {
    check_control(
        &maintenance_control(false),
        EvidenceStatus::MaintenanceActive,
    );
    check_control(
        &maintenance_control(true),
        EvidenceStatus::MaintenanceActive,
    );
    // A record's baseline ID is not proof that the referenced ZIP exists.
}

#[test]
fn recovery_required_takes_priority_over_a_ready_maintenance_baseline() {
    let mut control = maintenance_control(true);
    control["recoveryRequired"] = json!(true);
    control["readyForChanges"] = json!(false);
    control["lastError"] = json!({
        "code": "RECOVERY_REQUIRED",
        "message": "Synthetic retained operation needs attention",
        "details": {"operationOutcome": "needs_attention", "taskId": TASK},
        "retryable": false
    });
    check_control(&control, EvidenceStatus::RecoveryRequired);
}

#[test]
fn empty_objects_and_missing_current_control_keys_are_not_defaulted() {
    check_control(&json!({}), EvidenceStatus::UnrecognizedRecord);
    let complete = ready_control();
    for key in complete.as_object().unwrap().keys() {
        let mut record = complete.clone();
        record.as_object_mut().unwrap().remove(key);
        let bytes = serde_json::to_vec(&record).unwrap();
        let assessment = assess(
            EvidenceKind::MaintenanceControl,
            PROJECT,
            EvidenceInput::Bytes(&bytes),
        );
        assert_eq!(
            assessment.status,
            EvidenceStatus::UnrecognizedRecord,
            "missing {key}"
        );
    }
}

#[test]
fn future_control_fields_and_versions_are_preserved_as_unrecognized() {
    for (key, value) in [
        ("futurePermission", json!(true)),
        ("schemaVersion", json!(1)),
        ("schemaVersion", json!(999)),
    ] {
        let mut record = ready_control();
        record[key] = value;
        check_control(&record, EvidenceStatus::UnrecognizedRecord);
    }
    let mut unknown_mode = ready_control();
    unknown_mode["mode"] = json!("future-ready-mode");
    check_control(&unknown_mode, EvidenceStatus::UnrecognizedRecord);
}

#[test]
fn wrong_control_field_types_do_not_coerce_to_inactive() {
    for (key, value) in [
        ("mode", json!(false)),
        ("maintenanceId", json!([])),
        ("reason", json!(42)),
        ("startedAt", json!(-1)),
        ("baselineBackupId", json!(true)),
        ("protectedBackupIds", json!([42])),
        ("readyForChanges", json!(0)),
        ("recoveryRequired", json!("false")),
        ("baselineInputDigest", json!({})),
        ("pendingBuild", json!(null)),
        ("pendingBuildReason", json!(false)),
        ("hadArtifact", json!("false")),
        ("lastError", json!("none")),
        ("lastTaskId", json!(0)),
    ] {
        let mut record = ready_control();
        record[key] = value;
        check_control(&record, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn malformed_error_shape_is_not_accepted_as_current_control() {
    let mut record = ready_control();
    record["lastError"] = json!({"code": "X", "message": "missing fields"});
    check_control(&record, EvidenceStatus::UnrecognizedRecord);
}

#[test]
fn a_ready_label_does_not_hide_live_maintenance_fields() {
    for (key, value) in [
        ("maintenanceId", json!(MAINTENANCE)),
        ("readyForChanges", json!(true)),
        ("baselineBackupId", json!(BACKUP)),
        ("protectedBackupIds", json!([BACKUP])),
    ] {
        let mut record = ready_control();
        record[key] = value;
        check_control(&record, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn malformed_or_non_object_json_never_becomes_missing_evidence() {
    for bytes in [
        b"".as_slice(),
        b" ".as_slice(),
        b"{".as_slice(),
        b"{\"mode\":\"ready\"".as_slice(),
        b"{\"mode\":\"\xff\"}".as_slice(),
        b"\xff\xfe".as_slice(),
    ] {
        check_bytes(
            EvidenceKind::MaintenanceControl,
            PROJECT,
            bytes,
            EvidenceStatus::InvalidRecord,
        );
    }
    for value in [json!(null), json!([]), json!(false), json!("ready")] {
        check_control(&value, EvidenceStatus::UnrecognizedRecord);
    }
    let trailing = format!("{} {{}}", ready_control());
    check_bytes(
        EvidenceKind::MaintenanceControl,
        PROJECT,
        trailing.as_bytes(),
        EvidenceStatus::InvalidRecord,
    );
}

#[test]
fn duplicate_mode_is_rejected_in_both_last_value_wins_directions() {
    for (first_mode, control) in [
        ("maintenance", ready_control()),
        ("ready", maintenance_control(false)),
    ] {
        let complete = serde_json::to_string(&control).unwrap();
        let duplicate = format!("{{\"mode\":\"{first_mode}\",{}", &complete[1..]);
        check_bytes(
            EvidenceKind::MaintenanceControl,
            PROJECT,
            duplicate.as_bytes(),
            EvidenceStatus::InvalidRecord,
        );
    }
}

#[test]
fn escaped_json_keys_cannot_bypass_duplicate_mode_detection() {
    let serialized = serde_json::to_string(&ready_control()).unwrap();
    let duplicate = format!("{{\"\\u006dode\":\"maintenance\",{}", &serialized[1..]);
    check_bytes(
        EvidenceKind::MaintenanceControl,
        PROJECT,
        duplicate.as_bytes(),
        EvidenceStatus::InvalidRecord,
    );
}

#[test]
fn the_size_limit_includes_valid_but_oversized_json_without_rejecting_its_boundary() {
    let mut bytes = serde_json::to_vec(&ready_control()).unwrap();
    bytes.resize(MAX_RECORD_BYTES, b' ');
    check_bytes(
        EvidenceKind::MaintenanceControl,
        PROJECT,
        &bytes,
        EvidenceStatus::InactiveControl,
    );
    bytes.push(b' ');
    check_bytes(
        EvidenceKind::MaintenanceControl,
        PROJECT,
        &bytes,
        EvidenceStatus::TooLarge,
    );
    check_bytes(
        EvidenceKind::RestoreJournal,
        TASK,
        &bytes,
        EvidenceStatus::TooLarge,
    );
}

#[test]
fn invalid_owner_syntax_is_distinct_from_record_content() {
    let bytes = serde_json::to_vec(&ready_control()).unwrap();
    for owner in [
        "",
        "../project",
        "prj/elsewhere",
        "UPPERCASE",
        "prj with spaces",
    ] {
        check_bytes(
            EvidenceKind::MaintenanceControl,
            owner,
            &bytes,
            EvidenceStatus::IdentityMismatch,
        );
    }
}

#[test]
fn both_precommit_and_committed_unresolved_restores_need_attention() {
    check_restore(
        &restore_journal(false, false),
        EvidenceStatus::UnresolvedOperation,
    );
    check_restore(
        &restore_journal(true, false),
        EvidenceStatus::UnresolvedOperation,
    );
}

#[test]
fn resolved_restoration_and_resolved_rollback_are_history_not_current_health() {
    check_restore(
        &restore_journal(true, true),
        EvidenceStatus::HistoricalResolved,
    );
    // The production rollback path does not invent a finalControl snapshot.
    let rolled_back = restore_journal(false, true);
    assert!(rolled_back["finalControl"].is_null());
    check_restore(&rolled_back, EvidenceStatus::HistoricalResolved);
}

#[test]
fn a_journal_cannot_claim_another_operation_directory() {
    let bytes = serde_json::to_vec(&restore_journal(true, true)).unwrap();
    check_bytes(
        EvidenceKind::RestoreJournal,
        "task_another_operation",
        &bytes,
        EvidenceStatus::IdentityMismatch,
    );
}

#[test]
fn incomplete_restore_records_and_future_versions_are_not_resolved_history() {
    let complete = restore_journal(true, true);
    for key in complete.as_object().unwrap().keys() {
        let mut record = complete.clone();
        record.as_object_mut().unwrap().remove(key);
        check_restore(&record, EvidenceStatus::UnrecognizedRecord);
    }
    for key in ["schemaVersion", "futureRestorationPhase"] {
        let mut record = complete.clone();
        record[key] = json!(1);
        check_restore(&record, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn a_committed_journal_without_final_control_is_not_recoverable_history() {
    for resolved in [false, true] {
        let mut record = restore_journal(true, resolved);
        record["finalControl"] = Value::Null;
        check_restore(&record, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn an_uncommitted_journal_cannot_also_claim_a_published_final_control() {
    for resolved in [false, true] {
        let mut record = restore_journal(false, resolved);
        record["finalControl"] = maintenance_control(true);
        check_restore(&record, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn committed_final_control_must_describe_the_actual_restored_maintenance_baseline() {
    for (key, value) in [
        ("mode", json!("ready")),
        ("recoveryRequired", json!(true)),
        ("readyForChanges", json!(false)),
        ("baselineBackupId", Value::Null),
        (
            "baselineBackupId",
            json!("backup_ffffffffffffffffffffffffffffffff"),
        ),
        ("protectedBackupIds", json!([])),
    ] {
        for resolved in [false, true] {
            let mut record = restore_journal(true, resolved);
            record["finalControl"][key] = value.clone();
            check_restore(&record, EvidenceStatus::UnrecognizedRecord);
        }
    }
}

#[test]
fn a_historical_previous_recovery_flag_does_not_describe_current_health() {
    for committed in [false, true] {
        let mut record = restore_journal(committed, true);
        record["previousControl"]["recoveryRequired"] = json!(true);
        record["previousControl"]["readyForChanges"] = json!(false);
        check_restore(&record, EvidenceStatus::HistoricalResolved);
    }
}

#[test]
fn restore_ids_cannot_contain_paths_or_claim_a_different_owner() {
    for (key, value) in [
        ("taskId", "task_other"),
        ("taskId", "../task"),
        ("projectId", "../project"),
        ("projectId", "prj/escape"),
        ("projectId", "UPPERCASE"),
    ] {
        let mut record = restore_journal(true, true);
        record[key] = json!(value);
        check_restore(&record, EvidenceStatus::IdentityMismatch);
    }
}

#[test]
fn backup_ids_keep_the_existing_exact_hex_format_including_uppercase_digits() {
    let uppercase_backup = "backup_ABCDEF0123456789ABCDEF0123456789";
    let mut record = restore_journal(true, true);
    record["backupId"] = json!(uppercase_backup);
    record["finalControl"]["baselineBackupId"] = json!(uppercase_backup);
    record["finalControl"]["protectedBackupIds"] = json!([uppercase_backup]);
    check_restore(&record, EvidenceStatus::HistoricalResolved);

    for backup in [
        "backup_0123456789abcdef0123456789abcde",
        "backup_0123456789abcdef0123456789abcdef0",
        "backup_0123456789abcdef0123456789abcdeg",
        "backup_0123456789abcdef0123456789abcdeé",
        "../backup_0123456789abcdef0123456789abcdef",
        "backup_0123456789abcdef/123456789abcdef",
    ] {
        let mut record = restore_journal(false, false);
        record["backupId"] = json!(backup);
        check_restore(&record, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn nested_controls_are_validated_before_any_journal_completion_label() {
    for committed in [false, true] {
        for resolved in [false, true] {
            let mut record = restore_journal(committed, resolved);
            record["previousControl"] = json!({});
            check_restore(&record, EvidenceStatus::UnrecognizedRecord);
        }
    }
    for field in ["previousControl", "finalControl"] {
        let mut record = restore_journal(true, true);
        record[field]["futureField"] = json!(true);
        check_restore(&record, EvidenceStatus::UnrecognizedRecord);
        let mut record = restore_journal(true, true);
        record[field]["mode"] = json!("future-mode");
        check_restore(&record, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn duplicate_nested_control_fields_are_not_erased_by_a_value_round_trip() {
    let serialized = serde_json::to_string(&restore_journal(true, true)).unwrap();
    for field in ["previousControl", "finalControl"] {
        let needle = format!("\"{field}\":{{");
        let replacement = format!("\"{field}\":{{\"mode\":\"ready\",");
        let duplicate = serialized.replacen(&needle, &replacement, 1);
        assert_ne!(duplicate, serialized);
        check_bytes(
            EvidenceKind::RestoreJournal,
            TASK,
            duplicate.as_bytes(),
            EvidenceStatus::InvalidRecord,
        );
    }
}

#[test]
fn duplicate_fields_in_opaque_previous_provenance_still_make_json_ambiguous() {
    let serialized = serde_json::to_string(&restore_journal(true, true)).unwrap();
    for provenance in [
        r#"{"buildId":"first","buildId":"second"}"#,
        r#"{"history":[{"label":1,"label":2}]}"#,
    ] {
        let duplicate = serialized.replacen(
            "\"previousProvenance\":null",
            &format!("\"previousProvenance\":{provenance}"),
            1,
        );
        assert_ne!(duplicate, serialized);
        check_bytes(
            EvidenceKind::RestoreJournal,
            TASK,
            duplicate.as_bytes(),
            EvidenceStatus::InvalidRecord,
        );
    }
}

#[test]
fn restore_boolean_identity_and_control_types_are_not_coerced() {
    for (key, value) in [
        ("committed", json!("true")),
        ("resolved", json!(1)),
        ("originalIdentity", json!([1])),
        ("originalIdentity", json!([1, 2, 3])),
        ("candidateIdentity", json!([1, -1])),
        ("candidateIdentity", json!(["1", "2"])),
        ("previousControl", Value::Null),
        ("finalControl", json!([])),
    ] {
        let mut record = restore_journal(true, true);
        record[key] = value;
        check_restore(&record, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn valid_unrelated_provenance_is_opaque_historical_data() {
    let mut record = restore_journal(true, true);
    record["previousProvenance"] = json!({
        "schemaVersion": 7,
        "buildId": "synthetic-old-build",
        "extraHistoricalDetails": {"source": "not-current-runtime-state"}
    });
    check_restore(&record, EvidenceStatus::HistoricalResolved);
}

#[test]
fn assessment_does_not_echo_control_reasons_or_error_payloads() {
    const PRIVATE_MARKER: &str = "synthetic-private-content-not-for-diagnostics";
    let mut control = maintenance_control(false);
    control["reason"] = json!(PRIVATE_MARKER);
    control["lastError"] = json!({
        "code": "SYNTHETIC_ERROR",
        "message": PRIVATE_MARKER,
        "details": {"syntheticPayload": PRIVATE_MARKER},
        "retryable": false
    });
    let bytes = serde_json::to_vec(&control).unwrap();
    let assessment = assess(
        EvidenceKind::MaintenanceControl,
        PROJECT,
        EvidenceInput::Bytes(&bytes),
    );
    assert_eq!(assessment.status, EvidenceStatus::MaintenanceActive);
    assert!(!format!("{assessment:?}").contains(PRIVATE_MARKER));
}

#[test]
fn restore_identity_tuple_legacy_object_and_stable_object_formats_can_coexist() {
    let identities = [
        json!([1, 200]),
        json!({"device":1,"inode":200}),
        json!({"device":1,"inode":200,"volumeUuid":"6b4045a5-f1cb-472d-9581-70d079b17a6f"}),
        json!({"device":1,"inode":200,"volumeUuid":"6B4045A5-F1CB-472D-9581-70D079B17A6F"}),
    ];
    for original in &identities {
        for candidate in &identities {
            for committed in [false, true] {
                for resolved in [false, true] {
                    let mut record = restore_journal(committed, resolved);
                    record["originalIdentity"] = original.clone();
                    record["candidateIdentity"] = candidate.clone();
                    check_restore(
                        &record,
                        if resolved {
                            EvidenceStatus::HistoricalResolved
                        } else {
                            EvidenceStatus::UnresolvedOperation
                        },
                    );
                }
            }
        }
    }
}

#[test]
fn malformed_or_future_identity_objects_are_not_recognized_as_history() {
    for malformed in [
        json!({}),
        json!({"device":1}),
        json!({"inode":200}),
        json!({"device":"1","inode":200}),
        json!({"device":1,"inode":-1}),
        json!({"device":1.0,"inode":200}),
        json!({"device":1,"inode":200,"futureIdentity":true}),
        json!({"device":1,"inode":200,"volumeUuid":null}),
        json!({"device":1,"inode":200,"volumeUuid":true}),
        json!({"device":1,"inode":200,"volume_uuid":"6b4045a5-f1cb-472d-9581-70d079b17a6f"}),
        json!({"device":1,"inode":200,"volumeUuid":"6b4045a5-f1cb-472d-9581-70d079b17a6f","futureIdentity":true}),
    ] {
        for field in ["originalIdentity", "candidateIdentity"] {
            let mut record = restore_journal(true, true);
            record[field] = malformed.clone();
            check_restore(&record, EvidenceStatus::UnrecognizedRecord);
        }
    }
}

#[test]
fn stable_identity_requires_an_exact_non_nil_hyphenated_uuid() {
    for uuid in [
        "",
        "invalid",
        "00000000-0000-0000-0000-000000000000",
        "6b4045a5f1cb472d958170d079b17a6f",
        "6b4045a5-f1cb-472d-9581-70d079b17a6",
        "6b4045a5-f1cb-472d-9581-70d079b17a6ff",
        "6b4045a5-f1cb-472d-9581-70d079b17a6g",
        "6b4045a5_f1cb-472d-9581-70d079b17a6f",
        "6b4045a5-f1cb-472d-9581-70d079b17aé",
        "{6b4045a5-f1cb-472d-9581-70d079b17a6f}",
        "urn:uuid:6b4045a5-f1cb-472d-9581-70d079b17a6f",
    ] {
        let mut record = restore_journal(true, true);
        record["originalIdentity"] = json!({"device":1,"inode":200,"volumeUuid":uuid});
        check_restore(&record, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn duplicate_identity_keys_are_invalid_even_when_escaped_or_nested() {
    let mut record = restore_journal(true, true);
    record["originalIdentity"] =
        json!({"device":1,"inode":200,"volumeUuid":"6b4045a5-f1cb-472d-9581-70d079b17a6f"});
    record["candidateIdentity"] = record["originalIdentity"].clone();
    let serialized = serde_json::to_string(&record).unwrap();
    for (field, key, value) in [
        ("originalIdentity", "device", "9"),
        (
            "candidateIdentity",
            "\\u0076olumeUuid",
            "\"0875f92b-f48e-46d1-b569-e0bd2a076c5c\"",
        ),
    ] {
        let duplicate = serialized.replacen(
            &format!("\"{field}\":{{"),
            &format!("\"{field}\":{{\"{key}\":{value},"),
            1,
        );
        assert_ne!(duplicate, serialized);
        check_bytes(
            EvidenceKind::RestoreJournal,
            TASK,
            duplicate.as_bytes(),
            EvidenceStatus::InvalidRecord,
        );
    }
}

#[test]
fn well_formed_stable_identity_classifies_history_without_verifying_any_volume() {
    // These synthetic numbers and UUID are deliberately not captured from the
    // host. This pure classifier cannot determine whether they name anything.
    for resolved in [false, true] {
        let mut record = restore_journal(true, resolved);
        record["originalIdentity"] = json!({"device":u64::MAX,"inode":200,
            "volumeUuid":"0875f92b-f48e-46d1-b569-e0bd2a076c5c"});
        record["candidateIdentity"] = json!({"device":u64::MAX,"inode":201,
            "volumeUuid":"6b4045a5-f1cb-472d-9581-70d079b17a6f"});
        check_restore(
            &record,
            if resolved {
                EvidenceStatus::HistoricalResolved
            } else {
                EvidenceStatus::UnresolvedOperation
            },
        );
    }
}
