use super::*;

const A: &str = "prj_disposable_git_a";
const B: &str = "prj_disposable_git_b";

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-git-inventory-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    (temporary, root)
}

fn scope(root: &Path) -> PathBuf {
    COMPONENTS
        .iter()
        .fold(root.to_path_buf(), |path, name| path.join(name))
}

#[test]
fn missing_and_empty_project_scopes_are_real_and_ignore_unrelated_material() {
    let (_temporary, root) = fixture();
    fs::write(root.join("retained"), b"unrelated disposable bytes").unwrap();
    check_project(&root, A).unwrap();
    check_workspace(&root).unwrap();
    assert!(!root.join(".platform").exists());
    let git = scope(&root);
    fs::create_dir_all(git.join(B)).unwrap();
    fs::write(
        git.join(B).join("pending.json"),
        b"other project retained record",
    )
    .unwrap();
    check_project(&root, A).unwrap();
    fs::create_dir(git.join(A)).unwrap();
    let before = native::open_no_reparse(&git.join(A)).unwrap().identity;
    check_project(&root, A).unwrap();
    assert!(before.same_object(&native::open_no_reparse(&git.join(A)).unwrap().identity));
    assert_eq!(fs::read_dir(git.join(A)).unwrap().count(), 0);
    assert_eq!(
        check_workspace(&root).unwrap_err().code,
        "PLATFORM_FEATURE_UNAVAILABLE"
    );
    assert_eq!(
        fs::read(root.join("retained")).unwrap(),
        b"unrelated disposable bytes"
    );
    assert_eq!(
        fs::read(git.join(B).join("pending.json")).unwrap(),
        b"other project retained record"
    );
}

#[test]
fn a_workspace_with_only_real_empty_project_directories_is_clear() {
    let (_temporary, root) = fixture();
    let git = scope(&root);
    for id in [A, B] {
        fs::create_dir_all(git.join(id)).unwrap();
    }
    fs::write(
        git.parent().unwrap().join("unrelated.json"),
        b"unrelated control evidence",
    )
    .unwrap();
    check_workspace(&root).unwrap();
    assert_eq!(fs::read_dir(&git).unwrap().count(), 2);
    assert_eq!(
        fs::read(git.parent().unwrap().join("unrelated.json")).unwrap(),
        b"unrelated control evidence"
    );
}

#[test]
fn pending_terminal_malformed_and_unknown_records_are_all_retained_without_interpretation() {
    for name in [
        "task_pending.json",
        "task_completed.json",
        "unrecognized-material",
    ] {
        let (_temporary, root) = fixture();
        let project = scope(&root).join(A);
        fs::create_dir_all(&project).unwrap();
        let path = project.join(name);
        let bytes = if name == "task_completed.json" {
            br#"{"schemaVersion":1,"phase":"completed"}"#.as_slice()
        } else {
            b"unverified retained bytes".as_slice()
        };
        fs::write(&path, bytes).unwrap();
        fs::hard_link(&path, root.join("retained-hardlink")).unwrap();
        let before = native::open_no_reparse(&path).unwrap().identity;
        for error in [
            check_project(&root, A).unwrap_err(),
            check_workspace(&root).unwrap_err(),
        ] {
            assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
            assert_eq!(error.details["operationOutcome"], "not_started");
            assert_eq!(error.details["inventory"], "retained");
        }
        assert!(before.same_object(&native::open_no_reparse(&path).unwrap().identity));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read(root.join("retained-hardlink")).unwrap(), bytes);
        assert_eq!(fs::read_dir(project).unwrap().count(), 1);
    }
}

#[test]
fn a_new_root_or_child_during_observation_cannot_be_reported_clear() {
    for empty in [false, true] {
        let (_temporary, root) = fixture();
        if empty {
            fs::create_dir_all(scope(&root).join(A)).unwrap();
        }
        let mut material = None;
        let error = inspect_with(&root, Some(A), |actual, path| {
            assert_eq!(
                actual,
                if empty {
                    Observation::Empty
                } else {
                    Observation::Missing
                }
            );
            if !empty {
                fs::create_dir(path).unwrap();
            }
            let appeared = path.join("appeared");
            fs::write(&appeared, b"retain appeared evidence").unwrap();
            material = Some(appeared);
            Ok(())
        })
        .unwrap_err();
        assert!(matches!(
            error.code.as_str(),
            "PROJECT_GIT_DATA_INVENTORY_UNCONFIRMED" | "PLATFORM_FEATURE_UNAVAILABLE"
        ));
        assert_eq!(
            fs::read(material.unwrap()).unwrap(),
            b"retain appeared evidence"
        );
    }
}

#[test]
fn earlier_project_cannot_become_nonempty_during_workspace_observation() {
    let (_temporary, root) = fixture();
    let git = scope(&root);
    for id in [A, B] {
        fs::create_dir_all(git.join(id)).unwrap();
    }
    let appeared = git.join(A).join("new-pending.json");
    let error = inspect_with(&root, None, |_, path| {
        if path.file_name().unwrap() == B {
            fs::write(&appeared, b"new pending evidence").unwrap();
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
    assert_eq!(fs::read(appeared).unwrap(), b"new pending evidence");
}

#[test]
fn invalid_ids_and_non_directory_scopes_do_not_create_or_hide_material() {
    let (_temporary, root) = fixture();
    assert_eq!(
        check_project(&root, "../escape").unwrap_err().code,
        "INVALID_PROJECT_ID"
    );
    assert!(!root.join(".platform").exists());
    fs::write(root.join(".platform"), b"retained ordinary file").unwrap();
    assert_eq!(
        check_workspace(&root).unwrap_err().code,
        "PROJECT_GIT_DATA_INVENTORY_UNCONFIRMED"
    );
    assert_eq!(
        fs::read(root.join(".platform")).unwrap(),
        b"retained ordinary file"
    );
}

#[test]
fn mixed_separators_and_junctions_cannot_bypass_the_fixed_scope() {
    let (_temporary, root) = fixture();
    let workspace = root.join("nested/workspace");
    fs::create_dir_all(scope(&workspace).join(A)).unwrap();
    let record = scope(&workspace).join(A).join("pending.json");
    fs::write(&record, b"retained real record").unwrap();
    let mut raw = root.as_os_str().to_os_string();
    raw.push(r"\nested/workspace");
    assert_eq!(
        check_project(&PathBuf::from(raw), A).unwrap_err().code,
        "PLATFORM_FEATURE_UNAVAILABLE"
    );
    let linked_workspace = root.join("linked-workspace");
    fs::create_dir(&linked_workspace).unwrap();
    let link = linked_workspace.join(".platform");
    let status = std::process::Command::new(std::env::var_os("ComSpec").unwrap())
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(workspace.join(".platform"))
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_eq!(
        check_project(&linked_workspace, A).unwrap_err().code,
        "PROJECT_GIT_DATA_INVENTORY_UNCONFIRMED"
    );
    assert_eq!(fs::read(record).unwrap(), b"retained real record");
    fs::remove_dir(link).unwrap();
}
