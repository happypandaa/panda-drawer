use super::*;
use crate::{
    builds::BuildManager, git_environment::GitEnvironmentManager,
    node_project_ownership::NodeProjectOwnership, ProjectStore, RuntimePaths,
};
use std::{path::Path, sync::Arc};

fn sample() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("panda-disposable-source-publication-中文 space-")
        .tempdir()
        .unwrap()
}

type Fixture = (
    tempfile::TempDir,
    RuntimePaths,
    Arc<ProjectStore>,
    Arc<BuildManager>,
    Arc<SourceAcquisitionManager>,
);
fn fixture() -> Fixture {
    let (temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1".into()).unwrap();
    let git =
        GitEnvironmentManager::with_processes(paths.data_root().into(), None, store.processes());
    let manager =
        SourceAcquisitionManager::new(paths.clone(), store.clone(), tasks.clone(), git).unwrap();
    (temporary, paths, store, tasks, manager)
}

fn ready_source(manager: &SourceAcquisitionManager, tasks: &BuildManager) -> SourceCandidate {
    let task = tasks.create_workspace_operation("sourceAcquire").unwrap();
    let id = format!("src_{}", Uuid::new_v4().simple());
    private_dir(&manager.root()).unwrap();
    let slot = manager.root().join(&id);
    new_dir(&slot).unwrap();
    fs::create_dir_all(slot.join("repository/.git/objects")).unwrap();
    fs::write(slot.join("repository/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(slot.join("repository/app.js"), "disposable author source").unwrap();
    let record = SourceRecord {
        schema_version: 1,
        source_id: id.clone(),
        owner_app_id: Uuid::new_v4().to_string(),
        task_id: task.task_id.clone(),
        name: "可丢弃·源码发布".into(),
        status: "ready".into(),
        phase: "completed".into(),
        url: "https://github.com/example/disposable-repo.git".into(),
        branch: None,
        provider: "github".into(),
        created_at: 1,
        completed_at: Some(2),
        repository: "repository".into(),
        shallow: true,
        git: None,
        commit: Some("a".repeat(40)),
        file_count: Some(1),
        total_bytes: Some(24),
        content_digest: Some(directory_digest(&slot.join("repository")).unwrap()),
        inspection: None,
        error: None,
    };
    let ownership = owner(manager);
    ownership.initialize(&record.owner_app_id).unwrap();
    ownership
        .record_task(&record.owner_app_id, &record.task_id)
        .unwrap();
    ownership
        .record_source(&record.owner_app_id, &record.source_id)
        .unwrap();
    manager.save_record(&record).unwrap();
    let candidate = manager.view(manager.read_record(&id).unwrap()).unwrap();
    tasks
        .finish(&task.task_id, Ok(serde_json::json!(candidate)))
        .unwrap();
    candidate
}

fn owner(manager: &SourceAcquisitionManager) -> NodeProjectOwnership {
    NodeProjectOwnership::new(
        manager.paths.data_root().into(),
        manager.paths.workspace_root().into(),
    )
}

fn restarted(
    paths: &RuntimePaths,
    store: &Arc<ProjectStore>,
    tasks: &Arc<BuildManager>,
) -> Arc<SourceAcquisitionManager> {
    SourceAcquisitionManager::new(
        paths.clone(),
        store.clone(),
        tasks.clone(),
        GitEnvironmentManager::with_processes(paths.data_root().into(), None, store.processes()),
    )
    .unwrap()
}

fn journal(manager: &SourceAcquisitionManager, app: &str) -> PathBuf {
    manager
        .paths
        .workspace_root()
        .join(".platform/source-publications")
        .join(app)
}

fn publication(manager: &SourceAcquisitionManager, app: &str) -> Publication {
    serde_json::from_slice(&fs::read(journal(manager, app).join("publication.json")).unwrap())
        .unwrap()
}

fn author_reservation(
    manager: &SourceAcquisitionManager,
    app: &str,
    directory: &str,
) -> serde_json::Value {
    let path = manager
        .paths
        .data_root()
        .join("node-project-ownership")
        .join(app)
        .join("ownership.json");
    let record: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    record["artifacts"][format!("author:{directory}")].clone()
}

fn prepared(
    manager: &SourceAcquisitionManager,
    source: &SourceCandidate,
    app: &str,
) -> Publication {
    let workspace = Anchor::open(manager.paths.workspace_root().into()).unwrap();
    let root = workspace.child("project-sources", true).unwrap();
    owner(manager).record_publication(app).unwrap();
    let receipt = workspace
        .child(".platform", false)
        .unwrap()
        .child("source-publications", true)
        .unwrap()
        .child(app, true)
        .unwrap();
    owner(manager).record_publication(app).unwrap();
    let original = Anchor::open(source.source_path.clone().unwrap()).unwrap();
    let (payload, stage) = fresh_payload(&receipt).unwrap();
    let digest = author_digest(&original.file, Some(&stage.file)).unwrap();
    assert_eq!(digest, author_digest(&stage.file, None).unwrap());
    let publication = Publication {
        schema_version: 1,
        source_id: source.source_id.clone(),
        source_task_id: source.task_id.clone(),
        source_digest: source.content_digest.clone().unwrap(),
        author_digest: digest,
        app_id: app.into(),
        directory: available_name(&root.file, "disposable-repo").unwrap(),
        phase: "ready".into(),
        payload,
        payload_identity: PersistedIdentity::from_file(&stage.file).unwrap(),
    };
    save_publication(&receipt, &mut None, &publication).unwrap();
    publication
}

fn junction(link: &Path, target: &Path) {
    assert!(link.is_absolute() && target.is_absolute());
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn actual_copy_preserves_readonly_unicode_git_and_original_identity() {
    let temporary = sample();
    let base = temporary.path().canonicalize().unwrap();
    fs::create_dir_all(base.join("source/.git/objects")).unwrap();
    fs::create_dir(base.join("target")).unwrap();
    fs::write(base.join("source/.git/HEAD"), b"ref: refs/heads/main\n").unwrap();
    let path = base.join("source/中文 space.js");
    fs::write(&path, vec![b'a'; 128 * 1024 + 3]).unwrap();
    native::set_read_only(&path, true).unwrap();
    let before = PersistedIdentity::capture(&path).unwrap();
    let source = Anchor::open(base.join("source")).unwrap();
    let target = Anchor::open(base.join("target")).unwrap();
    let (digest, bytes) = author_snapshot(&source.file, Some(&target.file)).unwrap();
    assert_eq!(digest, author_digest(&source.file, None).unwrap());
    assert_eq!(digest, author_digest(&target.file, None).unwrap());
    assert_eq!(
        bytes,
        (128 * 1024 + 3 + b"ref: refs/heads/main\n".len()) as u64
    );
    assert!(before.matches_path(&path).unwrap());
    assert!(!before
        .matches_path(&base.join("target/中文 space.js"))
        .unwrap());
    assert!(fs::metadata(base.join("target/中文 space.js"))
        .unwrap()
        .permissions()
        .readonly());
    native::verify_private(&native::open_read_file(&base.join("target/中文 space.js")).unwrap())
        .unwrap();
    assert_eq!(
        fs::read(base.join("target/.git/HEAD")).unwrap(),
        b"ref: refs/heads/main\n"
    );
    native::set_read_only(&path, false).unwrap();
    native::set_read_only(&base.join("target/中文 space.js"), false).unwrap();
}

#[test]
fn actual_copy_handles_nested_unicode_paths_beyond_max_path_without_changing_budgets() {
    let temporary = sample();
    let base = temporary.path().canonicalize().unwrap();
    let mut relative = PathBuf::new();
    for depth in 0..8 {
        relative.push(format!("目录-{depth}-{}", "segment".repeat(5)));
    }
    relative.push("真实源码.js");
    let source_path = base.join("source").join(&relative);
    assert!(source_path.as_os_str().len() > 260);
    fs::create_dir_all(source_path.parent().unwrap()).unwrap();
    fs::write(&source_path, b"long path author source").unwrap();
    fs::create_dir(base.join("target")).unwrap();
    let original = PersistedIdentity::capture(&source_path).unwrap();
    let source = Anchor::open(base.join("source")).unwrap();
    let target = Anchor::open(base.join("target")).unwrap();
    let digest = author_digest(&source.file, Some(&target.file)).unwrap();
    assert_eq!(digest, author_digest(&target.file, None).unwrap());
    assert!(original.matches_path(&source_path).unwrap());
    assert_eq!(
        fs::read(base.join("target").join(relative)).unwrap(),
        b"long path author source"
    );
}

#[test]
fn long_unicode_ancestors_and_file_components_keep_original_handles_after_name_replacement() {
    let temporary = sample();
    let root = temporary.path().canonicalize().unwrap();
    let source_parent = root.join("中".repeat(86));
    let target_parent = root.join("文".repeat(86));
    let filename = format!("{}.js", "长".repeat(86));
    assert!(filename.len() > 255 && filename.encode_utf16().count() < 255);
    fs::create_dir_all(source_parent.join("source")).unwrap();
    fs::create_dir_all(target_parent.join("target")).unwrap();
    fs::create_dir(root.join("outside")).unwrap();
    fs::write(
        source_parent.join("source").join(&filename),
        b"original long name source",
    )
    .unwrap();
    fs::write(
        root.join("outside/foreign"),
        b"outside must remain untouched",
    )
    .unwrap();
    let original =
        PersistedIdentity::capture(&source_parent.join("source").join(&filename)).unwrap();
    let source = Anchor::open(source_parent.join("source")).unwrap();
    let target = Anchor::open(target_parent.join("target")).unwrap();
    fs::rename(
        source_parent.join("source"),
        source_parent.join("preserved-source"),
    )
    .unwrap();
    junction(&source_parent.join("source"), &root.join("outside"));
    fs::rename(
        target_parent.join("target"),
        target_parent.join("preserved-target"),
    )
    .unwrap();
    junction(&target_parent.join("target"), &root.join("outside"));
    let digest = author_digest(&source.file, Some(&target.file)).unwrap();
    assert_eq!(digest, author_digest(&target.file, None).unwrap());
    assert!(source.verify().is_err());
    assert!(target.verify().is_err());
    assert!(original
        .matches_path(&source_parent.join("preserved-source").join(&filename))
        .unwrap());
    assert_eq!(
        fs::read(target_parent.join("preserved-target").join(&filename)).unwrap(),
        b"original long name source"
    );
    assert_eq!(
        fs::read(root.join("outside/foreign")).unwrap(),
        b"outside must remain untouched"
    );
    assert_eq!(fs::read_dir(root.join("outside")).unwrap().count(), 1);
    fs::remove_dir(source_parent.join("source")).unwrap();
    fs::remove_dir(target_parent.join("target")).unwrap();
}

#[test]
fn actual_copy_rejects_shared_hardlinks_and_junctions_without_touching_originals() {
    for linked_directory in [false, true] {
        let temporary = sample();
        let base = temporary.path().canonicalize().unwrap();
        for name in ["source", "target", "outside"] {
            fs::create_dir(base.join(name)).unwrap();
        }
        fs::write(base.join("outside/secret"), b"outside must remain").unwrap();
        let outside = PersistedIdentity::capture(&base.join("outside/secret")).unwrap();
        if linked_directory {
            junction(&base.join("source/linked"), &base.join("outside"));
        } else {
            fs::hard_link(base.join("outside/secret"), base.join("source/linked")).unwrap();
        }
        let source = Anchor::open(base.join("source")).unwrap();
        let target = Anchor::open(base.join("target")).unwrap();
        assert!(author_digest(&source.file, Some(&target.file)).is_err());
        assert!(names(&target.file).unwrap().is_empty());
        assert!(outside.matches_path(&base.join("outside/secret")).unwrap());
        assert_eq!(
            fs::read(base.join("outside/secret")).unwrap(),
            b"outside must remain"
        );
        if linked_directory {
            fs::remove_dir(base.join("source/linked")).unwrap();
        }
    }
}

#[test]
fn actual_anchors_copy_original_handles_and_reject_replaced_names() {
    let temporary = sample();
    let root = temporary.path().canonicalize().unwrap();
    for name in ["source", "target", "outside"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    fs::write(root.join("source/public.js"), b"owned fixture").unwrap();
    fs::write(
        root.join("outside/public.js"),
        b"outside must not be copied",
    )
    .unwrap();
    let source = Anchor::open(root.join("source")).unwrap();
    let target = Anchor::open(root.join("target")).unwrap();
    fs::rename(root.join("source"), root.join("preserved-source")).unwrap();
    junction(&root.join("source"), &root.join("outside"));
    fs::rename(root.join("target"), root.join("preserved-target")).unwrap();
    junction(&root.join("target"), &root.join("outside"));
    author_digest(&source.file, Some(&target.file)).unwrap();
    assert!(source.verify().is_err());
    assert!(target.verify().is_err());
    assert_eq!(
        fs::read(root.join("preserved-target/public.js")).unwrap(),
        b"owned fixture"
    );
    assert_eq!(
        fs::read(root.join("outside/public.js")).unwrap(),
        b"outside must not be copied"
    );
    fs::remove_dir(root.join("source")).unwrap();
    fs::remove_dir(root.join("target")).unwrap();
}

#[test]
fn real_exclusive_publication_preserves_every_existing_destination_kind() {
    for kind in ["empty", "directory", "file", "junction"] {
        let temporary = sample();
        let base = temporary.path().canonicalize().unwrap();
        fs::create_dir_all(base.join("receipt/payload")).unwrap();
        fs::create_dir(base.join("root")).unwrap();
        fs::create_dir(base.join("outside")).unwrap();
        fs::write(base.join("receipt/payload/ours"), b"original source").unwrap();
        let destination = base.join("root/author");
        match kind {
            "empty" => fs::create_dir(&destination).unwrap(),
            "directory" => {
                fs::create_dir(&destination).unwrap();
                fs::write(destination.join("foreign"), b"foreign").unwrap();
            }
            "file" => fs::write(&destination, b"foreign file").unwrap(),
            _ => junction(&destination, &base.join("outside")),
        }
        let before = removal::inspect(&destination).unwrap().identity;
        let expected = PersistedIdentity::capture(&base.join("receipt/payload")).unwrap();
        let receipt = Anchor::open(base.join("receipt")).unwrap();
        let root = Anchor::open(base.join("root")).unwrap();
        assert!(
            publish_exclusive(&receipt.file, "payload", &root.file, "author", &expected).is_err()
        );
        assert!(before.same_object(&removal::inspect(&destination).unwrap().identity));
        assert!(expected
            .matches_path(&base.join("receipt/payload"))
            .unwrap());
        assert_eq!(
            fs::read(base.join("receipt/payload/ours")).unwrap(),
            b"original source"
        );
        if kind == "junction" {
            fs::remove_dir(&destination).unwrap();
        }
    }
}

#[test]
fn exclusive_publication_uses_original_full_id_not_replacement_payload() {
    let temporary = sample();
    let base = temporary.path().canonicalize().unwrap();
    fs::create_dir_all(base.join("receipt/payload")).unwrap();
    fs::create_dir(base.join("root")).unwrap();
    fs::write(base.join("receipt/payload/ours"), b"original source").unwrap();
    let expected = PersistedIdentity::capture(&base.join("receipt/payload")).unwrap();
    fs::rename(base.join("receipt/payload"), base.join("receipt/original")).unwrap();
    fs::create_dir(base.join("receipt/payload")).unwrap();
    fs::write(base.join("receipt/payload/foreign"), b"replacement").unwrap();
    let receipt = Anchor::open(base.join("receipt")).unwrap();
    let root = Anchor::open(base.join("root")).unwrap();
    assert!(publish_exclusive(&receipt.file, "payload", &root.file, "author", &expected).is_err());
    assert!(names(&root.file).unwrap().is_empty());
    assert!(expected
        .matches_path(&base.join("receipt/original"))
        .unwrap());
    assert_eq!(
        fs::read(base.join("receipt/payload/foreign")).unwrap(),
        b"replacement"
    );
}

#[tokio::test]
async fn known_precommit_collision_forgets_only_unpublished_reservation_and_preserves_foreign() {
    for race in [false, true] {
        let (_temporary, _paths, store, tasks, manager) = fixture();
        let source = ready_source(&manager, &tasks);
        let app = manager.owner_app_id(&source.source_id).unwrap();
        let record = prepared(&manager, &source, &app);
        let receipt = Anchor::open(journal(&manager, &app)).unwrap();
        let root = Anchor::open(manager.paths.workspace_root().join("project-sources")).unwrap();
        let destination = root.path.join(&record.directory);
        let foreign = || {
            fs::create_dir(&destination).unwrap();
            fs::write(destination.join("foreign"), b"another project's source").unwrap();
        };
        if !race {
            foreign();
        }
        let result = with_publication_reservation(&owner(&manager), &app, &record, || {
            if race {
                foreign();
            }
            publish_exclusive(
                &receipt.file,
                &record.payload,
                &root.file,
                &record.directory,
                &record.payload_identity,
            )
        });
        assert!(result.is_err());
        assert!(author_reservation(&manager, &app, &record.directory).is_null());
        assert!(!owner(&manager)
            .owned_paths(&app)
            .unwrap()
            .contains(&destination));
        assert!(record
            .payload_identity
            .matches_path(&receipt.path.join(&record.payload))
            .unwrap());
        assert_eq!(
            fs::read(destination.join("foreign")).unwrap(),
            b"another project's source"
        );
        tasks.shutdown().await;
        store.shutdown().await;
    }
}

#[tokio::test]
async fn real_published_author_git_survives_new_manager_and_later_user_edits() {
    let (_temporary, paths, store, tasks, manager) = fixture();
    let source = ready_source(&manager, &tasks);
    let app = manager.owner_app_id(&source.source_id).unwrap();
    let lease = store
        .reserve_task(&format!("source:{}", source.source_id), "publication-test")
        .unwrap();
    let destination = manager
        .publish_author_source(&source, &app, "publication-test")
        .unwrap();
    let identity = PersistedIdentity::capture(&destination).unwrap();
    fs::write(destination.join("app.js"), b"later uncommitted author edit").unwrap();
    let next = restarted(&paths, &store, &tasks);
    assert_eq!(
        next.publish_author_source(&source, &app, "publication-test")
            .unwrap(),
        destination
    );
    assert_eq!(
        next.reconcile_author_publication(&source.source_id, &app, &source.task_id)
            .unwrap(),
        Some(destination.clone())
    );
    assert!(identity.matches_path(&destination).unwrap());
    assert_eq!(
        fs::read(destination.join("app.js")).unwrap(),
        b"later uncommitted author edit"
    );
    assert_eq!(
        fs::read(destination.join(".git/HEAD")).unwrap(),
        b"ref: refs/heads/main\n"
    );
    assert_eq!(
        fs::read(source.source_path.unwrap().join("app.js")).unwrap(),
        b"disposable author source"
    );
    assert_eq!(
        fs::read_dir(paths.workspace_root().join("project-sources"))
            .unwrap()
            .count(),
        1
    );
    drop(lease);
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn valid_long_unicode_file_and_directory_publish_and_new_manager_retry_keeps_identity_and_edits(
) {
    let (_temporary, paths, store, tasks, manager) = fixture();
    let original = ready_source(&manager, &tasks);
    let directory = "中".repeat(86);
    let filename = format!("{}.js", "文".repeat(86));
    assert!(directory.len() > 255 && directory.encode_utf16().count() < 255);
    assert!(filename.len() > 255 && filename.encode_utf16().count() < 255);
    let relative = PathBuf::from(&directory).join(&filename);
    let source_root = original.source_path.as_ref().unwrap();
    fs::create_dir(source_root.join(&directory)).unwrap();
    fs::write(
        source_root.join(&relative),
        b"valid long Unicode source bytes",
    )
    .unwrap();
    let file_identity = PersistedIdentity::capture(&source_root.join(&relative)).unwrap();
    let directory_identity = PersistedIdentity::capture(&source_root.join(&directory)).unwrap();
    let mut record = manager.read_record(&original.source_id).unwrap();
    record.content_digest = Some(directory_digest(source_root).unwrap());
    record.file_count = Some(2);
    record.total_bytes = Some(24 + b"valid long Unicode source bytes".len() as u64);
    manager.save_record(&record).unwrap();
    let source = manager
        .view(manager.read_record(&original.source_id).unwrap())
        .unwrap();
    let app = manager.owner_app_id(&source.source_id).unwrap();
    let lease = store
        .reserve_task(&format!("source:{}", source.source_id), "publication-test")
        .unwrap();
    let destination = manager
        .publish_author_source(&source, &app, "publication-test")
        .unwrap();
    let identity = PersistedIdentity::capture(&destination).unwrap();
    assert_eq!(
        fs::read(destination.join(&relative)).unwrap(),
        b"valid long Unicode source bytes"
    );
    assert!(!file_identity
        .matches_path(&destination.join(&relative))
        .unwrap());
    assert!(!directory_identity
        .matches_path(&destination.join(&directory))
        .unwrap());
    native::verify_private(&native::open_read_file(&destination.join(&relative)).unwrap()).unwrap();
    fs::write(
        destination.join(&relative),
        b"later author edit in long name",
    )
    .unwrap();
    let next = restarted(&paths, &store, &tasks);
    assert_eq!(
        next.publish_author_source(&source, &app, "publication-test")
            .unwrap(),
        destination
    );
    assert_eq!(
        next.reconcile_author_publication(&source.source_id, &app, &source.task_id)
            .unwrap(),
        Some(destination.clone())
    );
    assert!(identity.matches_path(&destination).unwrap());
    assert!(file_identity
        .matches_path(&source_root.join(&relative))
        .unwrap());
    assert!(directory_identity
        .matches_path(&source_root.join(&directory))
        .unwrap());
    assert_eq!(
        fs::read(source_root.join(&relative)).unwrap(),
        b"valid long Unicode source bytes"
    );
    assert_eq!(
        fs::read(destination.join(&relative)).unwrap(),
        b"later author edit in long name"
    );
    assert_eq!(
        fs::read_dir(paths.workspace_root().join("project-sources"))
            .unwrap()
            .count(),
        1
    );
    drop(lease);
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn actual_existing_project_file_is_preserved_and_suffix_is_published() {
    let (_temporary, paths, store, tasks, manager) = fixture();
    let source = ready_source(&manager, &tasks);
    fs::create_dir(paths.workspace_root().join("project-sources")).unwrap();
    let foreign = paths
        .workspace_root()
        .join("project-sources/disposable-repo");
    fs::write(&foreign, b"unrelated user's file").unwrap();
    let foreign_id = PersistedIdentity::capture(&foreign).unwrap();
    let lease = store
        .reserve_task(&format!("source:{}", source.source_id), "publication-test")
        .unwrap();
    let path = manager
        .publish_author_source(
            &source,
            &manager.owner_app_id(&source.source_id).unwrap(),
            "publication-test",
        )
        .unwrap();
    assert_eq!(path.file_name().unwrap(), "disposable-repo-2");
    assert!(foreign_id.matches_path(&foreign).unwrap());
    assert_eq!(fs::read(&foreign).unwrap(), b"unrelated user's file");
    drop(lease);
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn valid_provider_device_and_trailing_dot_names_publish_readable_local_names_and_keep_urls() {
    for repository in ["CON", "NUL.txt", "foo."] {
        let (_temporary, paths, store, tasks, manager) = fixture();
        let original = ready_source(&manager, &tasks);
        let mut record = manager.read_record(&original.source_id).unwrap();
        let url = format!("https://github.com/example/{repository}.git");
        assert_eq!(validate_url(&url).unwrap().2, repository);
        record.url = url.clone();
        manager.save_record(&record).unwrap();
        let source = manager
            .view(manager.read_record(&original.source_id).unwrap())
            .unwrap();
        let app = manager.owner_app_id(&source.source_id).unwrap();
        let expected = crate::project_directory_name::readable_directory_name(repository, &app);
        let lease = store
            .reserve_task(&format!("source:{}", source.source_id), "publication-test")
            .unwrap();
        let destination = manager
            .publish_author_source(&source, &app, "publication-test")
            .unwrap();
        assert_eq!(destination.file_name().unwrap(), expected.as_str());
        assert_eq!(files::parts(&expected).unwrap().len(), 1);
        let identity = PersistedIdentity::capture(&destination).unwrap();
        fs::write(destination.join("app.js"), b"later author edit").unwrap();
        let next = restarted(&paths, &store, &tasks);
        assert_eq!(
            next.publish_author_source(&source, &app, "publication-test")
                .unwrap(),
            destination
        );
        assert!(identity.matches_path(&destination).unwrap());
        assert_eq!(next.read_record(&source.source_id).unwrap().url, url);
        assert_eq!(source.url, url);
        assert_eq!(
            fs::read(destination.join("app.js")).unwrap(),
            b"later author edit"
        );
        assert_eq!(
            fs::read(destination.join(".git/HEAD")).unwrap(),
            b"ref: refs/heads/main\n"
        );
        assert_eq!(
            fs::read_dir(paths.workspace_root().join("project-sources"))
                .unwrap()
                .count(),
            1
        );
        drop(lease);
        tasks.shutdown().await;
        store.shutdown().await;
    }
}

#[tokio::test]
async fn actual_partial_payload_is_retained_without_a_second_payload_or_project() {
    let (_temporary, paths, store, tasks, manager) = fixture();
    let source = ready_source(&manager, &tasks);
    let app = manager.owner_app_id(&source.source_id).unwrap();
    let lease = store
        .reserve_task(&format!("source:{}", source.source_id), "publication-test")
        .unwrap();
    let mut record = prepared(&manager, &source, &app);
    let receipt = Anchor::open(journal(&manager, &app)).unwrap();
    let partial = receipt.path.join(&record.payload);
    fs::remove_file(partial.join("app.js")).unwrap();
    let partial_id = PersistedIdentity::capture(&partial).unwrap();
    let mut bytes = record_bytes(&receipt.file).unwrap();
    record.phase = "copying".into();
    save_publication(&receipt, &mut bytes, &record).unwrap();
    let next = restarted(&paths, &store, &tasks);
    assert!(next
        .publish_author_source(&source, &app, "publication-test")
        .is_err());
    assert!(partial_id.matches_path(&partial).unwrap());
    assert!(partial.join(".git/HEAD").is_file());
    assert_eq!(publication(&next, &app).payload, record.payload);
    assert_eq!(
        fs::read_dir(receipt.path.clone())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("payload_"))
            .count(),
        1
    );
    assert_eq!(
        fs::read_dir(paths.workspace_root().join("project-sources"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        next.reconcile_author_publication(&source.source_id, &app, &source.task_id)
            .unwrap(),
        None
    );
    drop(lease);
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn actual_post_move_journal_failure_is_committed_and_new_owner_retry_preserves_edits() {
    let (_temporary, paths, store, tasks, manager) = fixture();
    let source = ready_source(&manager, &tasks);
    let app = manager.owner_app_id(&source.source_id).unwrap();
    let lease = store
        .reserve_task(&format!("source:{}", source.source_id), "publication-test")
        .unwrap();
    let record = prepared(&manager, &source, &app);
    let path = journal(&manager, &app).join("publication.json");
    native::set_read_only(&path, true).unwrap();
    let failure = manager
        .publish_author_source(&source, &app, "publication-test")
        .unwrap_err();
    assert_eq!(failure.details["operationOutcome"], "committed");
    let destination = paths
        .workspace_root()
        .join("project-sources")
        .join(&record.directory);
    assert!(record.payload_identity.matches_path(&destination).unwrap());
    assert!(!journal(&manager, &app).join(&record.payload).exists());
    assert_eq!(publication(&manager, &app).phase, "ready");
    assert_eq!(
        author_reservation(&manager, &app, &record.directory),
        serde_json::to_value(&record.payload_identity).unwrap()
    );
    fs::write(destination.join("app.js"), b"edit after committed error").unwrap();
    native::set_read_only(&path, false).unwrap();
    let next = restarted(&paths, &store, &tasks);
    assert_eq!(
        next.reconcile_author_publication(&source.source_id, &app, &source.task_id)
            .unwrap(),
        Some(destination.clone())
    );
    assert_eq!(
        next.publish_author_source(&source, &app, "publication-test")
            .unwrap(),
        destination
    );
    assert_eq!(
        fs::read(destination.join("app.js")).unwrap(),
        b"edit after committed error"
    );
    assert_eq!(publication(&next, &app).phase, "published");
    drop(lease);
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn uncertain_reply_after_actual_move_keeps_original_reservation_and_never_copies_again() {
    for replacement in [false, true] {
        let (_temporary, paths, store, tasks, manager) = fixture();
        let source = ready_source(&manager, &tasks);
        let app = manager.owner_app_id(&source.source_id).unwrap();
        let lease = store
            .reserve_task(&format!("source:{}", source.source_id), "publication-test")
            .unwrap();
        let record = prepared(&manager, &source, &app);
        let receipt = Anchor::open(journal(&manager, &app)).unwrap();
        let root = Anchor::open(paths.workspace_root().join("project-sources")).unwrap();
        let destination = root.path.join(&record.directory);
        let displaced = root.path.join("preserved-actual-published-source");
        // This narrow seam tests the publisher's uncertain-result handling,
        // using a real exclusive move and real name replacement. It does not
        // inject a failure into the original native confirmation callback.
        let result = with_publication_reservation(&owner(&manager), &app, &record, || {
            publish_exclusive(
                &receipt.file,
                &record.payload,
                &root.file,
                &record.directory,
                &record.payload_identity,
            )?;
            fs::rename(&destination, &displaced)?;
            if replacement {
                fs::create_dir(&destination)?;
                fs::write(destination.join("foreign"), b"foreign replacement")?;
            }
            Err(invalid("controlled uncertain publication reply").outcome("unknown"))
        });
        assert_eq!(result.unwrap_err().details["operationOutcome"], "unknown");
        let expected = serde_json::to_value(&record.payload_identity).unwrap();
        assert_eq!(
            author_reservation(&manager, &app, &record.directory),
            expected
        );
        let next = restarted(&paths, &store, &tasks);
        let failure = next
            .reconcile_author_publication(&source.source_id, &app, &source.task_id)
            .unwrap_err();
        assert_eq!(failure.details["operationOutcome"], "unknown");
        let remaining: Vec<PathBuf> =
            serde_json::from_value(failure.details["remainingPaths"].clone()).unwrap();
        assert!(remaining.contains(&receipt.path));
        assert!(remaining.contains(&receipt.path.join(&record.payload)));
        assert!(remaining.contains(&destination));
        assert_eq!(
            next.publish_author_source(&source, &app, "publication-test")
                .unwrap_err()
                .details["operationOutcome"],
            "unknown"
        );
        assert_eq!(author_reservation(&next, &app, &record.directory), expected);
        assert!(record.payload_identity.matches_path(&displaced).unwrap());
        assert!(!receipt.path.join(&record.payload).exists());
        assert_eq!(publication(&next, &app).payload, record.payload);
        assert_eq!(
            fs::read_dir(&root.path).unwrap().count(),
            if replacement { 2 } else { 1 }
        );
        if replacement {
            assert_eq!(
                fs::read(destination.join("foreign")).unwrap(),
                b"foreign replacement"
            );
        }
        drop(lease);
        tasks.shutdown().await;
        store.shutdown().await;
    }
}

#[tokio::test]
async fn prepared_receipt_and_no_receipt_reconcile_without_writes_but_absent_reserved_record_is_unknown(
) {
    let (_temporary, paths, store, tasks, manager) = fixture();
    let source = ready_source(&manager, &tasks);
    let app = manager.owner_app_id(&source.source_id).unwrap();
    assert_eq!(
        manager
            .reconcile_author_publication(&source.source_id, &app, &source.task_id)
            .unwrap(),
        None
    );
    assert!(!journal(&manager, &app).exists());
    let record = prepared(&manager, &source, &app);
    let receipt = journal(&manager, &app).join("publication.json");
    let before = fs::read(&receipt).unwrap();
    let identity = PersistedIdentity::capture(&receipt).unwrap();
    assert_eq!(
        manager
            .reconcile_author_publication(&source.source_id, &app, &source.task_id)
            .unwrap(),
        None
    );
    assert!(identity.matches_path(&receipt).unwrap());
    assert_eq!(fs::read(&receipt).unwrap(), before);
    owner(&manager)
        .reserve_published_source_identity(&app, &record.directory, &record.payload_identity)
        .unwrap();
    fs::remove_file(&receipt).unwrap();
    let next = restarted(&paths, &store, &tasks);
    assert_eq!(
        next.reconcile_author_publication(&source.source_id, &app, &source.task_id)
            .unwrap_err()
            .details["operationOutcome"],
        "unknown"
    );
    assert!(record
        .payload_identity
        .matches_path(&journal(&next, &app).join(&record.payload))
        .unwrap());
    assert!(!receipt.exists());
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn bad_or_mismatched_receipts_are_preserved_and_never_authorize_cleanup() {
    for malformed in [false, true] {
        let (_temporary, paths, store, tasks, manager) = fixture();
        let source = ready_source(&manager, &tasks);
        let app = manager.owner_app_id(&source.source_id).unwrap();
        let mut record = prepared(&manager, &source, &app);
        let receipt = journal(&manager, &app).join("publication.json");
        record.source_task_id = format!("task_{}", Uuid::new_v4().simple());
        let bytes = if malformed {
            b"{unknown receipt".to_vec()
        } else {
            serde_json::to_vec(&record).unwrap()
        };
        fs::write(&receipt, &bytes).unwrap();
        let identity = PersistedIdentity::capture(&receipt).unwrap();
        let next = restarted(&paths, &store, &tasks);
        let failure = next
            .reconcile_author_publication(&source.source_id, &app, &source.task_id)
            .unwrap_err();
        assert_eq!(failure.details["operationOutcome"], "unknown");
        assert_eq!(
            failure.details["remainingPaths"],
            serde_json::json!([journal(&next, &app)])
        );
        assert!(identity.matches_path(&receipt).unwrap());
        assert_eq!(fs::read(&receipt).unwrap(), bytes);
        assert!(record
            .payload_identity
            .matches_path(&journal(&next, &app).join(&record.payload))
            .unwrap());
        tasks.shutdown().await;
        store.shutdown().await;
    }
}

#[tokio::test]
async fn original_metadata_budget_still_bounds_a_receipt_and_preserves_oversized_bytes() {
    let (_temporary, _paths, store, tasks, manager) = fixture();
    let source = ready_source(&manager, &tasks);
    let app = manager.owner_app_id(&source.source_id).unwrap();
    let record = prepared(&manager, &source, &app);
    let receipt = journal(&manager, &app).join("publication.json");
    let bytes = vec![b'x'; MAX_METADATA as usize + 1];
    fs::write(&receipt, &bytes).unwrap();
    let identity = PersistedIdentity::capture(&receipt).unwrap();
    assert_eq!(
        manager
            .reconcile_author_publication(&source.source_id, &app, &source.task_id)
            .unwrap_err()
            .details["operationOutcome"],
        "unknown"
    );
    assert!(identity.matches_path(&receipt).unwrap());
    assert_eq!(fs::read(&receipt).unwrap(), bytes);
    assert!(record
        .payload_identity
        .matches_path(&journal(&manager, &app).join(&record.payload))
        .unwrap());
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn unleased_source_and_git_junction_fail_before_formal_project_creation() {
    let (_temporary, paths, store, tasks, manager) = fixture();
    let source = ready_source(&manager, &tasks);
    let app = manager.owner_app_id(&source.source_id).unwrap();
    assert_eq!(
        manager
            .publish_author_source(&source, &app, "publication-test")
            .unwrap_err()
            .code,
        "SOURCE_LEASE_REQUIRED"
    );
    let original = source.source_path.as_ref().unwrap();
    let external = paths.workspace_root().join("disposable-outside-git-target");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("foreign"), b"outside original").unwrap();
    junction(&original.join(".git/linked-object"), &external);
    let lease = store
        .reserve_task(&format!("source:{}", source.source_id), "publication-test")
        .unwrap();
    assert!(manager
        .publish_author_source(&source, &app, "publication-test")
        .is_err());
    assert!(!paths.workspace_root().join("project-sources").exists());
    assert_eq!(
        fs::read(external.join("foreign")).unwrap(),
        b"outside original"
    );
    fs::remove_dir(original.join(".git/linked-object")).unwrap();
    drop(lease);
    tasks.shutdown().await;
    store.shutdown().await;
}
