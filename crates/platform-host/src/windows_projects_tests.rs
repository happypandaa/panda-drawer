//! Synthetic bundles and new disposable NTFS roots only. No application root
//! override exists in production; all filesystem tests use the real backend.
use super::*;
use std::{fs, path::Path, sync::MutexGuard};

pub(super) fn test_bundle() -> BundledProject {
    let mut files = BTreeMap::from([
        (
            "template/app/package.json".into(),
            br#"{"name":"synthetic","scripts":{"build":"synthetic-only"}}"#.to_vec(),
        ),
        (
            "template/app/package-lock.json".into(),
            br#"{"lockfileVersion":3}"#.to_vec(),
        ),
        (
            "template/app/src/App.tsx".into(),
            b"export default function App(){return 'Windows sample'}".to_vec(),
        ),
        (
            "template/app/README.md".into(),
            b"Synthetic source only".to_vec(),
        ),
        (
            "template/app/vendor/local-projects-sdk-0.1.0.tgz".into(),
            b"synthetic pinned sdk bytes".to_vec(),
        ),
        (
            "sdk/releases/local-projects-sdk-0.1.0.tgz".into(),
            b"synthetic pinned sdk bytes".to_vec(),
        ),
        (
            "sdk/README.md".into(),
            b"Synthetic unchanged SDK guide".to_vec(),
        ),
        (
            "skills/local-projects-app/SKILL.md".into(),
            b"Synthetic project-owned skill".to_vec(),
        ),
        (
            "template/starter/index.html".into(),
            b"<!doctype html><html><head></head><body>Windows sample</body></html>".to_vec(),
        ),
    ]);
    let distribution = StarterDistribution {
        schema_version: 1,
        origin: "bundledTemplate".into(),
        template_version: "synthetic-w2".into(),
        sdk_version: "0.1.0".into(),
        source_digest: tree_digest(&tree(&files, "template/app/")),
        artifact_digest: tree_digest(&tree(&files, "template/starter/")),
        entry_relative: "index.html".into(),
        built_at: 1,
        node_version: "synthetic-packaging-only".into(),
        npm_version: "synthetic-packaging-only".into(),
    };
    files.insert(
        "template/starter.json".into(),
        serde_json::to_vec(&distribution).unwrap(),
    );
    files.insert(
        "template/template.json".into(),
        serde_json::to_vec(&json!({"templateVersion":"synthetic-w2","sdkVersion":"0.1.0"}))
            .unwrap(),
    );
    BundledProject {
        files: files.into_iter().collect(),
    }
}

fn versioned_test_bundle(version: &str, label: &str) -> BundledProject {
    // Different synthetic source/artifact pairs exercise compatibility without
    // claiming that an actual GUI, the real application or its SDK was run.
    let mut files: BTreeMap<_, _> = test_bundle().files.into_iter().collect();
    files.insert(
        "template/app/src/App.tsx".into(),
        format!("export default function App(){{return '{label}'}}").into_bytes(),
    );
    files.insert(
        "template/starter/index.html".into(),
        format!("<!doctype html><html><head></head><body>{label}</body></html>").into_bytes(),
    );
    let mut distribution: StarterDistribution =
        strict_json(&files["template/starter.json"]).unwrap();
    distribution.template_version = version.into();
    distribution.source_digest = tree_digest(&tree(&files, "template/app/"));
    distribution.artifact_digest = tree_digest(&tree(&files, "template/starter/"));
    files.insert(
        "template/starter.json".into(),
        serde_json::to_vec(&distribution).unwrap(),
    );
    files.insert(
        "template/template.json".into(),
        serde_json::to_vec(&json!({"templateVersion":version,"sdkVersion":"0.1.0"})).unwrap(),
    );
    BundledProject {
        files: files.into_iter().collect(),
    }
}

fn sample_snapshot(store: &WindowsProjectStore, sample: &ProjectInfo) -> BTreeMap<String, Vec<u8>> {
    let active = store.active.lock().unwrap();
    let workspace = &active.as_ref().unwrap().workspace;
    let root = workspace.root();
    let work = root.open_directory("workspace").unwrap();
    let project = work
        .open_directory("projects")
        .unwrap()
        .open_directory(&sample.id)
        .unwrap();
    let mut result: BTreeMap<_, _> = collect(&project)
        .unwrap()
        .into_iter()
        .map(|(path, (bytes, _))| (format!("project/{path}"), bytes))
        .collect();
    let platform = work.open_directory(".platform").unwrap();
    let state = platform.open_directory("project-state").unwrap();
    result.insert(
        "registration.json".into(),
        state
            .read_bounded(&format!("{}.json", sample.id), FILE_LIMIT)
            .unwrap()
            .bytes,
    );
    result.insert(
        "root/settings.json".into(),
        root.read_bounded("settings.json", FILE_LIMIT)
            .unwrap()
            .bytes,
    );
    result.insert(
        "root/.environment.json".into(),
        root.read_bounded(".environment.json", FILE_LIMIT)
            .unwrap()
            .bytes,
    );
    result.insert(
        "workspace/.platform/windows-stage.json".into(),
        platform
            .read_bounded("windows-stage.json", FILE_LIMIT)
            .unwrap()
            .bytes,
    );
    result
}

