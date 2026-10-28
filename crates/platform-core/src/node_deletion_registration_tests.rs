use super::*;

struct Fixture {
    _temporary: tempfile::TempDir,
    data: PathBuf,
    source: PathBuf,
    apps: NodeApps,
    id: String,
    before: String,
}
impl Fixture {
    async fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let data = root.join("disposable-registration-capture-data");
        let source = root.join("disposable-registration-capture-author");
        fs::create_dir(&data).unwrap();
        fs::create_dir_all(source.join("public")).unwrap();
        fs::create_dir(source.join("server")).unwrap();
        fs::write(source.join("public/index.html"), "disposable").unwrap();
        fs::write(source.join("server/main.mjs"), "// never executed").unwrap();
        fs::write(source.join("author-sentinel"), b"keep external author data").unwrap();
        fs::write(source.join("local-app.json"), serde_json::to_vec(&json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":"test.disposable.registrationcapture","name":"Disposable registration capture","version":"1"},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"protocolVersion":1,"root":"server","entry":"main.mjs","nodeMajor":24}
        })).unwrap()).unwrap();
        let apps = NodeApps::new(data.clone(), "/disposable/no-node".into()).unwrap();
        let id = apps.register(source.clone()).await.unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let before =
            fs::read_to_string(data.join("node-apps").join(&id).join("registration.json")).unwrap();
        Self {
            _temporary: temporary,
            data,
            source,
            apps,
            id,
            before,
        }
    }
    fn slot(&self) -> PathBuf {
        self.data.join("node-apps").join(&self.id)
    }
    fn journal(&self) -> NodeDeletionJournal {
        NodeDeletionJournal::create(&self.data, json!({
            "appId":self.id,"name":"Disposable registration capture","ownership":"external",
            "sourcePath":self.source,"expectedRegistrationRevision":deletion::revision(&self.slot()).unwrap(),
            "registrationBefore":serde_json::from_str::<Value>(&self.before).unwrap(),"registrationBeforeText":self.before,
            "rootIdentity":PersistedIdentity::capture(&self.slot()).unwrap(),"preservedPaths":[self.source,self.slot().join("runs")]
        })).unwrap()
    }
    fn capture_and_interrupt(&self, journal: &mut NodeDeletionJournal) -> PathBuf {
        journal.stage_all().unwrap();
        while journal.remove_next_staged().unwrap() {}
        journal.set_phase("removing").unwrap();
        let error = self
            .apps
            .remove_confirmed_registration_with(journal, || {
                Err(fail("disposable interruption after registration capture"))
            })
            .unwrap_err();
        assert!(error.message.contains("disposable interruption"));
        assert!(!self.slot().join("registration.json").exists());
        assert!(!fs::read_dir(self.slot()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(START_INTENT_CLEANUP_PREFIX)));
        let capture = self
            .apps
            .registration_transaction(journal)
            .join("captured-original");
        assert_eq!(fs::read_to_string(&capture).unwrap(), self.before);
        capture
    }
    fn preserved(&self) {
        assert_eq!(
            fs::read(self.source.join("author-sentinel")).unwrap(),
            b"keep external author data"
        );
    }
}

#[tokio::test]
async fn registration_capture_resumes_in_the_same_boot_without_a_run_cleanup_gate() {
    let fixture = Fixture::new().await;
    let mut journal = fixture.journal();
    let capture = fixture.capture_and_interrupt(&mut journal);
    let task_id = journal.task_id().to_owned();
    let restored = NodeApps::new(fixture.data.clone(), "/disposable/no-node".into()).unwrap();
    assert!(!has_start_intent_cleanup(&fixture.slot()).unwrap());
    restored.resume_deletions().await.unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        restored.wait_deletion_result(&task_id, &fixture.id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result["operationOutcome"], "deleted");
    assert!(!capture.parent().unwrap().exists());
    assert!(!fixture.slot().join("registration.json").exists());
    fixture.preserved();
}

#[tokio::test]
async fn registration_capture_never_removes_a_replacement_at_the_public_name() {
    let fixture = Fixture::new().await;
    let mut journal = fixture.journal();
    let capture = fixture.capture_and_interrupt(&mut journal);
    let replacement = format!("{}\n", fixture.before);
    fs::write(fixture.slot().join("registration.json"), &replacement).unwrap();
    let task_id = journal.task_id().to_owned();
    let restored = NodeApps::new(fixture.data.clone(), "/disposable/no-node".into()).unwrap();
    restored.resume_deletions().await.unwrap();
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        restored.wait_deletion_result(&task_id, &fixture.id),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.code, "NODE_APP_DELETE_CHANGED");
    assert_eq!(error.details["operationOutcome"], "delete_incomplete");
    assert_eq!(fs::read_to_string(&capture).unwrap(), fixture.before);
    assert_eq!(
        fs::read_to_string(fixture.slot().join("registration.json")).unwrap(),
        replacement
    );
    let task = restored
        .deletion_task_status(&task_id, &fixture.id)
        .unwrap();
    assert!(task["remaining"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value["state"] == "registration"));
    fixture.preserved();
}

#[tokio::test]
async fn no_directory_rename_abandonment_restores_captured_or_unlinked_registration() {
    for unlink in [false, true] {
        let fixture = Fixture::new().await;
        for name in ["configuration", "data", "cache"] {
            fs::remove_dir_all(fixture.slot().join(name)).unwrap();
        }
        let mut journal = fixture.journal();
        let capture = fixture.capture_and_interrupt(&mut journal);
        assert_eq!(journal.snapshot()["renamedAny"], false);
        if unlink {
            fs::remove_file(&capture).unwrap();
        }
        let task_id = journal.task_id().to_owned();
        let restored = NodeApps::new(fixture.data.clone(), "/disposable/no-node".into()).unwrap();
        let task = restored
            .deletion_task_status(&task_id, &fixture.id)
            .unwrap();
        assert_eq!(task["canAbandon"], true);
        restored
            .abandon_deletion(
                &task_id,
                &fixture.id,
                task["journalRevision"].as_u64().unwrap(),
            )
            .await
            .unwrap();
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            restored.wait_deletion_result(&task_id, &fixture.id),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error.code, "NODE_DELETION_ABANDONED");
        assert_eq!(
            fs::read_to_string(fixture.slot().join("registration.json")).unwrap(),
            fixture.before
        );
        assert!(!capture.parent().unwrap().exists());
        fixture.preserved();
    }
}
