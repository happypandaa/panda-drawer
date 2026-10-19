//! Synthetic migration journal bytes only. Path strings never refer to files
//! read by these tests, and no assessment selects a workspace or admits runtime.
use super::*;
use serde_json::{json, Value};

const IDENTIFIER: &str = "dev.localprojects.platform.m0";
const ENVIRONMENT: &str = "development";
const TASK: &str = "task_0123456789abcdef0123456789abcdef";
const PROJECT: &str = "prj_0123456789abcdef0123456789abcdef";
const EPOCH: &str = "01234567-89ab-4cde-8fab-0123456789ab";

fn context() -> EvidenceContext<'static> {
    EvidenceContext {
        kind: EvidenceKind::MigrationJournal,
        owner_id: TASK,
        expected_environment: Some(EnvironmentIdentity {
            identifier: IDENTIFIER,
            environment: ENVIRONMENT,
        }),
    }
}

// The current writer serializes every one of these 21 keys, including nullable
// values and the three fields its legacy reader can default when absent.
fn queued() -> Value {
    json!({
        "schemaVersion": 1,
        "identifier": IDENTIFIER,
        "environment": ENVIRONMENT,
        "taskId": TASK,
        "sourcePath": "/synthetic/source-workspace",
        "destinationPath": "/synthetic/destination-workspace",
        "sourceIdentity": {"device": 1, "inode": 100},
        "targetEmptyIdentity": {"device": 1, "inode": 200},
        "destinationIdentity": null,
        "stagePath": "/synthetic/.local-projects-stage.partial",
        "phase": "queued",
        "published": false,
        "committed": false,
        "resolved": false,
        "epochs": {},
        "epochsApplied": false,
        "blockedProjects": {},
        "fileCount": 0,
        "totalBytes": 0,
        "startedAt": 1_800_000_000_000_u64,
        "retainedSourceWarning": null
    })
}

fn copying() -> Value {
    let mut record = queued();
    record["phase"] = json!("copying");
    record["epochs"] = json!({PROJECT: EPOCH});
    record["fileCount"] = json!(12);
    record["totalBytes"] = json!(1024);
    record
}

fn publishing() -> Value {
    let mut record = copying();
    record["phase"] = json!("publishing");
    record["destinationIdentity"] = json!({"device": 1, "inode": 201});
    record
}

fn committing() -> Value {
    let mut record = publishing();
    record["phase"] = json!("committing");
    record["published"] = json!(true);
    record
}

fn completed() -> Value {
    let mut record = committing();
    record["committed"] = json!(true);
    record["epochsApplied"] = json!(true);
    record["resolved"] = json!(true);
    record["phase"] = json!("completed");
    record
}

fn project_error() -> Value {
    json!({
        "code": "DATABASE_MISSING",
        "message": "Synthetic project retains its diagnostic state",
        "details": {"operationOutcome": "not_started"},
        "retryable": false
    })
}

fn check_input(input: EvidenceInput<'_>, expected: EvidenceStatus) {
    assert_eq!(
        inspect_record(context(), input),
        Assessment {
            kind: EvidenceKind::MigrationJournal,
            status: expected,
        }
    );
}

fn check(record: &Value, expected: EvidenceStatus) {
    check_input(
        EvidenceInput::Bytes(&serde_json::to_vec(record).unwrap()),
        expected,
    );
}

#[test]
fn migration_needs_trusted_environment_context_even_before_observing_input() {
    let bytes = serde_json::to_vec(&completed()).unwrap();
    let oversized = vec![b' '; MAX_RECORD_BYTES + 1];
    let mut missing_context = context();
    missing_context.expected_environment = None;
    for input in [
        EvidenceInput::Missing,
        EvidenceInput::Unavailable,
        EvidenceInput::Bytes(&bytes),
        EvidenceInput::Bytes(&[0xff]),
        EvidenceInput::Bytes(&oversized),
    ] {
        assert_eq!(
            inspect_record(missing_context, input).status,
            EvidenceStatus::IdentityMismatch
        );
    }
}

#[test]
fn absent_or_unavailable_migration_evidence_is_not_a_completed_migration() {
    check_input(EvidenceInput::Missing, EvidenceStatus::NoEvidence);
    check_input(EvidenceInput::Unavailable, EvidenceStatus::Unavailable);
}

