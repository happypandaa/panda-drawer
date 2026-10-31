//! Real SQLite/WAL, file handles, full identities and native sharing policy.
//! Every path belongs to this test's newly created disposable TempDir.
use super::*;
use platform_native::windows::files as native;
use rusqlite::{Connection, OpenFlags};
use std::{
    fs,
    io::{Read, Write},
};

#[test]
fn sqlite_snapshot_flushes_the_original_rw_handle_and_keeps_wal_content() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-snapshot-flush-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let source = root.join("原数据库 WAL.sqlite");
    let destination = root.join("快照 保存.sqlite");
    let database = Connection::open(&source).unwrap();
    database.pragma_update(None, "journal_mode", "WAL").unwrap();
    database.execute_batch("CREATE TABLE disposable_note(value TEXT); INSERT INTO disposable_note VALUES('uncheckpointed WAL content');").unwrap();
    assert!(root.join("原数据库 WAL.sqlite-wal").is_file());
    let source_before = fs::read(&source).unwrap();
    crate::backups::sqlite_snapshot(&source, &destination).unwrap();
    assert_eq!(
        fs::read(&source).unwrap(),
        source_before,
        "Snapshot must not checkpoint or rewrite the live database"
    );
    let file = native::open_read_file_allowing_hard_links(&destination).unwrap();
    let expected = native::identity(&file).unwrap();
    drop(file);
    let saved =
        Connection::open_with_flags(&destination, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        saved
            .query_row("SELECT value FROM disposable_note", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "uncheckpointed WAL content"
    );
    assert_eq!(
        saved
            .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
            .unwrap(),
        "delete"
    );
    saved.close().unwrap();
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut name = destination.as_os_str().to_os_string();
        name.push(suffix);
        assert!(!std::path::PathBuf::from(name).exists());
    }
    assert!(expected.same_object(&native::open_no_reparse(&destination).unwrap().identity));
    database.close().unwrap();
}

#[test]
fn retained_rw_handle_can_flush_with_a_readonly_observer_without_changing_acl_or_hardlinks() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("ordinary snapshot");
    let alias = temporary.path().join("same object other name");
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(b"disposable payload").unwrap();
    fs::hard_link(&path, &alias).unwrap();
    let before = native::identity(&file).unwrap();
    let read = File::open(&path).unwrap();
    let snapshot = SnapshotFile::retain(file, &path).unwrap();
    snapshot.sync(&path).unwrap();
    assert!(before.same_object(&native::open_no_reparse(&alias).unwrap().identity));
    assert_eq!(fs::read(&alias).unwrap(), b"disposable payload");
    let mut bytes = Vec::new();
    (&read).read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"disposable payload");
}

#[test]
fn sqlite_name_is_pinned_and_a_wrong_original_handle_never_modifies_replacement() {
    let temporary = tempfile::tempdir().unwrap();
    let original = temporary.path().join("original");
    let held = temporary.path().join("held");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&original)
        .unwrap();
    let snapshot = SnapshotFile::retain(file, &original).unwrap();
    assert!(fs::rename(&original, &held).is_err());
    snapshot.sync(&original).unwrap();
    let first = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&original)
        .unwrap();
    fs::rename(&original, &held).unwrap();
    fs::write(&original, b"foreign replacement").unwrap();
    assert!(SnapshotFile::retain(first, &original).is_err());
    assert_eq!(fs::read(&original).unwrap(), b"foreign replacement");
    assert_eq!(fs::read(held).unwrap(), b"");
}

#[test]
fn sqlite_companion_suffix_preserves_the_original_windows_utf16_path() {
    use std::{
        ffi::OsString,
        os::windows::ffi::{OsStrExt, OsStringExt},
    };
    let original = vec![
        b'C' as u16,
        b':' as u16,
        b'\\' as u16,
        0x4e2d,
        0x6587,
        32,
        0xd800,
        b'.' as u16,
        b'd' as u16,
        b'b' as u16,
    ];
    let path = std::path::PathBuf::from(OsString::from_wide(&original));
    let result = sqlite_companion(&path, "-wal");
    let mut expected = original;
    expected.extend("-wal".encode_utf16());
    assert_eq!(
        result.as_os_str().encode_wide().collect::<Vec<_>>(),
        expected
    );
}
