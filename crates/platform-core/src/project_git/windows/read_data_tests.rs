//! Only fresh disposable project data is seeded by these fixtures. The actual
//! reader is not allowed to publish, recover, repair or modify that material.
use super::*;
use crate::{file_identity::PersistedIdentity, CreateProjectInput};

struct Reader {
    context: Context,
    _stop: watch::Sender<bool>,
    _cancel: watch::Sender<bool>,
}
impl std::ops::Deref for Reader {
    type Target = Context;
    fn deref(&self) -> &Context {
        &self.context
    }
}
fn context() -> Reader {
    let (stop_sender, stop) = watch::channel(false);
    let (cancel_sender, cancel) = watch::channel(false);
    Reader {
        context: Context::new(stop, cancel).unwrap(),
        _stop: stop_sender,
        _cancel: cancel_sender,
    }
}

async fn fixture() -> (
    tempfile::TempDir,
    RuntimePaths,
    Arc<ProjectStore>,
    ProjectHandle,
) {
    let (temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let info = store
        .create(CreateProjectInput {
            name: "Disposable Windows readonly data version".into(),
            description: None,
        })
        .await
        .unwrap();
    let project = store.project(&info.id).unwrap();
    (temporary, paths, store, project)
}
fn data_file(path: &Path) -> version_data::DataFile {
    let bytes = fs::read(path).unwrap();
    version_data::DataFile {
        bytes: bytes.len() as u64,
        sha256: sha256(&bytes),
    }
}
fn seed_snapshot(project: &ProjectHandle) -> PathBuf {
    let root = project.root.join(version_data::DIRECTORY);
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("attachments")).unwrap();
    fs::create_dir(root.join("attachments/人工 空目录")).unwrap();
    fs::write(
        root.join("attachments/人工 留言.txt"),
        b"disposable saved attachment",
    )
    .unwrap();
    // Seed a real previously saved SQLite version, not a product save request.
    // This step only reads history and must not depend on the closed save path.
    let database = rusqlite::Connection::open(root.join("database.sqlite")).unwrap();
    database
        .pragma_update(None, "journal_mode", "DELETE")
        .unwrap();
    database.execute_batch("CREATE TABLE disposable_saved_note(value TEXT); INSERT INTO disposable_saved_note VALUES('previous saved data');").unwrap();
    database.close().unwrap();
    let manifest = version_data::DataSnapshotManifest {
        format_version: 1,
        project_id: project.manifest.id.clone(),
        sdk_api_version: project.manifest.sdk_api_version,
        database_path: project.manifest.database.clone(),
        attachments_dir: project.manifest.attachments_dir.clone(),
        database: data_file(&root.join("database.sqlite")),
        attachments: BTreeMap::from([(
            "人工 留言.txt".into(),
            data_file(&root.join("attachments/人工 留言.txt")),
        )]),
        directories: vec!["人工 空目录".into()],
    };
    fs::write(
        root.join(version_data::MANIFEST),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    root
}
#[derive(Debug)]
struct FileBefore {
    identity: PersistedIdentity,
    bytes: Vec<u8>,
    read_only: bool,
}
fn capture(root: &Path) -> BTreeMap<PathBuf, FileBefore> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                pending.push(path);
            } else {
                result.insert(
                    path.clone(),
                    FileBefore {
                        identity: PersistedIdentity::capture(&path).unwrap(),
                        bytes: fs::read(&path).unwrap(),
                        read_only: metadata.permissions().readonly(),
                    },
                );
            }
        }
    }
    result
}
fn unchanged(before: &BTreeMap<PathBuf, FileBefore>, root: &Path) {
    let after = capture(root);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    for (path, before) in before {
        let after = &after[path];
        assert!(
            before.identity.matches_captured(&after.identity),
            "{}",
            path.display()
        );
        assert_eq!(before.bytes, after.bytes);
        assert_eq!(before.read_only, after.read_only);
    }
}

#[tokio::test]
async fn missing_snapshot_is_readonly_none_and_scope_fields_keep_original_meaning() {
    let (_temporary, _paths, store, project) = fixture().await;
    let reader = context();
    assert!(inspect_current_data(&project, &reader).unwrap().is_none());
    assert!(!project.root.join(version_data::DIRECTORY).exists());
    for (scope, state) in [
        ("project-root", "supported"),
        ("source-only", "source-only"),
        ("absent", "not-supported"),
    ] {
        let status = data_status(None, scope);
        assert_eq!(status["state"], state);
        assert_eq!(status["canSave"], false);
        assert!(status["currentSavedSnapshot"].is_null());
    }
    store.shutdown().await;
}