#[test]
fn real_intermediate_journal_states_remain_unresolved() {
    for record in [queued(), copying(), publishing(), committing()] {
        check(&record, EvidenceStatus::UnresolvedOperation);
    }
    let mut committed = committing();
    committed["committed"] = json!(true);
    check(&committed, EvidenceStatus::UnresolvedOperation);
    committed["epochsApplied"] = json!(true);
    check(&committed, EvidenceStatus::UnresolvedOperation);
}

#[test]
fn an_allocated_destination_identity_can_precede_actual_publication() {
    let record = publishing();
    assert!(!record["destinationIdentity"].is_null());
    assert_eq!(record["published"], false);
    check(&record, EvidenceStatus::UnresolvedOperation);
}

#[test]
fn a_false_journal_commit_flag_does_not_prove_settings_still_select_the_source() {
    // Both the before-settings-rename state and a crash after that rename can
    // have these same bytes. This classifier never consults settings or chooses
    // either recorded location, so both remain unresolved evidence.
    check(&committing(), EvidenceStatus::UnresolvedOperation);
}

#[test]
fn completed_migration_bytes_are_historical_not_current_workspace_health() {
    check(&completed(), EvidenceStatus::HistoricalResolved);
    let mut old_source_unavailable = completed();
    old_source_unavailable["retainedSourceWarning"] =
        json!("Retained source is unavailable or its identity changed; it was not modified");
    check(&old_source_unavailable, EvidenceStatus::HistoricalResolved);
}

#[test]
fn completed_migration_can_preserve_blocked_project_diagnostics() {
    let mut record = completed();
    record["blockedProjects"] = json!({PROJECT: project_error()});
    check(&record, EvidenceStatus::HistoricalResolved);
}

#[test]
fn mac_and_windows_paths_are_opaque_protocol_strings_in_either_host_build() {
    for (source, destination, stage) in [
        (
            "/synthetic/mac/source",
            "/synthetic/mac/destination",
            "/synthetic/mac/.stage",
        ),
        (
            r"D:\synthetic\source",
            r"D:\synthetic\destination",
            r"D:\synthetic\.stage",
        ),
    ] {
        let mut record = completed();
        record["sourcePath"] = json!(source);
        record["destinationPath"] = json!(destination);
        record["stagePath"] = json!(stage);
        check(&record, EvidenceStatus::HistoricalResolved);
    }
}

#[test]
fn any_missing_current_migration_field_is_unrecognized_including_legacy_defaults() {
    let complete = completed();
    assert_eq!(complete.as_object().unwrap().len(), 21);
    for field in complete.as_object().unwrap().keys() {
        let mut record = complete.clone();
        record.as_object_mut().unwrap().remove(field);
        let bytes = serde_json::to_vec(&record).unwrap();
        assert_eq!(
            inspect_record(context(), EvidenceInput::Bytes(&bytes)).status,
            EvidenceStatus::UnrecognizedRecord,
            "missing {field}"
        );
    }
    let mut missing_old_defaults = complete;
    for field in ["blockedProjects", "startedAt", "retainedSourceWarning"] {
        missing_old_defaults.as_object_mut().unwrap().remove(field);
    }
    check(&missing_old_defaults, EvidenceStatus::UnrecognizedRecord);
}

#[test]
fn migration_versions_and_future_fields_are_not_silently_accepted() {
    for version in [json!(0), json!(2), json!("1"), Value::Null] {
        let mut record = completed();
        record["schemaVersion"] = version;
        check(&record, EvidenceStatus::UnrecognizedRecord);
    }
    let mut future_record = completed();
    future_record["futureSelectionPolicy"] = json!("other");
    check(&future_record, EvidenceStatus::UnrecognizedRecord);
}

#[test]
fn migration_identity_must_match_the_caller_trusted_environment_and_owner() {
    for (field, value) in [
        ("identifier", "dev.synthetic.other-platform"),
        ("environment", "production"),
        ("taskId", "task_another_migration"),
        ("taskId", "../migration"),
    ] {
        let mut record = completed();
        record[field] = json!(value);
        check(&record, EvidenceStatus::IdentityMismatch);
    }
    let bytes = serde_json::to_vec(&completed()).unwrap();
    for owner in ["", "../task", "task/escape", "UPPERCASE"] {
        let mut wrong_owner = context();
        wrong_owner.owner_id = owner;
        assert_eq!(
            inspect_record(wrong_owner, EvidenceInput::Bytes(&bytes)).status,
            EvidenceStatus::IdentityMismatch
        );
    }
}