fn serial() -> MutexGuard<'static, ()> {
    crate::WINDOWS_FILESYSTEM_TESTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn fixture_parent() -> PathBuf {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let artifacts = repository.join(".test-artifacts");
    fs::create_dir_all(&artifacts).unwrap();
    use std::os::windows::fs::MetadataExt;
    assert_eq!(
        fs::symlink_metadata(&artifacts).unwrap().file_attributes() & 0x400,
        0
    );
    let path = artifacts.join(format!(
        "windows-workspace-disposable-{}",
        Uuid::new_v4().simple()
    ));
    crate::windows_workspace::create_private_fixture_directory(&path).unwrap();
    path
}

fn create(store: &WindowsProjectStore, name: &str) -> ProjectInfo {
    store
        .create(CreateProjectInput {
            name: name.into(),
            description: Some("Disposable synthetic W2.1 sample".into()),
        })
        .unwrap()
}

#[test]
fn trusted_bundle_validation_rejects_mismatched_source_sdk_and_name_collisions() {
    assert!(WindowsProjectStore::new(test_bundle()).is_ok());
    for path in [
        "template/app/src/App.tsx",
        "template/starter/index.html",
        "template/app/vendor/local-projects-sdk-0.1.0.tgz",
    ] {
        let mut bundle = test_bundle();
        bundle
            .files
            .iter_mut()
            .find(|(name, _)| name == path)
            .unwrap()
            .1
            .push(b'!');
        assert_eq!(
            WindowsProjectStore::new(bundle).err().unwrap().code,
            "INVALID_TEMPLATE"
        );
    }
    let mut bundle = test_bundle();
    bundle
        .files
        .push(("template/app/PACKAGE.json".into(), b"{}".to_vec()));
    assert_eq!(
        WindowsProjectStore::new(bundle).err().unwrap().code,
        "INVALID_TEMPLATE"
    );
    for path in [
        "../outside",
        "app/../outside",
        "C:/outside",
        "app/NUL.txt",
        "app/stream:alias",
        "app/a.",
    ] {
        let mut bundle = test_bundle();
        bundle.files.push((path.into(), vec![]));
        assert_eq!(
            WindowsProjectStore::new(bundle).err().unwrap().code,
            "INVALID_PATH"
        );
    }
}

#[test]
fn sqlite_schema_is_a_real_readonly_snapshot_and_never_fakes_external_tables_as_empty() {
    let bytes = empty_database().unwrap();
    assert!(bytes.starts_with(b"SQLite format 3\0"));
    assert_eq!(
        empty_schema(&bytes).unwrap(),
        json!({"tables":[],"indexes":[],"views":[],"triggers":[]})
    );
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE externally_added(value TEXT)")
        .unwrap();
    let changed = connection.serialize(MAIN_DB).unwrap().to_vec();
    assert_eq!(
        empty_schema(&changed).unwrap_err().code,
        "W2_DATABASE_CHANGED"
    );
    assert!(empty_schema(b"not SQLite").is_err());
    assert!(empty_schema(&[]).is_err());
}

#[test]
fn nested_duplicate_control_keys_are_rejected_before_typed_map_decoding() {
    for raw in [
        br#"{"files":{"x":{"sha256":"first"},"x":{"sha256":"last"}}}"#.as_slice(),
        br#"{"files":{"x":1,"\u0078":2}}"#.as_slice(),
    ] {
        assert!(strict_json::<Value>(raw).is_err());
    }
}

#[test]
fn missing_root_is_not_created_by_constructor_list_open_or_schema() {
    let _serial = serial();
    let parent = fixture_parent();
    let store = WindowsProjectStore::for_test(test_bundle(), parent.clone()).unwrap();
    assert!(!parent.join(APP_IDENTIFIER).exists());
    assert_eq!(store.list().unwrap_err().code, "FIXED_ROOT_MISSING");
    assert_eq!(
        store
            .open("prj_0123456789abcdef0123456789abcdef")
            .unwrap_err()
            .code,
        "FIXED_ROOT_MISSING"
    );
    assert_eq!(
        store
            .schema("prj_0123456789abcdef0123456789abcdef")
            .unwrap_err()
            .code,
        "FIXED_ROOT_MISSING"
    );
    assert!(!parent.join(APP_IDENTIFIER).exists());
}

