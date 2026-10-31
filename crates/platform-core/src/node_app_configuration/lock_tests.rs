use super::*;
use std::{
    process::{Command, Stdio},
    sync::Barrier,
    time::{Duration, Instant},
};

const CONTENTION_HOST: &str =
    "node_app_configuration::windows_lock_tests::disposable_configuration_contention_host";
const CONTENTION_ROOT: &str = "PANDA_DISPOSABLE_CONFIGURATION_CONTENTION_ROOT";

fn disposable_store(directory: PathBuf) -> NodeConfigurationStore {
    let instance = Uuid::new_v4();
    let vault = Arc::new(LocalFileVault::new(directory.join("credentials"), instance).unwrap());
    NodeConfigurationStore::new(directory, instance, vault).unwrap()
}

#[test]
fn actual_configuration_lock_is_unlocked_before_its_last_duplicate_handle_closes() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-config-explicit-unlock-")
        .tempdir()
        .unwrap();
    let directory = temporary.path().canonicalize().unwrap();
    let store = disposable_store(directory.clone());
    let owner = store.lock().unwrap();
    let duplicate = owner.0.try_clone().unwrap();
    let contender =
        open_configuration_file(&directory.join("configuration.lock"), true, false).unwrap();
    assert!(fs2::FileExt::try_lock_exclusive(&contender).is_err());
    assert!(configuration_native::identity(&duplicate)
        .unwrap()
        .same_object(&configuration_native::identity(&contender).unwrap()));

    // Retaining the duplicate makes CloseHandle-only release insufficient.
    // This proves that the guard issues a real unlock before closing, rather
    // than depending on eventual OS cleanup of the last HANDLE.
    drop(owner);
    fs2::FileExt::try_lock_exclusive(&contender).unwrap();
    fs2::FileExt::unlock(&contender).unwrap();
    drop(duplicate);
}

#[test]
fn actual_concurrent_save_and_view_complete_with_all_worker_threads_joined() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-config-contention-")
        .tempdir()
        .unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CONTENTION_HOST, "--ignored", "--nocapture"])
        .env(CONTENTION_ROOT, temporary.path())
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "disposable contention host failed");
            return;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("disposable configuration contention host did not finish");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "Only the bounded disposable configuration contention parent starts this host"]
fn disposable_configuration_contention_host() {
    let directory = PathBuf::from(std::env::var_os(CONTENTION_ROOT).unwrap())
        .canonicalize()
        .unwrap();
    assert!(directory
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("panda-disposable-config-contention-"));
    let store = Arc::new(disposable_store(directory));
    let declaration = crate::node_app_manifest::TrialManifest::parse_runtime(
        &serde_json::to_vec(&serde_json::json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":"disposable.config.contention","name":"Disposable configuration","version":"1"},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"protocolVersion":1,"root":"server","entry":"main.cjs","nodeMajor":24},
            "configuration":{"version":1,"fields":[
                {"name":"GREETING","label":"Greeting","type":"string","required":false,"secret":false,
                 "description":"Artificial ordinary value","howToGet":["Use test text"]},
                {"name":"TEST_SECRET","label":"Secret","type":"string","required":true,"secret":true,
                 "description":"Artificial fixture secret","howToGet":["Use test fixture only"]}
            ]}
        }))
        .unwrap(),
    )
    .unwrap();
    let declaration = Arc::new(declaration);
    let request = |revision| SaveConfigurationRequest {
        revision,
        manifest_digest: None,
        values: BTreeMap::from([("GREETING".into(), Value::String("fixture".into()))]),
        secrets: BTreeMap::from([(
            "TEST_SECRET".into(),
            SecretAction::Replace {
                value: "artificial-disposable-contention-secret".into(),
            },
        )]),
    };
    store.save(&declaration, request(0)).unwrap();
    let barrier = Arc::new(Barrier::new(4));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let store = store.clone();
        let declaration = declaration.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..96 {
                let view = store.view(&declaration).unwrap();
                assert!(view.revision >= 1);
                assert_eq!(view.secrets["TEST_SECRET"], SecretStatus::Set);
            }
        }));
    }
    let writer_store = store.clone();
    let writer_declaration = declaration.clone();
    let writer_barrier = barrier.clone();
    workers.push(std::thread::spawn(move || {
        writer_barrier.wait();
        for revision in 1..=32 {
            writer_store
                .save(&writer_declaration, request(revision))
                .unwrap();
            assert!(writer_store.view(&writer_declaration).unwrap().can_start);
        }
    }));
    barrier.wait();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(store.view(&declaration).unwrap().revision, 33);
}