#[test]
fn legacy_directory_identities_are_objects_not_restore_tuples_or_future_windows_ids() {
    for field in [
        "sourceIdentity",
        "targetEmptyIdentity",
        "destinationIdentity",
    ] {
        for bad_identity in [
            json!([1, 201]),
            json!({}),
            json!({"device": 1}),
            json!({"device": 1, "inode": -1}),
            json!({"device": "1", "inode": 201}),
            json!({"device": 1, "inode": 201, "futureField": 0}),
            json!({"volumeSerial": 1, "fileId": "synthetic"}),
        ] {
            let mut record = completed();
            record[field] = bad_identity;
            check(&record, EvidenceStatus::UnrecognizedRecord);
        }
    }
}

#[test]
fn journal_phases_do_not_inherit_unrelated_task_progress_labels() {
    for phase in [
        "inventory",
        "verifying",
        "reopening",
        "failed",
        "interrupted",
        "future-phase",
    ] {
        let mut record = completed();
        record["phase"] = json!(phase);
        check(&record, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn completed_phase_requires_all_persisted_completion_flags() {
    for field in ["published", "committed", "resolved", "epochsApplied"] {
        let mut record = completed();
        record[field] = json!(false);
        check(&record, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn precompletion_phases_cannot_claim_unwritten_completion_states() {
    let mut resolved_without_completed_phase = committing();
    resolved_without_completed_phase["committed"] = json!(true);
    resolved_without_completed_phase["epochsApplied"] = json!(true);
    resolved_without_completed_phase["resolved"] = json!(true);
    check(
        &resolved_without_completed_phase,
        EvidenceStatus::UnrecognizedRecord,
    );
    let mut applied_without_commit = committing();
    applied_without_commit["epochsApplied"] = json!(true);
    check(&applied_without_commit, EvidenceStatus::UnrecognizedRecord);
    let mut published_without_phase_change = publishing();
    published_without_phase_change["published"] = json!(true);
    check(
        &published_without_phase_change,
        EvidenceStatus::UnrecognizedRecord,
    );
}

#[test]
fn destination_identity_presence_tracks_the_actual_saved_publication_phases() {
    for mut record in [queued(), copying()] {
        record["destinationIdentity"] = json!({"device": 1, "inode": 201});
        check(&record, EvidenceStatus::UnrecognizedRecord);
    }
    for mut record in [publishing(), committing(), completed()] {
        record["destinationIdentity"] = Value::Null;
        check(&record, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn wrong_migration_types_do_not_become_resolved_history() {
    for (field, value) in [
        ("identifier", json!(false)),
        ("environment", Value::Null),
        ("taskId", json!(0)),
        ("sourcePath", json!([])),
        ("destinationPath", Value::Null),
        ("stagePath", json!({})),
        ("phase", json!(true)),
        ("published", json!("true")),
        ("committed", json!(1)),
        ("resolved", Value::Null),
        ("epochsApplied", json!("true")),
        ("fileCount", json!(-1)),
        ("totalBytes", json!("1024")),
        ("startedAt", json!(false)),
        ("retainedSourceWarning", json!({})),
    ] {
        let mut record = completed();
        record[field] = value;
        check(&record, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn epoch_entries_need_project_ids_and_uuid_values_instead_of_untyped_metadata() {
    for epochs in [
        Value::Null,
        json!([]),
        json!({PROJECT: 1}),
        json!({PROJECT: "not-a-document-epoch"}),
        json!({PROJECT: "0123456789ab4cde8fab0123456789ab"}),
        json!({PROJECT: "01234567-89ab-4cde-8fab-0123456789ag"}),
        json!({"../project": EPOCH}),
        json!({"UPPERCASE": EPOCH}),
    ] {
        let mut record = completed();
        record["epochs"] = epochs;
        check(&record, EvidenceStatus::UnrecognizedRecord);
    }
}

#[test]
fn blocked_projects_are_typed_diagnostics_for_projects_in_the_epoch_inventory() {
    for blocked in [
        Value::Null,
        json!([]),
        json!({PROJECT: "blocked"}),
        json!({PROJECT: {"code": "X", "message": "missing current error fields"}}),
        json!({"../project": project_error()}),
        json!({"prj_not_in_epochs": project_error()}),
    ] {
        let mut record = completed();
        record["blockedProjects"] = blocked;
        check(&record, EvidenceStatus::UnrecognizedRecord);
    }
    let mut malformed_error = project_error();
    malformed_error["retryable"] = json!("false");
    let mut record = completed();
    record["blockedProjects"] = json!({PROJECT: malformed_error});
    check(&record, EvidenceStatus::UnrecognizedRecord);

    let mut future_error = project_error();
    future_error["futurePermission"] = json!(true);
    record["blockedProjects"] = json!({PROJECT: future_error});
    check(&record, EvidenceStatus::UnrecognizedRecord);
}

#[test]
fn full_current_defaults_and_opaque_error_details_do_not_grant_current_health() {
    let mut record = completed();
    record["startedAt"] = json!(0);
    // UUID text shape is recognizable without claiming a particular version,
    // random source, project identity or actual persisted document epoch.
    record["epochs"][PROJECT] = json!("01234567-89ab-0cde-0fab-0123456789ab");
    let mut error = project_error();
    error["details"] = json!({"futureDiagnosticData": [null, {"note": "historical-only"}]});
    record["blockedProjects"] = json!({PROJECT: error});
    check(&record, EvidenceStatus::HistoricalResolved);
}

#[test]
fn migration_json_must_be_valid_and_size_bounded() {
    for bytes in [
        b"".as_slice(),
        b"{".as_slice(),
        b"\xff".as_slice(),
        b"{\"resolved\":true".as_slice(),
    ] {
        check_input(EvidenceInput::Bytes(bytes), EvidenceStatus::InvalidRecord);
    }
    for value in [json!(null), json!([]), json!("completed"), json!({})] {
        check(&value, EvidenceStatus::UnrecognizedRecord);
    }
    let mut bytes = serde_json::to_vec(&completed()).unwrap();
    bytes.resize(MAX_RECORD_BYTES, b' ');
    check_input(
        EvidenceInput::Bytes(&bytes),
        EvidenceStatus::HistoricalResolved,
    );
    bytes.push(b' ');
    check_input(EvidenceInput::Bytes(&bytes), EvidenceStatus::TooLarge);
}

#[test]
fn migration_duplicate_keys_are_rejected_including_escaped_and_nested_keys() {
    let serialized = serde_json::to_string(&completed()).unwrap();
    for prefix in [r#"{"resolved":false,"#, r#"{"\u0072esolved":false,"#] {
        let duplicate = format!("{prefix}{}", &serialized[1..]);
        check_input(
            EvidenceInput::Bytes(duplicate.as_bytes()),
            EvidenceStatus::InvalidRecord,
        );
    }
    let needle = "\"sourceIdentity\":{\"device\":1,";
    let duplicate =
        serialized.replacen(needle, "\"sourceIdentity\":{\"device\":2,\"device\":1,", 1);
    assert_ne!(duplicate, serialized);
    check_input(
        EvidenceInput::Bytes(duplicate.as_bytes()),
        EvidenceStatus::InvalidRecord,
    );
}

#[test]
fn duplicate_epoch_and_blocked_diagnostic_keys_are_never_collapsed() {
    let serialized = serde_json::to_string(&completed()).unwrap();
    let duplicate_epoch = serialized.replacen(
        "\"epochs\":{",
        &format!("\"epochs\":{{\"{PROJECT}\":\"{EPOCH}\","),
        1,
    );
    assert_ne!(duplicate_epoch, serialized);
    check_input(
        EvidenceInput::Bytes(duplicate_epoch.as_bytes()),
        EvidenceStatus::InvalidRecord,
    );

    let mut with_error = completed();
    with_error["blockedProjects"] = json!({PROJECT: project_error()});
    let serialized = serde_json::to_string(&with_error).unwrap();
    let duplicate_error_detail = serialized.replacen(
        "\"details\":{",
        "\"details\":{\"synthetic\":1,\"synthetic\":2,",
        1,
    );
    assert_ne!(duplicate_error_detail, serialized);
    check_input(
        EvidenceInput::Bytes(duplicate_error_detail.as_bytes()),
        EvidenceStatus::InvalidRecord,
    );
}

#[test]
fn migration_assessment_never_echoes_recorded_paths_warnings_or_diagnostics() {
    const PRIVATE_MARKER: &str = "synthetic-private-migration-text";
    let mut record = completed();
    record["sourcePath"] = json!(PRIVATE_MARKER);
    record["retainedSourceWarning"] = json!(PRIVATE_MARKER);
    let mut error = project_error();
    error["message"] = json!(PRIVATE_MARKER);
    record["blockedProjects"] = json!({PROJECT: error});
    let bytes = serde_json::to_vec(&record).unwrap();
    let assessment = inspect_record(context(), EvidenceInput::Bytes(&bytes));
    assert_eq!(assessment.status, EvidenceStatus::HistoricalResolved);
    assert!(!format!("{assessment:?}").contains(PRIVATE_MARKER));
}
