//! Public Windows admission uses only a fresh disposable RuntimeService. The
//! internal copy/recovery engine has separate tests; this does not open it.
use super::*;
use std::{collections::BTreeMap, path::Path};

fn observe_tree(root: &Path) -> BTreeMap<PathBuf, Value> {
    fn visit(root: &Path, path: &Path, values: &mut BTreeMap<PathBuf, Value>) {
        let metadata = std::fs::symlink_metadata(path).unwrap();
        let identity = crate::file_identity::PersistedIdentity::capture(path).unwrap();
        values.insert(path.strip_prefix(root).unwrap().to_owned(), json!({
            "identity":identity,
            "directory":metadata.is_dir(),
            "length":if metadata.is_file(){Some(metadata.len())}else{None},
            "modified":metadata.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_nanos().to_string(),
            "readOnly":metadata.permissions().readonly(),
        }));
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), values);
            }
        }
    }
    let mut values = BTreeMap::new();
    visit(root, root, &mut values);
    values
}

#[tokio::test]
async fn new_public_windows_migration_rejects_without_task_directory_or_journal_writes() {
    let (temporary, paths, service, client) = tests::start().await;
    let root = temporary.path().canonicalize().unwrap();
    let absent = root.join("disposable-never-created-destination");
    let retained = root.join("disposable-retained-destination");
    std::fs::create_dir(&retained).unwrap();
    std::fs::write(retained.join("keep"), b"preserve artificial destination").unwrap();
    let handle = service.handle();
    let before = observe_tree(&root);
    for input in [
        json!({"destinationPath":absent}),
        json!({"destinationPath":retained}),
        json!({}),
    ] {
        let error = handle.workspace_migrate(input).await.unwrap_err();
        assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
        assert_eq!(error.details["capability"], "workspace-migration");
        assert_eq!(error.details["operationOutcome"], "not_started");
    }
    // HTTP callers must use the management UI; the desktop handle retains the
    // Windows capability refusal. No real discovery or fixed data root is read.
    let response = client
        .post(format!(
            "{}/api/v1/workspace/migrations",
            service.info().base_url
        ))
        .bearer_auth(&service.shared.snapshot().token)
        .json(&json!({"destinationPath":absent}))
        .send()
        .await
        .unwrap();
    assert!(!response.status().is_success());
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["error"]["code"], "MANAGEMENT_UI_REQUIRED");
    assert!(!service.shared.migration_active.load(Ordering::Acquire));
    assert!(!absent.exists());
    assert_eq!(
        std::fs::read(retained.join("keep")).unwrap(),
        b"preserve artificial destination"
    );
    assert_eq!(
        before,
        observe_tree(&root),
        "rejected public admission wrote disposable migration/task material"
    );
    assert!(paths.workspace_root().exists());
    service.shutdown().await.unwrap();
}
