use super::*;

fn fixture() -> (tempfile::TempDir, PathBuf, ProjectManifest) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-unversioned-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap().join("project");
    fs::create_dir_all(root.join("app")).unwrap();
    let manifest = serde_json::from_value(serde_json::json!({
        "manifestVersion":1,"id":"prj_disposable_git","name":"Disposable Git guard",
        "sdkApiVersion":1,"sourceDir":"app","outputDir":"dist","entry":"dist/index.html",
        "database":"data/database.sqlite","attachmentsDir":"data/attachments",
        "build":{"script":"build"},"docs":{}
    }))
    .unwrap();
    (temporary, root, manifest)
}

#[test]
fn an_unversioned_marker_scan_preserves_source_and_unrelated_files() {
    let (_temporary, root, manifest) = fixture();
    fs::write(
        root.join("git-toolchain.json"),
        b"malformed settings, never read",
    )
    .unwrap();
    fs::write(root.join("app/index.js"), b"retained disposable source").unwrap();
    let guard = BuildGitGuard::capture_at(&root, &manifest).unwrap();
    guard.recheck_at(&root, &manifest).unwrap();
    assert_eq!(
        fs::read(root.join("git-toolchain.json")).unwrap(),
        b"malformed settings, never read"
    );
    assert_eq!(
        fs::read(root.join("app/index.js")).unwrap(),
        b"retained disposable source"
    );
    assert!(!root.join(".git").exists());
    assert!(!root.join(".platform").exists());
}

#[tokio::test]
async fn real_project_admission_does_not_resolve_invalid_git_settings_or_start_a_process() {
    let (_temporary, paths) = crate::RuntimePaths::for_test();
    let store = crate::ProjectStore::new(paths.workspace_root().into()).unwrap();
    let created = store
        .create(crate::CreateProjectInput {
            name: "Disposable unversioned build admission".into(),
            description: None,
        })
        .await
        .unwrap();
    let project = store.project(&created.id).unwrap();
    let lease = store
        .reserve_development_task(&created.id, "task_disposable_no_git")
        .unwrap();
    let operation = project.operation_gate.clone().read_owned().await;
    let protection = Arc::new((lease, operation));
    let settings = paths.data_root().join("git-toolchain.json");
    fs::write(&settings, b"malformed actual Git settings").unwrap();
    let environment = GitEnvironmentManager::new(
        paths.data_root().into(),
        Some(paths.data_root().join("missing-managed-git")),
    );
    let processes = store.project_processes(&created.id).unwrap();
    let (_stop, receiver) = watch::channel(false);
    let guard = BuildGitGuard::capture_with_keep_alive(
        &project,
        &environment,
        receiver.clone(),
        processes.clone(),
        protection.clone(),
    )
    .await
    .unwrap();
    guard
        .recheck_with_keep_alive(&project, receiver, processes.clone(), protection)
        .await
        .unwrap();
    assert!(processes.snapshot().is_empty());
    assert_eq!(
        fs::read(settings).unwrap(),
        b"malformed actual Git settings"
    );
    assert!(!project.root.join(".git").exists());
}

#[test]
fn a_repository_added_during_build_is_rejected_without_claiming_a_clean_index() {
    let (_temporary, root, manifest) = fixture();
    let guard = BuildGitGuard::capture_at(&root, &manifest).unwrap();
    fs::create_dir(root.join(".git")).unwrap();
    fs::write(root.join(".git/index"), b"retained uninspected index").unwrap();
    assert_eq!(
        guard.recheck_at(&root, &manifest).unwrap_err().code,
        "BUILD_GIT_STATE_CHANGED"
    );
    let error = BuildGitGuard::capture_at(&root, &manifest).err().unwrap();
    assert_eq!(error.code, "BUILD_GIT_CHECK_UNAVAILABLE");
    assert_eq!(error.details["capability"], "build-git-index-check");
    assert_eq!(error.details["operationOutcome"], "not_started");
    assert_eq!(
        fs::read(root.join(".git/index")).unwrap(),
        b"retained uninspected index"
    );
}

