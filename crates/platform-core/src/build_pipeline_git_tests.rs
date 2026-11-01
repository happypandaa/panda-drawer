use super::*;
use std::os::unix::fs::PermissionsExt;
use std::io::Write;

fn preserved(project: &ProjectHandle) -> (String, String) {
    (
        tree_digest(&project.root.join("data"), false).unwrap(),
        tree_digest(&project.root.join("dist"), false).unwrap(),
    )
}
fn old_page(project: &ProjectHandle) {
    ensure_dir(&project.root.join("dist")).unwrap();
    fs::write(project.root.join("dist/index.html"), "old working page").unwrap();
}
#[tokio::test]
async fn no_git_project_build_ignores_broken_git_settings_and_absent_managed_resource() {
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = project(&store, "No Git build still works").await;
    fixture(&project, BUILD);
    fs::write(
        paths.data_root().join("git-toolchain.json"),
        "invalid Git selection",
    )
    .unwrap();
    let manager = BuildManager::with_managed_node_and_git(
        paths.clone(),
        store.clone(),
        "http://127.0.0.1:1234".into(),
        None,
        Some(paths.data_root().join("missing-development-git")),
    )
    .unwrap();
    let task = manager.submit(&project.manifest.id).await.unwrap();
    let done = wait(&manager, &task.task_id).await;
    assert_eq!(done.status, TaskStatus::Succeeded, "{done:?}");
    assert_eq!(
        fs::read_to_string(paths.data_root().join("git-toolchain.json")).unwrap(),
        "invalid Git selection"
    );
    manager.shutdown().await;
    store.shutdown().await;
}
#[tokio::test]
async fn unfinished_merge_fails_before_dependencies_and_preserves_the_entire_existing_project() {
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = project(&store, "Build must not publish unresolved merge").await;
    fixture(&project, BUILD);
    old_page(&project);
    ensure_dir(&project.root.join(".git")).unwrap();
    fs::write(
        project.root.join(".git/MERGE_HEAD"),
        "evidence only; never opened",
    )
    .unwrap();
    let before = tree_digest(&project.root, false).unwrap();
    let manager = BuildManager::new(paths, store.clone(), "http://127.0.0.1:1234".into()).unwrap();
    let task = manager.submit(&project.manifest.id).await.unwrap();
    let done = wait(&manager, &task.task_id).await;
    assert_eq!(done.error.unwrap().code, "BUILD_GIT_OPERATION_IN_PROGRESS");
    assert_eq!(tree_digest(&project.root, false).unwrap(), before);
    assert!(!project.root.join("app/node_modules").exists());
    manager.shutdown().await;
    store.shutdown().await;
}
fn real_git(
    tool: &Path,
    root: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    success: bool,
) -> Vec<u8> {
    use std::process::{Command, Stdio};
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(tool)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Disposable build test")
        .env("GIT_COMMITTER_NAME", "Disposable build test")
        .env("GIT_AUTHOR_EMAIL", "test@localhost")
        .env("GIT_COMMITTER_EMAIL", "test@localhost")
        .current_dir(root)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "commit.gpgSign=false",
        ])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.success(),
        success,
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
#[tokio::test]
#[ignore = "requires LOCAL_PROJECTS_TEST_GIT_ROOT disposable official Git and local Node/npm"]
async fn official_git_dirty_staged_and_untracked_build_but_docs_conflict_and_mid_build_merge_never_publish(
) {
    let tool_root = PathBuf::from(std::env::var("LOCAL_PROJECTS_TEST_GIT_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    assert!(tool_root.starts_with(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.test-artifacts")
            .canonicalize()
            .unwrap()
    ));
    let git = tool_root.join("bin/git");
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = project(&store, "Disposable official Git build conflict").await;
    fixture(&project, BUILD);
    old_page(&project);
    fs::write(project.root.join("docs/merge.txt"), "base\n").unwrap();
    real_git(
        &git,
        &project.root,
        &["init", "--initial-branch=main"],
        None,
        true,
    );
    real_git(&git, &project.root, &["add", "app", "docs"], None, true);
    real_git(
        &git,
        &project.root,
        &["commit", "-m", "base source"],
        None,
        true,
    );
    fs::write(
        project.root.join("docs/merge.txt"),
        "ordinary dirty documentation\n",
    )
    .unwrap();
    fs::write(
        project.root.join("docs/staged.txt"),
        "normal staged material\n",
    )
    .unwrap();
    real_git(&git, &project.root, &["add", "docs/staged.txt"], None, true);
    fs::write(
        project.root.join("app/untracked.txt"),
        "normal untracked source",
    )
    .unwrap();
    let trap = paths.data_root().join("must-not-execute");
    let hook = project.root.join(".git/hooks/forbidden");
    fs::write(
        &hook,
        format!("#!/bin/sh\nprintf executed > '{}'\n", trap.display()),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    real_git(
        &git,
        &project.root,
        &["config", "core.fsmonitor", hook.to_str().unwrap()],
        None,
        true,
    );
    real_git(
        &git,
        &project.root,
        &["config", "filter.bad.clean", hook.to_str().unwrap()],
        None,
        true,
    );
    let before_index = fs::read(project.root.join(".git/index")).unwrap();
    let before_git = tree_digest(&project.root.join(".git"), false).unwrap();
    let before_data = tree_digest(&project.root.join("data"), false).unwrap();
    let manager = BuildManager::with_managed_node_and_git(
        paths.clone(),
        store.clone(),
        "http://127.0.0.1:1234".into(),
        None,
        Some(tool_root),
    )
    .unwrap();
    let task = manager.submit(&project.manifest.id).await.unwrap();
    let done = wait(&manager, &task.task_id).await;
    assert_eq!(done.status, TaskStatus::Succeeded, "{done:?}");
    assert_eq!(
        fs::read(project.root.join(".git/index")).unwrap(),
        before_index
    );
    assert_eq!(
        tree_digest(&project.root.join(".git"), false).unwrap(),
        before_git
    );
    assert_eq!(
        tree_digest(&project.root.join("data"), false).unwrap(),
        before_data
    );
    assert!(!trap.exists());
    let previous = preserved(&project);
    let previous_provenance = fs::read(provenance_path(
        paths.workspace_root(),
        &project.manifest.id,
    ))
    .unwrap();

    // Config includes are inspected without following them, even if opening
    // the named file would block. The guard uses a verified private config copy.
    let config_path = project.root.join(".git/config");
    let old_config = fs::read(&config_path).unwrap();
    let fifo = paths.data_root().join("must-not-open-config");
    let fifo_c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
    let mut config = old_config.clone();
    config.extend_from_slice(format!("\n[include]\npath = {:?}\n", fifo).as_bytes());
    fs::write(&config_path, config).unwrap();
    let task = manager.submit(&project.manifest.id).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), wait(&manager, &task.task_id))
            .await
            .unwrap()
            .error
            .unwrap()
            .code,
        "BUILD_GIT_CHECK_UNAVAILABLE"
    );
    assert_eq!(preserved(&project), previous);
    fs::write(&config_path, old_config).unwrap();

    // Index-only unresolved records, no MERGE_HEAD: docs are outside inputDigest.
    let blob = real_git(
        &git,
        &project.root,
        &["rev-parse", "HEAD:docs/merge.txt"],
        None,
        true,
    );
    let blob = std::str::from_utf8(&blob).unwrap().trim();
    let conflict = format!("100644 {blob} 1\tdocs/conflict.txt\n100644 {blob} 2\tdocs/conflict.txt\n100644 {blob} 3\tdocs/conflict.txt\n");
    real_git(
        &git,
        &project.root,
        &["update-index", "--index-info"],
        Some(conflict.as_bytes()),
        true,
    );
    let conflict_index = fs::read(project.root.join(".git/index")).unwrap();
    let task = manager.submit(&project.manifest.id).await.unwrap();
    assert_eq!(
        wait(&manager, &task.task_id).await.error.unwrap().code,
        "BUILD_GIT_CONFLICT"
    );
    assert_eq!(
        fs::read(project.root.join(".git/index")).unwrap(),
        conflict_index
    );
    assert_eq!(preserved(&project), previous);
    assert_eq!(
        fs::read(provenance_path(
            paths.workspace_root(),
            &project.manifest.id
        ))
        .unwrap(),
        previous_provenance
    );

    // Fixture-only reset restores the artificial conflict; platform never repairs Git.
    real_git(
        &git,
        &project.root,
        &["reset", "--hard", "HEAD"],
        None,
        true,
    );
    real_git(&git, &project.root, &["switch", "-c", "other"], None, true);
    fs::write(project.root.join("docs/merge.txt"), "other branch\n").unwrap();
    real_git(&git, &project.root, &["add", "docs/merge.txt"], None, true);
    real_git(
        &git,
        &project.root,
        &["commit", "-m", "other documentation"],
        None,
        true,
    );
    real_git(&git, &project.root, &["switch", "main"], None, true);
    fs::write(project.root.join("docs/merge.txt"), "main branch\n").unwrap();
    real_git(&git, &project.root, &["add", "docs/merge.txt"], None, true);
    real_git(
        &git,
        &project.root,
        &["commit", "-m", "main documentation"],
        None,
        true,
    );
    let started = paths.data_root().join("build-started");
    let release = paths.data_root().join("build-release");
    fs::write(project.root.join("app/build.cjs"), format!("require('fs').writeFileSync({},'started');const timer=setInterval(()=>{{if(require('fs').existsSync({})){{clearInterval(timer);{BUILD}}}}},20);", serde_json::to_string(&started).unwrap(), serde_json::to_string(&release).unwrap())).unwrap();
    let task = manager.submit(&project.manifest.id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    real_git(
        &git,
        &project.root,
        &["merge", "--no-commit", "other"],
        None,
        false,
    );
    let index = fs::read(project.root.join(".git/index")).unwrap();
    let head = fs::read(project.root.join(".git/HEAD")).unwrap();
    fs::write(release, "continue disposable build").unwrap();
    let done = wait(&manager, &task.task_id).await;
    assert_eq!(done.error.unwrap().code, "BUILD_GIT_OPERATION_IN_PROGRESS");
    assert_eq!(preserved(&project), previous);
    assert_eq!(fs::read(project.root.join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(project.root.join(".git/HEAD")).unwrap(), head);
    assert_eq!(
        fs::read(provenance_path(
            paths.workspace_root(),
            &project.manifest.id
        ))
        .unwrap(),
        previous_provenance
    );
    assert!(!trap.exists());
    let task = manager.submit(&project.manifest.id).await.unwrap();
    assert_eq!(
        wait(&manager, &task.task_id).await.error.unwrap().code,
        "BUILD_GIT_OPERATION_IN_PROGRESS"
    );
    assert_eq!(preserved(&project), previous);
    manager.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn manager_shutdown_cancels_git_guard_before_waiting_for_build_runner() {
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = project(&store, "Disposable stuck build Git probe").await;
    fixture(&project, BUILD);
    old_page(&project);
    let git_dir = project.root.join(".git");
    ensure_dir(&git_dir.join("objects")).unwrap();
    ensure_dir(&git_dir.join("refs")).unwrap();
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        git_dir.join("config"),
        "[core]\nrepositoryformatversion=0\n",
    )
    .unwrap();
    let before = tree_digest(&project.root, false).unwrap();
    let pid_path = paths.data_root().join("stuck-git-pid");
    let executable = paths.data_root().join("synthetic-stuck-git");
    fs::write(&executable, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'git version 2.56.0'; exit; fi\nfor arg do\ncase \"$arg\" in config) exit 1;; ls-files) echo $$ > '{}'; sleep 60 & wait; exit 0;; esac\ndone\nexit 1\n", pid_path.display())).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    atomic_json(
        &paths.data_root().join("git-toolchain.json"),
        &json!({"schemaVersion":1,"gitPath":executable}),
    )
    .unwrap();
    let manager = BuildManager::new(paths, store.clone(), "http://127.0.0.1:1234".into()).unwrap();
    let task = manager.submit(&project.manifest.id).await.unwrap();
    let pid: i32 = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(pid) = fs::read_to_string(&pid_path)
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), manager.shutdown())
        .await
        .unwrap();
    let task = manager.task(&task.task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Interrupted);
    assert_eq!(task.error.unwrap().code, "BUILD_INTERRUPTED");
    assert_eq!(unsafe { libc::kill(-pid, 0) }, -1);
    assert_eq!(tree_digest(&project.root, false).unwrap(), before);
    store.shutdown().await;
}

