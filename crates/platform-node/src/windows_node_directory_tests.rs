//! Real disposable NTFS objects; no Node process, dependencies or old projects.
use super::*;
use platform_host::WindowsWorkspace;
use std::fs;

struct Fixture {
    _workspace: WindowsWorkspace,
    parent: WindowsSourceDirectory,
    source: PathBuf,
    apps: Arc<NodeApps>,
}
impl Fixture {
    fn new() -> Self {
        let artifacts = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(".test-artifacts");
        fs::create_dir_all(&artifacts).unwrap();
        let path = artifacts.join(format!(
            "windows-node-disposable-directory-{}",
            Uuid::new_v4()
        ));
        let workspace = platform_host::disposable_node_workspace(&path).unwrap();
        let parent = WindowsSourceDirectory::open_private(&path).unwrap();
        let source = parent
            .create_private_directory("external selected folder")
            .unwrap()
            .path()
            .to_owned();
        let apps = Arc::new(
            NodeApps::new_windows(workspace.root(), PathBuf::from("not-executed.exe")).unwrap(),
        );
        Self {
            _workspace: workspace,
            parent,
            source,
            apps,
        }
    }
    fn input(&self) -> DirectoryInitializeInput {
        DirectoryInitializeInput {
            expected_revision: self.apps.inspect_directory(&self.source).unwrap()
                ["expectedRevision"]
                .as_str()
                .unwrap()
                .into(),
            name: "Disposable <Windows> & starter".into(),
        }
    }
    fn initialize(&self) -> Value {
        self.apps
            .initialize_directory(&self.source, self.input())
            .unwrap()
    }
}

#[test]
fn checks_are_read_only_and_hidden_or_unrelated_files_require_review() {
    let fixture = Fixture::new();
    let empty = fixture.apps.inspect_directory(&fixture.source).unwrap();
    assert_eq!(empty["state"], "empty");
    assert_eq!(fs::read_dir(&fixture.source).unwrap().count(), 0);
    assert!(!fixture.apps.root.exists());
    fs::write(fixture.source.join(".env"), b"artificial-never-read-secret").unwrap();
    let review = fixture.apps.inspect_directory(&fixture.source).unwrap();
    assert_eq!(review["state"], "needs-review");
    assert_ne!(review["expectedRevision"], empty["expectedRevision"]);
    assert!(review["reviewPrompt"]
        .as_str()
        .unwrap()
        .contains(REVIEW_SKILL));
    assert!(!review.to_string().contains("artificial-never-read-secret"));
    assert_eq!(
        fs::read(fixture.source.join(".env")).unwrap(),
        b"artificial-never-read-secret"
    );
    assert!(!fixture.source.join("local-app.json").exists());
}

#[test]
fn empty_initialization_creates_complete_shared_starter_without_registration_or_execution() {
    let fixture = Fixture::new();
    let ready = fixture.initialize();
    assert_eq!(ready["state"], "ready");
    assert_eq!(ready["sourcePath"], json!(fixture.source));
    assert!(!ready["inspection"]["expectedRevision"]
        .as_str()
        .unwrap()
        .is_empty());
    assert_eq!(ready["inspection"]["configuration"]["fields"], json!([]));
    let manifest: Value =
        serde_json::from_slice(&fs::read(fixture.source.join("local-app.json")).unwrap()).unwrap();
    assert_eq!(manifest["runtime"], "node-service");
    assert_eq!(manifest["backend"]["nodeMajor"], 24);
    assert_eq!(manifest["dataContract"]["version"], 1);
    assert_eq!(
        fs::read(fixture.source.join("backend/server.mjs")).unwrap(),
        SERVER.as_bytes()
    );
    assert_eq!(
        fs::read(fixture.source.join("web/app.js")).unwrap(),
        FRONTEND.as_bytes()
    );
    assert!(fs::read_to_string(fixture.source.join("web/index.html"))
        .unwrap()
        .contains("Disposable &lt;Windows&gt; &amp; starter"));
    assert_eq!(
        fs::read(
            fixture
                .source
                .join(".agents/skills/local-folder-review/SKILL.md")
        )
        .unwrap(),
        REVIEW_SKILL.as_bytes()
    );
    let lock: Value =
        serde_json::from_slice(&fs::read(fixture.source.join("package-lock.json")).unwrap())
            .unwrap();
    assert_eq!(lock["lockfileVersion"], 3);
    assert_eq!(lock["packages"].as_object().unwrap().len(), 1);
    assert!(!fixture.source.join("node_modules").exists());
    assert!(!fixture.apps.root.exists());
    assert!(
        fixture.apps.inspect_directory(&fixture.source).unwrap()["expectedRevision"].is_string()
    );
}