#[test]
fn native_separators_preserve_all_mac_dependency_and_credential_exclusions() {
    let (_temporary, root, manifest) = fixture();
    for relative in [
        "app/Node_Modules/pkg/.git",
        "app/credentials/private/.git",
        "app/configuration/private/.git",
        "app/.env.local/private/.git",
        "app/.cache/pkg/.git",
        "app/key.pem/private/.git",
    ] {
        fs::create_dir_all(root.join(relative)).unwrap();
        fs::write(root.join(relative).join("retained"), b"excluded evidence").unwrap();
    }
    let guard = BuildGitGuard::capture_at(&root, &manifest).unwrap();
    guard.recheck_at(&root, &manifest).unwrap();
    assert!(protected(Path::new(r"app\node_modules\pkg"), &manifest).unwrap());
    assert!(protected(Path::new(r"app\credentials\secret"), &manifest).unwrap());
    assert!(protected(Path::new(r"dist\asset"), &manifest).unwrap());
    assert!(!protected(Path::new(r"app\data\ordinary"), &manifest).unwrap());
}

#[test]
fn ancestor_nested_gitfiles_and_multiple_repositories_remain_closed() {
    for relative in ["../.git", "app/src/.git", ".git", "app/.git"] {
        let (_temporary, root, manifest) = fixture();
        let marker = root.join(relative);
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        if relative == ".git" {
            fs::write(&marker, b"gitdir: retained-external").unwrap();
        } else {
            fs::create_dir(&marker).unwrap();
        }
        assert_eq!(
            BuildGitGuard::capture_at(&root, &manifest)
                .err()
                .unwrap()
                .code,
            "BUILD_GIT_CHECK_UNAVAILABLE"
        );
        assert!(marker.exists());
    }
    let (_temporary, root, manifest) = fixture();
    fs::create_dir(root.join(".git")).unwrap();
    fs::create_dir(root.join("app/.git")).unwrap();
    assert_eq!(
        discover(&root, &manifest).err().unwrap().code,
        "BUILD_GIT_CHECK_UNAVAILABLE"
    );
}

#[test]
fn uppercase_git_spelling_names_one_actual_repository() {
    let (_temporary, root, manifest) = fixture();
    let git = root.join("app/.GIT");
    fs::create_dir(&git).unwrap();
    let found = discover(&root, &manifest).unwrap().unwrap();
    assert!(found
        ._identity
        .same_object(&native::open_no_reparse(&git).unwrap().identity));
    assert_eq!(
        BuildGitGuard::capture_at(&root, &manifest)
            .err()
            .unwrap()
            .details["capability"],
        "build-git-index-check"
    );
}

#[test]
fn deep_unversioned_sources_add_no_new_depth_budget_and_mixed_spelling_uses_real_scope() {
    let (_temporary, root, manifest) = fixture();
    let mut deep = root.join("app");
    for _ in 0..80 {
        deep.push("d");
    }
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("input.js"), b"retained deep source").unwrap();
    let guard = BuildGitGuard::capture_at(&root, &manifest).unwrap();
    let mut raw = root.parent().unwrap().as_os_str().to_os_string();
    raw.push(r"/project");
    guard.recheck_at(&PathBuf::from(raw), &manifest).unwrap();
    assert_eq!(
        fs::read(deep.join("input.js")).unwrap(),
        b"retained deep source"
    );
}

#[test]
fn ordinary_source_junctions_are_not_followed_into_the_target_repository() {
    let (_temporary, root, manifest) = fixture();
    let target = root.parent().unwrap().join("retained-external");
    fs::create_dir_all(target.join(".git")).unwrap();
    fs::write(target.join(".git/index"), b"external index retained").unwrap();
    let link = root.join("app/linked-source");
    let status = std::process::Command::new(std::env::var_os("ComSpec").unwrap())
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()
        .unwrap();
    assert!(status.status.success());
    let guard = BuildGitGuard::capture_at(&root, &manifest).unwrap();
    guard.recheck_at(&root, &manifest).unwrap();
    assert_eq!(
        fs::read(target.join(".git/index")).unwrap(),
        b"external index retained"
    );
    fs::remove_dir(link).unwrap();
}

