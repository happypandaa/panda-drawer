//! Disposable state-machine fixtures. The integration layer additionally proves
//! runtime/native drain ordering and the shared descriptor-relative remover.
use super::*;
use sha2::{Digest, Sha256};
use std::{cell::Cell, os::unix::fs::PermissionsExt};

struct Fixture {
    _temporary: tempfile::TempDir,
    data: PathBuf,
    source: PathBuf,
    destination: PathBuf,
    source_identity: PersistedIdentity,
    destination_identity: PersistedIdentity,
    excluded: BTreeSet<PathBuf>,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let data = root.join("disposable-fixed-data");
        let source = root.join("disposable-old-workspace");
        let destination = root.join("disposable-new-workspace");
        for path in [&data, &source, &destination] {
            fs::create_dir(path).unwrap();
        }
        fs::create_dir_all(source.join(".platform/cache/storage")).unwrap();
        fs::create_dir_all(source.join("projects/disposable-project")).unwrap();
        fs::write(source.join("README.md"), "disposable original").unwrap();
        fs::write(
            source.join("projects/disposable-project/value.txt"),
            "old value",
        )
        .unwrap();
        fs::create_dir_all(destination.join("projects/disposable-project")).unwrap();
        fs::write(
            destination.join("projects/disposable-project/value.txt"),
            "old value",
        )
        .unwrap();
        fs::create_dir_all(destination.join(".platform")).unwrap();
        json_file(
            &destination.join(SELECTION),
            &json!({
                "schemaVersion":1,
                "taskId":"task_disposable_cleanup",
                "identifier":crate::APP_IDENTIFIER,
                "environment":crate::ENVIRONMENT,
            }),
        );
        json_file(
            &data.join("settings.json"),
            &json!({
                "migrationId":"task_disposable_cleanup",
                "workspaceRoot":destination,
            }),
        );
        let excluded = BTreeSet::from([
            PathBuf::from(".platform/cache/storage"),
            PathBuf::from(".platform/workspace-selection.json"),
            PathBuf::from(".platform/workspace-retired.json"),
            PathBuf::from(".platform/identity-binding.json"),
        ]);
        let fixture = Self {
            source_identity: PersistedIdentity::capture(&source).unwrap(),
            destination_identity: PersistedIdentity::capture(&destination).unwrap(),
            _temporary: temporary,
            data,
            source,
            destination,
            excluded,
        };
        fixture.write_copy_receipt();
        fixture
    }

    fn request(&self) -> CleanupRequest<'_> {
        CleanupRequest {
            data_root: &self.data,
            migration_id: "task_disposable_cleanup",
            source: &self.source,
            source_identity: &self.source_identity,
            destination: &self.destination,
            destination_identity: &self.destination_identity,
            published: true,
            committed: true,
            epochs_applied: true,
        }
    }

    fn state(&self) -> SourceCleanup {
        let mut state = SourceCleanup::awaiting_reopen();
        state.record_copy_exclusions(self.excluded.clone()).unwrap();
        state
    }

    fn metadata(&self, name: &str) -> PathBuf {
        self.data
            .join("migrations/task_disposable_cleanup")
            .join(name)
    }

    fn write_copy_receipt(&self) {
        let mut snapshot = snapshot(&self.source);
        snapshot.retain(|entry| {
            !self
                .excluded
                .iter()
                .any(|excluded| entry.path == *excluded || entry.path.starts_with(excluded))
        });
        let value = serde_json::to_value(snapshot).unwrap();
        json_file(&self.metadata("manifest.json"), &value);
        json_file(&self.metadata("target-manifest.json"), &value);
    }

    fn remove(&self, verified: &VerifiedSourceCleanup) -> Result<Value, ApiError> {
        verified.verify_snapshot(&serde_json::to_value(snapshot(&self.source)).unwrap())?;
        // Only the fixture's clearly named disposable root is removed here.
        // Production uses the shared prepared descriptor-relative snapshot.
        fs::remove_dir_all(&self.source)?;
        Ok(json!({"completed":true}))
    }
}