#[test]
fn stale_empty_confirmations_never_cover_new_files_or_replaced_directory_identities() {
    let fixture = Fixture::new();
    let input = fixture.input();
    fs::write(fixture.source.join("retained.txt"), b"user material").unwrap();
    assert_eq!(
        fixture
            .apps
            .initialize_directory(&fixture.source, input)
            .unwrap_err()
            .code,
        "NODE_DIRECTORY_CHANGED"
    );
    assert_eq!(
        fs::read(fixture.source.join("retained.txt")).unwrap(),
        b"user material"
    );
    assert!(!fixture.source.join("web").exists());
    let other = fixture
        .parent
        .create_private_directory("replacement target")
        .unwrap()
        .path()
        .to_owned();
    let review = fixture.apps.inspect_directory(&other).unwrap();
    let displaced = fixture.parent.path().join("retained displaced source");
    fs::rename(&other, &displaced).unwrap();
    fs::create_dir(&other).unwrap();
    let error = fixture
        .apps
        .initialize_directory(
            &other,
            DirectoryInitializeInput {
                expected_revision: review["expectedRevision"].as_str().unwrap().into(),
                name: "Replacement must not be initialized".into(),
            },
        )
        .unwrap_err();
    assert_eq!(error.code, "NODE_DIRECTORY_CHANGED");
    assert_eq!(fs::read_dir(&other).unwrap().count(), 0);
    assert!(displaced.is_dir());
}

