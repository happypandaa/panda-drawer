use super::*;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    staged: PathBuf,
    current: PathBuf,
    next: DirectoryId,
    previous: Option<DirectoryId>,
}
impl Fixture {
    fn new(existing: bool) -> Self {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-artifact-exchange-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        // Ordinary caller-owned directories deliberately do not require the
        // journal's private DACL policy on artifact roots or parent directories.
        let task = root.join("task 中文 space");
        let project = root.join("project");
        fs::create_dir(&task).unwrap();
        fs::create_dir(&project).unwrap();
        let staged = task.join("publication");
        let current = project.join("dist");
        write_directory(&staged, b"new artifact");
        if existing {
            write_directory(&current, b"old artifact");
        }
        let next = directory(&staged).unwrap().unwrap();
        let previous = directory(&current).unwrap();
        Self {
            _temporary: temporary,
            root,
            staged,
            current,
            next,
            previous,
        }
    }
    fn interrupt(&self, point: Point) {
        let error = replace_with(&self.staged, &self.current, &mut |at, _| {
            if at == point {
                Err(ApiError::new(
                    "TEST_EXCHANGE_INTERRUPTED",
                    "Disposable exchange breakpoint",
                ))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        unknown(&error);
        assert!(pending(&self.staged, &self.current).unwrap());
    }
    fn complete(&self) {
        assert_eq!(directory(&self.current).unwrap(), Some(self.next));
        assert_eq!(
            fs::read(self.current.join("index.html")).unwrap(),
            b"new artifact"
        );
        assert_eq!(directory(&self.staged).unwrap(), self.previous);
        if self.previous.is_some() {
            assert_eq!(
                fs::read(self.staged.join("index.html")).unwrap(),
                b"old artifact"
            );
        }
        assert!(!pending(&self.staged, &self.current).unwrap());
        assert!(!resume_pending(&self.staged, &self.current).unwrap());
        assert!(!journal_path(&self.staged).exists());
        assert!(fs::read_dir(self.staged.parent().unwrap())
            .unwrap()
            .all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".artifact-directory-")
            }));
    }
    fn transaction(&self) -> Transaction {
        serde_json::from_slice(&fs::read(journal_path(&self.staged)).unwrap()).unwrap()
    }
    fn held(&self) -> PathBuf {
        self.staged.parent().unwrap().join(self.transaction().held)
    }
}
fn write_directory(path: &Path, bytes: &[u8]) {
    fs::create_dir(path).unwrap();
    fs::write(path.join("index.html"), bytes).unwrap();
}
fn unknown(error: &ApiError) {
    assert_eq!(error.code, "BUILD_ARTIFACT_EXCHANGE_UNCONFIRMED");
    assert_eq!(error.details["operationOutcome"], "unknown");
    assert_eq!(error.details["artifactExchangePending"], true);
}

#[test]
fn fresh_publication_and_existing_exchange_have_the_original_swap_result() {
    for existing in [false, true] {
        let fixture = Fixture::new(existing);
        replace_directory(&fixture.staged, &fixture.current).unwrap();
        fixture.complete();
    }
}

#[test]
fn every_exchange_breakpoint_resumes_only_the_exact_recorded_state() {
    for point in [
        Point::JournalReady,
        Point::PreviousHeld,
        Point::NextPublished,
        Point::Exchanged,
    ] {
        let fixture = Fixture::new(true);
        fixture.interrupt(point);
        assert!(resume_pending(&fixture.staged, &fixture.current).unwrap());
        fixture.complete();
    }
}

#[test]
fn fresh_publication_breakpoints_resume_without_requiring_the_moved_source_name() {
    for point in [Point::JournalReady, Point::NextPublished] {
        let fixture = Fixture::new(false);
        fixture.interrupt(point);
        // The production entry point recognizes a pending transaction before
        // rechecking the source, which is absent after its successful move.
        replace_directory(&fixture.staged, &fixture.current).unwrap();
        fixture.complete();
    }
}

#[test]
fn same_object_exchange_is_a_noop_and_missing_query_does_not_create_a_parent() {
    let fixture = Fixture::new(false);
    replace_directory(&fixture.staged, &fixture.staged).unwrap();
    assert_eq!(directory(&fixture.staged).unwrap(), Some(fixture.next));
    assert!(!journal_path(&fixture.staged).exists());
    let absent_current = fixture.root.join("not-created/dist");
    assert!(!pending(&fixture.staged, &absent_current).unwrap());
    assert!(!resume_pending(&fixture.staged, &absent_current).unwrap());
    assert!(!absent_current.parent().unwrap().exists());
}

