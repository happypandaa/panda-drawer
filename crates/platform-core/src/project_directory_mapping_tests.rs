//! Disposable filesystem regressions for full IDs and readable directory bindings.
use super::*;

fn fixture() -> (tempfile::TempDir, ProjectStore) {
    let temp = tempfile::tempdir().unwrap();
    let store = ProjectStore::new(temp.path().canonicalize().unwrap()).unwrap();
    (temp, store)
}

async fn create_fixture(store: &ProjectStore, name: &str) -> ProjectInfo {
    store
        .create(CreateProjectInput {
            name: name.into(),
            description: None,
        })
        .await
        .unwrap()
}

fn state_path(store: &ProjectStore, id: &str) -> PathBuf {
    registration_path(store.workspace_root(), id)
}

fn state(store: &ProjectStore, id: &str) -> Value {
    read_json(&state_path(store, id)).unwrap()
}

fn save_state(store: &ProjectStore, id: &str, value: &Value) {
    fs::write(
        state_path(store, id),
        serde_json::to_vec_pretty(value).unwrap(),
    )
    .unwrap();
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

#[tokio::test]
async fn readable_directory_is_persisted_and_display_name_changes_do_not_rename_it() {
    let (_temp, store) = fixture();
    let project = create_fixture(&store, "孩子的学习计划").await;
    assert!(project.id.starts_with("prj_"));
    assert_eq!(project.id.len(), 36);
    let directory = project.project_root.file_name().unwrap().to_str().unwrap();
    assert_eq!(
        directory,
        crate::project_directory_name::readable_directory_name("孩子的学习计划", &project.id)
    );
    assert_ne!(directory, project.id);
    assert!(directory.is_ascii());
    assert_eq!(state(&store, &project.id)["directoryName"], directory);
    let mut manifest: Value = read_json(&project.project_root.join("project.json")).unwrap();
    manifest["name"] = json!("Changed display name");
    fs::write(
        project.project_root.join("project.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let reopened = ProjectStore::new(store.workspace_root().to_owned()).unwrap();
    assert_eq!(
        reopened.project(&project.id).unwrap().root,
        project.project_root
    );
    assert_eq!(
        reopened.info(&project.id).await.unwrap().name,
        "Changed display name"
    );
    assert_eq!(reopened.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn missing_current_directory_binding_is_rejected_and_an_unregistered_id_directory_is_never_adopted(
) {
    let (_temp, store) = fixture();
    let project = create_fixture(&store, "Disposable missing registration").await;
    let unregistered = store.workspace_root().join("projects").join(&project.id);
    fs::rename(&project.project_root, &unregistered).unwrap();
    let mut record = state(&store, &project.id);
    record["projectRoot"] = json!(unregistered);
    record.as_object_mut().unwrap().remove("directoryName");
    save_state(&store, &project.id, &record);
    let bytes = fs::read(state_path(&store, &project.id)).unwrap();
    assert_eq!(
        store.project(&project.id).err().unwrap().code,
        "PROJECT_DIRECTORY_MAPPING_INVALID"
    );
    assert_eq!(fs::read(state_path(&store, &project.id)).unwrap(), bytes);
    fs::remove_file(state_path(&store, &project.id)).unwrap();
    assert_eq!(
        store.project(&project.id).err().unwrap().code,
        "PROJECT_NOT_REGISTERED"
    );
    assert!(store.list().await.unwrap().is_empty());
    assert!(enumerate_project_roots(store.workspace_root())
        .unwrap()
        .is_empty());
    assert!(!state_path(&store, &project.id).exists());
    assert!(unregistered.is_dir());
}

#[tokio::test]
async fn arbitrary_slug_manifest_cannot_claim_an_existing_or_unregistered_id() {
    let (_temp, store) = fixture();
    let project = create_fixture(&store, "Disposable legitimate mapping").await;
    let rogue = store
        .workspace_root()
        .join("projects/unregistered-readable-deadbeef");
    copy_tree(&project.project_root, &rogue);
    assert_eq!(
        enumerate_project_roots(store.workspace_root()).unwrap(),
        vec![(project.id.clone(), project.project_root.clone())]
    );
    assert_eq!(store.list().await.unwrap().len(), 1);
    fs::remove_file(state_path(&store, &project.id)).unwrap();
    assert!(enumerate_project_roots(store.workspace_root())
        .unwrap()
        .is_empty());
    assert!(store.list().await.unwrap().is_empty());
    assert!(store.project(&project.id).is_err());
    assert!(rogue.join("project.json").is_file());
}

#[tokio::test]
async fn editing_manifest_id_does_not_redirect_a_registered_directory() {
    let (_temp, store) = fixture();
    let first = create_fixture(&store, "Disposable mapping first").await;
    let second = create_fixture(&store, "Disposable mapping second").await;
    let path = first.project_root.join("project.json");
    let mut manifest: Value = read_json(&path).unwrap();
    manifest["id"] = json!(second.id);
    fs::write(path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    assert_eq!(
        store.project(&first.id).err().unwrap().code,
        "INVALID_PROJECT_MANIFEST"
    );
    assert_eq!(store.project_root(&first.id).unwrap(), first.project_root);
    assert_eq!(store.project(&second.id).unwrap().root, second.project_root);
    assert_eq!(store.list().await.unwrap().len(), 2);
}

#[tokio::test]
async fn corrupt_registration_is_diagnostic_without_hiding_healthy_projects_or_guessing_a_root() {
    let (_temp, store) = fixture();
    let broken = create_fixture(&store, "Disposable broken mapping").await;
    let healthy = create_fixture(&store, "Disposable healthy mapping").await;
    let legacy = store.workspace_root().join("projects").join(&broken.id);
    copy_tree(&broken.project_root, &legacy);
    fs::write(state_path(&store, &broken.id), b"{broken").unwrap();
    assert!(store.project_root(&broken.id).is_err());
    assert!(enumerate_project_roots(store.workspace_root()).is_err());
    let listed = store.list().await.unwrap();
    assert_eq!(listed.len(), 2);
    let diagnostic = listed
        .iter()
        .find(|project| project.id == broken.id)
        .unwrap();
    assert_eq!(diagnostic.state, "error");
    assert!(diagnostic.document_epoch.is_empty());
    assert_eq!(diagnostic.project_root, state_path(&store, &broken.id));
    assert_ne!(
        listed
            .iter()
            .find(|project| project.id == healthy.id)
            .unwrap()
            .state,
        "error"
    );
    assert!(store.project(&broken.id).is_err());
}

#[tokio::test]
async fn unsafe_or_mismatching_binding_never_falls_back_to_the_id_directory() {
    let (_temp, store) = fixture();
    let project = create_fixture(&store, "Disposable unsafe mapping").await;
    let original = state(&store, &project.id);
    for name in [
        "../outside",
        "/outside",
        ".hidden",
        "Uppercase",
        "nested/name",
    ] {
        let mut record = original.clone();
        record["directoryName"] = json!(name);
        save_state(&store, &project.id, &record);
        assert_eq!(
            store.project_root(&project.id).unwrap_err().code,
            "PROJECT_DIRECTORY_MAPPING_INVALID"
        );
    }
    let mut mismatch = original.clone();
    mismatch["projectRoot"] = json!(store.workspace_root().join("projects").join(&project.id));
    save_state(&store, &project.id, &mismatch);
    assert!(store.project_root(&project.id).is_err());
    save_state(&store, &project.id, &original);
    let name = "symlink-mapping-deadbeef";
    std::os::unix::fs::symlink(
        &project.project_root,
        store.workspace_root().join("projects").join(name),
    )
    .unwrap();
    let mut linked = original;
    linked["directoryName"] = json!(name);
    linked["projectRoot"] = json!(store.workspace_root().join("projects").join(name));
    save_state(&store, &project.id, &linked);
    assert_eq!(
        store.project_root(&project.id).unwrap_err().code,
        "UNSAFE_PATH"
    );
}

#[tokio::test]
async fn a_missing_directory_owned_by_another_registration_stays_occupied() {
    let (_temp, store) = fixture();
    let owner = create_fixture(&store, "Disposable directory owner").await;
    let id = "prj_0123456789abcdef0123456789abcdef";
    let name = "Reserved readable name";
    let candidate = crate::project_directory_name::readable_directory_name(name, id);
    let mut record = state(&store, &owner.id);
    record["directoryName"] = json!(candidate);
    record["projectRoot"] = json!(store.workspace_root().join("projects").join(&candidate));
    save_state(&store, &owner.id, &record);
    let allocated = store
        .reserve_named_identity(id, "task_missing_owner", name)
        .unwrap();
    assert_ne!(allocated.file_name().unwrap().to_str().unwrap(), candidate);
    assert_eq!(
        allocated.file_name().unwrap().to_str().unwrap(),
        format!("{candidate}-2")
    );
    assert_eq!(
        store.project_root(&owner.id).unwrap(),
        store.workspace_root().join("projects").join(candidate)
    );
}

#[tokio::test]
async fn a_same_task_reservation_survives_restart_and_does_not_admit_a_crash_candidate() {
    let (_temp, store) = fixture();
    let source = create_fixture(&store, "Disposable candidate source").await;
    let id = "prj_1123456789abcdef0123456789abcdef";
    let reserved = store
        .reserve_named_identity(id, "task_candidate_mapping", "Candidate project")
        .unwrap();
    copy_tree(&source.project_root, &reserved);
    let mut manifest: Value = read_json(&reserved.join("project.json")).unwrap();
    manifest["id"] = json!(id);
    fs::write(
        reserved.join("project.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let staging = store
        .workspace_root()
        .join(format!("projects/.creating-{id}"));
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join("evidence.txt"), b"retained candidate evidence").unwrap();
    let reopened = ProjectStore::new(store.workspace_root().to_owned()).unwrap();
    assert_eq!(
        reopened
            .reserve_named_identity(id, "task_candidate_mapping", "Changed display name")
            .unwrap(),
        reserved
    );
    reopened
        .reserve_identity(id, "task_candidate_mapping")
        .unwrap();
    assert_eq!(reopened.project_root(id).unwrap(), reserved);
    assert_eq!(
        reopened.project(id).err().unwrap().code,
        "PROJECT_REGISTRATION_PENDING"
    );
    assert_eq!(reopened.list().await.unwrap().len(), 1);
    assert!(!state_path(&reopened, id).exists());
    assert!(staging.join("evidence.txt").is_file());
    assert!(reserved.join("project.json").is_file());
    assert_eq!(
        reopened
            .reserve_named_identity(id, "task_different_owner", "Other project")
            .unwrap_err()
            .code,
        "PROJECT_ID_CONFLICT"
    );
}

#[tokio::test]
async fn partial_deletion_retains_the_readable_target_even_after_files_disappear() {
    let (_temp, store) = fixture();
    let project = create_fixture(&store, "Disposable partial deletion").await;
    let epoch = Uuid::new_v4().to_string();
    let marker = store
        .begin_deletion(
            &project.id,
            "task_named_deletion",
            &epoch,
            &identity(&project.project_root).unwrap(),
        )
        .unwrap();
    fs::remove_file(project.project_root.join("project.json")).unwrap();
    fs::remove_file(project.project_root.join("data/main.sqlite")).unwrap();
    assert_eq!(
        store.deletion_root(&project.id).unwrap().0,
        project.project_root
    );
    fs::remove_dir_all(&project.project_root).unwrap();
    assert_eq!(
        store.deletion_root(&project.id).unwrap().0,
        project.project_root
    );
    assert_eq!(
        store
            .reserve_named_identity(&project.id, "task_reuse_deleted", "New application")
            .unwrap_err()
            .code,
        "PROJECT_ID_CONFLICT"
    );
    store.complete_deleted_project_identity(&marker).unwrap();
    assert!(store.list().await.unwrap().is_empty());
    assert!(!state_path(&store, &project.id).exists());
    assert!(store.pending_deletion(&project.id).unwrap().is_none());
    assert!(enumerate_project_roots(store.workspace_root())
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn migration_requires_explicit_rebinding_and_preserves_the_actual_basename() {
    let (_source_temp, source) = fixture();
    let project = create_fixture(&source, "Disposable named migration").await;
    let (_target_temp, target) = fixture();
    let destination = target
        .workspace_root()
        .join("projects")
        .join(project.project_root.file_name().unwrap());
    copy_tree(&project.project_root, &destination);
    fs::copy(
        state_path(&source, &project.id),
        state_path(&target, &project.id),
    )
    .unwrap();
    assert_eq!(
        target.project_root(&project.id).unwrap_err().code,
        "PROJECT_DIRECTORY_MAPPING_INVALID"
    );
    let epoch = Uuid::new_v4().to_string();
    target.rebind_migrated(&project.id, &epoch).unwrap();
    assert_eq!(target.project(&project.id).unwrap().root, destination);
    assert_eq!(
        target.registered_epoch(&project.id).unwrap().as_deref(),
        Some(epoch.as_str())
    );
    assert_eq!(target.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn duplicate_registered_directory_bindings_are_not_enumerated_as_two_projects() {
    let (_temp, store) = fixture();
    let first = create_fixture(&store, "Disposable duplicate first").await;
    let second = create_fixture(&store, "Disposable duplicate second").await;
    let mut record = state(&store, &second.id);
    record["projectRoot"] = json!(first.project_root);
    record["directoryName"] = state(&store, &first.id)["directoryName"].clone();
    save_state(&store, &second.id, &record);
    assert_eq!(
        enumerate_project_roots(store.workspace_root())
            .unwrap_err()
            .code,
        "PROJECT_DIRECTORY_MAPPING_INVALID"
    );
    assert!(store.project(&second.id).is_err());
    assert!(store.project(&first.id).is_ok());
}

#[test]
fn exclusive_publication_keeps_an_existing_empty_destination_and_the_candidate() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let candidate = root.join("candidate");
    let existing = root.join("existing");
    fs::create_dir(&candidate).unwrap();
    fs::write(candidate.join("candidate.txt"), b"retained").unwrap();
    fs::create_dir(&existing).unwrap();
    let before = identity(&existing).unwrap();
    assert_eq!(
        publish_directory_noreplace(&candidate, &existing)
            .unwrap_err()
            .code,
        "PROJECT_DIRECTORY_PUBLISH_FAILED"
    );
    assert!(identity_matches(&before, &existing).unwrap());
    assert!(candidate.join("candidate.txt").is_file());
    fs::remove_dir(&existing).unwrap();
    publish_directory_noreplace(&candidate, &existing).unwrap();
    assert!(existing.join("candidate.txt").is_file());
    assert!(!candidate.exists());
}

#[tokio::test]
async fn safe_conflicting_directory_claims_both_remain_reserved_without_adopting_either() {
    let (_temp, store) = fixture();
    let owner = create_fixture(&store, "Disposable ambiguous safe owner").await;
    let first_id = "prj_1123456789abcdef0123456789abcde1";
    let second_id = "prj_1123456789abcdef0123456789abcde2";
    let first = crate::project_directory_name::readable_directory_name("First candidate", first_id);
    let second =
        crate::project_directory_name::readable_directory_name("Second candidate", second_id);
    let mut record = state(&store, &owner.id);
    record["directoryName"] = json!(first);
    // This path is only parsed lexically. No old path needs to exist.
    record["projectRoot"] = json!(format!("/unopened-history/workspace/projects/{second}"));
    save_state(&store, &owner.id, &record);
    let retained = fs::read(state_path(&store, &owner.id)).unwrap();
    for (id, name, base, task) in [
        (first_id, "First candidate", &first, "task_first_claim"),
        (second_id, "Second candidate", &second, "task_second_claim"),
    ] {
        let allocated = store.reserve_named_identity(id, task, name).unwrap();
        assert_eq!(
            allocated.file_name().unwrap().to_str().unwrap(),
            format!("{base}-2")
        );
        assert!(!allocated.exists());
    }
    assert_eq!(
        store.project_root(&owner.id).unwrap_err().code,
        "PROJECT_DIRECTORY_MAPPING_INVALID"
    );
    assert_eq!(fs::read(state_path(&store, &owner.id)).unwrap(), retained);
}

#[tokio::test]
async fn stale_claim_does_not_resolve_old_absolute_path_or_a_claimed_current_symlink() {
    let (_temp, store) = fixture();
    let owner = create_fixture(&store, "Disposable old symlink owner").await;
    let external = tempfile::tempdir().unwrap();
    let external = external.path().canonicalize().unwrap();
    fs::write(external.join("sentinel"), b"preserved external evidence").unwrap();
    let old_parent = tempfile::tempdir().unwrap();
    let old_parent = old_parent.path().canonicalize().unwrap();
    std::os::unix::fs::symlink(&external, old_parent.join("projects")).unwrap();
    let id = "prj_2123456789abcdef0123456789abcdef";
    let candidate = crate::project_directory_name::readable_directory_name("Symlink claim", id);
    let mut record = state(&store, &owner.id);
    record["directoryName"] = json!(candidate);
    record["projectRoot"] = json!(old_parent.join("projects").join(&candidate));
    save_state(&store, &owner.id, &record);
    let claimed = store.workspace_root().join("projects").join(&candidate);
    std::os::unix::fs::symlink(&external, &claimed).unwrap();
    let allocated = store
        .reserve_named_identity(id, "task_symlink_claim", "Symlink claim")
        .unwrap();
    assert_eq!(
        allocated.file_name().unwrap().to_str().unwrap(),
        format!("{candidate}-2")
    );
    assert!(fs::symlink_metadata(&claimed)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::read(external.join("sentinel")).unwrap(),
        b"preserved external evidence"
    );
    assert_eq!(fs::read_dir(&external).unwrap().count(), 1);
    assert!(store.project_root(&owner.id).is_err());
}

#[tokio::test]
async fn orphan_deletion_and_reservation_markers_keep_their_names_after_restart() {
    for kind in ["deleting", "reservations"] {
        let (_temp, store) = fixture();
        let owner = "prj_3123456789abcdef0123456789abcdef";
        let id = "prj_4123456789abcdef0123456789abcdef";
        let candidate =
            crate::project_directory_name::readable_directory_name("Retained marker", id);
        let path = store
            .workspace_root()
            .join(".platform/project-state")
            .join(kind)
            .join(format!("{owner}.json"));
        ensure_dir(path.parent().unwrap()).unwrap();
        let marker =
            json!({"projectId":owner,"taskId":"task_disposable_marker","directoryName":candidate});
        fs::write(&path, serde_json::to_vec_pretty(&marker).unwrap()).unwrap();
        let original = fs::read(&path).unwrap();
        let reopened = ProjectStore::new(store.workspace_root().to_owned()).unwrap();
        let allocated = reopened
            .reserve_named_identity(id, "task_after_restart", "Retained marker")
            .unwrap();
        assert_eq!(
            allocated.file_name().unwrap().to_str().unwrap(),
            format!("{candidate}-2")
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!allocated.exists());
    }
}

#[tokio::test]
async fn ambiguous_or_unsafe_history_still_rejects_allocation_before_any_new_write() {
    for damage in [
        "json",
        "schema",
        "id",
        "epoch",
        "name",
        "relative",
        "traversal",
        "unbounded-parent",
        "root-type",
    ] {
        let (_temp, store) = fixture();
        let owner = create_fixture(&store, "Disposable unsafe historical claim").await;
        let mut record = state(&store, &owner.id);
        match damage {
            "schema" => record["schemaVersion"] = json!(99),
            "id" => record["id"] = json!("different_owner"),
            "epoch" => record["documentEpoch"] = json!("unknown-epoch"),
            "name" => record["directoryName"] = json!("../outside"),
            "relative" => record["projectRoot"] = json!("projects/old-name"),
            "traversal" => record["projectRoot"] = json!("/old/../workspace/projects/old-name"),
            "unbounded-parent" => record["projectRoot"] = json!("/old/arbitrary/old-name"),
            "root-type" => record["projectRoot"] = json!(17),
            "json" => (),
            _ => unreachable!(),
        }
        if damage == "json" {
            fs::write(state_path(&store, &owner.id), b"{broken").unwrap();
        } else {
            save_state(&store, &owner.id, &record);
        }
        let before = fs::read(state_path(&store, &owner.id)).unwrap();
        let id = "prj_5123456789abcdef0123456789abcdef";
        let project_entries = fs::read_dir(store.workspace_root().join("projects"))
            .unwrap()
            .count();
        assert!(
            store
                .reserve_named_identity(id, "task_unknown_claim", "Disposable new project")
                .is_err(),
            "{damage}"
        );
        assert!(!store.reservation_path(id).exists(), "{damage}");
        assert_eq!(
            fs::read_dir(store.workspace_root().join("projects"))
                .unwrap()
                .count(),
            project_entries
        );
        assert_eq!(fs::read(state_path(&store, &owner.id)).unwrap(), before);
        assert!(store.project_root(&owner.id).is_err(), "{damage}");
    }
}

#[tokio::test]
async fn maximal_retained_basename_is_kept_without_expanding_it_for_unrelated_new_projects() {
    let (_temp, store) = fixture();
    let owner = create_fixture(&store, "Disposable maximal historical claim").await;
    let maximal = "a".repeat(128);
    let mut record = state(&store, &owner.id);
    record["directoryName"] = json!(maximal);
    record["projectRoot"] = json!(format!("/unopened-history/projects/{maximal}"));
    save_state(&store, &owner.id, &record);
    let names = store
        .retained_directory_names(None, Path::new("/not-a-reservation"))
        .unwrap();
    assert!(names.contains(&maximal));
    let fresh = create_fixture(&store, "Disposable unrelated short new name").await;
    assert!(fresh.project_root.file_name().unwrap().len() <= 63);
    assert_eq!(state(&store, &owner.id), record);
}

#[tokio::test]
async fn malformed_retained_name_markers_never_make_a_protected_name_reusable() {
    for kind in ["deleting", "reservations"] {
        for damage in ["json", "owner", "name", "root"] {
            let (_temp, store) = fixture();
            let owner = "prj_6123456789abcdef0123456789abcdef";
            let id = "prj_7123456789abcdef0123456789abcdef";
            let path = store
                .workspace_root()
                .join(".platform/project-state")
                .join(kind)
                .join(format!("{owner}.json"));
            ensure_dir(path.parent().unwrap()).unwrap();
            let mut record = json!({"projectId":owner,"taskId":"task_legacy_marker","directoryName":"retained-safe-name"});
            match damage {
                "owner" => record["projectId"] = json!("different_owner"),
                "name" => record["directoryName"] = json!("../unknown"),
                "root" => record["projectRoot"] = json!("/old/unknown-parent/safe-name"),
                "json" => (),
                _ => unreachable!(),
            }
            let bytes = if damage == "json" {
                b"{broken".to_vec()
            } else {
                serde_json::to_vec_pretty(&record).unwrap()
            };
            fs::write(&path, &bytes).unwrap();
            assert!(
                store
                    .reserve_named_identity(id, "task_new_marker_claim", "Disposable new project")
                    .is_err(),
                "{kind}/{damage}"
            );
            assert!(!store.reservation_path(id).exists());
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(
                fs::read_dir(store.workspace_root().join("projects"))
                    .unwrap()
                    .count(),
                0
            );
        }
    }
}
