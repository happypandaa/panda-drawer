use super::*;
use crate::{control::ControlState, CreateProjectInput, RuntimePaths};
#[cfg(unix)]
use std::os::unix::fs::{symlink, MetadataExt};
use std::sync::atomic::Ordering;

async fn fixture() -> (
    tempfile::TempDir,
    Arc<ProjectStore>,
    MetadataManager,
    String,
    PathBuf,
) {
    let (temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
    let project = store
        .create(CreateProjectInput {
            name: "Metadata fixture".into(),
            description: Some("Before".into()),
        })
        .await
        .unwrap();
    let manager = MetadataManager::new(store.clone(), "instance_metadata_test".into());
    (temp, store, manager, project.id, project.project_root)
}
fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&vec![127; (width * height * 4) as usize])
            .unwrap();
    }
    bytes
}
fn manifest(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("project.json")).unwrap()).unwrap()
}
fn write_manifest(root: &Path, value: &Value) {
    fs::write(
        root.join("project.json"),
        serde_json::to_vec(value).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn metadata_reads_and_edits_without_sqlite_or_build_and_preserves_unknown_fields() {
    let (_temp, store, manager, id, root) = fixture().await;
    let epoch = store.registered_epoch(&id).unwrap();
    let mut raw = manifest(&root);
    raw["futureExtension"] = json!({"keep":true});
    write_manifest(&root, &raw);
    let database = root.join(raw["database"].as_str().unwrap());
    fs::write(&database, b"not a database").unwrap();
    let info = manager.info(&id).await.unwrap();
    assert_eq!(info["iconStatus"]["state"], "default");
    let updated=manager.update(&id,json!({"expectedRevision":info["metadataRevision"],"name":"  Renamed  ","description":null})).await.unwrap();
    assert_eq!(updated["name"], "Renamed");
    assert!(updated["manifest"].get("description").is_none());
    assert_eq!(updated["manifest"]["futureExtension"]["keep"], true);
    assert_eq!(store.registered_epoch(&id).unwrap(), epoch);
    assert!(store.active_task(&id).is_none());
    assert_eq!(fs::read(database).unwrap(), b"not a database");
}
#[tokio::test]
async fn revisions_reject_manifest_icon_and_instance_changes_without_overwriting() {
    let (_temp, store, manager, id, root) = fixture().await;
    let before = manager.info(&id).await.unwrap();
    let mut raw = manifest(&root);
    raw["description"] = json!("external edit");
    write_manifest(&root, &raw);
    let error = manager
        .update(
            &id,
            json!({"expectedRevision":before["metadataRevision"],"name":"lost"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "METADATA_CHANGED");
    assert_eq!(manifest(&root)["description"], "external edit");
    let current = manager.info(&id).await.unwrap();
    let fresh = MetadataManager::new(store, "instance_after_restart".into())
        .info(&id)
        .await
        .unwrap();
    assert_ne!(current["metadataRevision"], fresh["metadataRevision"]);
    fs::create_dir_all(root.join("meta")).unwrap();
    fs::write(root.join("meta/custom.png"), png_bytes(2, 2)).unwrap();
    raw["icon"] = json!("meta/custom.png");
    write_manifest(&root, &raw);
    let icon = manager.info(&id).await.unwrap();
    fs::write(root.join("meta/custom.png"), png_bytes(3, 2)).unwrap();
    assert_ne!(
        icon["metadataRevision"],
        manager.info(&id).await.unwrap()["metadataRevision"]
    );
}
#[tokio::test]
async fn icon_import_is_independent_and_old_icons_survive_replacement_and_removal() {
    let (temp, _store, manager, id, root) = fixture().await;
    let source = temp.path().canonicalize().unwrap().join("selected.png");
    fs::write(&source, png_bytes(2, 2)).unwrap();
    let info = manager.info(&id).await.unwrap();
    let first = manager
        .import_icon(
            &id,
            &source,
            info["metadataRevision"].as_str().unwrap(),
            None,
        )
        .await
        .unwrap();
    let first_path = root.join(first["icon"].as_str().unwrap());
    assert_eq!(first["iconStatus"]["state"], "valid");
    assert!(first["iconDataUrl"]
        .as_str()
        .unwrap()
        .starts_with("data:image/png;base64,"));
    #[cfg(unix)]
    assert_ne!(
        fs::metadata(&source).unwrap().ino(),
        fs::metadata(&first_path).unwrap().ino()
    );
    #[cfg(windows)]
    assert!(!platform_native::tool_files::Facts::capture(&source)
        .unwrap()
        .same_object(&platform_native::tool_files::Facts::capture(&first_path).unwrap()));
    let second = manager
        .import_icon(
            &id,
            &source,
            first["metadataRevision"].as_str().unwrap(),
            None,
        )
        .await
        .unwrap();
    assert_ne!(first["icon"], second["icon"]);
    assert!(first_path.exists());
    let removed = manager
        .update(
            &id,
            json!({"expectedRevision":second["metadataRevision"],"icon":null}),
        )
        .await
        .unwrap();
    assert_eq!(removed["iconStatus"]["state"], "default");
    assert!(first_path.exists());
    assert!(root.join(second["icon"].as_str().unwrap()).exists());
    assert!(manager
        .import_icon(
            &id,
            &source,
            first["metadataRevision"].as_str().unwrap(),
            None
        )
        .await
        .is_err());
    assert_eq!(fs::read_dir(root.join("meta")).unwrap().count(), 2);
}
#[tokio::test]
async fn invalid_icons_fall_back_and_explicit_bad_references_are_rejected() {
    let (_temp, _store, manager, id, root) = fixture().await;
    fs::create_dir_all(root.join("meta")).unwrap();
    fs::write(root.join("meta/bad.png"), b"not png").unwrap();
    let mut raw = manifest(&root);
    raw["icon"] = json!("meta/bad.png");
    write_manifest(&root, &raw);
    let before = manager.info(&id).await.unwrap();
    assert_eq!(before["iconStatus"]["state"], "invalid");
    assert!(before["iconDataUrl"].is_null());
    assert!(before["metadataRevision"].is_string());
    let updated = manager
        .update(
            &id,
            json!({"expectedRevision":before["metadataRevision"],"name":"Still usable"}),
        )
        .await
        .unwrap();
    for path in [
        "../outside.png",
        "/etc/image.png",
        "data:image/png;base64,AA",
        "meta/bad.png",
    ] {
        assert!(manager
            .update(
                &id,
                json!({"expectedRevision":updated["metadataRevision"],"icon":path})
            )
            .await
            .is_err());
    }
    fs::write(root.join("outside.png"), png_bytes(2, 2)).unwrap();
    #[cfg(unix)]
    {
        symlink(root.join("outside.png"), root.join("meta/link.png")).unwrap();
        assert!(icon_path(&root, "meta/link.png").is_err());
    }
    #[cfg(windows)]
    {
        let junction = root.join("meta/linked-directory");
        assert!(std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&root)
            .output()
            .unwrap()
            .status
            .success());
        assert!(icon_path(&root, "meta/linked-directory/outside.png").is_err());
        fs::remove_dir(junction).unwrap();
    }
    fs::hard_link(root.join("outside.png"), root.join("meta/hard.png")).unwrap();
    assert!(read_file(&root.join("meta/hard.png"), ICON_MAX).is_err());
}
#[tokio::test]
async fn metadata_obeys_maintenance_task_freeze_recovery_and_workspace_gates() {
    let (_temp, store, manager, id, _root) = fixture().await;
    let info = manager.info(&id).await.unwrap();
    let patch = json!({"expectedRevision":info["metadataRevision"],"name":"Gate edit"});
    let lease = store.reserve_task(&id, "task_metadata_hold").unwrap();
    assert_eq!(
        manager.update(&id, patch.clone()).await.unwrap_err().code,
        "PROJECT_BUSY"
    );
    drop(lease);
    let control = store.control(&id).unwrap();
    control.frozen.store(true, Ordering::Release);
    assert_eq!(
        manager.update(&id, patch.clone()).await.unwrap_err().code,
        "PROJECT_BUSY"
    );
    control.frozen.store(false, Ordering::Release);
    control
        .set(ControlState {
            mode: "maintenance".into(),
            maintenance_id: Some("maintenance_test".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        manager.update(&id, patch.clone()).await.unwrap_err().code,
        "PROJECT_MAINTENANCE"
    );
    let mut state = control.get();
    state.recovery_required = true;
    control.set(state).unwrap();
    assert_eq!(
        manager.update(&id, patch.clone()).await.unwrap_err().code,
        "RECOVERY_REQUIRED"
    );
    control.set(ControlState::default()).unwrap();
    store.begin_workspace_migration().unwrap();
    assert_eq!(
        manager.update(&id, patch).await.unwrap_err().code,
        "WORKSPACE_MIGRATING"
    );
}
#[test]
fn png_validation_decodes_content_and_rejects_truncation_animation_and_oversized_dimensions() {
    let valid = png_bytes(2, 3);
    assert_eq!(decode_png(&valid).unwrap(), (2, 3));
    assert!(decode_png(&valid[..valid.len() - 8]).is_err());
    let mut animated = valid.clone();
    animated.splice(8..8, [0, 0, 0, 0, b'a', b'c', b'T', b'L', 0, 0, 0, 0]);
    assert!(decode_png(&animated).is_err());
    assert!(decode_png(&png_bytes(4097, 1)).is_err());
    let mut trailing = valid;
    trailing.push(0);
    assert!(decode_png(&trailing).is_err());
}

#[tokio::test]
async fn malformed_decorative_icon_keeps_project_usable_and_can_be_removed() {
    let (_temp, store, manager, id, root) = fixture().await;
    let mut raw = manifest(&root);
    raw["icon"] = json!({"unexpected": "decorative value"});
    write_manifest(&root, &raw);
    assert!(store.project(&id).is_ok());
    let current = manager.info(&id).await.unwrap();
    assert_eq!(current["iconStatus"]["state"], "invalid");
    assert!(current["metadataRevision"].is_string());
    let repaired = manager
        .update(
            &id,
            json!({"expectedRevision": current["metadataRevision"], "icon": null}),
        )
        .await
        .unwrap();
    assert_eq!(repaired["iconStatus"]["state"], "default");
    assert!(store.project(&id).is_ok());
}

#[tokio::test]
async fn task_registered_while_metadata_waits_for_gate_prevents_commit() {
    let (_temp, store, manager, id, root) = fixture().await;
    let info = manager.info(&id).await.unwrap();
    let original = fs::read(root.join("project.json")).unwrap();
    let guard = store.gate(&id).write_owned().await;
    let task_id = id.clone();
    let edit = tokio::spawn(async move {
        manager
            .update(
                &task_id,
                json!({"expectedRevision":info["metadataRevision"], "name":"Must not commit"}),
            )
            .await
    });
    tokio::task::yield_now().await;
    let lease = store
        .reserve_task(&id, "task_registered_during_metadata_wait")
        .unwrap();
    drop(guard);
    assert_eq!(edit.await.unwrap().unwrap_err().code, "PROJECT_BUSY");
    assert_eq!(fs::read(root.join("project.json")).unwrap(), original);
    drop(lease);
}

#[tokio::test]
async fn ready_maintenance_requires_current_id_and_preserves_pending_build() {
    let (_temp, store, manager, id, _root) = fixture().await;
    let info = manager.info(&id).await.unwrap();
    let control = store.control(&id).unwrap();
    let backup_id = format!("backup_{}", Uuid::new_v4().simple());
    let backups = store.workspace_root().join("backups").join(&id);
    fs::create_dir_all(&backups).unwrap();
    fs::write(
        backups.join(format!("{backup_id}.zip")),
        b"baseline presence fixture",
    )
    .unwrap();
    control
        .set(ControlState {
            mode: "maintenance".into(),
            maintenance_id: Some("maintenance_current".into()),
            baseline_backup_id: Some(backup_id),
            ready_for_changes: true,
            pending_build: true,
            pending_build_reason: Some("source changed before metadata edit".into()),
            ..Default::default()
        })
        .unwrap();
    let mut patch = json!({"expectedRevision": info["metadataRevision"], "name": "Maintenance metadata", "maintenanceId": "maintenance_old"});
    assert_eq!(
        manager.update(&id, patch.clone()).await.unwrap_err().code,
        "PROJECT_MAINTENANCE"
    );
    patch["maintenanceId"] = json!("maintenance_current");
    manager.update(&id, patch).await.unwrap();
    assert!(control.get().pending_build);
    assert_eq!(control.get().mode, "maintenance");
}
