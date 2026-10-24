use super::*;
use crate::{CreateProjectInput, ProjectStore, RuntimePaths, SqlCaller, SqlOptions, SqlRequest};
use rusqlite::Connection;
use std::sync::Arc;

async fn fixture() -> (tempfile::TempDir, Arc<ProjectStore>, ProjectHandle, PathBuf) {
    let (temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let info = store
        .create(CreateProjectInput {
            name: "Disposable data version fixture".into(),
            description: None,
        })
        .await
        .unwrap();
    let project = store.project(&info.id).unwrap();
    let candidate = paths.workspace_root().join("snapshot-candidate");
    (temp, store, project, candidate)
}
fn channels() -> (
    watch::Sender<bool>,
    watch::Receiver<bool>,
    watch::Sender<bool>,
    watch::Receiver<bool>,
) {
    let (s, stop) = watch::channel(false);
    let (c, cancel) = watch::channel(false);
    (s, stop, c, cancel)
}
async fn capture(project: ProjectHandle, root: PathBuf) -> PreparedDataSnapshot {
    let (_s, stop, _c, cancel) = channels();
    let lease = SnapshotLease::acquire(project, stop.clone(), cancel.clone())
        .await
        .unwrap();
    tokio::task::spawn_blocking(move || {
        prepare_snapshot(lease, &root, SnapshotLimits::default(), &stop, &cancel)
    })
    .await
    .unwrap()
    .unwrap()
}

#[tokio::test]
async fn sqlite_wal_and_complete_attachments_are_saved_without_configuration_or_live_companions() {
    let (_temp, store, project, candidate) = fixture().await;
    let db = Connection::open(project.root.join(&project.manifest.database)).unwrap();
    db.pragma_update(None, "journal_mode", "WAL").unwrap();
    db.execute_batch("CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT); INSERT INTO notes VALUES(1, 'saved in WAL');").unwrap();
    fs::write(
        project.root.join("app/.env"),
        "ARTIFICIAL_SECRET=never-copy",
    )
    .unwrap();
    fs::create_dir_all(project.root.join("data/attachments/空 文件夹")).unwrap();
    fs::write(
        project.root.join("data/attachments/留言.txt"),
        "完整附件\nsecond line",
    )
    .unwrap();
    let snapshot = capture(project.clone(), candidate.clone()).await;
    assert_eq!(snapshot.manifest.attachments.len(), 1);
    assert_eq!(snapshot.manifest.directories, ["空 文件夹"]);
    assert_eq!(
        fs::read_to_string(candidate.join("attachments/留言.txt")).unwrap(),
        "完整附件\nsecond line"
    );
    let saved = Connection::open_with_flags(
        candidate.join("database.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        saved
            .query_row("SELECT body FROM notes WHERE id=1", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "saved in WAL"
    );
    let journal: String = saved
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(journal, "delete");
    assert!(!candidate.join("database.sqlite-wal").exists());
    assert!(!candidate.join("app").exists());
    assert!(!project.root.join(DIRECTORY).exists());
    assert_eq!(tracked_files(&snapshot).len(), 3);
    assert!(tracked_files(&snapshot)
        .keys()
        .all(|path| path.starts_with("version-data/")));
    drop(db);
    store.shutdown().await;
}

#[tokio::test]
async fn snapshot_lease_drains_and_blocks_real_sql_and_attachment_publication() {
    let (_temp, store, project, candidate) = fixture().await;
    let request = |sql: &str| SqlRequest {
        sql: sql.into(),
        params: vec![],
        options: SqlOptions::default(),
    };
    store
        .execute(
            &project.manifest.id,
            request("CREATE TABLE notes(body TEXT)"),
            None,
            SqlCaller::Management,
        )
        .await
        .unwrap();
    let attachment_root = project.root.join("data/attachments");
    fs::write(attachment_root.join("value.txt"), "before").unwrap();
    let (_s, stop, _c, cancel) = channels();
    let lease = SnapshotLease::acquire(project.clone(), stop.clone(), cancel.clone())
        .await
        .unwrap();
    let sql = store
        .execute(
            &project.manifest.id,
            request("INSERT INTO notes VALUES('during snapshot')"),
            None,
            SqlCaller::Management,
        )
        .await
        .unwrap_err();
    assert_eq!(sql.code, "PROJECT_BUSY");
    let gate = project.operation_gate.clone();
    let path = attachment_root.join("value.txt");
    let upload = tokio::spawn(async move {
        let _read = gate.read().await;
        fs::write(path, "after").unwrap();
    });
    tokio::task::yield_now().await;
    assert!(!upload.is_finished());
    let saved = tokio::task::spawn_blocking(move || {
        prepare_snapshot(lease, &candidate, SnapshotLimits::default(), &stop, &cancel)
    })
    .await
    .unwrap()
    .unwrap();
    upload.await.unwrap();
    assert_eq!(
        fs::read_to_string(saved.root.join("attachments/value.txt")).unwrap(),
        "before"
    );
    assert_eq!(
        fs::read_to_string(attachment_root.join("value.txt")).unwrap(),
        "after"
    );
    store
        .execute(
            &project.manifest.id,
            request("INSERT INTO notes VALUES('after snapshot')"),
            None,
            SqlCaller::Management,
        )
        .await
        .unwrap();
    let saved_db = Connection::open(saved.root.join("database.sqlite")).unwrap();
    assert_eq!(
        saved_db
            .query_row("SELECT count(*) FROM notes", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    store.shutdown().await;
}

#[tokio::test]
async fn validation_rejects_missing_extra_modified_or_wrong_project_payloads() {
    let (_temp, store, project, candidate) = fixture().await;
    fs::write(project.root.join("data/attachments/file.txt"), "original").unwrap();
    let snapshot = capture(project.clone(), candidate.clone()).await;
    let (_s, stop, _c, cancel) = channels();
    fs::write(candidate.join("attachments/file.txt"), "changed").unwrap();
    assert_eq!(
        validate_snapshot(
            &candidate,
            &project.manifest,
            SnapshotLimits::default(),
            &stop,
            &cancel
        )
        .unwrap_err()
        .code,
        "PROJECT_DATA_SNAPSHOT_CHANGED"
    );
    fs::write(candidate.join("attachments/file.txt"), "original").unwrap();
    fs::write(candidate.join("database.sqlite-wal"), "unexpected").unwrap();
    assert!(validate_snapshot(
        &candidate,
        &project.manifest,
        SnapshotLimits::default(),
        &stop,
        &cancel
    )
    .is_err());
    fs::remove_file(candidate.join("database.sqlite-wal")).unwrap();
    let mut wrong = project.manifest.clone();
    wrong.id = "prj_other".into();
    assert!(validate_snapshot(
        &candidate,
        &wrong,
        SnapshotLimits::default(),
        &stop,
        &cancel
    )
    .is_err());
    fs::remove_file(candidate.join("attachments/file.txt")).unwrap();
    assert!(validate_snapshot(
        &candidate,
        &project.manifest,
        SnapshotLimits::default(),
        &stop,
        &cancel
    )
    .is_err());
    assert!(!snapshot.manifest_digest.is_empty());
    store.shutdown().await;
}

#[tokio::test]
async fn links_and_budget_failures_keep_original_data_and_release_snapshot_freeze() {
    let (_temp, store, project, candidate) = fixture().await;
    let original = project.root.join("data/attachments/original.txt");
    fs::write(&original, "not truncated").unwrap();
    std::os::unix::fs::symlink(&original, project.root.join("data/attachments/link.txt")).unwrap();
    let (_s, stop, _c, cancel) = channels();
    let lease = SnapshotLease::acquire(project.clone(), stop.clone(), cancel.clone())
        .await
        .unwrap();
    assert!(
        prepare_snapshot(lease, &candidate, SnapshotLimits::default(), &stop, &cancel).is_err()
    );
    assert_eq!(fs::read_to_string(&original).unwrap(), "not truncated");
    assert!(!project
        .control
        .frozen
        .load(std::sync::atomic::Ordering::Acquire));
    fs::remove_file(project.root.join("data/attachments/link.txt")).unwrap();
    let lease = SnapshotLease::acquire(project.clone(), stop.clone(), cancel.clone())
        .await
        .unwrap();
    let second = candidate.with_extension("limited");
    let failure = prepare_snapshot(
        lease,
        &second,
        SnapshotLimits {
            max_file_bytes: 4,
            ..SnapshotLimits::default()
        },
        &stop,
        &cancel,
    )
    .unwrap_err();
    assert_eq!(failure.code, "PROJECT_DATA_SNAPSHOT_LIMIT");
    assert_eq!(fs::read_to_string(&original).unwrap(), "not truncated");
    assert!(second.exists());
    store.shutdown().await;
}

#[tokio::test]
async fn cancellation_while_waiting_for_upload_does_not_leave_frozen_project() {
    let (_temp, store, project, _candidate) = fixture().await;
    let read = project.operation_gate.clone().read_owned().await;
    let (_s, stop, sender, cancel) = channels();
    let target = project.clone();
    let task = tokio::spawn(async move { SnapshotLease::acquire(target, stop, cancel).await });
    tokio::task::yield_now().await;
    sender.send(true).unwrap();
    assert_eq!(
        task.await.unwrap().err().unwrap().code,
        "PROJECT_DATA_SNAPSHOT_CANCELLED"
    );
    assert!(!project
        .control
        .frozen
        .load(std::sync::atomic::Ordering::Acquire));
    drop(read);
    store.shutdown().await;
}

#[tokio::test]
async fn sqlite_copy_cancellation_is_checked_between_real_backup_steps() {
    let (_temp, store, project, candidate) = fixture().await;
    let db = Connection::open(project.root.join(&project.manifest.database)).unwrap();
    db.execute_batch(
        "CREATE TABLE payload(data BLOB); INSERT INTO payload VALUES(zeroblob(4194304));",
    )
    .unwrap();
    let mut calls = 0;
    let failure = backups::sqlite_snapshot_cancellable(
        &project.root.join(&project.manifest.database),
        &candidate,
        || {
            calls += 1;
            if calls >= 3 {
                Err(cancelled())
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    assert_eq!(failure.code, "PROJECT_DATA_SNAPSHOT_CANCELLED");
    assert!(candidate.exists());
    assert_eq!(
        db.query_row("SELECT length(data) FROM payload", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        4194304
    );
    drop(db);
    store.shutdown().await;
}