#[test]
fn explicit_create_persists_two_independent_samples_and_reopens_exact_source_sdk_and_schema() {
    let _serial = serial();
    let parent = fixture_parent();
    let store = WindowsProjectStore::for_test(test_bundle(), parent.clone()).unwrap();
    let a = create(&store, "中文 · 第一份样本");
    let b = create(&store, "第二份样本");
    assert_ne!(a.id, b.id);
    assert_ne!(a.document_epoch, b.document_epoch);
    assert_eq!(store.list().unwrap().len(), 2);
    assert_eq!(store.schema(&a.id).unwrap()["tables"], json!([]));
    assert_eq!(
        store.read_static(&a.id, "index.html").unwrap(),
        b"<!doctype html><html><head></head><body>Windows sample</body></html>"
    );
    assert!(store.read_static(&a.id, "../data/main.sqlite").is_err());
    let database = fs::read(a.project_root.join("data/main.sqlite")).unwrap();
    assert!(database.starts_with(b"SQLite format 3\0"));
    assert!(!a.project_root.join("data/main.sqlite-wal").exists());
    assert!(!a.project_root.join("data/main.sqlite-shm").exists());
    assert_eq!(
        fs::read(
            a.project_root
                .join("app/vendor/local-projects-sdk-0.1.0.tgz")
        )
        .unwrap(),
        b"synthetic pinned sdk bytes"
    );
    let registration = parent
        .join(APP_IDENTIFIER)
        .join("workspace/.platform/project-state")
        .join(format!("{}.json", a.id));
    let value: Value = strict_json(&fs::read(registration).unwrap()).unwrap();
    assert_eq!(value["identityKind"], "windowsFileId128");
    assert_eq!(
        value["databaseIdentity"]["fileId128"]
            .as_array()
            .unwrap()
            .len(),
        16
    );
    assert!(value["databaseIdentity"].get("inode").is_none());
    drop(store);
    let restarted = WindowsProjectStore::for_test(test_bundle(), parent).unwrap();
    assert_eq!(restarted.list().unwrap().len(), 2);
    assert_eq!(
        restarted.open(&a.id).unwrap().document_epoch,
        a.document_epoch
    );
    assert_eq!(
        fs::read(a.project_root.join("data/main.sqlite")).unwrap(),
        database
    );
}

#[test]
fn repackaging_the_host_does_not_rewrite_or_invalidate_existing_project_distribution() {
    let _serial = serial();
    let parent = fixture_parent();
    let store = WindowsProjectStore::for_test(test_bundle(), parent.clone()).unwrap();
    let sample = create(&store, "Disposable pinned distribution");
    let starter_path = sample.project_root.join(".platform/starter.json");
    let before = fs::read(&starter_path).unwrap();
    drop(store);
    let mut rebuilt = test_bundle();
    let (_, bytes) = rebuilt
        .files
        .iter_mut()
        .find(|(path, _)| path == "template/starter.json")
        .unwrap();
    let mut distribution: StarterDistribution = strict_json(bytes).unwrap();
    distribution.built_at += 10;
    distribution.npm_version = "repackaged-npm".into();
    *bytes = serde_json::to_vec(&distribution).unwrap();
    let restarted = WindowsProjectStore::for_test(rebuilt, parent).unwrap();
    assert_eq!(
        restarted.open(&sample.id).unwrap().document_epoch,
        sample.document_epoch
    );
    assert_eq!(fs::read(starter_path).unwrap(), before);
}