fn json_file(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn snapshot(root: &Path) -> Vec<CopyEntry> {
    fn walk(root: &Path, relative: &Path, output: &mut Vec<CopyEntry>) {
        let mut children = fs::read_dir(root.join(relative))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let path = relative.join(child.file_name());
            let meta = fs::symlink_metadata(child.path()).unwrap();
            let (kind, bytes, digest, target) = if meta.file_type().is_symlink() {
                (
                    "symlink",
                    0,
                    None,
                    Some(fs::read_link(child.path()).unwrap()),
                )
            } else if meta.is_dir() {
                ("directory", 0, None, None)
            } else {
                (
                    "file",
                    meta.len(),
                    Some(format!(
                        "{:x}",
                        Sha256::digest(fs::read(child.path()).unwrap())
                    )),
                    None,
                )
            };
            output.push(CopyEntry {
                path: path.clone(),
                kind: kind.into(),
                bytes,
                digest,
                target,
                mode: meta.permissions().mode() & 0o777,
            });
            if kind == "directory" {
                walk(root, &path, output);
            }
        }
    }
    let mut output = Vec::new();
    walk(root, Path::new(""), &mut output);
    output
}

#[test]
fn source_is_preserved_until_the_runtime_acknowledges_reopen() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    let called = Cell::new(false);
    let result = cleanup_source(
        &fixture.request(),
        &mut state,
        |_| Ok(()),
        |_| {
            called.set(true);
            panic!("must not remove before reopen")
        },
    );
    assert!(result.is_err());
    assert!(!called.get());
    assert!(fixture.source.is_dir());
    assert_eq!(state.phase, SourceCleanupPhase::AwaitingReopen);
}

#[test]
fn reopened_copy_is_removed_only_after_a_durable_start_intent() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state.acknowledge_reopened();
    let mut saved = Vec::new();
    let result = cleanup_source(
        &fixture.request(),
        &mut state,
        |state| {
            saved.push(serde_json::to_value(state).unwrap());
            Ok(())
        },
        |verified| fixture.remove(verified),
    )
    .unwrap();
    assert_eq!(saved[0]["phase"], "pending");
    assert_eq!(saved[0]["started"], true);
    assert_eq!(saved.last().unwrap()["phase"], "removed");
    assert_eq!(result["cleanupPending"], false);
    assert_eq!(result["sourceRetained"], false);
    assert!(!fixture.source.exists());
    assert_eq!(
        fs::read_to_string(
            fixture
                .destination
                .join("projects/disposable-project/value.txt")
        )
        .unwrap(),
        "old value"
    );
}

#[test]
fn new_target_business_writes_do_not_invalidate_the_committed_copy_receipt() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state.acknowledge_reopened();
    fs::write(
        fixture
            .destination
            .join("projects/disposable-project/value.txt"),
        "new valid business write",
    )
    .unwrap();
    let result = cleanup_source(
        &fixture.request(),
        &mut state,
        |_| Ok(()),
        |verified| fixture.remove(verified),
    )
    .unwrap();
    assert_eq!(result["completed"], true);
    assert_eq!(
        fs::read_to_string(
            fixture
                .destination
                .join("projects/disposable-project/value.txt")
        )
        .unwrap(),
        "new valid business write"
    );
}

#[test]
fn modified_or_new_source_content_is_retained_without_touching_the_target() {
    for changed_existing in [true, false] {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        state.acknowledge_reopened();
        let changed = if changed_existing {
            fixture.source.join("README.md")
        } else {
            fixture.source.join("new-user-file.txt")
        };
        fs::write(&changed, "new author content").unwrap();
        let result = cleanup_source(
            &fixture.request(),
            &mut state,
            |_| Ok(()),
            |verified| fixture.remove(verified),
        )
        .unwrap();
        assert_eq!(result["cleanupPending"], true);
        assert_eq!(
            result["error"]["details"]["causeCode"],
            "OLD_WORKSPACE_MODIFIED"
        );
        assert_eq!(fs::read_to_string(&changed).unwrap(), "new author content");
        fs::write(
            fixture.destination.join("still-usable.txt"),
            "business may continue",
        )
        .unwrap();
    }
}

#[test]
fn a_partial_failure_retries_only_remaining_unchanged_material() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state.acknowledge_reopened();
    let result = cleanup_source(
        &fixture.request(),
        &mut state,
        |_| Ok(()),
        |verified| {
            verified.verify_snapshot(&serde_json::to_value(snapshot(&fixture.source)).unwrap())?;
            fs::remove_file(fixture.source.join("README.md"))?;
            Err(ApiError::new(
                "DISPOSABLE_REMOVAL_FAILED",
                "Injected removal failure",
            ))
        },
    )
    .unwrap();
    assert_eq!(result["cleanupPending"], true);
    assert!(state.started);
    let restored: SourceCleanup =
        serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    state = restored;
    let result = cleanup_source(
        &fixture.request(),
        &mut state,
        |_| Ok(()),
        |verified| fixture.remove(verified),
    )
    .unwrap();
    assert_eq!(result["completed"], true);
    assert!(!fixture.source.exists());
    assert!(
        !fixture.destination.join("README.md").exists(),
        "cleanup must not rebuild source material"
    );
}

