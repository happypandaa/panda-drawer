//! Only disposable resource markers are written; no selected tool is executed.
use super::*;
use std::fs;

fn resources(root: &std::path::Path) -> PathBuf {
    for relative in [
        "sdk/sdk.js",
        "sdk/panda.d.ts",
        "schemas/panda.schema.json",
        "template/template.json",
        "template/app/package.json",
        "template/app/package-lock.json",
        "template/app/README.md",
        "template/app/src/panda.d.ts",
        "skills/local-projects-app/SKILL.md",
        "skills/local-projects-app/references/app-window-runtime.md",
    ] {
        let file = root.join(relative);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, b"disposable installation marker; never execute").unwrap();
    }
    let cli = root.parent().unwrap().join("local-projects-dev.exe");
    fs::write(&cli, b"disposable CLI marker; never execute").unwrap();
    cli
}

#[tokio::test]
async fn desktop_selected_resources_reach_info_storage_and_download_boundary() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let root = paths
        .data_root()
        .parent()
        .unwrap()
        .join("disposable-desktop-installation");
    let resources_root = root.join("resources");
    let cli = resources(&resources_root);
    fs::create_dir(root.join("unselected-neighbor")).unwrap();
    fs::write(root.join("unselected-neighbor/large"), vec![0; 10000]).unwrap();
    let lock = paths.acquire_lock().unwrap();
    let service = RuntimeService::start_desktop_with_resources(
        paths.clone(),
        lock,
        resources_root.clone(),
        cli.clone(),
    )
    .await
    .unwrap();
    let handle = service.handle();
    handle.complete_native_startup();
    let info = handle.platform_info();
    assert_eq!(info["installationScope"]["kind"], "selectedWindowsMembers");
    let members = info["installationScope"]["members"].as_array().unwrap();
    assert_eq!(
        members[0]["path"],
        json!(fs::canonicalize(std::env::current_exe().unwrap()).unwrap())
    );
    assert_eq!(members[1]["path"], json!(cli));
    assert_eq!(members[2]["path"], json!(resources_root));
    let response = handle.workspace_storage().await.unwrap();
    assert_eq!(response["platformProgram"]["complete"], true, "{response}");
    assert!(response["disks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|disk| !disk["availableBytes"].is_null()));
    assert_eq!(
        response["platformProgram"]["roots"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        handle
            .validate_download_destination(&resources_root.join("sdk/save.zip"))
            .unwrap_err()
            .code,
        "INSTALLATION_DESTINATION_PROTECTED"
    );
    assert_eq!(
        handle.validate_download_destination(&cli).unwrap_err().code,
        "INSTALLATION_DESTINATION_PROTECTED"
    );
    handle
        .validate_download_destination(&root.join("unselected-neighbor/completed-delivery.zip"))
        .unwrap();
    let first = response["platformProgram"]["installationBinding"]["resourcesSignature"].clone();
    fs::write(
        resources_root.join("new-installed-resource"),
        b"freshly measured resource",
    )
    .unwrap();
    let refreshed = handle.workspace_storage().await.unwrap();
    assert_eq!(
        refreshed["platformProgram"]["complete"], true,
        "{refreshed}"
    );
    assert_ne!(
        refreshed["platformProgram"]["installationBinding"]["resourcesSignature"],
        first
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn headless_resource_constructor_keeps_the_program_scope_unknown() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let root = paths
        .data_root()
        .parent()
        .unwrap()
        .join("disposable-headless-resources");
    let cli = resources(&root);
    let lock = paths.acquire_lock().unwrap();
    let service = RuntimeService::start_with_resources(paths, lock, root.clone(), cli)
        .await
        .unwrap();
    let handle = service.handle();
    assert!(handle.platform_info()["installationScope"].is_null());
    let response = handle.workspace_storage().await.unwrap();
    assert!(response["platformProgram"]["totalBytes"].is_null());
    assert_eq!(response["platformProgram"]["complete"], false);
    service.shutdown().await.unwrap();
}