#[test]
fn fresh_target_can_create_its_missing_parent_at_the_original_checkpoint() {
    let mut fixture = Fixture::new(false);
    fixture.current = fixture.root.join("not-created/nested/dist");
    assert!(!fixture.current.parent().unwrap().exists());
    replace_directory(&fixture.staged, &fixture.current).unwrap();
    fixture.complete();
}

#[test]
fn native_utf16_leaf_names_survive_journaling_and_exchange_without_text_admission() {
    let mut fixture = Fixture::new(true);
    let mut units: Vec<u16> = "publication-中文-".encode_utf16().collect();
    units.push(0xd800);
    let selected = fixture
        .staged
        .parent()
        .unwrap()
        .join(OsString::from_wide(&units));
    fs::rename(&fixture.staged, &selected).unwrap();
    fixture.staged = selected;
    fixture.interrupt(Point::PreviousHeld);
    assert_eq!(fixture.transaction().staged_name.0, units);
    assert!(resume_pending(&fixture.staged, &fixture.current).unwrap());
    fixture.complete();
}

#[test]
fn occupied_held_or_current_names_are_not_overwritten_even_when_empty() {
    for occupied_at in [Point::JournalReady, Point::PreviousHeld] {
        let fixture = Fixture::new(true);
        let mut foreign = None;
        let mut foreign_id = None;
        let error = replace_with(
            &fixture.staged,
            &fixture.current,
            &mut |point, transaction| {
                if point == occupied_at {
                    let path = if point == Point::JournalReady {
                        fixture.staged.parent().unwrap().join(&transaction.held)
                    } else {
                        fixture.current.clone()
                    };
                    fs::create_dir(&path).unwrap();
                    foreign_id = directory(&path).unwrap();
                    foreign = Some(path);
                }
                Ok(())
            },
        )
        .unwrap_err();
        unknown(&error);
        let foreign = foreign.unwrap();
        assert_eq!(directory(&foreign).unwrap(), foreign_id);
        assert_eq!(fs::read_dir(&foreign).unwrap().count(), 0);
        assert!(resume_pending(&fixture.staged, &fixture.current).is_err());
        assert_eq!(directory(&foreign).unwrap(), foreign_id);
        assert_eq!(directory(&fixture.staged).unwrap(), Some(fixture.next));
        let old = if occupied_at == Point::JournalReady {
            fixture.current.clone()
        } else {
            fixture.held()
        };
        assert_eq!(directory(&old).unwrap(), fixture.previous);
    }
}

#[test]
fn substituted_source_or_held_objects_are_retained_without_further_moves() {
    for point in [Point::PreviousHeld, Point::NextPublished] {
        let fixture = Fixture::new(true);
        fixture.interrupt(point);
        let selected = if point == Point::PreviousHeld {
            fixture.staged.clone()
        } else {
            fixture.held()
        };
        let original = directory(&selected).unwrap();
        let retained = fixture.root.join("retained-original");
        fs::rename(&selected, &retained).unwrap();
        write_directory(&selected, b"foreign replacement");
        let foreign = directory(&selected).unwrap();
        unknown(&resume_pending(&fixture.staged, &fixture.current).unwrap_err());
        assert_eq!(directory(&selected).unwrap(), foreign);
        assert_eq!(directory(&retained).unwrap(), original);
        assert_eq!(
            fs::read(selected.join("index.html")).unwrap(),
            b"foreign replacement"
        );
        if point == Point::PreviousHeld {
            assert!(directory(&fixture.current).unwrap().is_none());
        } else {
            assert_eq!(directory(&fixture.current).unwrap(), Some(fixture.next));
        }
        assert!(journal_path(&fixture.staged).is_file());
    }
}

#[test]
fn parent_replacement_is_rejected_even_when_the_original_leaf_is_moved_back() {
    let fixture = Fixture::new(true);
    fixture.interrupt(Point::JournalReady);
    let parent = fixture.current.parent().unwrap();
    let held_parent = fixture.root.join("retained-original-parent");
    fs::rename(parent, &held_parent).unwrap();
    fs::create_dir(parent).unwrap();
    fs::rename(held_parent.join("dist"), &fixture.current).unwrap();
    assert_eq!(directory(&fixture.current).unwrap(), fixture.previous);
    unknown(&resume_pending(&fixture.staged, &fixture.current).unwrap_err());
    assert_eq!(directory(&fixture.current).unwrap(), fixture.previous);
    assert_eq!(directory(&fixture.staged).unwrap(), Some(fixture.next));
    assert!(held_parent.is_dir());
    assert!(journal_path(&fixture.staged).is_file());
}

