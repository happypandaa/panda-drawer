//! Exercise the same directory checkpoint used by initialize_at without
//! opening the unfinished Windows runtime gate or using any real data root.
use super::*;

fn fixture() -> (tempfile::TempDir, RuntimePaths) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-workspace-volume-checkpoint-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap().join("data");
    ensure_directory_tree(&root).unwrap();
    (temporary, RuntimePaths::at(root))
}

#[test]
fn missing_default_is_checked_then_created_without_publishing_selection() {
    let (_temporary, paths) = fixture();
    assert!(!paths.workspace_root().exists());
    paths.initialize_workspace_directories().unwrap();
    assert!(paths.workspace_root().join("projects").is_dir());
    assert!(paths.workspace_root().join(".platform").is_dir());
    assert!(!paths.settings_path().exists());
    assert!(!paths.workspace_root().join(HOME_WORKSPACE_MARKER).exists());
    workspace_admission::require_volume(paths.workspace_root()).unwrap();
}

#[test]
fn existing_nonempty_selected_workspace_is_admitted_without_content_or_acl_changes() {
    let (_temporary, paths) = fixture();
    ensure_directory_tree(paths.workspace_root()).unwrap();
    let file = paths.workspace_root().join("retained.txt");
    fs::write(&file, b"retained disposable source").unwrap();
    let before = platform_native::windows::files::open_no_reparse(paths.workspace_root())
        .unwrap()
        .identity;
    workspace_admission::require_volume(paths.workspace_root()).unwrap();
    assert!(before.same_object(
        &platform_native::windows::files::open_no_reparse(paths.workspace_root())
            .unwrap()
            .identity
    ));
    assert_eq!(fs::read(file).unwrap(), b"retained disposable source");
    assert_eq!(fs::read_dir(paths.workspace_root()).unwrap().count(), 1);
    assert!(!paths.settings_path().exists());
}

#[test]
fn ordinary_file_obstruction_is_rejected_before_creating_workspace_material() {
    let (_temporary, mut paths) = fixture();
    let obstruction = paths.data_root().join("retained.txt");
    fs::write(&obstruction, b"retain obstruction").unwrap();
    paths.workspace_root = obstruction.join("missing-workspace");
    assert!(paths.initialize_workspace_directories().is_err());
    assert_eq!(fs::read(obstruction).unwrap(), b"retain obstruction");
    assert!(!paths.workspace_root().exists());
    assert!(!paths.settings_path().exists());
}