#[test]
fn ordinary_source_additions_and_renames_add_no_directory_timestamp_admission() {
    let (_temporary, root, manifest) = fixture();
    let source = root.join("app");
    fs::write(source.join("before.js"), b"ordinary disposable source").unwrap();
    let mut observed = false;
    assert!(discover_with(&root, &manifest, |directory| {
        if directory == source {
            fs::rename(source.join("before.js"), source.join("renamed.js")).unwrap();
            fs::write(source.join("added.js"), b"new ordinary source").unwrap();
            observed = true;
        }
        Ok(())
    })
    .unwrap()
    .is_none());
    assert!(observed);
    assert_eq!(
        fs::read(source.join("renamed.js")).unwrap(),
        b"ordinary disposable source"
    );
    assert_eq!(
        fs::read(source.join("added.js")).unwrap(),
        b"new ordinary source"
    );
}

#[tokio::test]
async fn capture_and_recheck_keep_the_actual_lease_and_read_gate_until_scan_returns() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Protection {
        _lease: crate::projects::ProjectTaskLease,
        _gate: tokio::sync::OwnedRwLockReadGuard<()>,
        dropped: Arc<AtomicBool>,
    }
    impl Drop for Protection {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }
    struct ResetObserver;
    impl Drop for ResetObserver {
        fn drop(&mut self) {
            TEST_SCAN_OBSERVER.with(|observer| {
                observer.borrow_mut().take();
            });
        }
    }

    let (_temporary, paths) = crate::RuntimePaths::for_test();
    let store = crate::ProjectStore::new(paths.workspace_root().into()).unwrap();
    let created = store
        .create(crate::CreateProjectInput {
            name: "Disposable Git phase lifetime".into(),
            description: None,
        })
        .await
        .unwrap();
    let project = store.project(&created.id).unwrap();
    let environment = GitEnvironmentManager::new(paths.data_root().into(), None);
    let guard = BuildGitGuard::capture_at(&project.root, &project.manifest).unwrap();
    let (_stop, receiver) = watch::channel(false);
    for recheck in [false, true] {
        let dropped = Arc::new(AtomicBool::new(false));
        let owner = Arc::new(Protection {
            _lease: store
                .reserve_development_task(&created.id, "task_disposable_phase_lifetime")
                .unwrap(),
            _gate: project.operation_gate.clone().read_owned().await,
            dropped: dropped.clone(),
        });
        let weak = Arc::downgrade(&owner);
        let gate = project.operation_gate.clone();
        let observed = Arc::new(AtomicUsize::new(0));
        let observer_count = observed.clone();
        let observer_dropped = dropped.clone();
        TEST_SCAN_OBSERVER.with(|observer| {
            assert!(observer
                .borrow_mut()
                .replace(Box::new(move |_| {
                    assert!(!observer_dropped.load(Ordering::SeqCst));
                    assert!(
                        weak.upgrade().is_some(),
                        "the actual phase owner must survive the scan"
                    );
                    assert!(
                        gate.try_write().is_err(),
                        "the actual project read gate must remain occupied"
                    );
                    observer_count.fetch_add(1, Ordering::SeqCst);
                }))
                .is_none());
        });
        let reset = ResetObserver;
        if recheck {
            guard
                .recheck_with_keep_alive(
                    &project,
                    receiver.clone(),
                    store.project_processes(&created.id).unwrap(),
                    owner,
                )
                .await
                .unwrap();
        } else {
            BuildGitGuard::capture_with_keep_alive(
                &project,
                &environment,
                receiver.clone(),
                store.project_processes(&created.id).unwrap(),
                owner,
            )
            .await
            .unwrap();
        }
        drop(reset);
        assert!(observed.load(Ordering::SeqCst) > 0);
        assert!(dropped.load(Ordering::SeqCst));
        assert!(project.operation_gate.try_write().is_ok());
        assert!(store.active_task(&created.id).is_none());
    }
}
