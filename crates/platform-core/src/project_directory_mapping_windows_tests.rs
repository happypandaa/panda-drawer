//! Only newly created disposable Windows project registrations are modified.
use super::*;

#[tokio::test]
async fn stale_native_root_reserves_its_name_without_rebinding_or_opening_that_target() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-web-claims-中文 space-")
        .tempdir()
        .unwrap();
    let workspace = temporary.path().canonicalize().unwrap();
    let store = ProjectStore::new(workspace.clone()).unwrap();
    let project = store
        .create(CreateProjectInput {
            name: "Disposable retained owner".into(),
            description: None,
        })
        .await
        .unwrap();
    let sentinel = project.project_root.join("retained-sentinel");
    fs::write(&sentinel, b"preserve disposable original").unwrap();
    let id = "prj_0123456789abcdef0123456789abcdef";
    let task = "task_windows_retained_claim";
    let name = "Disposable allocation";
    let base = crate::project_directory_name::readable_directory_name(name, id);
    let path = registration_path(&workspace, &project.id);
    let mut record: Registration = read_json(&path).unwrap();
    record.project_root = workspace
        .join("unopened-old-workspace/projects")
        .join(&base);
    let before = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(&path, &before).unwrap();

    let allocated = store.reserve_named_identity(id, task, name).unwrap();
    assert_eq!(
        allocated.file_name().unwrap().to_str().unwrap(),
        format!("{base}-2")
    );
    assert!(!allocated.exists());
    assert!(!workspace.join("unopened-old-workspace").exists());
    assert_eq!(
        store.project_root(&project.id).unwrap_err().code,
        "PROJECT_DIRECTORY_MAPPING_INVALID"
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        fs::read(&sentinel).unwrap(),
        b"preserve disposable original"
    );
    store.shutdown().await;
}

#[tokio::test]
async fn traversing_retained_root_fails_before_a_new_reservation_is_written() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-invalid-web-claims-")
        .tempdir()
        .unwrap();
    let workspace = temporary.path().canonicalize().unwrap();
    let store = ProjectStore::new(workspace.clone()).unwrap();
    let project = store
        .create(CreateProjectInput {
            name: "Disposable malformed claim".into(),
            description: None,
        })
        .await
        .unwrap();
    let path = registration_path(&workspace, &project.id);
    let mut record: Registration = read_json(&path).unwrap();
    // PathBuf::push normalizes '..' for a verbatim base; preserve the malformed
    // serialized spelling here so the product actually receives traversal.
    record.project_root = PathBuf::from(format!(
        "{}\\old\\..\\projects\\reserved-name",
        workspace.display()
    ));
    let before = serde_json::to_vec_pretty(&record).unwrap();
    fs::write(&path, &before).unwrap();
    let id = "prj_0123456789abcdef0123456789abcdef";
    let error = store
        .reserve_named_identity(id, "task_invalid_windows_claim", "Disposable new project")
        .unwrap_err();
    assert_eq!(error.code, "PROJECT_DIRECTORY_MAPPING_INVALID");
    assert!(!store.reservation_path(id).exists());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(project.project_root.join("project.json").is_file());
    store.shutdown().await;
}
