// Backend-only public NodeApps deletion with the pinned official Node. All
// roots are freshly named TempDirs. No WebView is created: the actual browser
// ledger must be Absent, never replaced with a successful fake purge callback.
use super::*;
use crate::{
    durable_writer::{self, WriterOwner},
    file_identity::PersistedIdentity,
};
use std::{fs::File, os::windows::fs::OpenOptionsExt};

const LIMIT: Duration = Duration::from_secs(45);
const ARTIFICIAL_SECRET: &str = "artificial-w5-disposable-configuration-only";

struct DeletionFixture {
    _temporary: Option<tempfile::TempDir>,
    source: PathBuf,
    data: PathBuf,
    node: PathBuf,
}
impl DeletionFixture {
    fn new() -> Self {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-w5-nodeapps-delete-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        Self::create(root, Some(temporary))
    }
    fn create(root: PathBuf, temporary: Option<tempfile::TempDir>) -> Self {
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("panda-disposable-w5-nodeapps-delete-"));
        assert!(
            fs::read_dir(&root).unwrap().next().is_none(),
            "only a new empty named disposable root may be initialized"
        );
        let source = root.join("selected-user-source 中文 space");
        let data = root.join("disposable-platform-data");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&data).unwrap();
        Self {
            _temporary: temporary,
            source,
            data,
            node: official_node(),
        }
    }
    fn manager(&self) -> NodeApps {
        NodeApps::new(self.data.clone(), self.node.clone()).unwrap()
    }
    fn slot(&self, id: &str) -> PathBuf {
        self.data.join("node-apps").join(id)
    }
    async fn generate(&self, apps: &NodeApps) -> String {
        let preview = apps.inspect_directory(&self.source).unwrap();
        assert_eq!(preview["state"], "empty");
        let generated = apps
            .initialize_directory(
                &self.source,
                DirectoryInitializeInput {
                    expected_revision: preview["expectedRevision"].as_str().unwrap().into(),
                    name: "Disposable W5 real generated application".into(),
                },
            )
            .unwrap();
        let pending = generated["pendingAppId"].as_str().unwrap().to_owned();
        // This is an ordinary author edit of the freshly generated source,
        // followed by the public reinspection/registration path. Product
        // configuration checks and the fixed Node digest remain unchanged.
        let manifest = self.source.join("panda.json");
        let mut declaration: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        declaration["configuration"] = json!({"version":1,"fields":[
            {"name":"GREETING","label":"Disposable text","type":"string","required":true,"secret":false,"description":"Artificial deletion fixture text","howToGet":["Use artificial text"]},
            {"name":"API_KEY","label":"Disposable secret","type":"string","required":true,"secret":true,"description":"Artificial credential; no remote service","howToGet":["Use artificial test text"]}
        ]});
        fs::write(&manifest, serde_json::to_vec_pretty(&declaration).unwrap()).unwrap();
        let backend = self.source.join("backend/server.mjs");
        let original = fs::read_to_string(&backend).unwrap();
        let observation = r#"import { writeFileSync as w5Write, existsSync as w5Exists, readFileSync as w5Read } from 'node:fs';
import { join as w5Join } from 'node:path';
const w5CountFile=w5Join(process.env.APP_DATA_DIR,'w5-launch-count');
w5Write(w5CountFile,String(w5Exists(w5CountFile)?Number(w5Read(w5CountFile,'utf8'))+1:1));
w5Write(w5Join(process.env.APP_DATA_DIR,'w5-pid'),String(process.pid));
w5Write(w5Join(process.env.APP_DATA_DIR,'w5-business.json'),JSON.stringify({greeting:process.env.GREETING}));
process.stderr.write('W5 actual official Node backend log\n');
"#;
        fs::write(&backend, format!("{observation}{original}")).unwrap();
        let inspected = apps.inspect_directory(&self.source).unwrap();
        assert_eq!(inspected["state"], "ready");
        let registered = apps
            .register_directory(
                self.source.clone(),
                inspected["expectedRevision"].as_str().unwrap().into(),
            )
            .await
            .unwrap();
        assert_eq!(registered["id"], pending);
        assert_eq!(registered["ownership"], "external");
        assert_eq!(
            apps.start(&pending).await.unwrap_err().code,
            "NODE_CONFIGURATION_REQUIRED"
        );
        assert!(apps.owned_process_ids().await.is_empty());
        configure(apps, &pending, "disposable W5 greeting", ARTIFICIAL_SECRET).await;
        pending
    }
    async fn run(&self, apps: &NodeApps, id: &str) -> CapturedProcess {
        apps.start(id).await.unwrap();
        let browser = Browser::open(apps, id).await;
        let response = browser
            .client
            .get(format!("{}/api/hello", browser.origin))
            .header("cookie", &browser.cookie)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        assert_eq!(
            response.json::<Value>().await.unwrap()["message"],
            "你的本地应用已经连通。"
        );
        let pid = fs::read_to_string(self.source.join("data/w5-pid"))
            .unwrap()
            .parse::<u32>()
            .unwrap();
        let captured = CapturedProcess::capture(pid);
        let log = self.slot(id).join("logs/current.log");
        tokio::time::timeout(LIMIT, async {
            while !fs::read_to_string(&log)
                .unwrap_or_default()
                .contains("W5 actual official Node backend log")
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the actual Node stderr pump must publish its log");
        captured
    }
    fn backend_only_absent(&self, id: &str) {
        assert!(!crate::webview_storage::StoreLedger::new(self.data.clone())
            .contains(crate::project_cleanup::ProjectKind::Node, id)
            .unwrap());
        assert!(!self
            .data
            .join("webview-stores")
            .join(format!("node-{id}"))
            .exists());
    }
    fn assert_deleted(&self, apps: &NodeApps, id: &str, owned: &[(PathBuf, PersistedIdentity)]) {
        for (path, identity) in owned {
            assert!(identity.windows_file_id.is_some());
            assert!(identity.volume_uuid.is_none());
            assert!(
                identity.absent_on_expected_volume(path).unwrap(),
                "{}",
                path.display()
            );
        }
        absent(&self.slot(id));
        assert!(self.source.is_dir());
        assert!(self.source.join("data/w5-business.json").is_file());
        absent(&self.data.join("node-project-ownership").join(id));
        assert!(apps.deletion_tasks().unwrap().is_empty());
        assert!(
            crate::node_deletion_journal::NodeDeletionJournal::list(&self.data)
                .unwrap()
                .is_empty()
        );
        self.backend_only_absent(id);
        for container in [
            self.data.join("node-deletion-tasks"),
            self.data.join("node-source-owners"),
        ] {
            if container.is_dir() {
                for entry in fs::read_dir(container).unwrap() {
                    let entry = entry.unwrap();
                    assert_eq!(entry.file_name(), ".pending-deletions");
                    assert!(fs::read_dir(entry.path()).unwrap().next().is_none());
                }
            }
        }
    }
}

fn capture_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<(PathBuf, PersistedIdentity)> {
    paths
        .into_iter()
        .filter(|path| path.exists())
        .map(|path| {
            let identity = PersistedIdentity::capture(&path).unwrap();
            (path, identity)
        })
        .collect()
}
async fn input(apps: &NodeApps, id: &str) -> DeleteNodeAppInput {
    let preview = apps.delete_preview(id).await.unwrap();
    assert!(preview["blocker"].is_null(), "{preview}");
    DeleteNodeAppInput {
        expected_revision: preview["expectedRevision"].as_str().unwrap().into(),
        force: false,
    }
}
async fn terminal(apps: &NodeApps, task: &str, id: &str) -> Value {
    tokio::time::timeout(LIMIT, async {
        loop {
            let status = apps.deletion_task_status(task, id).unwrap();
            if matches!(
                status["phase"].as_str(),
                Some("deleted" | "failed" | "abandoned")
            ) && status["active"] != true
            {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "deletion did not settle: {:?}",
            apps.deletion_task_status(task, id)
        )
    })
}
async fn close(apps: &NodeApps) {
    tokio::time::timeout(LIMIT, apps.stop_all())
        .await
        .expect("bounded actual NodeApps closure")
        .unwrap();
    assert!(apps.processes.shutdown().await.is_empty());
    assert!(apps
        .processes
        .snapshot()
        .iter()
        .all(|record| record.resource_closed()));
    assert!(apps.owned_process_ids().await.is_empty());
}
fn owned(apps: &NodeApps, id: &str) -> Vec<(PathBuf, PersistedIdentity)> {
    let mut paths = vec![apps.root.join(id)];
    paths.extend(apps.project_ownership().owned_paths(id).unwrap());
    capture_paths(paths)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Explicit pinned PANDA_W3_OFFICIAL_NODE; backend-only real generated Node project deletion in new TempDir"]
async fn official_selected_starter_deletion_clears_private_material_and_preserves_source_data() {
    let fixture = DeletionFixture::new();
    let apps = fixture.manager();
    let id = fixture.generate(&apps).await;
    let process = fixture.run(&apps, &id).await;
    let slot = fixture.slot(&id);
    fixture.backend_only_absent(&id);
    assert!(slot.join("configuration/configuration.json").is_file());
    assert!(fs::read_dir(slot.join("configuration/credentials"))
        .unwrap()
        .next()
        .is_some());
    assert!(fixture.source.join("data/w5-business.json").is_file());
    assert!(slot.join("logs/current.log").is_file());
    fs::create_dir_all(fixture.source.join(".git/objects/aa")).unwrap();
    let git_object = fixture
        .source
        .join(".git/objects/aa/disposable-readonly-object");
    fs::write(&git_object, b"disposable author Git material").unwrap();
    native_files::set_read_only(&git_object, true).unwrap();
    let paths = owned(&apps, &id);
    let expected_slot = PersistedIdentity::capture(&slot).unwrap();
    let admitted = apps
        .admit_operation(&id, "disposable actual host file writer")
        .unwrap();
    let mut writer = WriterOwner::node(&fixture.data, &id)
        .unwrap()
        .begin("disposable-actual-file-writer")
        .unwrap();
    let owner = apps.project_ownership().directory(&id).unwrap();
    let marker = fs::read_dir(owner.join("writers"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap();
    let marker_id = PersistedIdentity::capture(&marker).unwrap();
    let marker_bytes = fs::read(&marker).unwrap();
    let mut output = File::create(fixture.source.join("last-admitted-writer.txt")).unwrap();
    output.write_all(b"genuine admitted host I/O").unwrap();
    let submitted = apps
        .submit_deletion(&id, input(&apps, &id).await)
        .await
        .unwrap();
    let task_id = submitted["taskId"].as_str().unwrap();
    assert_eq!(submitted["phase"], "waiting");
    let persisted =
        crate::node_deletion_journal::NodeDeletionJournal::load(&fixture.data, task_id).unwrap();
    let persisted_slot: PersistedIdentity =
        serde_json::from_value(persisted.metadata()["rootIdentity"].clone()).unwrap();
    assert!(persisted_slot.matches_captured(&expected_slot));
    assert_eq!(persisted.metadata()["ownedPaths"][0], json!(slot));
    drop(persisted);
    assert!(expected_slot.matches_path(&slot).unwrap());
    assert_eq!(fs::read(&marker).unwrap(), marker_bytes);
    assert!(marker_id.matches_path(&marker).unwrap());
    assert!(
        durable_writer::require_owner_clear(&WriterOwner::node(&fixture.data, &id).unwrap())
            .is_err()
    );
    output.sync_all().unwrap();
    drop(output); // actual no-child writer I/O is closed before its finish receipt
    writer.finish().unwrap();
    drop(writer); // release its owner/directory pins before namespace retirement
    absent(&marker);
    durable_writer::require_owner_clear(&WriterOwner::node(&fixture.data, &id).unwrap()).unwrap();
    drop(admitted);
    let result = tokio::time::timeout(LIMIT, apps.wait_deletion_result(task_id, &id))
        .await
        .unwrap();
    let settled = terminal(&apps, task_id, &id).await;
    close(&apps).await;
    let result = result.unwrap();
    assert_eq!(result["operationOutcome"], "deleted", "{settled}");
    assert_eq!(result["evidenceRetained"], false);
    assert_eq!(result["sourcePreserved"], true);
    stopped(&[process]).await;
    fixture.assert_deleted(&apps, &id, &paths);
    native_files::set_read_only(&git_object, false).unwrap();
    // Local generation has no download publication or external delivery. Do
    // not invent those authorities to make the deletion scope look broader.
    absent(
        &apps
            .deletion_workspace()
            .join(".platform/source-publications")
            .join(&id),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Explicit pinned PANDA_W3_OFFICIAL_NODE; backend-only external author source preservation in new TempDir"]
async fn official_external_original_remains_the_same_full_identity_and_bytes_after_private_deletion(
) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w3-nodeapps-w5-external-delete-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let fixture = Fixture::create(&root, "normal");
    fs::create_dir(fixture.source.join(".git")).unwrap();
    fs::write(
        fixture.source.join(".git/author-version"),
        b"preserve author Git bytes",
    )
    .unwrap();
    let source_id = PersistedIdentity::capture(&fixture.source).unwrap();
    let apps = fixture.manager();
    let id = apps.register(fixture.source.clone()).await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    configure(
        &apps,
        &id,
        "external disposable greeting",
        ARTIFICIAL_SECRET,
    )
    .await;
    apps.start(&id).await.unwrap();
    let state = Browser::open(&apps, &id).await.state().await;
    let process = CapturedProcess::capture(state["pid"].as_u64().unwrap().try_into().unwrap());
    let paths = owned(&apps, &id);
    let originals = tree(&fixture.source);
    let preview = apps.delete_preview(&id).await.unwrap();
    assert_eq!(preview["ownership"], "external");
    assert_eq!(preview["preservedPaths"], json!([fixture.source]));
    assert!(
        !crate::webview_storage::StoreLedger::new(fixture.data.clone())
            .contains(crate::project_cleanup::ProjectKind::Node, &id)
            .unwrap()
    );
    let result = tokio::time::timeout(LIMIT, apps.delete(&id, input(&apps, &id).await))
        .await
        .unwrap();
    close(&apps).await;
    let result = result.unwrap();
    assert_eq!(result["operationOutcome"], "deleted");
    assert_eq!(result["sourcePreserved"], true);
    assert_eq!(result["evidenceRetained"], false);
    stopped(&[process]).await;
    for (path, identity) in paths {
        assert!(
            identity.absent_on_expected_volume(&path).unwrap(),
            "{}",
            path.display()
        );
    }
    assert!(source_id.matches_path(&fixture.source).unwrap());
    assert_tree(&fixture.source, &originals);
    assert!(apps.deletion_tasks().unwrap().is_empty());
    assert!(
        crate::node_deletion_journal::NodeDeletionJournal::list(&fixture.data)
            .unwrap()
            .is_empty()
    );
}

type Tree = BTreeMap<PathBuf, (native_files::FileIdentity, Option<Vec<u8>>)>;
fn tree(root: &Path) -> Tree {
    fn visit(root: &Path, directory: &Path, out: &mut Tree) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let opened = native_files::open_no_reparse(&path).unwrap();
            let directory = opened.identity.directory;
            let bytes = if directory {
                None
            } else {
                Some(fs::read(&path).unwrap())
            };
            out.insert(
                path.strip_prefix(root).unwrap().into(),
                (opened.identity, bytes),
            );
            if directory {
                visit(root, &path, out);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
fn assert_tree(root: &Path, expected: &Tree) {
    let actual = tree(root);
    assert_eq!(actual.len(), expected.len());
    for (path, (identity, bytes)) in expected {
        let (current, current_bytes) = actual.get(path).unwrap();
        assert!(identity.same_object(current), "{}", path.display());
        assert_eq!(bytes, current_bytes, "{}", path.display());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Explicit pinned PANDA_W3_OFFICIAL_NODE; actual held file failure and new-manager durable deletion recovery"]
async fn official_occupied_owner_failure_is_preserved_and_new_manager_resumes_the_same_captured_task(
) {
    let fixture = DeletionFixture::new();
    let apps = fixture.manager();
    let id = fixture.generate(&apps).await;
    let process = fixture.run(&apps, &id).await;
    let paths = owned(&apps, &id);
    let owner = apps.project_ownership().directory(&id).unwrap();
    let blocked = owner.join("disposable-open-diagnostic");
    fs::write(&blocked, b"genuine held owner file").unwrap();
    let retained = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&blocked)
        .unwrap();
    let blocked_id = PersistedIdentity::from_file(&retained).unwrap();
    let submitted = apps
        .submit_deletion(&id, input(&apps, &id).await)
        .await
        .unwrap();
    let task_id = submitted["taskId"].as_str().unwrap().to_owned();
    let failed = terminal(&apps, &task_id, &id).await;
    assert_eq!(failed["phase"], "failed", "{failed}");
    assert_eq!(failed["canRetry"], true);
    assert_eq!(
        failed["renamedAny"], true,
        "the first private slot must already be captured before the blocked final owner"
    );
    assert!(!failed["remaining"].as_array().unwrap().is_empty());
    assert_eq!(
        failed["error"]["details"]["operationOutcome"],
        "delete_incomplete"
    );
    assert!(blocked_id.matches_open_runtime(&retained).unwrap());
    tokio::time::timeout(LIMIT, apps.wait_deletion_workers())
        .await
        .unwrap()
        .unwrap();
    stopped(&[process]).await;
    close(&apps).await;
    let journal_directory = fixture.data.join("node-deletion-tasks").join(&task_id);
    let journal_id = PersistedIdentity::capture(&journal_directory).unwrap();
    let persisted = fs::read(journal_directory.join("task.json")).unwrap();
    drop(apps);
    let recovered = fixture.manager();
    let loaded = recovered.deletion_tasks().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0]["taskId"], task_id);
    assert!(journal_id.matches_path(&journal_directory).unwrap());
    assert_eq!(
        fs::read(journal_directory.join("task.json")).unwrap(),
        persisted
    );
    assert!(blocked_id.matches_open_runtime(&retained).unwrap());
    drop(retained);
    recovered.resume_deletions().await.unwrap();
    // resume admits the real owner synchronously, while the retained cached
    // phase may still be the previous failure. Wait for that owner to finish
    // before reading the final result instead of accepting the old snapshot.
    tokio::time::timeout(LIMIT, recovered.wait_deletion_workers())
        .await
        .expect("the resumed deletion owner must actually finish")
        .unwrap();
    let result = tokio::time::timeout(LIMIT, recovered.wait_deletion_result(&task_id, &id))
        .await
        .unwrap();
    terminal(&recovered, &task_id, &id).await;
    close(&recovered).await;
    assert_eq!(result.unwrap()["operationOutcome"], "deleted");
    fixture.assert_deleted(&recovered, &id, &paths);
    fs::create_dir(&fixture.source).unwrap();
    fs::write(
        fixture.source.join("later-independent.txt"),
        b"do not replay an old task",
    )
    .unwrap();
    let later = PersistedIdentity::capture(&fixture.source).unwrap();
    recovered.resume_deletions().await.unwrap();
    let again = recovered
        .retry_deletion(&task_id, &id, failed["journalRevision"].as_u64().unwrap())
        .await
        .unwrap();
    assert_eq!(again["result"]["alreadyAbsent"], true);
    assert!(later.matches_path(&fixture.source).unwrap());
    assert_eq!(
        fs::read(fixture.source.join("later-independent.txt")).unwrap(),
        b"do not replay an old task"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Explicit pinned PANDA_W3_OFFICIAL_NODE; public abandon before any namespace move, no automatic restart"]
async fn official_abandon_before_rename_preserves_original_objects_and_never_restarts_the_backend()
{
    let fixture = DeletionFixture::new();
    let apps = fixture.manager();
    let id = fixture.generate(&apps).await;
    let process = fixture.run(&apps, &id).await;
    let paths = owned(&apps, &id);
    let mut pending_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(fixture.source.join("abandon-admitted-write.txt"))
        .unwrap();
    pending_file
        .write_all(b"genuine admitted file before abandonment")
        .unwrap();
    pending_file.sync_all().unwrap();
    let source_before = tree(&fixture.source);
    let writer = apps
        .admit_operation(&id, "genuine pending disposable file admission")
        .unwrap();
    let submitted = apps
        .submit_deletion(&id, input(&apps, &id).await)
        .await
        .unwrap();
    let task_id = submitted["taskId"].as_str().unwrap();
    assert_eq!(submitted["phase"], "waiting");
    apps.abandon_deletion(task_id, &id, submitted["journalRevision"].as_u64().unwrap())
        .await
        .unwrap();
    let abandoned = terminal(&apps, task_id, &id).await;
    drop(pending_file);
    drop(writer);
    assert_eq!(abandoned["phase"], "abandoned", "{abandoned}");
    assert_eq!(abandoned["renamedAny"], false);
    stopped(&[process]).await;
    for (path, identity) in paths {
        assert!(identity.matches_path(&path).unwrap(), "{}", path.display());
    }
    assert_tree(&fixture.source, &source_before);
    assert_eq!(
        fs::read_to_string(fixture.source.join("data/w5-launch-count")).unwrap(),
        "1"
    );
    assert!(apps.running_ids().await.is_empty());
    assert!(apps
        .admit_operation(&id, "later explicit user admission")
        .is_ok());
    assert!(apps.deletion_tasks().unwrap().is_empty());
    assert!(
        crate::node_deletion_journal::NodeDeletionJournal::list(&fixture.data)
            .unwrap()
            .is_empty()
    );
    close(&apps).await;
    fixture.backend_only_absent(&id);
}

const DELETION_HOST: &str =
    "node_apps::windows_runtime_tests::deletion::disposable_captured_deletion_host";
const DELETION_HOST_ROOT: &str = "PANDA_DISPOSABLE_W5_DELETION_HOST_ROOT";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeletionHostReady {
    app_id: String,
    task_id: String,
    process: CapturedProcess,
    owned: Vec<(PathBuf, PersistedIdentity)>,
    journal_identity: PersistedIdentity,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Exact child only; requires a named empty parent-owned TempDir and the pinned official Node"]
async fn disposable_captured_deletion_host() {
    let root = PathBuf::from(
        std::env::var_os(DELETION_HOST_ROOT).expect("explicit disposable child root"),
    )
    .canonicalize()
    .unwrap();
    let fixture = DeletionFixture::create(root.clone(), None);
    let apps = fixture.manager();
    let id = fixture.generate(&apps).await;
    let process = fixture.run(&apps, &id).await;
    let paths = owned(&apps, &id);
    let blocked = apps
        .project_ownership()
        .directory(&id)
        .unwrap()
        .join("held-until-original-host-exits");
    fs::write(&blocked, b"genuine child-host file lock").unwrap();
    let retained = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&blocked)
        .unwrap();
    let task = apps
        .submit_deletion(&id, input(&apps, &id).await)
        .await
        .unwrap();
    let task_id = task["taskId"].as_str().unwrap().to_owned();
    let failed = terminal(&apps, &task_id, &id).await;
    assert_eq!(failed["phase"], "failed", "{failed}");
    assert_eq!(failed["renamedAny"], true);
    assert_eq!(failed["canRetry"], true);
    assert!(!failed["remaining"].as_array().unwrap().is_empty());
    let journal = fixture.data.join("node-deletion-tasks").join(&task_id);
    write_new_json(
        &root.join("captured-deletion-ready.json"),
        &DeletionHostReady {
            app_id: id,
            task_id,
            process,
            owned: paths,
            journal_identity: PersistedIdentity::capture(&journal).unwrap(),
        },
    )
    .unwrap();
    // The parent terminates this original host HANDLE after observing the
    // actual capture and persisted failure. No Rust Drop closes this lock or
    // finalizes the task. This does not claim a crash during every CAS syscall.
    std::future::pending::<()>().await;
    drop(retained);
    drop(apps);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Pinned official PANDA_W3_OFFICIAL_NODE and an actual bounded abrupt host; backend-only, no WebView"]
async fn abrupt_original_host_exit_preserves_the_real_captured_task_for_public_new_manager_recovery(
) {
    let node = official_node();
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w5-nodeapps-delete-parent-")
        .tempdir()
        .unwrap();
    let base = temporary.path().canonicalize().unwrap();
    let root = base.join("panda-disposable-w5-nodeapps-delete-abrupt-child-root 中文 space");
    fs::create_dir(&root).unwrap();
    let log = File::create(base.join("actual-deletion-host.log")).unwrap();
    let mut host = RescueHost(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                DELETION_HOST,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(DELETION_HOST_ROOT, &root)
            .env("PANDA_W3_OFFICIAL_NODE", &node)
            .env_remove("TERM") // avoid the libtest Unix terminfo UNC lookup, fixture only
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let ready = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let Some(ready) = fs::read(root.join("captured-deletion-ready.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<DeletionHostReady>(&bytes).ok())
            {
                break ready;
            }
            assert!(
                host.0.try_wait().unwrap().is_none(),
                "host exited before durable capture: {}",
                fs::read_to_string(base.join("actual-deletion-host.log")).unwrap()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bounded official child host must reach a real captured deletion task");
    let data = root.join("disposable-platform-data");
    let journal = data.join("node-deletion-tasks").join(&ready.task_id);
    assert!(ready.journal_identity.matches_path(&journal).unwrap());
    let before = fs::read(journal.join("task.json")).unwrap();
    host.0.kill().unwrap();
    assert!(!host.0.wait().unwrap().success());
    stopped(&[ready.process]).await;
    let recovered = NodeApps::new(data.clone(), node).unwrap();
    let loaded = recovered.deletion_tasks().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0]["taskId"], ready.task_id);
    assert_eq!(loaded[0]["appId"], ready.app_id);
    assert!(ready.journal_identity.matches_path(&journal).unwrap());
    assert_eq!(fs::read(journal.join("task.json")).unwrap(), before);
    recovered.resume_deletions().await.unwrap();
    tokio::time::timeout(LIMIT, recovered.wait_deletion_workers())
        .await
        .expect("the interrupted host's resumed deletion owner must actually finish")
        .unwrap();
    let result = tokio::time::timeout(
        LIMIT,
        recovered.wait_deletion_result(&ready.task_id, &ready.app_id),
    )
    .await
    .unwrap();
    terminal(&recovered, &ready.task_id, &ready.app_id).await;
    close(&recovered).await;
    assert_eq!(result.unwrap()["operationOutcome"], "deleted");
    for (path, identity) in ready.owned {
        assert!(
            path.starts_with(&root),
            "all deletion authority must remain inside this named disposable root"
        );
        assert!(
            identity.absent_on_expected_volume(&path).unwrap(),
            "{}",
            path.display()
        );
    }
    assert!(
        crate::node_deletion_journal::NodeDeletionJournal::list(&data)
            .unwrap()
            .is_empty()
    );
    assert!(!crate::webview_storage::StoreLedger::new(data)
        .contains(crate::project_cleanup::ProjectKind::Node, &ready.app_id)
        .unwrap());
    assert!(recovered.deletion_tasks().unwrap().is_empty());
}