#[test]
fn a_new_file_after_partial_cleanup_is_not_adopted_by_retry() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state.acknowledge_reopened();
    state.started = true;
    fs::remove_file(fixture.source.join("README.md")).unwrap();
    fs::write(fixture.source.join("new-after-failure.txt"), "preserve me").unwrap();
    let result = cleanup_source(
        &fixture.request(),
        &mut state,
        |_| Ok(()),
        |verified| fixture.remove(verified),
    )
    .unwrap();
    assert_eq!(result["cleanupPending"], true);
    assert_eq!(
        fs::read_to_string(fixture.source.join("new-after-failure.txt")).unwrap(),
        "preserve me"
    );
}

#[test]
fn already_absent_copied_entries_need_not_be_recreated_before_cleanup() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state.acknowledge_reopened();
    fs::remove_file(fixture.source.join("README.md")).unwrap();
    let result = cleanup_source(
        &fixture.request(),
        &mut state,
        |_| Ok(()),
        |verified| fixture.remove(verified),
    )
    .unwrap();
    assert_eq!(result["completed"], true);
    assert!(!fixture.source.exists());
}

#[test]
fn replacement_source_or_target_directories_never_authorize_deletion() {
    for replace_source in [true, false] {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        state.acknowledge_reopened();
        let replaced = if replace_source {
            &fixture.source
        } else {
            &fixture.destination
        };
        let retained = replaced.with_extension("original-retained");
        fs::rename(replaced, &retained).unwrap();
        fs::create_dir(replaced).unwrap();
        fs::write(replaced.join("foreign.txt"), "foreign directory").unwrap();
        let called = Cell::new(false);
        let result = cleanup_source(
            &fixture.request(),
            &mut state,
            |_| Ok(()),
            |_| {
                called.set(true);
                panic!("replacement must not reach removal")
            },
        )
        .unwrap();
        assert_eq!(result["cleanupPending"], true);
        assert!(!called.get());
        assert_eq!(
            fs::read_to_string(replaced.join("foreign.txt")).unwrap(),
            "foreign directory"
        );
        assert!(retained.is_dir());
    }
}

#[test]
fn an_absent_source_requires_its_original_volume_and_never_deletes_another_path() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state.acknowledge_reopened();
    fs::remove_dir_all(&fixture.source).unwrap();
    let result = cleanup_source(
        &fixture.request(),
        &mut state,
        |_| Ok(()),
        |_| panic!("already absent"),
    )
    .unwrap();
    assert_eq!(result["phase"], "absent");
    assert_eq!(result["alreadyAbsent"], true);
    assert!(fixture.destination.is_dir());

    let fixture = Fixture::new();
    let mut state = fixture.state();
    state.acknowledge_reopened();
    fs::remove_dir_all(&fixture.source).unwrap();
    let mut wrong = fixture.source_identity.clone();
    wrong.volume_uuid = Some("11111111-1111-4111-8111-111111111111".into());
    let mut request = fixture.request();
    request.source_identity = &wrong;
    let result =
        cleanup_source(&request, &mut state, |_| Ok(()), |_| panic!("wrong volume")).unwrap();
    assert_eq!(result["cleanupPending"], true);
}

#[test]
fn mismatched_copy_proof_or_control_selection_keeps_the_source() {
    for mismatch in ["copy", "selection", "settings"] {
        let fixture = Fixture::new();
        let mut state = fixture.state();
        state.acknowledge_reopened();
        match mismatch {
            "copy" => json_file(&fixture.metadata("target-manifest.json"), &json!([])),
            "selection" => json_file(
                &fixture.destination.join(SELECTION),
                &json!({"taskId":"another-task"}),
            ),
            "settings" => json_file(
                &fixture.data.join("settings.json"),
                &json!({"migrationId":"another-task","workspaceRoot":fixture.destination}),
            ),
            _ => unreachable!(),
        }
        let result = cleanup_source(
            &fixture.request(),
            &mut state,
            |_| Ok(()),
            |_| panic!("invalid proof"),
        )
        .unwrap();
        assert_eq!(result["cleanupPending"], true);
        assert!(fixture.source.join("README.md").is_file());
    }
}