#[test]
fn extra_material_during_initialization_stops_writes_and_retains_every_created_path() {
    let fixture = Fixture::new();
    let error = fixture
        .apps
        .initialize_directory_with_hook(&fixture.source, fixture.input(), |path| {
            if path == Path::new("web/index.html") {
                assert!(fs::rename(
                    fixture.source.join("web"),
                    fixture.source.join("must-not-replace-web")
                )
                .is_err());
                fs::write(
                    fixture.source.join("web/retained-race.txt"),
                    b"concurrent material",
                )
                .unwrap();
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(
        error.details["operationOutcome"],
        "initialization_incomplete"
    );
    assert_eq!(error.details["partialFilesRetained"], true);
    assert_eq!(
        fs::read(fixture.source.join("web/retained-race.txt")).unwrap(),
        b"concurrent material"
    );
    assert!(!fixture.source.join("web/index.html").exists());
    assert!(!fixture.source.join("local-app.json").exists());
    assert!(error.details["createdPaths"].as_array().unwrap().len() >= 2);
}

#[test]
fn failure_after_a_created_file_keeps_partial_starter_and_requires_reinspection() {
    let fixture = Fixture::new();
    let input = fixture.input();
    let old_revision = input.expected_revision.clone();
    let error = fixture
        .apps
        .initialize_directory_with_hook(&fixture.source, input, |path| {
            if path == Path::new("web/app.js") {
                Err(ApiError::new(
                    "DISPOSABLE_WRITE_FAILURE",
                    "Injected after the first completed file",
                ))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    assert_eq!(
        error.details["operationOutcome"],
        "initialization_incomplete"
    );
    assert!(error.details["createdPaths"]
        .as_array()
        .unwrap()
        .contains(&json!("web/index.html")));
    assert!(fixture.source.join("web/index.html").is_file());
    assert!(!fixture.source.join("web/app.js").exists());
    assert_eq!(
        fixture.apps.inspect_directory(&fixture.source).unwrap()["state"],
        "needs-review"
    );
    assert_eq!(
        fixture
            .apps
            .initialize_directory(
                &fixture.source,
                DirectoryInitializeInput {
                    expected_revision: old_revision,
                    name: "No replay".into()
                }
            )
            .unwrap_err()
            .code,
        "NODE_DIRECTORY_CHANGED"
    );
    assert!(!fixture.source.join("local-app.json").exists());
}

#[test]
fn hardlinks_and_occupied_files_cannot_approve_a_ready_source() {
    let fixture = Fixture::new();
    fixture.initialize();
    let writer = fs::OpenOptions::new()
        .write(true)
        .open(fixture.source.join("backend/server.mjs"))
        .unwrap();
    assert_ne!(
        fixture.apps.inspect_directory(&fixture.source).unwrap()["state"],
        "ready"
    );
    drop(writer);
    fs::hard_link(
        fixture.source.join("local-app.json"),
        fixture.parent.path().join("retained-manifest-hardlink"),
    )
    .unwrap();
    assert!(fixture.apps.inspect_directory(&fixture.source).is_err());
    assert!(fixture.source.join("local-app.json").is_file());
}

#[tokio::test]
async fn ready_registration_binds_directory_and_source_revisions_without_rewriting_external_files()
{
    let fixture = Fixture::new();
    let apps = fixture.apps.clone();
    let source = fixture.source.clone();
    let input = fixture.input();
    let ready = tokio::task::spawn_blocking(move || apps.initialize_directory(&source, input))
        .await
        .unwrap()
        .unwrap();
    let revision = ready["expectedRevision"].as_str().unwrap().to_owned();
    fs::write(
        fixture.source.join("web/index.html"),
        b"changed after preview",
    )
    .unwrap();
    assert_eq!(
        fixture
            .apps
            .register_directory(fixture.source.clone(), revision)
            .await
            .unwrap_err()
            .code,
        "NODE_DIRECTORY_CHANGED"
    );
    assert!(!fixture.apps.root.exists());
    let review = fixture.apps.inspect_directory(&fixture.source).unwrap();
    let registered = fixture
        .apps
        .register_directory(
            fixture.source.clone(),
            review["expectedRevision"].as_str().unwrap().into(),
        )
        .await
        .unwrap();
    assert_eq!(registered["state"], "stopped");
    assert_eq!(registered["sourcePath"], json!(fixture.source));
    assert_eq!(
        fs::read(fixture.source.join("web/index.html")).unwrap(),
        b"changed after preview"
    );
    assert_eq!(fixture.apps.list().await.unwrap().len(), 1);
    assert!(fixture.apps.owned_process_ids().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn normal_shutdown_waits_for_accepted_initialization_and_rejects_later_initializations() {
    let fixture = Fixture::new();
    let input = fixture.input();
    let (accepted_send, accepted_recv) = tokio::sync::oneshot::channel();
    let (release_send, release_recv) = std::sync::mpsc::channel();
    let apps = fixture.apps.clone();
    let source = fixture.source.clone();
    let initialization = tokio::task::spawn_blocking(move || {
        let mut accepted_send = Some(accepted_send);
        apps.initialize_directory_with_hook(&source, input, |path| {
            if path == Path::new("backend") {
                accepted_send.take().unwrap().send(()).unwrap();
                release_recv.recv().unwrap();
            }
            Ok(())
        })
    });
    accepted_recv.await.unwrap();
    let apps = fixture.apps.clone();
    let shutdown = tokio::spawn(async move { apps.stop_all().await });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert!(!shutdown.is_finished());
    assert!(
        fixture.apps.inner.lock().await.shutting_down,
        "Waiting for source writes must already block new runtime launches"
    );
    release_send.send(()).unwrap();
    assert_eq!(initialization.await.unwrap().unwrap()["state"], "ready");
    shutdown.await.unwrap().unwrap();
    let empty = fixture
        .parent
        .create_private_directory("after shutdown")
        .unwrap()
        .path()
        .to_owned();
    let revision = fixture.apps.inspect_directory(&empty).unwrap()["expectedRevision"]
        .as_str()
        .unwrap()
        .to_owned();
    let apps = fixture.apps.clone();
    let error = tokio::task::spawn_blocking(move || {
        apps.initialize_directory(
            &empty,
            DirectoryInitializeInput {
                expected_revision: revision,
                name: "Rejected".into(),
            },
        )
    })
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.code, "NODE_DIRECTORY_STOPPING");
}