#[tokio::test]
async fn existing_valid_readonly_snapshot_is_verified_without_repair_or_new_companions() {
    let (_temporary, _paths, store, project) = fixture().await;
    let root = seed_snapshot(&project);
    let paths = capture(&root).into_keys().collect::<Vec<_>>();
    for path in &paths {
        platform_native::windows::files::set_read_only(path, true).unwrap();
    }
    let before = capture(&root);
    let snapshot = inspect_current_data(&project, &context()).unwrap().unwrap();
    assert_eq!(
        snapshot.manifest_digest,
        sha256(&fs::read(root.join(version_data::MANIFEST)).unwrap())
    );
    assert_eq!(snapshot.manifest.attachments.len(), 1);
    assert_eq!(snapshot.manifest.directories, ["人工 空目录"]);
    let status = data_status(Some(&snapshot), "project-root");
    assert_eq!(status["state"], "supported");
    assert_eq!(status["canSave"], false);
    assert_eq!(
        status["currentSavedSnapshot"]["manifestDigest"],
        snapshot.manifest_digest
    );
    assert_eq!(
        status["currentSavedSnapshot"]["databaseBytes"],
        snapshot.manifest.database.bytes
    );
    assert_eq!(status["currentSavedSnapshot"]["attachmentCount"], 1);
    unchanged(&before, &root);
    for path in paths {
        platform_native::windows::files::set_read_only(&path, false).unwrap();
    }
    store.shutdown().await;
}

#[tokio::test]
async fn bad_manifest_changed_payload_and_extra_files_remain_untouched() {
    for variation in ["manifest", "database", "attachment", "extra"] {
        let (_temporary, _paths, store, project) = fixture().await;
        let root = seed_snapshot(&project);
        match variation {
            "manifest" => fs::write(root.join(version_data::MANIFEST), b"not valid JSON").unwrap(),
            "database" => {
                fs::write(root.join("database.sqlite"), b"not saved database bytes").unwrap()
            }
            "attachment" => fs::write(
                root.join("attachments/人工 留言.txt"),
                b"changed attachment",
            )
            .unwrap(),
            _ => fs::write(root.join("unknown evidence"), b"preserve unowned evidence").unwrap(),
        }
        let before = capture(&root);
        assert!(inspect_current_data(&project, &context()).is_err());
        unchanged(&before, &root);
        store.shutdown().await;
    }
}

#[tokio::test]
async fn snapshot_reader_keeps_the_original_hardlink_rejection_without_deleting_either_name() {
    let (_temporary, _paths, store, project) = fixture().await;
    let root = seed_snapshot(&project);
    let source = root.join("attachments/人工 留言.txt");
    let alias = project.root.join("outside snapshot original.txt");
    fs::hard_link(&source, &alias).unwrap();
    let expected = PersistedIdentity::capture(&alias).unwrap();
    let before = capture(&root);
    assert!(inspect_current_data(&project, &context()).is_err());
    unchanged(&before, &root);
    assert!(expected.matches_path(&alias).unwrap());
    assert_eq!(fs::read(&alias).unwrap(), b"disposable saved attachment");
    store.shutdown().await;
}

#[tokio::test]
async fn cancelled_existing_snapshot_is_preserved_and_never_recovered() {
    let (_temporary, _paths, store, project) = fixture().await;
    let root = seed_snapshot(&project);
    let before = capture(&root);
    let (_stop, stop) = watch::channel(false);
    let (_cancel, cancel) = watch::channel(true);
    let reader = Context::new(stop, cancel).unwrap();
    let error = inspect_current_data(&project, &reader).unwrap_err();
    assert_eq!(error.code, "PROJECT_DATA_SNAPSHOT_CANCELLED");
    unchanged(&before, &root);
    store.shutdown().await;
}

#[test]
fn data_publication_schema_and_data_path_classification_match_existing_contract() {
    let value = json!({"manifestDigest":"a".repeat(64),"previousManifestDigest":null,"previousIndex":null,"preparedIndex":"index.prepared",
        "lockIdentity":[42,73],"repositoryIdentity":"recorded identity","operationIdentity":"recorded operation",
        "files":{"version-data/database.sqlite":{"mode":"100644","digest":"b".repeat(64),"size":17}}});
    let parsed: DataPublication = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), value);
    let mut invalid = value;
    invalid["unrecognized"] = json!(true);
    assert!(serde_json::from_value::<DataPublication>(invalid).is_err());
    assert!(is_data_path("version-data"));
    assert!(is_data_path("version-data/database.sqlite"));
    for path in [
        "version-data-other/file",
        "Version-Data/file",
        "app/version-data/file",
    ] {
        assert!(!is_data_path(path));
    }
}

#[test]
fn pending_data_transactions_remain_closed_and_unknown_material_is_preserved() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let id = "prj_disposable_readonly_data";
    check_data_operations_available(&root, id).unwrap();
    check_data_workspace_available(&root).unwrap();
    let directory = root.join(".platform/project-state/git").join(id);
    fs::create_dir_all(&directory).unwrap();
    let evidence = directory.join("task_disposable.json");
    fs::write(&evidence, b"disposable retained data operation").unwrap();
    let identity = PersistedIdentity::capture(&evidence).unwrap();
    for error in [
        check_data_operations_available(&root, id).unwrap_err(),
        check_data_workspace_available(&root).unwrap_err(),
        check_data_recovery_record(
            &root,
            id,
            &GitDataSaveFinalizeInput {
                save_task_id: "task_disposable".into(),
                expected_journal_digest: "a".repeat(64),
            },
        )
        .unwrap_err(),
    ] {
        assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
        assert_eq!(error.details["operationOutcome"], "not_started");
    }
    assert!(identity.matches_path(&evidence).unwrap());
    assert_eq!(
        fs::read(evidence).unwrap(),
        b"disposable retained data operation"
    );
}
