use super::*;

#[tokio::test]
async fn export_manager_uses_selected_scope_without_claiming_its_parent() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
    let maintenance = MaintenanceManager::new(store.clone(), tasks.clone());
    let lifecycle =
        LifecycleManager::new(paths.clone(), store.clone(), tasks.clone(), maintenance).unwrap();
    let root = paths
        .data_root()
        .parent()
        .unwrap()
        .join("disposable-export-installation");
    fs::create_dir(&root).unwrap();
    let manager = root.join("panda-drawer.exe");
    let cli = root.join("local-projects-dev.exe");
    let resources = root.join("resources");
    fs::write(&manager, b"manager marker").unwrap();
    fs::write(&cli, b"CLI marker").unwrap();
    fs::create_dir(&resources).unwrap();
    let source = paths.workspace_root().join("disposable-source");
    fs::create_dir(&source).unwrap();
    lifecycle.set_installation_scope(Some(
        crate::installation_scope::InstallationScope::for_test(&manager, &cli, &resources),
    ));
    for destination in [resources.join("export.zip"), manager, cli] {
        assert_eq!(
            lifecycle
                .export_destination(&destination, &source, false)
                .unwrap_err()
                .code,
            "INSTALLATION_DESTINATION_PROTECTED"
        );
    }
    let delivered = root.join("completed-external-delivery.zip");
    assert_eq!(
        lifecycle
            .export_destination(&delivered, &source, false)
            .unwrap(),
        delivered
    );
    lifecycle.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}
