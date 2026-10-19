//! All writes target new named disposable native SQLite fixtures. The stock
//! setup connection is a test-fixture generator, not production initialization.
use super::*;
use crate::{SqliteFileIdentities, WindowsWorkspace, WorkspaceDirectory};
use std::{collections::BTreeMap, fs, os::windows::fs::MetadataExt, path::PathBuf};

fn serial() -> std::sync::MutexGuard<'static, ()> {
    crate::WINDOWS_FILESYSTEM_TESTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn fixture() -> (WindowsWorkspace, WorkspaceDirectory, SqliteFileIdentities) {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let artifacts = repository.join(".test-artifacts");
    fs::create_dir_all(&artifacts).unwrap();
    assert_eq!(
        fs::symlink_metadata(&artifacts).unwrap().file_attributes() & 0x400,
        0
    );
    let parent = artifacts.join(format!(
        "windows-workspace-disposable-{}",
        Uuid::new_v4().simple()
    ));
    crate::windows_workspace::create_private_fixture_directory(&parent).unwrap();
    let root = WindowsWorkspace::for_test_path(parent, true).unwrap();
    let data = root.root().create_directory("SQLite 数据 空格").unwrap();
    let expected = SqliteFileIdentities {
        data_directory: data.identity().clone(),
        main: data.write_new("main.sqlite", b"").unwrap(),
        wal: data.write_new("main.sqlite-wal", b"").unwrap(),
        shm: data.write_new("main.sqlite-shm", b"").unwrap(),
    };
    let connection = Connection::open_with_flags(
        data.path().join("main.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    let mut persist = 1;
    assert_eq!(
        unsafe {
            ffi::sqlite3_file_control(
                connection.handle(),
                c"main".as_ptr(),
                ffi::SQLITE_FCNTL_PERSIST_WAL,
                (&mut persist as *mut c_int).cast(),
            )
        },
        ffi::SQLITE_OK
    );
    connection.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE records(id INTEGER PRIMARY KEY, value TEXT NOT NULL UNIQUE); INSERT INTO records(value) VALUES('fixture committed'); CREATE TABLE evidence(id INTEGER PRIMARY KEY, note TEXT); INSERT INTO evidence(note) VALUES('independent page');").unwrap();
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    connection.close().unwrap();
    // No ACL adoption or byte overwrite: native SQLite used the three private
    // files created above. Their original identities must still be present.
    data.lease_sqlite_files(&expected).unwrap();
    (root, data, expected)
}
fn bytes(data: &WorkspaceDirectory) -> BTreeMap<PathBuf, Vec<u8>> {
    fs::read_dir(data.path())
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let value = fs::read(&path).unwrap();
            (path, value)
        })
        .collect()
}

