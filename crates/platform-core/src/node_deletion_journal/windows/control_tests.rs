use super::*;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w5-record-cas-中文 space-")
        .tempdir()
        .unwrap();
    let directory = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("private-task");
    private_directory(&directory, true).unwrap();
    atomic_record(&directory, None, br#"{"revision":1}"#).unwrap();
    (temporary, directory)
}

#[test]
fn every_native_cas_boundary_recovers_only_its_exact_flushed_revision() {
    for point in [
        Point::IntentPublished,
        Point::PreviousCaptured,
        Point::NextPublished,
        Point::PreviousRemoved,
    ] {
        let (_temporary, directory) = fixture();
        let original_id = PersistedIdentity::capture(&directory.join(RECORD_NAME)).unwrap();
        assert!(atomic_with(
            &directory,
            Some(br#"{"revision":1}"#),
            br#"{"revision":2}"#,
            |seen| if seen == point {
                Err(error("disposable interruption"))
            } else {
                Ok(())
            }
        )
        .is_err());
        assert!(directory.join(CHANGE).is_file());
        assert!(authored_next(&directory, br#"{"revision":2}"#).unwrap());
        // Read-only inventory does not retire the intent or move any name.
        let before = fs::read(directory.join(CHANGE)).unwrap();
        assert_eq!(inventory_record(&directory).unwrap(), br#"{"revision":2}"#);
        assert_eq!(fs::read(directory.join(CHANGE)).unwrap(), before);
        resume(&directory).unwrap();
        assert_eq!(
            read_record(&directory.join(RECORD_NAME)).unwrap().1,
            br#"{"revision":2}"#
        );
        assert!(!original_id
            .matches_path(&directory.join(RECORD_NAME))
            .unwrap());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    }
}

#[test]
fn stale_expected_bytes_never_overwrite_the_actual_record() {
    let (_temporary, directory) = fixture();
    atomic_record(&directory, Some(br#"{"revision":1}"#), br#"{"revision":2}"#).unwrap();
    let identity = PersistedIdentity::capture(&directory.join(RECORD_NAME)).unwrap();
    assert!(atomic_record(&directory, Some(br#"{"revision":1}"#), br#"{"revision":3}"#).is_err());
    assert!(identity.matches_path(&directory.join(RECORD_NAME)).unwrap());
    assert_eq!(
        fs::read(directory.join(RECORD_NAME)).unwrap(),
        br#"{"revision":2}"#
    );
}

#[test]
fn foreign_current_candidate_or_capture_is_preserved_and_blocks_resume() {
    for selected in ["current", "candidate", "capture"] {
        let (_temporary, directory) = fixture();
        assert!(atomic_with(
            &directory,
            Some(br#"{"revision":1}"#),
            br#"{"revision":2}"#,
            |point| if point == Point::PreviousCaptured {
                Err(error("disposable interruption"))
            } else {
                Ok(())
            }
        )
        .is_err());
        let scope = directory_file(&directory).unwrap();
        let active = Active::open(&directory.join(CHANGE), &scope)
            .unwrap()
            .unwrap();
        let path = match selected {
            "current" => directory.join(RECORD_NAME),
            "candidate" => active.change.candidate(&directory),
            _ => active.change.captured(&directory),
        };
        drop(active);
        if path.exists() {
            fs::rename(&path, path.with_extension("retained-original")).unwrap();
        }
        let mut file = native::create_private_file(&path).unwrap();
        file.write_all(b"disposable foreign control bytes").unwrap();
        file.sync_all().unwrap();
        drop(file);
        let identity = PersistedIdentity::capture(&path).unwrap();
        let before = fs::read(directory.join(CHANGE)).unwrap();
        assert!(resume(&directory).is_err());
        assert!(identity.matches_path(&path).unwrap());
        assert_eq!(
            fs::read(&path).unwrap(),
            b"disposable foreign control bytes"
        );
        assert_eq!(fs::read(directory.join(CHANGE)).unwrap(), before);
    }
}

#[test]
fn committed_readonly_record_load_and_cas_keep_the_original_record_file_policy() {
    let (_temporary, directory) = fixture();
    let record = directory.join(RECORD_NAME);
    native::set_read_only(&record, true).unwrap();
    assert_eq!(read_record(&record).unwrap().1, br#"{"revision":1}"#);
    atomic_record(&directory, Some(br#"{"revision":1}"#), br#"{"revision":2}"#).unwrap();
    assert_eq!(fs::read(record).unwrap(), br#"{"revision":2}"#);
}

#[test]
fn altered_intent_or_high_half_owner_id_never_acquires_recovery_authority() {
    for corrupt in [false, true] {
        let (_temporary, directory) = fixture();
        assert!(atomic_with(
            &directory,
            Some(br#"{"revision":1}"#),
            br#"{"revision":2}"#,
            |point| if point == Point::IntentPublished {
                Err(error("disposable interruption"))
            } else {
                Ok(())
            }
        )
        .is_err());
        let intent = directory.join(CHANGE);
        if corrupt {
            fs::write(&intent, b"malformed disposable intent").unwrap();
        } else {
            let mut change: Change = serde_json::from_slice(&fs::read(&intent).unwrap()).unwrap();
            change.owner.windows_file_id.as_mut().unwrap()[15] ^= 1;
            fs::write(&intent, serde_json::to_vec(&change).unwrap()).unwrap();
        }
        let bytes = fs::read(&intent).unwrap();
        assert!(resume(&directory).is_err());
        assert_eq!(fs::read(&intent).unwrap(), bytes);
        assert_eq!(
            fs::read(directory.join(RECORD_NAME)).unwrap(),
            br#"{"revision":1}"#
        );
    }
}

#[test]
fn identical_json_in_a_foreign_object_is_not_a_publication_receipt() {
    let (_temporary, directory) = fixture();
    let next = br#"{"revision":2}"#;
    let mut written = None;
    assert!(atomic_with_receipt(
        &directory,
        Some(br#"{"revision":1}"#),
        next,
        &mut written,
        |point| if point == Point::NextPublished {
            Err(error("disposable interruption"))
        } else {
            Ok(())
        }
    )
    .is_err());
    assert!(is_published(&directory, written.as_ref().unwrap()).unwrap());
    let current = directory.join(RECORD_NAME);
    let retained = directory.join("retained-original-next");
    fs::rename(&current, &retained).unwrap();
    let mut foreign = native::create_private_file(&current).unwrap();
    foreign.write_all(next).unwrap();
    foreign.sync_all().unwrap();
    drop(foreign);
    let identity = PersistedIdentity::capture(&current).unwrap();
    assert!(!is_published(&directory, written.as_ref().unwrap()).unwrap());
    assert!(resume(&directory).is_err());
    assert!(identity.matches_path(&current).unwrap());
    assert_eq!(fs::read(&current).unwrap(), next);
    assert_eq!(fs::read(&retained).unwrap(), next);
}