#[tokio::test]
async fn editing_source_while_final_git_check_waits_prevents_publication() {
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = project(&store, "Disposable final Git check race").await;
    fixture(&project, BUILD);
    old_page(&project);
    let git_dir = project.root.join(".git");
    ensure_dir(&git_dir.join("objects")).unwrap();
    ensure_dir(&git_dir.join("refs")).unwrap();
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        git_dir.join("config"),
        "[core]\nrepositoryformatversion=0\n",
    )
    .unwrap();
    let before = preserved(&project);
    let first = paths.data_root().join("first-git-check");
    let waiting = paths.data_root().join("final-git-check");
    let release = paths.data_root().join("release-git-check");
    let executable = paths.data_root().join("synthetic-waiting-git");
    fs::write(&executable, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'git version 2.56.0'; exit; fi\nfor arg do\ncase \"$arg\" in config) exit 1;; ls-files) if [ -f '{}' ]; then echo waiting > '{}'; while [ ! -f '{}' ]; do sleep 0.02; done; else echo first > '{}'; fi; exit 0;; esac\ndone\nexit 1\n", first.display(), waiting.display(), release.display(), first.display())).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    atomic_json(
        &paths.data_root().join("git-toolchain.json"),
        &json!({"schemaVersion":1,"gitPath":executable}),
    )
    .unwrap();
    let manager = BuildManager::new(paths, store.clone(), "http://127.0.0.1:1234".into()).unwrap();
    let task = manager.submit(&project.manifest.id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !waiting.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let edit = project.root.join("app/edit-during-git-check.txt");
    fs::write(&edit, "external editor change must survive").unwrap();
    fs::write(release, "release").unwrap();
    let done = wait(&manager, &task.task_id).await;
    assert_eq!(done.error.unwrap().code, "BUILD_INPUT_CHANGED");
    assert_eq!(preserved(&project), before);
    assert_eq!(
        fs::read(edit).unwrap(),
        b"external editor change must survive"
    );
    assert!(!project.root.join(".git/index").exists());
    manager.shutdown().await;
    store.shutdown().await;
}
