use super::*;

#[test]
fn absent_data_restore_guard_is_read_only_and_clear() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().canonicalize().unwrap();
    let state = inspect(&workspace, "project_disposable_guard");
    assert!(state.pending.is_empty() && state.issues.is_empty());
    check_business_available(&workspace, "project_disposable_guard").unwrap();
    check_workspace_available(&workspace).unwrap();
    assert!(!workspace.join(ROOT).exists());
}

#[test]
fn missing_corrupt_and_unknown_journals_block_without_adopting_or_cleaning_them() {
    for case in 0..3 {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().canonicalize().unwrap();
        let project = workspace.join(ROOT).join("project_disposable_guard");
        let task = project.join("task_disposable_guard");
        fs::create_dir_all(&task).unwrap();
        match case {
            0 => (),
            1 => fs::write(task.join("journal.json"), b"not a journal").unwrap(),
            _ => fs::write(project.join("foreign.txt"), b"unknown material must remain").unwrap(),
        }
        let state = inspect(&workspace, "project_disposable_guard");
        assert!(!state.issues.is_empty());
        assert!(check_business_available(&workspace, "project_disposable_guard").is_err());
        assert!(check_development_available(&workspace, "project_disposable_guard").is_err());
        assert!(check_workspace_available(&workspace).is_err());
        assert!(task.exists());
        if case == 2 {
            assert_eq!(
                fs::read(project.join("foreign.txt")).unwrap(),
                b"unknown material must remain"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn data_restore_guard_rejects_symlink_and_overbudget_record_without_following_it() {
    use std::os::unix::fs::symlink;
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().canonicalize().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("keep"), b"not owned").unwrap();
    let project = workspace.join(ROOT).join("project_disposable_guard");
    fs::create_dir_all(&project).unwrap();
    symlink(outside.path(), project.join("task_symlink")).unwrap();
    assert!(check_workspace_available(&workspace).is_err());
    assert_eq!(fs::read(outside.path().join("keep")).unwrap(), b"not owned");
    fs::remove_file(project.join("task_symlink")).unwrap();
    let task = project.join("task_oversize");
    fs::create_dir(&task).unwrap();
    let file = std::fs::File::create(task.join("journal.json")).unwrap();
    file.set_len(MAX_JOURNAL + 1).unwrap();
    let state = inspect(&workspace, "project_disposable_guard");
    assert_eq!(state.issues[0].code, "PROJECT_DATA_RESTORE_LIMIT");
    assert_eq!(
        fs::metadata(task.join("journal.json")).unwrap().len(),
        MAX_JOURNAL + 1
    );
}

#[test]
fn data_restore_capacity_rejects_before_any_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().canonicalize().unwrap();
    assert!(reserve_publication(
        &workspace,
        "project_disposable_guard",
        "task_new",
        MAX_JOURNAL + 1
    )
    .is_err());
    assert!(!workspace.join(ROOT).exists());
}
