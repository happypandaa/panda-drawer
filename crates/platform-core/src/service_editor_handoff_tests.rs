use super::*;

#[tokio::test]
async fn editor_handoff_rejects_stale_identity_and_holds_project_until_dispatch() {
    let (_temp, _paths, service, _client) = tests::start().await;
    let project = tests::create(&service, "Disposable editor handoff").await;
    let handle = service.handle();
    let root = PathBuf::from(&project.project_root);
    let instance = handle.info().instance_id;
    let state = handle.current();
    for (expected_root, expected_instance) in [
        (root.clone(), "old-instance".to_owned()),
        (root.join("retained-old-root"), instance.clone()),
    ] {
        let error = handle
            .editor_handoff_lease(&project.id, &expected_root, &expected_instance)
            .err()
            .unwrap();
        assert_eq!(error.code, "HANDOFF_CHANGED");
        assert!(!state.store.has_active_tasks());
        assert_eq!(state.active.load(Ordering::SeqCst), 0);
    }
    let lease = handle
        .editor_handoff_lease(&project.id, &root, &instance)
        .unwrap();
    lease.validate().unwrap();
    assert_eq!(lease.project_root, root);
    assert!(
        state.store.has_active_tasks(),
        "migration waits for editor dispatch"
    );
    assert!(state.store.active_task(&project.id).is_none());
    assert!(
        state.maintenance.status(&project.id).unwrap()["activeTask"].is_null(),
        "a transient dispatch must not be exposed as a nonexistent persistent task"
    );
    assert_eq!(
        state.active.load(Ordering::SeqCst),
        1,
        "shutdown drains editor dispatch"
    );
    assert_eq!(
        state
            .store
            .reserve_task(&project.id, "delete")
            .err()
            .unwrap()
            .code,
        "PROJECT_BUSY"
    );
    assert_eq!(
        handle
            .editor_handoff_lease(&project.id, &root, &instance)
            .err()
            .unwrap()
            .code,
        "PROJECT_BUSY"
    );
    assert_eq!(state.active.load(Ordering::SeqCst), 1);
    state.stopping.store(true, Ordering::SeqCst);
    assert_eq!(lease.validate().unwrap_err().code, "SERVICE_STOPPING");
    state.stopping.store(false, Ordering::SeqCst);
    drop(lease);
    assert!(!state.store.has_active_tasks());
    assert_eq!(state.active.load(Ordering::SeqCst), 0);
    service.shutdown().await.unwrap();
}
