//! Deletion lifecycle tests use only disposable directories and an injected
//! rename sink. They do not prove Finder, physical disk or OS permission UI.
use super::*;
use std::os::unix::fs::symlink;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    data: PathBuf,
    apps: Arc<NodeApps>,
    id: String,
}

impl Fixture {
    async fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("disposable-delete-author-source");
        let data = root.join("disposable-delete-platform-data");
        fs::create_dir(&data).unwrap();
        fs::create_dir_all(source.join("public")).unwrap();
        fs::create_dir_all(source.join("server")).unwrap();
        fs::write(source.join("public/index.html"), "disposable source").unwrap();
        fs::write(
            source.join("server/main.mjs"),
            "throw Error('never run during delete')",
        )
        .unwrap();
        fs::write(
            source.join("local-app.json"),
            serde_json::to_vec(&json!({
                "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
                "app":{"id":"disposable.delete.node","name":"可丢弃删除测试","version":"1"},
                "web":{"root":"public","entry":"index.html","routing":"files"},
                "backend":{"protocolVersion":1,"root":"server","entry":"main.mjs","nodeMajor":24}
            }))
            .unwrap(),
        )
        .unwrap();
        let apps =
            Arc::new(NodeApps::new(data.clone(), "/disposable-never-used-node".into()).unwrap());
        let app = apps.register(source.clone()).await.unwrap();
        let id = app["id"].as_str().unwrap().to_owned();
        for (path, contents) in [
            ("data/notes.sqlite", "artificial database"),
            ("data/attachment.txt", "artificial attachment"),
            (
                "configuration/artificial-credential",
                "artificial private sample",
            ),
            ("cache/sample", "artificial cache"),
            ("runs/sample", "artificial run"),
        ] {
            fs::write(data.join("node-apps").join(&id).join(path), contents).unwrap();
        }
        Self {
            _temporary: temporary,
            root,
            source,
            data,
            apps,
            id,
        }
    }

    fn slot(&self) -> PathBuf {
        self.data.join("node-apps").join(&self.id)
    }

    async fn preview(&self) -> Value {
        self.apps
            .delete_preview_supported(&self.id, Ok(()))
            .await
            .unwrap()
    }

    fn input(preview: &Value) -> DeleteNodeAppInput {
        serde_json::from_value(json!({"previewId":preview["previewId"]})).unwrap()
    }

    async fn move_to_disposable_sink(&self, preview: &Value) -> Result<Value> {
        let destination = self
            .root
            .join(format!("disposable-system-trash-{}", Uuid::new_v4()));
        let apps = self.apps.clone();
        self.apps
            .delete_with(&self.id, Self::input(preview), move |source, expected| {
                assert!(
                    apps.inner.try_lock().is_err(),
                    "slot mutation lock remains held through OS operation"
                );
                if !matches(expected, source) {
                    return Err(deletion_error(
                        "ARTIFICIAL_IDENTITY_CHANGED",
                        "Artificial replacement rejected",
                    ));
                }
                fs::rename(source, &destination)?;
                Ok(destination)
            })
            .await
    }
}