#[test]
fn welcome_and_split_bill_samples_keep_their_own_versions_and_bytes_across_bundle_switches() {
    let _serial = serial();
    let parent = fixture_parent();
    let welcome_bundle = versioned_test_bundle("react-ts-vite-0.1.4", "Synthetic welcome sample");
    let split_bundle =
        versioned_test_bundle("windows-split-bill-0.1.0", "Synthetic split bill sample");
    let original = WindowsProjectStore::for_test(welcome_bundle.clone(), parent.clone()).unwrap();
    let welcome = create(&original, "Disposable original welcome sample");
    let welcome_before = sample_snapshot(&original, &welcome);
    assert_eq!(
        original
            .open(&welcome.id)
            .unwrap()
            .starter
            .distribution
            .template_version,
        "react-ts-vite-0.1.4"
    );
    original.close().unwrap();
    drop(original);

    let updated = WindowsProjectStore::for_test(split_bundle.clone(), parent.clone()).unwrap();
    assert_eq!(
        updated.open(&welcome.id).unwrap().document_epoch,
        welcome.document_epoch
    );
    assert_eq!(sample_snapshot(&updated, &welcome), welcome_before);
    let split = create(&updated, "Disposable new split bill sample");
    let split_before = sample_snapshot(&updated, &split);
    assert_ne!(welcome.id, split.id);
    assert_ne!(welcome.document_epoch, split.document_epoch);
    assert_ne!(
        welcome_before["project/app/src/App.tsx"],
        split_before["project/app/src/App.tsx"]
    );
    assert_eq!(
        welcome_before["project/app/vendor/local-projects-sdk-0.1.0.tgz"],
        split_before["project/app/vendor/local-projects-sdk-0.1.0.tgz"]
    );
    assert_eq!(sample_snapshot(&updated, &welcome), welcome_before);
    updated.close().unwrap();
    drop(updated);

    // Both default-resource choices run this current backend. This does not
    // claim that an older executable understands a newly added template ID.
    for bundle in [welcome_bundle, split_bundle] {
        let restarted = WindowsProjectStore::for_test(bundle, parent.clone()).unwrap();
        let ids: BTreeSet<_> = restarted
            .list()
            .unwrap()
            .into_iter()
            .map(|sample| sample.id)
            .collect();
        assert_eq!(ids, BTreeSet::from([welcome.id.clone(), split.id.clone()]));
        for (sample, version, before) in [
            (&welcome, "react-ts-vite-0.1.4", &welcome_before),
            (&split, "windows-split-bill-0.1.0", &split_before),
        ] {
            let opened = restarted.open(&sample.id).unwrap();
            assert_eq!(opened.manifest.id, sample.id);
            assert_eq!(opened.document_epoch, sample.document_epoch);
            assert_eq!(opened.starter.distribution.template_version, version);
            assert_eq!(&sample_snapshot(&restarted, sample), before);
            assert_eq!(restarted.schema(&sample.id).unwrap()["tables"], json!([]));
            assert_eq!(
                &restarted.read_static(&sample.id, "index.html").unwrap(),
                &before["project/dist/index.html"]
            );
        }
        restarted.close().unwrap();
    }
}

#[test]
fn replacing_fixed_root_cannot_rebind_a_retained_workspace_by_copying_its_markers() {
    let _serial = serial();
    let parent = fixture_parent();
    let store = WindowsProjectStore::for_test(test_bundle(), parent.clone()).unwrap();
    let sample = create(&store, "Disposable root identity substitution");
    let root = parent.join(APP_IDENTIFIER);
    let marker = fs::read(root.join(".environment.json")).unwrap();
    let settings = fs::read(root.join("settings.json")).unwrap();
    let database = fs::read(sample.project_root.join("data/main.sqlite")).unwrap();
    drop(store);
    let retained = parent.join("retained-original-root");
    fs::rename(&root, &retained).unwrap();
    let replacement = WindowsWorkspace::for_test_path(parent.clone(), true).unwrap();
    let directory = replacement.root();
    directory.write_new(".environment.json", &marker).unwrap();
    directory.write_new("settings.json", &settings).unwrap();
    directory.create_directory("runtime").unwrap();
    directory.create_directory("migrations").unwrap();
    {
        let lock = directory.lock_file("instance.lock").unwrap();
        // Simulate an external replacement in a disposable fixture. The old
        // workspace keeps all its project/DB IDs and its copied settings path.
        fs::rename(retained.join("workspace"), root.join("workspace")).unwrap();
        drop(lock);
    }
    drop(directory);
    drop(replacement);
    let restarted = WindowsProjectStore::for_test(test_bundle(), parent).unwrap();
    assert_eq!(
        restarted.list().unwrap_err().code,
        "W2_ROOT_IDENTITY_MISMATCH"
    );
    assert_eq!(
        fs::read(sample.project_root.join("data/main.sqlite")).unwrap(),
        database
    );
    assert_eq!(fs::read(root.join("settings.json")).unwrap(), settings);
}