#[test]
fn bound_wal_connection_commits_rolls_back_and_reopens_real_data() {
    let _serial = serial();
    let (_root, data, expected) = fixture();
    let database =
        BoundSqliteDatabase::open_existing(data.lease_sqlite_files(&expected).unwrap()).unwrap();
    let conn = database.connection.as_ref().unwrap();
    assert!(rusqlite::version_number() >= 3_051_003);
    assert_eq!(
        conn.query_row("SELECT sqlite_version()", [], |r| r.get::<_, String>(0))
            .unwrap(),
        rusqlite::version()
    );
    assert_eq!(
        conn.query_row("SELECT value FROM records WHERE id=1", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "fixture committed"
    );
    conn.execute("INSERT INTO records(value) VALUES(?)", ["bound committed"])
        .unwrap();
    conn.execute_batch("BEGIN; INSERT INTO records(value) VALUES('rolled back'); ROLLBACK;")
        .unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM records", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    database.verify_binding().unwrap();
    database.close().unwrap();
    assert_eq!(data.list().unwrap().len(), 3);
    let reopened =
        BoundSqliteDatabase::open_existing(data.lease_sqlite_files(&expected).unwrap()).unwrap();
    assert_eq!(
        reopened
            .connection
            .as_ref()
            .unwrap()
            .query_row("SELECT value FROM records WHERE id=2", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "bound committed"
    );
    reopened.close().unwrap();
}

#[test]
fn bound_wal_keeps_required_configuration_and_rejects_other_paths_before_open() {
    let _serial = serial();
    let (root, data, expected) = fixture();
    let database =
        BoundSqliteDatabase::open_existing(data.lease_sqlite_files(&expected).unwrap()).unwrap();
    let conn = database.connection.as_ref().unwrap();
    assert_eq!(
        conn.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
    for (query, value) in [
        ("PRAGMA foreign_keys", 1),
        ("PRAGMA synchronous", 2),
        ("PRAGMA busy_timeout", 5000),
        ("PRAGMA temp_store", 2),
    ] {
        assert_eq!(
            conn.query_row(query, [], |r| r.get::<_, i64>(0)).unwrap(),
            value
        );
    }
    let outside = root.root().create_directory("retained-outside").unwrap();
    outside.write_new("evidence", b"never write here").unwrap();
    let escaped = outside.path().join("forbidden.sqlite");
    let statement = format!(
        "VACUUM INTO '{}'",
        escaped.to_str().unwrap().replace('\'', "''")
    );
    assert!(conn.execute_batch(&statement).is_err());
    // SQLITE_USE_URI can otherwise select another VFS in ATTACH/VACUUM INTO.
    let uri = format!(
        "file:{}?vfs=win32",
        escaped.to_str().unwrap().replace('\\', "/")
    );
    assert!(conn
        .execute("ATTACH DATABASE ? AS escaped", [&uri])
        .is_err());
    assert!(conn.execute("PRAGMA journal_mode=DELETE", []).is_err());
    assert!(!escaped.exists());
    assert_eq!(outside.list().unwrap().len(), 1);
    assert_eq!(
        fs::read(outside.path().join("evidence")).unwrap(),
        b"never write here"
    );
    database.close().unwrap();
}

#[test]
fn bound_vfs_rejects_unknown_journal_without_hiding_or_altering_it() {
    let _serial = serial();
    let (_root, data, expected) = fixture();
    let database =
        BoundSqliteDatabase::open_existing(data.lease_sqlite_files(&expected).unwrap()).unwrap();
    data.write_new("main.sqlite-journal", b"retained recovery evidence")
        .unwrap();
    let before = bytes(&data);
    assert!(database
        .connection
        .as_ref()
        .unwrap()
        .query_row("SELECT count(*) FROM records", [], |r| r.get::<_, i64>(0))
        .is_err());
    assert_eq!(bytes(&data), before);
    // Cleanup always releases handles even when admission has become blocked.
    assert!(database.close().is_err());
    assert_eq!(bytes(&data), before);
}

#[test]
fn handle_backed_shm_grows_multiple_regions_without_moving_previous_views() {
    let _serial = serial();
    let (_root, data, expected) = fixture();
    let before_main = fs::read(data.path().join("main.sqlite")).unwrap();
    let before_wal = fs::read(data.path().join("main.sqlite-wal")).unwrap();
    let files = data.lease_sqlite_files(&expected).unwrap();
    let mut shm = ShmConnection::open(&files).unwrap();
    let zero = shm.map(0, 32768, true).unwrap().cast::<u8>();
    unsafe {
        zero.add(32767).write_volatile(91);
    }
    let one = shm.map(1, 32768, true).unwrap().cast::<u8>();
    unsafe {
        one.add(32767).write_volatile(37);
    }
    let two = shm.map(2, 32768, true).unwrap().cast::<u8>();
    unsafe {
        two.write_volatile(12);
        assert_eq!(zero.add(32767).read_volatile(), 91);
        assert_eq!(one.add(32767).read_volatile(), 37);
    }
    assert_eq!(shm.map(0, 32768, false).unwrap(), zero.cast());
    let length = fs::metadata(data.path().join("main.sqlite-shm"))
        .unwrap()
        .len();
    assert!(shm.map(3, 32768, false).unwrap().is_null());
    assert_eq!(
        fs::metadata(data.path().join("main.sqlite-shm"))
            .unwrap()
            .len(),
        length
    );
    assert!(ShmConnection::open(&files).is_err());
    shm.close().unwrap();
    ShmConnection::open(&files).unwrap().close().unwrap();
    assert_eq!(
        fs::read(data.path().join("main.sqlite")).unwrap(),
        before_main
    );
    assert_eq!(
        fs::read(data.path().join("main.sqlite-wal")).unwrap(),
        before_wal
    );
}

fn stock(data: &WorkspaceDirectory) -> Connection {
    let connection = Connection::open_with_flags(
        data.path().join("main.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    let mut persist = 1;
    assert_eq!(
        unsafe {
            ffi::sqlite3_file_control(
                connection.handle(),
                c"main".as_ptr(),
                ffi::SQLITE_FCNTL_PERSIST_WAL,
                (&mut persist as *mut c_int).cast(),
            )
        },
        ffi::SQLITE_OK
    );
    connection.busy_timeout(Duration::ZERO).unwrap();
    connection
}

#[test]
fn bound_shm_coordinates_real_readers_and_writers_with_stock_win32_sqlite() {
    let _serial = serial();
    let (_root, data, expected) = fixture();
    let database =
        BoundSqliteDatabase::open_existing(data.lease_sqlite_files(&expected).unwrap()).unwrap();
    let conn = database.connection.as_ref().unwrap();
    conn.busy_timeout(Duration::ZERO).unwrap();
    let external = stock(&data);
    external.execute_batch("BEGIN;").unwrap();
    assert_eq!(
        external
            .query_row("SELECT count(*) FROM records", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    conn.execute(
        "INSERT INTO records(value) VALUES(?)",
        ["bound after stock reader"],
    )
    .unwrap();
    assert_eq!(
        external
            .query_row("SELECT count(*) FROM records", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    external.execute_batch("COMMIT;").unwrap();
    assert_eq!(
        external
            .query_row("SELECT count(*) FROM records", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    external
        .execute_batch("BEGIN IMMEDIATE; INSERT INTO records(value) VALUES('external pending');")
        .unwrap();
    let error = conn
        .execute("INSERT INTO records(value) VALUES('must not commit')", [])
        .unwrap_err();
    assert!(
        matches!(error, rusqlite::Error::SqliteFailure(code, _) if code.code == rusqlite::ErrorCode::DatabaseBusy)
    );
    external.execute_batch("ROLLBACK;").unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM records", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    external.close().unwrap();
    database.close().unwrap();
    data.lease_sqlite_files(&expected).unwrap();
}

#[test]
fn invalid_main_header_is_rejected_before_any_backing_file_is_changed() {
    let _serial = serial();
    let (_root, data, expected) = fixture();
    // This mutation is confined to the just-created corrupt-database fixture.
    let path = data.path().join("main.sqlite");
    let mut main = fs::read(&path).unwrap();
    main[0] ^= 1;
    fs::write(&path, main).unwrap();
    let before = bytes(&data);
    let error =
        match BoundSqliteDatabase::open_existing(data.lease_sqlite_files(&expected).unwrap()) {
            Err(error) => error,
            Ok(_) => panic!("corrupt main was accepted"),
        };
    assert_eq!(error.code, "DATABASE_INVALID_HEADER");
    assert_eq!(bytes(&data), before);
}

#[test]
fn failed_close_keeps_live_vfs_and_backing_leases_for_an_unfinalized_statement() {
    let _serial = serial();
    let (_root, data, expected) = fixture();
    let database =
        BoundSqliteDatabase::open_existing(data.lease_sqlite_files(&expected).unwrap()).unwrap();
    let name = database.registration.as_ref().unwrap().name.clone();
    let native = unsafe { database.connection.as_ref().unwrap().handle() };
    let mut statement = null_mut();
    assert_eq!(
        unsafe {
            ffi::sqlite3_prepare_v2(
                native,
                c"SELECT value FROM records".as_ptr(),
                -1,
                &mut statement,
                null_mut(),
            )
        },
        ffi::SQLITE_OK
    );
    let error = database.close().unwrap_err();
    assert_eq!(error.code, "DATABASE_CLOSE_FAILED");
    assert!(!unsafe { ffi::sqlite3_vfs_find(name.as_ptr()) }.is_null());
    assert!(fs::rename(
        data.path().join("main.sqlite"),
        data.path().join("not-moved")
    )
    .is_err());
    // The still-live statement can safely use the retained VFS. The intentional
    // fail-safe retention lasts until this disposable test process exits.
    assert_eq!(unsafe { ffi::sqlite3_step(statement) }, ffi::SQLITE_ROW);
    assert_eq!(unsafe { ffi::sqlite3_finalize(statement) }, ffi::SQLITE_OK);
}

fn probe(data: &WorkspaceDirectory, action: &str) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "windows_sqlite::tests::stock_subprocess_probe",
            "--nocapture",
        ])
        .env("WINDOWS_SQLITE_DISPOSABLE_PROBE_PATH", data.path())
        .env("WINDOWS_SQLITE_DISPOSABLE_PROBE_ACTION", action)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap()
}

#[test]
#[ignore = "invoked only by disposable subprocess integration tests"]
fn stock_subprocess_probe() {
    let data = PathBuf::from(std::env::var_os("WINDOWS_SQLITE_DISPOSABLE_PROBE_PATH").unwrap());
    let root = data.parent().unwrap();
    let disposable = root.parent().unwrap();
    assert_eq!(
        disposable.parent().unwrap().file_name().unwrap(),
        ".test-artifacts"
    );
    assert!(disposable
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("windows-workspace-disposable-"));
    assert_eq!(root.file_name().unwrap(), crate::APP_IDENTIFIER);
    let connection = Connection::open_with_flags(
        data.join("main.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    let mut persist = 1;
    assert_eq!(
        unsafe {
            ffi::sqlite3_file_control(
                connection.handle(),
                c"main".as_ptr(),
                ffi::SQLITE_FCNTL_PERSIST_WAL,
                (&mut persist as *mut c_int).cast(),
            )
        },
        ffi::SQLITE_OK
    );
    connection
        .execute_batch("PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    match std::env::var("WINDOWS_SQLITE_DISPOSABLE_PROBE_ACTION")
        .unwrap()
        .as_str()
    {
        "lock" => {
            connection.execute_batch("BEGIN IMMEDIATE; INSERT INTO records(value) VALUES('external process committed');").unwrap();
            fs::write(root.join("probe-ready"), b"write lock held").unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            while !root.join("probe-release").exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "parent did not release disposable helper"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            connection.execute_batch("COMMIT;").unwrap();
            connection.close().unwrap();
        }
        "commit_and_exit" => {
            connection
                .execute(
                    "INSERT INTO records(value) VALUES('uncheckpointed process commit')",
                    [],
                )
                .unwrap();
            // Controlled process exit after a durable SQLite commit, without
            // running connection destructors/checkpoints. Not a power-loss test.
            std::process::exit(0);
        }
        _ => panic!("unsupported disposable helper action"),
    }
}

#[test]
fn bound_shm_honors_a_separate_process_writer_and_reads_its_commit() {
    let _serial = serial();
    let (root, data, expected) = fixture();
    let database =
        BoundSqliteDatabase::open_existing(data.lease_sqlite_files(&expected).unwrap()).unwrap();
    let conn = database.connection.as_ref().unwrap();
    conn.busy_timeout(Duration::ZERO).unwrap();
    let mut child = probe(&data, "lock");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !root.path().join("probe-ready").exists() {
        if std::time::Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "disposable helper failed to acquire its lock: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let error = conn
        .execute(
            "INSERT INTO records(value) VALUES('must not commit across process')",
            [],
        )
        .unwrap_err();
    assert!(
        matches!(error, rusqlite::Error::SqliteFailure(code, _) if code.code == rusqlite::ErrorCode::DatabaseBusy)
    );
    fs::write(root.path().join("probe-release"), b"commit now").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        conn.query_row("SELECT value FROM records WHERE id=2", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "external process committed"
    );
    database.close().unwrap();
}

#[test]
fn bound_wal_reopens_a_commit_retained_only_in_wal_after_process_exit() {
    let _serial = serial();
    let (_root, data, expected) = fixture();
    let main_before = fs::read(data.path().join("main.sqlite")).unwrap();
    let output = probe(&data, "commit_and_exit").wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(data.path().join("main.sqlite")).unwrap(),
        main_before
    );
    assert!(
        fs::metadata(data.path().join("main.sqlite-wal"))
            .unwrap()
            .len()
            > 32
    );
    let database =
        BoundSqliteDatabase::open_existing(data.lease_sqlite_files(&expected).unwrap()).unwrap();
    assert_eq!(
        database
            .connection
            .as_ref()
            .unwrap()
            .query_row("SELECT value FROM records WHERE id=2", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "uncheckpointed process commit"
    );
    database.close().unwrap();
}

#[test]
fn corrupt_visible_page_does_not_checkpoint_an_uncheckpointed_wal_during_rejection() {
    let _serial = serial();
    let (_root, data, expected) = fixture();
    let external = stock(&data);
    let page: u32 = external
        .query_row(
            "SELECT rootpage FROM sqlite_schema WHERE name='evidence'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    external.close().unwrap();
    assert!(probe(&data, "commit_and_exit")
        .wait_with_output()
        .unwrap()
        .status
        .success());
    let mut main = fs::read(data.path().join("main.sqlite")).unwrap();
    let size = u16::from_be_bytes([main[16], main[17]]) as usize;
    let size = if size == 1 { 65536 } else { size };
    main[(page as usize - 1) * size] = 0xff;
    fs::write(data.path().join("main.sqlite"), &main).unwrap();
    let wal = fs::read(data.path().join("main.sqlite-wal")).unwrap();
    assert!(wal.len() > 32);
    let error =
        match BoundSqliteDatabase::open_existing(data.lease_sqlite_files(&expected).unwrap()) {
            Err(error) => error,
            Ok(_) => panic!("visible corrupt page was accepted"),
        };
    assert!(
        matches!(
            error.code.as_str(),
            "DATABASE_CORRUPT" | "DATABASE_SQLITE_ERROR"
        ),
        "{error:?}"
    );
    assert_eq!(fs::read(data.path().join("main.sqlite")).unwrap(), main);
    assert_eq!(fs::read(data.path().join("main.sqlite-wal")).unwrap(), wal);
    // The regenerable SHM cache may have been rebuilt while diagnosing pages;
    // this is deliberately not a claim that all corrupt-history evidence has
    // been archived. The future worker must capture such evidence before open.
    assert_eq!(data.list().unwrap().len(), 3);
}

#[test]
fn malformed_nonempty_wal_is_rejected_without_sqlite_ignoring_or_overwriting_it() {
    let _serial = serial();
    for mutation in [0usize, 24, 56, usize::MAX] {
        let (_root, data, expected) = fixture();
        assert!(probe(&data, "commit_and_exit")
            .wait_with_output()
            .unwrap()
            .status
            .success());
        let path = data.path().join("main.sqlite-wal");
        let mut wal = fs::read(&path).unwrap();
        assert!(wal.len() > 56);
        if mutation == usize::MAX {
            wal.extend_from_slice(b"torn frame");
        } else {
            wal[mutation] ^= 1;
        }
        fs::write(path, wal).unwrap();
        let before = bytes(&data);
        let error =
            match BoundSqliteDatabase::open_existing(data.lease_sqlite_files(&expected).unwrap()) {
                Err(error) => error,
                Ok(_) => panic!("malformed WAL was accepted"),
            };
        assert_eq!(error.code, "DATABASE_WAL_INVALID");
        assert_eq!(bytes(&data), before);
    }
}
