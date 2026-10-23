//! Native-location admission uses disposable registered projects only. These
//! tests do not launch Finder or prove the system's visual response.
use super::*;
use std::{fs, os::unix::fs::symlink};

#[tokio::test]
async fn local_directory_dispatch_holds_project_and_shutdown_leases() {
    let (_temporary, _paths, service, _client) = tests::start().await;
    let project = tests::create(&service, "Disposable local directory dispatch").await;
    let handle = service.handle();
    let state = handle.current();
    let lease = handle.project_location_lease(&project.id, "root").unwrap();
    assert_eq!(lease.location(), project.project_root);
    lease.validate().unwrap();
    assert!(state.store.has_active_tasks());
    assert!(state.store.active_task(&project.id).is_none());
    assert_eq!(state.active.load(Ordering::SeqCst), 1);
    assert_eq!(
        state
            .store
            .reserve_task(&project.id, "delete")
            .err()
            .unwrap()
            .code,
        "PROJECT_BUSY"
    );
    state.store.begin_workspace_migration().unwrap();
    assert_eq!(lease.validate().unwrap_err().code, "WORKSPACE_MIGRATING");
    state.store.resume_workspace();
    state.stopping.store(true, Ordering::SeqCst);
    assert_eq!(lease.validate().unwrap_err().code, "SERVICE_STOPPING");
    state.stopping.store(false, Ordering::SeqCst);
    drop(lease);
    assert!(!state.store.has_active_tasks());
    assert_eq!(state.active.load(Ordering::SeqCst), 0);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn local_directory_rejects_paths_missing_registration_and_replacement() {
    let (_temporary, _paths, service, _client) = tests::start().await;
    let project = tests::create(&service, "Disposable local directory boundaries").await;
    let handle = service.handle();
    for (id, target) in [
        ("../../outside", "root"),
        (&project.id, "/tmp"),
        ("prj_missing", "root"),
    ] {
        assert!(handle.project_location_lease(id, target).is_err());
    }
    let registration = handle
        .info()
        .workspace_root
        .join(".platform/project-state")
        .join(format!("{}.json", project.id));
    let registered_bytes = fs::read(&registration).unwrap();
    fs::remove_file(&registration).unwrap();
    assert_eq!(
        handle
            .project_location_lease(&project.id, "root")
            .err()
            .unwrap()
            .code,
        "PROJECT_IDENTITY_CHANGED"
    );
    fs::write(&registration, registered_bytes).unwrap();
    // A damaged project still has a useful local-directory action. The action
    // checks the registered directory, not database health or build readiness.
    fs::write(
        project.project_root.join("project.json"),
        "damaged disposable manifest",
    )
    .unwrap();
    let lease = handle.project_location_lease(&project.id, "root").unwrap();
    let retained = project
        .project_root
        .with_extension("retained-disposable-original");
    fs::rename(&project.project_root, &retained).unwrap();
    fs::create_dir(&project.project_root).unwrap();
    assert_eq!(
        lease.validate().unwrap_err().code,
        "PROJECT_LOCATION_CHANGED"
    );
    drop(lease);
    assert!(handle.project_location_lease(&project.id, "root").is_err());
    fs::remove_dir(&project.project_root).unwrap();
    symlink(&retained, &project.project_root).unwrap();
    assert!(handle.project_location_lease(&project.id, "root").is_err());
    assert!(retained.join("data/main.sqlite").is_file());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn readme_location_remains_registered_and_cannot_escape_project() {
    let (_temporary, _paths, service, _client) = tests::start().await;
    let project = tests::create(&service, "Disposable README location").await;
    let handle = service.handle();
    let lease = handle
        .project_location_lease(&project.id, "readme")
        .unwrap();
    assert_eq!(lease.location(), project.project_root.join("README.md"));
    drop(lease);
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(project.project_root.join("project.json")).unwrap())
            .unwrap();
    manifest["docs"]["readme"] = json!("../outside.md");
    fs::write(
        project.project_root.join("project.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert!(handle
        .project_location_lease(&project.id, "readme")
        .is_err());
    service.shutdown().await.unwrap();
}