#[test]
fn missing_nullable_settings_fields_are_not_silently_defaulted() {
    let _serial = serial();
    let parent = fixture_parent();
    let store = WindowsProjectStore::for_test(test_bundle(), parent.clone()).unwrap();
    create(&store, "Disposable incomplete settings");
    let path = parent.join(APP_IDENTIFIER).join("settings.json");
    let mut settings: Value = strict_json(&fs::read(&path).unwrap()).unwrap();
    settings.as_object_mut().unwrap().remove("migrationId");
    let bytes = serde_json::to_vec(&settings).unwrap();
    fs::write(&path, &bytes).unwrap();
    assert_eq!(store.list().unwrap_err().code, "W2_RECORD_INVALID");
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn an_existing_unrecognized_root_is_preserved_without_creating_even_a_lock_file() {
    let _serial = serial();
    let parent = fixture_parent();
    let old = WindowsWorkspace::for_test_path(parent.clone(), true).unwrap();
    old.root()
        .write_new("retained-evidence", b"must remain unchanged")
        .unwrap();
    let root = old.path().to_owned();
    drop(old);
    let store = WindowsProjectStore::for_test(test_bundle(), parent).unwrap();
    assert!(store.list().is_err());
    let error = store
        .create(CreateProjectInput {
            name: "No adoption".into(),
            description: None,
        })
        .unwrap_err();
    assert_ne!(error.details["operationOutcome"], "not_started");
    assert_eq!(
        fs::read(root.join("retained-evidence")).unwrap(),
        b"must remain unchanged"
    );
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    assert!(!root.join("instance.lock").exists());
}

#[test]
fn unknown_empty_directories_and_unregistered_publish_residuals_block_instead_of_autohealing() {
    let _serial = serial();
    let parent = fixture_parent();
    let store = WindowsProjectStore::for_test(test_bundle(), parent.clone()).unwrap();
    let sample = create(&store, "Disposable retained directory");
    {
        let active = store.active.lock().unwrap();
        active
            .as_ref()
            .unwrap()
            .workspace
            .root()
            .open_directory("workspace")
            .unwrap()
            .open_directory("projects")
            .unwrap()
            .open_directory(&sample.id)
            .unwrap()
            .create_directory("unknown-empty-directory")
            .unwrap();
    }
    assert_eq!(store.list().unwrap_err().code, "W2_RETAINED_EVIDENCE");
    assert!(sample.project_root.join("unknown-empty-directory").is_dir());
    drop(store);
    let parent = fixture_parent();
    let store = WindowsProjectStore::for_test(test_bundle(), parent.clone()).unwrap();
    create(&store, "Disposable publish residual");
    let unregistered = parent
        .join(APP_IDENTIFIER)
        .join("workspace/projects/prj_0123456789abcdef0123456789abcdef");
    fs::create_dir(&unregistered).unwrap();
    assert_eq!(store.list().unwrap_err().code, "W2_RETAINED_EVIDENCE");
    assert_eq!(fs::read_dir(unregistered).unwrap().count(), 0);
}

#[test]
fn changed_source_or_database_is_blocked_and_not_rewritten() {
    let _serial = serial();
    for relative in ["app/src/App.tsx", "data/main.sqlite"] {
        let parent = fixture_parent();
        let store = WindowsProjectStore::for_test(test_bundle(), parent).unwrap();
        let sample = create(&store, "Disposable changed evidence");
        let path = sample.project_root.join(relative);
        fs::write(&path, b"synthetic external change").unwrap();
        assert!(store.open(&sample.id).is_err());
        assert!(store.schema(&sample.id).is_err());
        assert_eq!(fs::read(path).unwrap(), b"synthetic external change");
    }
}

#[test]
fn strict_registration_map_decoding_rejects_duplicate_inventory_and_preserves_original_bytes() {
    let _serial = serial();
    let parent = fixture_parent();
    let store = WindowsProjectStore::for_test(test_bundle(), parent.clone()).unwrap();
    let sample = create(&store, "Disposable duplicate map");
    let path = parent
        .join(APP_IDENTIFIER)
        .join("workspace/.platform/project-state")
        .join(format!("{}.json", sample.id));
    let record: Value = strict_json(&fs::read(&path).unwrap()).unwrap();
    let inventory = &record["files"];
    let (key, stamp) = inventory.as_object().unwrap().iter().next().unwrap();
    let serialized_inventory = serde_json::to_string(inventory).unwrap();
    let duplicate = format!(
        "{{{}:{},{}",
        serde_json::to_string(key).unwrap(),
        serde_json::to_string(stamp).unwrap(),
        &serialized_inventory[1..]
    );
    let bytes = serde_json::to_string(&record)
        .unwrap()
        .replace(&serialized_inventory, &duplicate)
        .into_bytes();
    fs::write(&path, &bytes).unwrap();
    assert_eq!(store.list().unwrap_err().code, "W2_RECORD_INVALID");
    assert_eq!(fs::read(path).unwrap(), bytes);
}