#[tokio::test]
async fn deletes_complete_external_slot_and_preserves_author_sources_and_other_slots() {
    let fixture = Fixture::new().await;
    let sibling = fixture
        .apps
        .register({
            let source = fixture.root.join("disposable-other-author-source");
            fs::create_dir_all(source.join("public")).unwrap();
            fs::create_dir_all(source.join("server")).unwrap();
            for name in ["local-app.json", "public/index.html", "server/main.mjs"] {
                fs::copy(fixture.source.join(name), source.join(name)).unwrap();
            }
            source
        })
        .await
        .unwrap();
    let preview = fixture.preview().await;
    assert_eq!(preview["canDelete"], true);
    assert_eq!(preview["externalSourcePath"], json!(fixture.source));
    assert_eq!(preview["git"]["remoteSync"], "unverified");
    let result = fixture.move_to_disposable_sink(&preview).await.unwrap();
    assert_eq!(result["operationOutcome"], "system_trashed");
    assert_eq!(result["platformRestoreAvailable"], false);
    assert!(!fixture.slot().exists());
    let moved = PathBuf::from(result["systemTrashPath"].as_str().unwrap());
    for path in [
        "registration.json",
        "data/notes.sqlite",
        "data/attachment.txt",
        "configuration/artificial-credential",
        "cache/sample",
        "runs/sample",
    ] {
        assert!(moved.join(path).is_file(), "complete slot keeps {path}");
    }
    assert!(fixture.source.join("local-app.json").is_file());
    let apps = fixture.apps.list().await.unwrap();
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0]["id"], sibling["id"]);
    let receipt: Value =
        serde_json::from_slice(&fs::read(result["receiptPath"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(receipt["phase"], "system_trashed");
    assert!(!serde_json::to_string(&receipt)
        .unwrap()
        .contains("artificial private sample"));
}

#[tokio::test]
async fn missing_external_source_does_not_prevent_slot_deletion() {
    let fixture = Fixture::new().await;
    fs::rename(
        &fixture.source,
        fixture.root.join("retained-disposable-author-source"),
    )
    .unwrap();
    let preview = fixture.preview().await;
    assert_eq!(preview["canDelete"], true);
    fixture.move_to_disposable_sink(&preview).await.unwrap();
    assert!(!fixture.slot().exists());
}

#[tokio::test]
async fn managed_slot_delete_includes_generations_but_keeps_candidates_and_source_records() {
    let fixture = Fixture::new().await;
    let generation = format!("gen_{}", "a".repeat(32));
    let program = fixture.slot().join("generations").join(&generation);
    fs::create_dir_all(&program).unwrap();
    fs::write(
        program.join("disposable-program.mjs"),
        "disposable installed program",
    )
    .unwrap();
    let registration = json!({
        "version":2,"id":fixture.id,"kind":"managed","generation":generation,
        "manifestDigest":"b".repeat(64),"programDigest":"c".repeat(64),
        "provenance":{"sourceId":format!("src_{}", "d".repeat(32)),"sourceDigest":"e".repeat(64),
            "sourceCommit":null,"node":{"path":"/disposable-node","version":"artificial"},
            "npm":{"path":"/disposable-npm","version":"artificial"},"preparedAt":1}
    });
    crate::builds::atomic_json(&fixture.slot().join("registration.json"), &registration).unwrap();
    let candidate = fixture
        .data
        .join("node-apps/.installations/disposable-retained-candidate");
    let source_record = fixture
        .data
        .join("disposable-workspace/.platform/sources/disposable-retained-source");
    fs::create_dir_all(&candidate).unwrap();
    fs::create_dir_all(&source_record).unwrap();
    fs::write(candidate.join("evidence"), "artificial candidate evidence").unwrap();
    fs::write(source_record.join("evidence"), "artificial source evidence").unwrap();
    let preview = fixture.preview().await;
    assert_eq!(preview["canDelete"], true);
    assert!(preview["externalSourcePath"].is_null());
    let result = fixture.move_to_disposable_sink(&preview).await.unwrap();
    let moved = PathBuf::from(result["systemTrashPath"].as_str().unwrap());
    assert!(moved
        .join("generations")
        .join(&generation)
        .join("disposable-program.mjs")
        .is_file());
    assert!(moved.join("data/notes.sqlite").is_file());
    assert!(candidate.join("evidence").is_file());
    assert!(source_record.join("evidence").is_file());
    assert!(fixture.source.join("local-app.json").is_file());
}

#[tokio::test]
async fn active_marker_blocks_preview_and_fresh_commit() {
    let fixture = Fixture::new().await;
    let preview = fixture.preview().await;
    fs::write(
        fixture.slot().join("active.json"),
        "artificial unresolved runtime",
    )
    .unwrap();
    let blocked = fixture.preview().await;
    assert_eq!(blocked["canDelete"], false);
    assert!(!blocked["blockedReasons"].as_array().unwrap().is_empty());
    let error = fixture.move_to_disposable_sink(&blocked).await.unwrap_err();
    assert_eq!(error.code, "NODE_DELETE_BUSY");
    assert!(fixture.slot().join("data/notes.sqlite").is_file());
    let stale = fixture.move_to_disposable_sink(&preview).await.unwrap_err();
    assert_eq!(stale.code, "NODE_DELETE_PREVIEW_CHANGED");
}

#[tokio::test]
async fn registration_changes_and_directory_replacement_invalidate_confirmation() {
    let fixture = Fixture::new().await;
    let preview = fixture.preview().await;
    let path = fixture.slot().join("registration.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["manifestDigest"] = json!("a".repeat(64));
    crate::builds::atomic_json(&path, &record).unwrap();
    assert_eq!(
        fixture
            .move_to_disposable_sink(&preview)
            .await
            .unwrap_err()
            .code,
        "NODE_DELETE_PREVIEW_CHANGED"
    );
    let preview = fixture.preview().await;
    let retained = fixture.root.join("retained-disposable-original-slot");
    fs::rename(fixture.slot(), &retained).unwrap();
    fs::create_dir(fixture.slot()).unwrap();
    fs::copy(
        retained.join("registration.json"),
        fixture.slot().join("registration.json"),
    )
    .unwrap();
    assert_eq!(
        fixture
            .move_to_disposable_sink(&preview)
            .await
            .unwrap_err()
            .code,
        "NODE_DELETE_PREVIEW_CHANGED"
    );
    assert!(retained.join("data/notes.sqlite").is_file());
    assert!(fixture.slot().exists());
}

#[tokio::test]
async fn forged_or_expired_preview_never_calls_delete_sink() {
    let fixture = Fixture::new().await;
    let preview = fixture.preview().await;
    fixture
        .apps
        .inner
        .lock()
        .await
        .deletion_previews
        .get_mut(preview["previewId"].as_str().unwrap())
        .unwrap()
        .expires_at = 0;
    assert_eq!(
        fixture
            .move_to_disposable_sink(&preview)
            .await
            .unwrap_err()
            .code,
        "NODE_DELETE_PREVIEW_CHANGED"
    );
    let forged = json!({"previewId":"delete_preview_artificial"});
    assert_eq!(
        fixture
            .move_to_disposable_sink(&forged)
            .await
            .unwrap_err()
            .code,
        "NODE_DELETE_PREVIEW_CHANGED"
    );
    assert!(fixture.slot().is_dir());
}

#[tokio::test]
async fn native_failure_keeps_slot_and_does_not_fall_back_to_permanent_delete() {
    let fixture = Fixture::new().await;
    let preview = fixture.preview().await;
    let error = fixture
        .apps
        .delete_with(&fixture.id, Fixture::input(&preview), |_, _| {
            Err(deletion_error(
                "ARTIFICIAL_TRASH_DENIED",
                "Artificial system trash permission refusal",
            ))
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "ARTIFICIAL_TRASH_DENIED");
    assert_eq!(error.details["operationOutcome"], "not_started");
    assert!(fixture.slot().join("data/notes.sqlite").is_file());
    let receipt: Value =
        serde_json::from_slice(&fs::read(error.details["receiptPath"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(receipt["phase"], "not_moved");
    let next = fixture.preview().await;
    assert_eq!(
        next["canDelete"], true,
        "a new explicit confirmation can retry after an observed no-move failure"
    );
}

#[tokio::test]
async fn uncertain_native_result_retains_known_destination_and_blocks_every_mutation() {
    let fixture = Fixture::new().await;
    let preview = fixture.preview().await;
    let destination = fixture.root.join("disposable-unconfirmed-trash-location");
    let expected_destination = destination.clone();
    let error = fixture
        .apps
        .delete_with(&fixture.id, Fixture::input(&preview), move |_, _| {
            // Simulate a native error whose output is explicitly ambiguous, even
            // though this artificial sink leaves the original object in place.
            Err(deletion_error(
                "SYSTEM_TRASH_RESULT_UNCONFIRMED",
                "Artificial uncertain result",
            )
            .detail("systemTrashPath", json!(destination))
            .outcome("unknown"))
        })
        .await
        .unwrap_err();
    assert_eq!(error.details["operationOutcome"], "unknown");
    let receipt: Value =
        serde_json::from_slice(&fs::read(error.details["receiptPath"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(receipt["phase"], "needs_attention");
    assert_eq!(receipt["systemTrashPath"], json!(expected_destination));
    let next = fixture.preview().await;
    assert_eq!(next["canDelete"], false);
    assert_eq!(
        fixture.apps.start(&fixture.id).await.unwrap_err().code,
        "NODE_DELETE_RECOVERY_REQUIRED"
    );
    let save: SaveConfigurationRequest = serde_json::from_value(json!({
        "revision":0,"values":{},"secrets":{}
    }))
    .unwrap();
    assert_eq!(
        fixture.apps.save(&fixture.id, save).await.unwrap_err().code,
        "NODE_DELETE_RECOVERY_REQUIRED"
    );
    assert_eq!(
        fixture
            .apps
            .register(fixture.source.clone())
            .await
            .unwrap_err()
            .code,
        "NODE_DELETE_RECOVERY_REQUIRED"
    );
    assert_eq!(
        fixture
            .apps
            .install_target_snapshot(Some(&fixture.id))
            .await
            .unwrap_err()
            .code,
        "NODE_DELETE_RECOVERY_REQUIRED"
    );
    assert!(fixture.slot().join("data/notes.sqlite").is_file());
}

#[tokio::test]
async fn interrupted_intent_survives_restart_and_is_not_automatically_retried() {
    let fixture = Fixture::new().await;
    let slot = fixture.slot();
    let receipts = fixture.data.join("node-apps/.deletions");
    fs::create_dir(&receipts).unwrap();
    let record = DeleteReceipt {
        version: 1,
        deletion_id: "delete_artificial_interrupted".into(),
        app_id: fixture.id.clone(),
        original_path: slot.clone(),
        slot_identity: capture(&slot).unwrap(),
        registration_digest: "a".repeat(64),
        phase: "intent".into(),
        system_trash_path: None,
        error: None,
    };
    write_new_json(
        &receipts.join("delete_artificial_interrupted.json"),
        &record,
    )
    .unwrap();
    let restarted =
        NodeApps::new(fixture.data.clone(), "/disposable-never-used-node".into()).unwrap();
    let preview = restarted
        .delete_preview_supported(&fixture.id, Ok(()))
        .await
        .unwrap();
    assert_eq!(preview["canDelete"], false);
    assert_eq!(
        restarted.start(&fixture.id).await.unwrap_err().code,
        "NODE_DELETE_RECOVERY_REQUIRED"
    );
    assert!(slot.join("data/notes.sqlite").is_file());
    assert!(receipts
        .join("delete_artificial_interrupted.json")
        .is_file());
}

#[tokio::test]
async fn deletion_refuses_symlink_slots_and_unsupported_systems() {
    let fixture = Fixture::new().await;
    let preview = fixture
        .apps
        .delete_preview_supported(
            &fixture.id,
            Err(deletion_error(
                "SYSTEM_TRASH_UNSUPPORTED",
                "Artificial unsupported system",
            )),
        )
        .await
        .unwrap();
    assert_eq!(preview["canDelete"], false);
    let retained = fixture.root.join("retained-disposable-symlink-slot");
    fs::rename(fixture.slot(), &retained).unwrap();
    symlink(&retained, fixture.slot()).unwrap();
    assert!(fixture
        .apps
        .delete_preview_supported(&fixture.id, Ok(()))
        .await
        .is_err());
    assert!(retained.join("data/notes.sqlite").is_file());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn blocking_boundary_refuses_a_replacement_slot_before_native_trash_move() {
    let fixture = Fixture::new().await;
    let preview = fixture.preview().await;
    let retained = fixture.root.join("retained-disposable-blocking-original");
    let retained_in_sink = retained.clone();
    let error = fixture
        .apps
        .delete_with(
            &fixture.id,
            Fixture::input(&preview),
            move |source, expected| {
                // Simulate an external replacement after async admission/intent and
                // before the worker reaches the actual native filesystem boundary.
                fs::rename(source, &retained_in_sink)?;
                fs::create_dir(source)?;
                fs::copy(
                    retained_in_sink.join("registration.json"),
                    source.join("registration.json"),
                )?;
                fs::write(
                    source.join("disposable-replacement.txt"),
                    "replacement must remain here",
                )?;
                crate::system_trash::move_directory_with_identity(source, expected)
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "SYSTEM_TRASH_IDENTITY_CHANGED");
    assert_eq!(error.details["operationOutcome"], "not_started");
    assert!(error.details["systemTrashPath"].is_null());
    assert_eq!(
        fs::read_to_string(fixture.slot().join("disposable-replacement.txt")).unwrap(),
        "replacement must remain here"
    );
    assert!(retained.join("data/notes.sqlite").is_file());
    let receipt: Value =
        serde_json::from_slice(&fs::read(error.details["receiptPath"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(
        receipt["phase"], "needs_attention",
        "the old identity is no longer at its registered path"
    );
}

/// Explicit only: the fixture creates a private temporary platform root and
/// author source; only its newly registered slot enters the real system Trash.
/// The moved data/configuration remain there as disposable local evidence.
#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "moves one newly created disposable Node slot into the real macOS Trash"]
async fn native_disposable_node_delete_smoke() {
    let fixture = Fixture::new().await;
    let expected = capture(&fixture.slot()).unwrap();
    let preview = fixture.apps.delete_preview(&fixture.id).await.unwrap();
    assert_eq!(preview["canDelete"], true);
    let result = fixture
        .apps
        .delete(&fixture.id, Fixture::input(&preview))
        .await
        .unwrap();
    assert_eq!(result["operationOutcome"], "system_trashed");
    assert!(!fixture.slot().exists());
    let moved = PathBuf::from(result["systemTrashPath"].as_str().unwrap());
    assert!(crate::system_trash::moved_directory_matches(&expected, &moved).unwrap());
    assert_eq!(
        fs::read_to_string(moved.join("data/notes.sqlite")).unwrap(),
        "artificial database"
    );
    assert_eq!(
        fs::read_to_string(moved.join("data/attachment.txt")).unwrap(),
        "artificial attachment"
    );
    assert_eq!(
        fs::read_to_string(moved.join("configuration/artificial-credential")).unwrap(),
        "artificial private sample"
    );
    assert!(moved.join("registration.json").is_file());
    assert!(fixture.source.join("local-app.json").is_file());
    assert!(fixture.source.join("server/main.mjs").is_file());
    assert!(fixture.apps.list().await.unwrap().is_empty());
    let receipt: Value =
        serde_json::from_slice(&fs::read(result["receiptPath"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(receipt["phase"], "system_trashed");
    eprintln!(
        "disposable native Node deletion evidence: {}",
        moved.display()
    );
}