#[test]
fn durable_intent_failure_never_starts_removal() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state.acknowledge_reopened();
    let called = Cell::new(false);
    let result = cleanup_source(
        &fixture.request(),
        &mut state,
        |_| {
            Err(ApiError::new(
                "DISPOSABLE_PERSIST_FAILED",
                "Injected journal write failure",
            ))
        },
        |_| {
            called.set(true);
            panic!("no durable intent")
        },
    );
    assert!(result.is_err());
    assert!(!called.get());
    assert!(fixture.source.is_dir());
}

#[test]
fn completed_cleanup_never_deletes_a_later_same_path_replacement() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state.acknowledge_reopened();
    cleanup_source(
        &fixture.request(),
        &mut state,
        |_| Ok(()),
        |verified| fixture.remove(verified),
    )
    .unwrap();
    fs::create_dir(&fixture.source).unwrap();
    fs::write(fixture.source.join("foreign.txt"), "later unrelated folder").unwrap();
    cleanup_source(
        &fixture.request(),
        &mut state,
        |_| Ok(()),
        |_| panic!("must remain terminal"),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(fixture.source.join("foreign.txt")).unwrap(),
        "later unrelated folder"
    );
}

#[test]
fn excluded_cache_material_is_removed_but_arbitrary_exclusions_are_rejected() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    assert!(state
        .record_copy_exclusions(BTreeSet::from([PathBuf::from("projects")]))
        .is_err());
    state.acknowledge_reopened();
    fs::write(
        fixture
            .source
            .join(".platform/cache/storage/rebuilt-cache.json"),
        "discardable old cache",
    )
    .unwrap();
    let result = cleanup_source(
        &fixture.request(),
        &mut state,
        |_| Ok(()),
        |verified| fixture.remove(verified),
    )
    .unwrap();
    assert_eq!(result["completed"], true);
}

#[test]
fn downloaded_app_dependency_exclusion_requires_bound_root_and_rejects_replacement() {
    let mut fixture = Fixture::new();
    let root = PathBuf::from("project-sources/disposable-bound-app");
    let exclusion = root.join("node_modules");
    let original = fixture.source.join(&root);
    fs::create_dir_all(original.join("node_modules")).unwrap();
    fs::write(original.join("entry.txt"), b"copied author source").unwrap();
    fs::write(
        original.join("node_modules/rebuildable"),
        b"disposable dependency cache",
    )
    .unwrap();
    fixture.excluded.insert(exclusion.clone());
    fixture.write_copy_receipt();
    let mut state = SourceCleanup::awaiting_reopen();
    assert!(state
        .record_copy_exclusions(fixture.excluded.clone())
        .is_err());
    state
        .record_bound_copy_exclusions(
            fixture.excluded.clone(),
            BTreeMap::from([(
                exclusion,
                DependencyExclusion {
                    app_id: "00000000-0000-4000-8000-000000000751".into(),
                    root,
                    identity: PersistedIdentity::capture(&original).unwrap(),
                },
            )]),
        )
        .unwrap();
    state.acknowledge_reopened();
    assert!(authorize(&fixture.request(), &state).is_ok());
    let retained = original.with_file_name("disposable-original-bound-app");
    fs::rename(&original, &retained).unwrap();
    fs::create_dir_all(original.join("node_modules")).unwrap();
    fs::write(original.join("entry.txt"), b"copied author source").unwrap();
    fs::write(
        original.join("node_modules/replacement"),
        b"preserve foreign replacement",
    )
    .unwrap();
    let removed = Cell::new(false);
    let result = cleanup_source(
        &fixture.request(),
        &mut state,
        |_| Ok(()),
        |verified| {
            removed.set(true);
            fixture.remove(verified)
        },
    )
    .unwrap();
    assert_eq!(result["completed"], false);
    assert_eq!(
        result["error"]["details"]["causeCode"],
        "OLD_WORKSPACE_MODIFIED"
    );
    assert!(!removed.get());
    assert_eq!(
        fs::read(original.join("node_modules/replacement")).unwrap(),
        b"preserve foreign replacement"
    );
    assert_eq!(
        fs::read(retained.join("entry.txt")).unwrap(),
        b"copied author source"
    );
}