#[test]
fn staged_parent_replacement_is_rejected_even_with_the_original_journal_and_source() {
    let fixture = Fixture::new(true);
    fixture.interrupt(Point::JournalReady);
    let parent = fixture.staged.parent().unwrap();
    let retained = fixture.root.join("retained-original-task-parent");
    fs::rename(parent, &retained).unwrap();
    fs::create_dir(parent).unwrap();
    fs::rename(retained.join("publication"), &fixture.staged).unwrap();
    fs::rename(retained.join(JOURNAL), journal_path(&fixture.staged)).unwrap();
    unknown(&resume_pending(&fixture.staged, &fixture.current).unwrap_err());
    assert_eq!(directory(&fixture.staged).unwrap(), Some(fixture.next));
    assert_eq!(directory(&fixture.current).unwrap(), fixture.previous);
    assert!(retained.is_dir());
    assert!(journal_path(&fixture.staged).is_file());
}

#[test]
fn complete_file_id_and_exact_pair_are_required_for_journal_recovery() {
    let fixture = Fixture::new(true);
    fixture.interrupt(Point::JournalReady);
    let mut transaction = fixture.transaction();
    transaction.next.file_id[15] ^= 0x80;
    fs::write(
        journal_path(&fixture.staged),
        serde_json::to_vec(&transaction).unwrap(),
    )
    .unwrap();
    unknown(&pending(&fixture.staged, &fixture.current).unwrap_err());
    unknown(&resume_pending(&fixture.staged, &fixture.current).unwrap_err());
    let unrelated = fixture.root.join("other-dist");
    write_directory(&unrelated, b"unrelated current");
    let unrelated_identity = directory(&unrelated).unwrap();
    unknown(&resume_pending(&fixture.staged, &unrelated).unwrap_err());
    assert_eq!(directory(&unrelated).unwrap(), unrelated_identity);
    assert_eq!(directory(&fixture.current).unwrap(), fixture.previous);
    assert_eq!(directory(&fixture.staged).unwrap(), Some(fixture.next));
}

#[test]
fn locked_live_journal_serializes_another_exchange_invocation() {
    let fixture = Fixture::new(true);
    let mut observed = false;
    replace_with(&fixture.staged, &fixture.current, &mut |point, _| {
        if point == Point::JournalReady {
            unknown(&replace_directory(&fixture.staged, &fixture.current).unwrap_err());
            assert_eq!(directory(&fixture.current).unwrap(), fixture.previous);
            assert_eq!(directory(&fixture.staged).unwrap(), Some(fixture.next));
            observed = true;
        }
        Ok(())
    })
    .unwrap();
    assert!(observed);
    fixture.complete();
}

#[test]
fn a_post_move_replacement_is_unknown_and_is_never_used_as_rollback_authority() {
    let fixture = Fixture::new(true);
    let retained_new = fixture.root.join("retained-new-artifact");
    let mut replacement = None;
    let error = replace_with(&fixture.staged, &fixture.current, &mut |point, _| {
        if point == Point::NextPublished {
            fs::rename(&fixture.current, &retained_new).unwrap();
            write_directory(&fixture.current, b"foreign after publication");
            replacement = directory(&fixture.current).unwrap();
        }
        Ok(())
    })
    .unwrap_err();
    unknown(&error);
    assert_eq!(directory(&retained_new).unwrap(), Some(fixture.next));
    assert_eq!(directory(&fixture.current).unwrap(), replacement);
    assert_eq!(
        fs::read(fixture.current.join("index.html")).unwrap(),
        b"foreign after publication"
    );
    assert_eq!(directory(&fixture.held()).unwrap(), fixture.previous);
    assert!(!fixture.staged.exists());
    unknown(&resume_pending(&fixture.staged, &fixture.current).unwrap_err());
    assert_eq!(directory(&fixture.current).unwrap(), replacement);
}

#[test]
fn journal_replacement_after_the_final_move_preserves_both_records_and_directories() {
    let fixture = Fixture::new(true);
    let retained = fixture.root.join("retained-original-journal");
    let original_journal = journal_path(&fixture.staged);
    let error = replace_with(&fixture.staged, &fixture.current, &mut |point, _| {
        if point == Point::Exchanged {
            fs::rename(&original_journal, &retained).unwrap();
            fs::write(&original_journal, b"foreign journal").unwrap();
        }
        Ok(())
    })
    .unwrap_err();
    unknown(&error);
    assert_eq!(directory(&fixture.current).unwrap(), Some(fixture.next));
    assert_eq!(directory(&fixture.staged).unwrap(), fixture.previous);
    assert_eq!(fs::read(&original_journal).unwrap(), b"foreign journal");
    let original: Transaction = serde_json::from_slice(&fs::read(&retained).unwrap()).unwrap();
    assert_eq!(original.next, fixture.next);
    unknown(&resume_pending(&fixture.staged, &fixture.current).unwrap_err());
    assert!(retained.is_file());
    assert_eq!(fs::read(&original_journal).unwrap(), b"foreign journal");
}
