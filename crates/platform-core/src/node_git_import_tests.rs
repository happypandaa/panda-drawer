use super::*;
use crate::git_environment::GitEnvironmentManager;
use crate::TaskStatus;
use std::{os::unix::fs::PermissionsExt, process::Command, time::Duration};

fn git(root: &Path, args: &[&str]) {
    let result = Command::new("/usr/bin/git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Disposable Import",
            "-c",
            "user.email=disposable@example.invalid",
        ])
        .args(args)
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
fn repository(root: &Path) {
    fs::create_dir_all(root.join("web")).unwrap();
    fs::create_dir(root.join("server")).unwrap();
    fs::write(root.join("web/index.html"), "disposable import page").unwrap();
    fs::write(
        root.join("server/main.mjs"),
        "throw Error('application must never run during import')",
    )
    .unwrap();
    atomic_json(&root.join("panda.json"),&json!({"manifestVersion":1,"runtime":"node","app":{"name":"可丢弃·单目录导入","version":"1.0.0"},"web":{"root":"web","entry":"index.html"},"backend":{"root":"server","entry":"main.mjs","nodeMajor":24,"protocolVersion":1}})).unwrap();
}
struct Fixture {
    _temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    sources: Arc<SourceAcquisitionManager>,
    manager: Arc<NodeGitImportManager>,
    apps: Arc<NodeApps>,
    input: StartNodeGitImportInput,
    stop: watch::Sender<bool>,
}
async fn fixture(kind: &str) -> Fixture {
    let (temporary, paths) = RuntimePaths::for_test();
    let base = temporary.path().canonicalize().unwrap();
    let author = base.join("disposable-import-author");
    repository(&author);
    let script_marker = base.join("must-never-run");
    let package = json!({"name":"disposable-import","version":"1.0.0","scripts":{"preinstall":format!("touch {}",script_marker.display()),"build":format!("touch {}",script_marker.display())}});
    if [
        "missing-lock",
        "dual-root",
        "npmrc",
        "locked-dependencies",
        "prebuilt",
    ]
    .contains(&kind)
    {
        atomic_json(&author.join("package.json"), &package).unwrap();
    }
    if ["dual-root", "npmrc", "locked-dependencies", "prebuilt"].contains(&kind) {
        atomic_json(&author.join("package-lock.json"),&json!({"name":"disposable-import","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"disposable-import","version":"1.0.0"}}})).unwrap();
    }
    if kind == "dual-root" {
        atomic_json(&author.join("server/package.json"), &package).unwrap();
        atomic_json(
            &author.join("server/package-lock.json"),
            &json!({"lockfileVersion":3,"packages":{"":{}}}),
        )
        .unwrap();
    }
    if kind == "npmrc" {
        fs::write(
            author.join(".npmrc"),
            format!(
                "cache={}\nignore-scripts=false\n",
                base.join("must-not-create-npm-cache").display()
            ),
        )
        .unwrap();
    }
    if kind == "missing-build" {
        // Node has no development declaration: required runtime entry files
        // must be present before dependency preparation or registration.
        fs::remove_file(author.join("server/main.mjs")).unwrap();
        atomic_json(&author.join("package.json"), &package).unwrap();
    }
    if kind.starts_with("web-") {
        let mut manifest = json!({"manifestVersion":1,"runtime":"web","app":{"name":"可丢弃网页 Git"},"web":{"root":if kind == "web-prebuilt" {"web"} else {"dist"},"entry":"index.html"}});
        if kind != "web-prebuilt" && kind != "web-no-build" {
            manifest["development"] =
                json!({"sourceDir":".","build":{"kind":"npm","script":"build"}});
        }
        atomic_json(&author.join("panda.json"), &manifest).unwrap();
        atomic_json(&author.join("package.json"), &package).unwrap();
        atomic_json(&author.join("package-lock.json"), &json!({"name":"disposable-import","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"disposable-import","version":"1.0.0"}}})).unwrap();
    }
    git(&author, &["init", "-b", "main"]);
    git(&author, &["add", "."]);
    git(&author, &["commit", "-m", "disposable import input"]);
    let bin = base.join("disposable-import-tools");
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(&wrapper,format!("#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'git version 2.56.0\\n'; exit 0; fi\nis_clone=\nlast=\nfor argument do\n if [ \"$argument\" = clone ]; then is_clone=yes; fi\n last=$argument\ndone\nif [ \"$is_clone\" = yes ]; then GIT_ALLOW_PROTOCOL=file; export GIT_ALLOW_PROTOCOL; exec /usr/bin/git -c protocol.file.allow=always clone --no-checkout --no-local --depth=1 --single-branch --no-tags -- 'file://{}' \"$last\"; fi\nexec /usr/bin/git \"$@\"\n",author.display())).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let tasks = BuildManager::new(paths.clone(), store.clone()).unwrap();
    if ["locked-dependencies", "prebuilt"].contains(&kind) {
        let node = bin.join("node");
        fs::write(&node,"#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'v24.22.0\\n'; exit 0; fi\nexec /bin/sh \"$@\"\n").unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(bin.join("npm"),"set -eu\nif [ \"$1\" = --version ]; then printf '11.9.0\\n'; exit 0; fi\ncase \"$*\" in *--ignore-scripts*) ;; *) exit 81;; esac\ncase \"$*\" in *--omit=dev*) ;; *) exit 82;; esac\nmkdir -p node_modules\nprintf 'in-place dependency fixture' > node_modules/disposable-import-marker\nprintf '%s\\n' \"$@\" > node_modules/disposable-npm-arguments\n").unwrap();
        tasks
            .environment()
            .configure_node(Some(node))
            .await
            .unwrap();
    }
    if kind.starts_with("web-") && kind != "web-prebuilt" && kind != "web-no-build" {
        let node = bin.join("node");
        fs::write(
            &node,
            r##"#!/bin/sh
if [ "$1" = --version ]; then printf 'v24.22.0\n'; exit 0; fi
exec /bin/sh "$@"
"##,
        )
        .unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
        let failure = if kind == "web-build-fails" {
            "exit 23"
        } else {
            "mkdir -p dist; printf 'confirmed build page' > dist/index.html"
        };
        fs::write(
            bin.join("npm"),
            format!(
                r##"set -eu
if [ "$1" = --version ]; then printf '11.9.0\n'; exit 0; fi
for argument do
 case "$argument" in
 ci) printf 'installed' > disposable-ci-marker;;
 run) {failure};;
 esac
done
"##
            ),
        )
        .unwrap();
        tasks
            .environment()
            .configure_node(Some(node))
            .await
            .unwrap();
    }
    let environment =
        GitEnvironmentManager::with_processes(paths.data_root().into(), None, store.processes());
    environment.configure_git(Some(wrapper)).await.unwrap();
    let sources =
        SourceAcquisitionManager::new(paths.clone(), store.clone(), tasks.clone(), environment)
            .unwrap();
    let manager = NodeGitImportManager::new(
        paths.clone(),
        store.clone(),
        tasks.clone(),
        Ok(sources.clone()),
    )
    .unwrap();
    let apps = Arc::new(
        NodeApps::new(
            paths.data_root().into(),
            base.join("must-not-run-runtime-node"),
        )
        .unwrap(),
    );
    apps.set_source_workspace(paths.workspace_root().into())
        .unwrap();
    let (stop, _) = watch::channel(false);
    let input = StartNodeGitImportInput {
        selected_directory: None,
        alternate_url: None,
        catalog_id: None,
        import_id: Uuid::new_v4().to_string(),
        url: "https://github.com/example/disposable-import.git".into(),
        branch: None,
        name: None,
    };
    Fixture {
        _temporary: temporary,
        paths,
        store,
        tasks,
        sources,
        manager,
        apps,
        input,
        stop,
    }
}
async fn completed(f: &Fixture) -> ImportSnapshot {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let value = f.manager.status(&f.input.import_id).unwrap();
            if value.task.status.is_final() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[cfg(target_os = "macos")]
fn selected_directory(f: &mut Fixture, agent: bool) -> PathBuf {
    let author = f
        ._temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-import-author");
    if agent {
        fs::write(author.join("AGENTS.md"), "Disposable agent rules").unwrap();
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(author.join("panda.json")).unwrap()).unwrap();
        manifest["agent"] = json!({"instructions":"AGENTS.md"});
        atomic_json(&author.join("panda.json"), &manifest).unwrap();
        git(&author, &["add", "."]);
        git(&author, &["commit", "-m", "disposable agent declaration"]);
    }
    let root = f
        ._temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-selected-agent-root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join(".DS_Store"), "original system metadata").unwrap();
    f.input.selected_directory =
        Some(crate::source_acquisition::inspect_selected_directory(&root).unwrap());
    f.input.catalog_id = Some("disposable-agent".into());
    f.input.branch = Some("main".into());
    root
}

#[cfg(target_os = "macos")]
fn only_original_system_metadata(root: &Path) {
    assert_eq!(
        fs::read_to_string(root.join(".DS_Store")).unwrap(),
        "original system metadata"
    );
    assert_eq!(fs::read_dir(root).unwrap().count(), 1);
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn selected_directory_requires_catalog_empty_identity_and_agent_before_publication() {
    let mut f = fixture("zero-dependency").await;
    let root = selected_directory(&mut f, false);
    f.input.catalog_id = None;
    let rejected = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap_err();
    assert_eq!(rejected.details["accepted"], false);
    assert!(rejected.message.contains("推荐的 agent"));
    f.input.catalog_id = Some("disposable-agent".into());
    fs::write(root.join(".env"), "unrelated user file").unwrap();
    assert!(f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .is_err());
    assert_eq!(
        fs::read_to_string(root.join(".env")).unwrap(),
        "unrelated user file"
    );
    fs::remove_file(root.join(".env")).unwrap();
    f.input.selected_directory =
        Some(crate::source_acquisition::inspect_selected_directory(&root).unwrap());
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let result = completed(&f).await;
    assert_eq!(
        result.commit_state,
        CommitState::NotCommitted,
        "{:?}",
        result.task
    );
    assert!(!result.cleanup_pending, "{:?}", result.task);
    assert!(result
        .task
        .error
        .as_ref()
        .unwrap()
        .message
        .contains("缺少有效 agent"));
    only_original_system_metadata(&root);
    let mut changed = f.input.clone();
    changed
        .selected_directory
        .as_mut()
        .unwrap()
        .revision
        .push('x');
    assert!(f
        .manager
        .start(f.apps.clone(), changed, f.stop.subscribe())
        .await
        .unwrap_err()
        .message
        .contains("内容发生变化"));
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn selected_node_dependencies_publish_final_source_and_delete_preserves_the_user_root() {
    let mut f = fixture("locked-dependencies").await;
    let root = selected_directory(&mut f, true);
    let npm = f._temporary.path().join("disposable-import-tools/npm");
    let mut script = fs::read_to_string(&npm).unwrap();
    script.push_str("mkdir -p node_modules/.bin\nln -s ../disposable-import-marker node_modules/.bin/disposable\npwd > node_modules/disposable-working-directory\n");
    fs::write(npm, script).unwrap();
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let result = completed(&f).await;
    assert_eq!(
        result.commit_state,
        CommitState::Committed,
        "{:?}",
        result.task
    );
    assert!(!result.cleanup_pending, "{:?}", result.task);
    assert_eq!(result.source_path.as_ref(), Some(&root));
    let app = result.app.as_ref().unwrap();
    assert_eq!(app["ownership"], "external");
    assert_eq!(app["gitSource"]["catalogId"], "disposable-agent");
    assert_eq!(app["gitSource"]["branch"], "main");
    assert!(root.join(".git").is_dir());
    assert_eq!(
        fs::read_link(root.join("node_modules/.bin/disposable")).unwrap(),
        PathBuf::from("../disposable-import-marker")
    );
    let working =
        fs::read_to_string(root.join("node_modules/disposable-working-directory")).unwrap();
    assert_ne!(working.trim(), root.to_str().unwrap());
    assert!(working.contains("source-publications"));
    assert!(!NodeProjectOwnership::new(
        f.paths.data_root().into(),
        f.paths.workspace_root().into()
    )
    .owns_source_directory(&result.app_id, &root)
    .unwrap());
    fs::create_dir(root.join("data")).unwrap();
    fs::write(root.join("data/keep"), "user data").unwrap();
    f.manager.shutdown().await;
    let preview = f.apps.delete_preview(&result.app_id).await.unwrap();
    assert_eq!(preview["ownership"], "external");
    assert!(preview["preservedPaths"]
        .as_array()
        .unwrap()
        .contains(&json!(root)));
    f.apps
        .delete(
            &result.app_id,
            serde_json::from_value(json!({"expectedRevision":preview["expectedRevision"]}))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(root.join(".git").is_dir());
    assert_eq!(
        fs::read_to_string(root.join("data/keep")).unwrap(),
        "user data"
    );
    assert_eq!(
        fs::read_to_string(root.join(".DS_Store")).unwrap(),
        "original system metadata"
    );
    f.tasks.shutdown().await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn selected_web_prebuilt_and_confirmed_private_build_register_user_owned_final_source() {
    for kind in ["web-prebuilt", "web-build"] {
        let mut f = fixture(kind).await;
        let root = selected_directory(&mut f, true);
        f.manager
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap();
        if kind == "web-build" {
            let waiting = waiting_web(&f).await;
            let confirmation = waiting.build_confirmation.as_ref().unwrap();
            assert_ne!(confirmation.source_path, root);
            assert_eq!(
                waiting.source_path.as_ref(),
                Some(&confirmation.source_path)
            );
            only_original_system_metadata(&root);
            f.manager
                .confirm_build(&f.input.import_id, &confirmation.revision)
                .unwrap();
        }
        let result = completed(&f).await;
        assert_eq!(
            result.commit_state,
            CommitState::Committed,
            "{kind}: {:?}",
            result.task
        );
        assert!(!result.cleanup_pending, "{kind}: {:?}", result.task);
        assert_eq!(result.source_path.as_ref(), Some(&root));
        let info = f.store.info(&result.app_id).await.unwrap();
        assert_eq!(info.ownership, crate::projects::ProjectOwnership::User);
        assert!(info.source_preserved_on_delete);
        assert_eq!(
            info.git_source.as_ref().unwrap()["catalogId"],
            "disposable-agent"
        );
        assert_eq!(info.git_source.as_ref().unwrap()["branch"], "main");
        assert!(root
            .join(if kind == "web-build" {
                "dist/index.html"
            } else {
                "web/index.html"
            })
            .is_file());
        assert!(root.join(".git").is_dir());
        fs::create_dir(root.join("data")).unwrap();
        fs::write(root.join("data/keep"), "user data").unwrap();
        f.manager.shutdown().await;
        f.tasks.shutdown().await;
        let service =
            crate::RuntimeService::start(f.paths.clone(), f.paths.acquire_lock().unwrap())
                .await
                .unwrap();
        let handle = service.handle();
        let delete = handle
            .project_delete(
                &info.id,
                json!({"expectedDocumentEpoch":info.document_epoch}),
            )
            .await
            .unwrap();
        let finished = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let task = handle
                    .task_for_project(delete["taskId"].as_str().unwrap(), &info.id, "delete")
                    .await
                    .unwrap();
                if task["final"] == true {
                    break task;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(finished["status"], "succeeded", "{finished}");
        assert!(root.join(".git").is_dir());
        assert_eq!(
            fs::read_to_string(root.join("data/keep")).unwrap(),
            "user data"
        );
        assert_eq!(
            fs::read_to_string(root.join(".DS_Store")).unwrap(),
            "original system metadata"
        );
        service.shutdown().await.unwrap();
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn selected_preparation_failure_and_cancel_leave_the_user_root_untouched() {
    for kind in ["missing-lock", "web-build-fails", "web-build"] {
        let mut f = fixture(kind).await;
        let root = selected_directory(&mut f, true);
        f.manager
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap();
        if kind.starts_with("web-") {
            let waiting = waiting_web(&f).await;
            only_original_system_metadata(&root);
            if kind == "web-build" {
                f.manager.cancel(&waiting.task.task_id).unwrap();
            } else {
                f.manager
                    .confirm_build(
                        &f.input.import_id,
                        &waiting.build_confirmation.unwrap().revision,
                    )
                    .unwrap();
            }
        }
        let result = completed(&f).await;
        assert_eq!(
            result.commit_state,
            CommitState::NotCommitted,
            "{kind}: {:?}",
            result.task
        );
        assert!(!result.cleanup_pending, "{kind}: {:?}", result.task);
        only_original_system_metadata(&root);
        assert!(f.apps.list().await.unwrap().is_empty());
        assert!(f.store.list().await.unwrap().is_empty());
        f.manager.shutdown().await;
        f.tasks.shutdown().await;
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn selected_node_dependency_cancellation_drains_private_writers_without_touching_user_root() {
    let mut f = fixture("locked-dependencies").await;
    let root = selected_directory(&mut f, true);
    fs::write(f._temporary.path().join("disposable-import-tools/npm"),
        "set -eu\nif [ \"$1\" = --version ]; then printf '11.9.0\\n'; exit 0; fi\nmkdir -p node_modules\nprintf 'private writer started' > node_modules/disposable-started\nexec /bin/sleep 30\n").unwrap();
    let accepted = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let working = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = f.manager.status(&f.input.import_id).unwrap();
            assert!(!status.task.status.is_final(), "{:?}", status.task);
            if let Some(path) = status.source_path {
                if path.join("node_modules/disposable-started").exists() {
                    break path;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_ne!(working, root);
    only_original_system_metadata(&root);
    f.manager.cancel(&accepted.task_id).unwrap();
    let result = completed(&f).await;
    assert_eq!(
        result.commit_state,
        CommitState::NotCommitted,
        "{:?}",
        result.task
    );
    assert!(!result.cleanup_pending, "{:?}", result.task);
    only_original_system_metadata(&root);
    assert!(!working.exists());
    // The registry retains closed history; final import status requires the
    // actual process group and I/O to be closed, not the history to be empty.
    let snapshots = f.store.processes().snapshot();
    assert!(!snapshots.is_empty());
    assert!(snapshots
        .iter()
        .any(|snapshot| snapshot.component == "node-install/npm"));
    assert!(
        snapshots.iter().all(|snapshot| {
            snapshot.phase == crate::owned_process::ProcessPhase::Closed
                && snapshot.leader_reaped
                && snapshot.group_exited
                && snapshot.io_joined
                && snapshot.issues.is_empty()
        }),
        "{snapshots:?}"
    );
    crate::durable_writer::require_owner_clear(
        &crate::durable_writer::WriterOwner::node(f.paths.data_root(), &result.app_id).unwrap(),
    )
    .unwrap();
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn selected_private_preparation_recovery_removes_only_platform_stage_and_reservations() {
    for kind in ["zero-dependency", "web-build"] {
        let mut f = fixture(kind).await;
        let root = selected_directory(&mut f, true);
        let owner = Uuid::new_v4().to_string();
        f.manager.ownership().initialize(&owner).unwrap();
        let task = f
            .tasks
            .create_node_operation(&owner, "nodeGitImport")
            .unwrap();
        let source_id = format!("src_{}", Uuid::new_v4().simple());
        let (_cancel_sender, cancel) = watch::channel(false);
        let source = f
            .sources
            .acquire_for_import(
                AcquireSourceInput {
                    url: f.input.url.clone(),
                    branch: f.input.branch.clone(),
                    name: None,
                    alternate_url: None,
                    download_idle_timeout: true,
                },
                &owner,
                &task.task_id,
                &source_id,
                f.stop.subscribe(),
                cancel,
            )
            .await
            .unwrap();
        let lease = f
            .store
            .reserve_task(&format!("source:{source_id}"), &task.task_id)
            .unwrap();
        let working = f
            .sources
            .prepare_selected_source(
                &source,
                &owner,
                &task.task_id,
                f.input.selected_directory.as_ref().unwrap(),
            )
            .unwrap();
        drop(lease);
        fs::write(
            working.join("disposable-partial-generated-output"),
            "private partial output",
        )
        .unwrap();
        let web_id = (kind == "web-build")
            .then(|| format!("prj_{}", Uuid::parse_str(&owner).unwrap().simple()));
        let identity = capture(&root).unwrap();
        if let Some(id) = &web_id {
            f.store
                .reserve_import_source(
                    id,
                    &task.task_id,
                    &root,
                    &identity,
                    crate::projects::ProjectOwnership::User,
                )
                .unwrap();
            f.tasks.bind_web_operation(&task.task_id, id).unwrap();
        }
        let mut journal = Journal {
            version: 1,
            input: f.input.clone(),
            download_url: Some(source.url.clone()),
            app_id: owner.clone(),
            task_id: task.task_id.clone(),
            source_id,
            workspace: f.paths.workspace_root().into(),
            workspace_identity: capture(f.paths.workspace_root()).unwrap(),
            owner_identity: Some(
                capture(&f.manager.ownership().directory(&owner).unwrap()).unwrap(),
            ),
            runtime: Some(if web_id.is_some() {
                PandaRuntime::Web
            } else {
                PandaRuntime::Node
            }),
            web_project_id: web_id,
            build_confirmation: None,
            retiring_control: false,
            phase: "dependencies".into(),
            registration_attempted: false,
            commit_state: CommitState::NotCommitted,
            source_path: Some(working.clone()),
            source_identity: Some(capture(&working).unwrap()),
            selected_root_identity: Some(identity),
            manifest_digest: Some(format!(
                "{:x}",
                Sha256::digest(read_manifest(&working).unwrap())
            )),
            temporary: BTreeMap::new(),
            cleanup_pending: false,
            remaining_paths: vec![],
            app: None,
            final_task: None,
        };
        f.manager.capture_temporary(&mut journal).unwrap();
        let restarted = NodeGitImportManager::new(
            f.paths.clone(),
            f.store.clone(),
            f.tasks.clone(),
            Ok(f.sources.clone()),
        )
        .unwrap();
        let recovered = restarted.recover(f.apps.clone()).await.unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            recovered[0].commit_state,
            CommitState::NotCommitted,
            "{kind}: {:?}",
            recovered[0].task
        );
        assert!(
            !recovered[0].cleanup_pending,
            "{kind}: {:?}",
            recovered[0].task
        );
        only_original_system_metadata(&root);
        assert!(!working.exists());
        assert!(f.store.list().await.unwrap().is_empty());
        assert!(f.apps.list().await.unwrap().is_empty());
        restarted.shutdown().await;
        f.manager.shutdown().await;
        f.tasks.shutdown().await;
    }
}

#[tokio::test]
async fn recommended_download_retries_in_one_task_and_persists_the_successful_mirror() {
    let mut f = fixture("zero-dependency").await;
    f.input.catalog_id = Some("disposable-mirror-app".into());
    f.input.alternate_url = Some("https://gitee.com/example/disposable-import.git".into());
    let wrapper = f._temporary.path().join("disposable-import-tools/git");
    let original = fs::read_to_string(&wrapper).unwrap();
    let changed = original.replace(
        "if [ \"$is_clone\" = yes ]; then",
        "if [ \"$is_clone\" = yes ]; then case \"$*\" in *github.com*) exit 23;; esac;",
    );
    assert_ne!(changed, original);
    fs::write(&wrapper, changed).unwrap();
    let accepted = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let result = completed(&f).await;
    assert_eq!(
        result.commit_state,
        CommitState::Committed,
        "{:?}",
        result.task
    );
    assert_eq!(result.task.task_id, accepted.task_id);
    assert_eq!(result.download_url, f.input.alternate_url);
    assert_eq!(
        result.app.as_ref().unwrap()["gitSource"]["url"],
        f.input.alternate_url.as_ref().unwrap().as_str()
    );
    let repeated = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    assert_eq!(repeated.task_id, accepted.task_id);
    f.tasks.shutdown().await;
}
#[tokio::test]
async fn direct_import_uses_one_uuid_one_project_and_deduplicates_lost_replies() {
    let mut f = fixture("zero-dependency").await;
    f.input.catalog_id = Some("disposable-catalog-app".into());
    let accepted = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let duplicate = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    assert_eq!(accepted.task_id, duplicate.task_id);
    let result = completed(&f).await;
    assert_eq!(
        result.commit_state,
        CommitState::Committed,
        "{:?}",
        result.task
    );
    assert!(!result.cleanup_pending);
    let source = result.source_path.unwrap();
    assert!(source.join(".git").is_dir());
    let registration: Value = serde_json::from_slice(
        &fs::read(
            f.paths
                .data_root()
                .join("node-apps")
                .join(&result.app_id)
                .join("registration.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        registration["version"],
        crate::node_registration::DIRECT_NODE_REGISTRATION_VERSION
    );
    assert_eq!(registration["id"], result.app_id);
    assert_eq!(registration["sourcePath"], json!(source));
    assert_eq!(
        registration["provenance"]["catalogId"],
        "disposable-catalog-app"
    );
    assert_eq!(
        result.app.as_ref().unwrap()["gitSource"]["catalogId"],
        "disposable-catalog-app"
    );
    assert!(!f
        .paths
        .data_root()
        .join("node-apps")
        .join(&result.app_id)
        .join("generations")
        .exists());
    assert!(!f
        .paths
        .data_root()
        .join("node-apps/.installations")
        .exists());
    assert!(!f
        .paths
        .data_root()
        .join("onboarding/imports")
        .join(&accepted.task_id)
        .exists());
    assert!(!f
        .paths
        .workspace_root()
        .join(".platform/source-publications")
        .join(&result.app_id)
        .exists());
    assert!(!f
        .manager
        .journal_path(&f.manager.records.lock().unwrap()[&f.input.import_id])
        .unwrap()
        .exists());
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    assert_eq!(
        restarted
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap()
            .task_id,
        accepted.task_id
    );
    assert_eq!(
        restarted.status(&f.input.import_id).unwrap().commit_state,
        CommitState::Committed
    );
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
}
#[tokio::test]
async fn invalid_dependency_or_missing_build_inputs_rollback_without_executing_scripts() {
    for kind in ["missing-lock", "dual-root", "npmrc", "missing-build"] {
        let f = fixture(kind).await;
        f.manager
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap();
        let result = completed(&f).await;
        assert_eq!(
            result.commit_state,
            CommitState::NotCommitted,
            "{kind}: {:?}",
            result.task
        );
        assert_eq!(result.task.status, TaskStatus::Failed, "{kind}");
        let expected = if kind == "missing-build" {
            "SOURCE_NEEDS_ADAPTATION"
        } else {
            "NODE_DEPENDENCY_NEEDS_ADAPTATION"
        };
        assert_eq!(
            result.task.error.as_ref().unwrap().code,
            expected,
            "{kind}: {:?}",
            result.task
        );
        assert!(!result.cleanup_pending, "{kind}: {:?}", result.task);
        assert!(!f
            .paths
            .data_root()
            .join("node-project-ownership")
            .join(&result.app_id)
            .exists());
        assert!(!f._temporary.path().join("must-never-run").exists());
        assert!(!f
            ._temporary
            .path()
            .join("must-not-create-npm-cache")
            .exists());
        if let Some(source) = result.source_path {
            assert!(!source.exists(), "{kind}");
        }
        for dir in [
            f.paths.workspace_root().join(".platform/sources"),
            f.paths.workspace_root().join("project-sources"),
        ] {
            if dir.is_dir() {
                assert!(fs::read_dir(dir).unwrap().next().is_none(), "{kind}");
            }
        }
        f.manager.shutdown().await;
        f.tasks.shutdown().await;
    }
}

#[tokio::test]
async fn cancellation_waits_for_the_actual_materializer_before_removing_any_source() {
    let f = fixture("zero-dependency").await;
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    f.sources.pause_import_writer(entered, blocked);
    let accepted = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), ready)
        .await
        .unwrap()
        .unwrap();
    let j = f.manager.records.lock().unwrap()[&f.input.import_id].clone();
    let slot = f
        .paths
        .workspace_root()
        .join(".platform/sources")
        .join(&j.source_id);
    let busy = f
        .manager
        .start(
            f.apps.clone(),
            StartNodeGitImportInput {
                selected_directory: None,
                alternate_url: None,
                catalog_id: None,
                import_id: Uuid::new_v4().to_string(),
                ..f.input.clone()
            },
            f.stop.subscribe(),
        )
        .await
        .unwrap_err();
    assert_eq!(busy.details["accepted"], false);
    assert_eq!(busy.details["operationOutcome"], "not_started");
    f.manager.cancel(&accepted.task_id).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if f.store.processes().snapshot().iter().any(|p| {
                p.component == "source-acquisition/materialize"
                    && p.phase == crate::owned_process::ProcessPhase::CleanupPending
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(slot.is_dir());
    assert!(f.store.has_active_tasks());
    assert!(!f
        .manager
        .status(&f.input.import_id)
        .unwrap()
        .task
        .status
        .is_final());
    release.send(()).unwrap();
    let result = completed(&f).await;
    assert_eq!(result.commit_state, CommitState::NotCommitted);
    assert!(!result.cleanup_pending, "{:?}", result.task);
    assert!(!slot.exists());
    assert!(!f
        .paths
        .data_root()
        .join("node-project-ownership")
        .join(&j.app_id)
        .exists());
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn startup_reconciles_a_committed_registration_without_replaying_the_add() {
    let f = fixture("zero-dependency").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let result = completed(&f).await;
    assert_eq!(
        result.commit_state,
        CommitState::Committed,
        "{:?}",
        result.task
    );
    f.manager.shutdown().await;
    let mut j = f.manager.records.lock().unwrap()[&f.input.import_id].clone();
    j.registration_attempted = true;
    j.commit_state = CommitState::Unknown;
    j.phase = "registering".into();
    j.final_task = None;
    j.cleanup_pending = true;
    f.manager.save(&j).unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    let recovered = restarted.recover(f.apps.clone()).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].commit_state, CommitState::Committed);
    assert!(!recovered[0].cleanup_pending);
    assert_eq!(recovered[0].app_id, result.app_id);
    assert!(result.source_path.unwrap().is_dir());
    assert_eq!(
        restarted
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap()
            .task_id,
        result.task.task_id
    );
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn an_unreadable_registration_is_unknown_and_retry_never_deletes_the_project() {
    let f = fixture("zero-dependency").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let result = completed(&f).await;
    assert_eq!(
        result.commit_state,
        CommitState::Committed,
        "{:?}",
        result.task
    );
    f.manager.shutdown().await;
    let mut j = f.manager.records.lock().unwrap()[&f.input.import_id].clone();
    j.registration_attempted = true;
    j.commit_state = CommitState::Unknown;
    j.phase = "registering".into();
    j.final_task = None;
    j.cleanup_pending = true;
    f.manager.save(&j).unwrap();
    let registration = f
        .paths
        .data_root()
        .join("node-apps")
        .join(&j.app_id)
        .join("registration.json");
    let original = fs::read(&registration).unwrap();
    fs::write(&registration, b"invalid interrupted registration").unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    let recovered = restarted.recover(f.apps.clone()).await.unwrap();
    assert_eq!(recovered[0].commit_state, CommitState::Unknown);
    assert!(recovered[0].cleanup_pending);
    assert!(j.source_path.as_ref().unwrap().is_dir());
    assert!(restarted.require_migration_clear().is_err());
    fs::write(&registration, original).unwrap();
    let resolved = restarted
        .retry_cleanup(f.apps.clone(), &j.task_id)
        .await
        .unwrap();
    assert_eq!(resolved.commit_state, CommitState::Committed);
    assert!(!resolved.cleanup_pending);
    assert!(j.source_path.unwrap().is_dir());
    f.tasks.shutdown().await;
}

fn interrupted_before_registration(f: &Fixture) -> Journal {
    let ownership = f.manager.ownership();
    let id = Uuid::new_v4().to_string();
    ownership.initialize(&id).unwrap();
    let task = f.tasks.create_node_operation(&id, "nodeGitImport").unwrap();
    let source = f
        .paths
        .workspace_root()
        .join("project-sources/disposable-interrupted-import");
    private_dir(source.parent().unwrap()).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("partial"), "disposable interrupted project").unwrap();
    let identity = capture(&source).unwrap();
    ownership
        .reserve_published_source_identity(&id, "disposable-interrupted-import", &identity)
        .unwrap();
    let operation = f
        .paths
        .data_root()
        .join("onboarding/imports")
        .join(&task.task_id);
    ownership.record_onboarding(&id, &task.task_id).unwrap();
    private_dir(&operation).unwrap();
    ownership.record_onboarding(&id, &task.task_id).unwrap();
    fs::write(operation.join("partial-cache"), "temporary cache").unwrap();
    let mut temporary = BTreeMap::new();
    temporary.insert(operation.clone(), capture(&operation).unwrap());
    let j = Journal {
        selected_root_identity: None,
        download_url: None,
        version: 1,
        input: StartNodeGitImportInput {
            selected_directory: None,
            alternate_url: None,
            catalog_id: None,
            import_id: Uuid::new_v4().to_string(),
            ..f.input.clone()
        },
        app_id: id.clone(),
        task_id: task.task_id,
        source_id: format!("src_{}", Uuid::new_v4().simple()),
        workspace: f.paths.workspace_root().into(),
        workspace_identity: capture(f.paths.workspace_root()).unwrap(),
        owner_identity: Some(capture(&ownership.directory(&id).unwrap()).unwrap()),
        runtime: Some(PandaRuntime::Node),
        web_project_id: None,
        build_confirmation: None,
        retiring_control: false,
        phase: "dependencies".into(),
        registration_attempted: false,
        commit_state: CommitState::NotCommitted,
        source_path: Some(source),
        source_identity: Some(identity),
        manifest_digest: None,
        temporary,
        cleanup_pending: false,
        remaining_paths: vec![],
        app: None,
        final_task: None,
    };
    f.manager.save(&j).unwrap();
    j
}
#[tokio::test]
async fn startup_rolls_back_uncommitted_material_and_ownership() {
    let f = fixture("zero-dependency").await;
    let j = interrupted_before_registration(&f);
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    let recovered = restarted.recover(f.apps.clone()).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].commit_state, CommitState::NotCommitted);
    assert!(!recovered[0].cleanup_pending);
    assert!(!j.source_path.unwrap().exists());
    assert!(!f.manager.ownership().directory(&j.app_id).unwrap().exists());
    assert!(!f
        .paths
        .data_root()
        .join("onboarding/imports")
        .join(&j.task_id)
        .exists());
    f.tasks.shutdown().await;
}
#[tokio::test]
async fn cleanup_retry_keeps_a_replacement_directory_until_the_original_identity_is_restored() {
    let f = fixture("zero-dependency").await;
    let j = interrupted_before_registration(&f);
    let source = j.source_path.clone().unwrap();
    let held = f._temporary.path().join("held-disposable-original");
    fs::rename(&source, &held).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("foreign"), "foreign directory must remain").unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    let recovered = restarted.recover(f.apps.clone()).await.unwrap();
    assert!(recovered[0].cleanup_pending);
    assert_eq!(
        fs::read(source.join("foreign")).unwrap(),
        b"foreign directory must remain"
    );
    assert!(restarted.require_migration_clear().is_err());
    let foreign = f._temporary.path().join("held-disposable-foreign");
    fs::rename(&source, &foreign).unwrap();
    fs::rename(&held, &source).unwrap();
    let retried = restarted
        .retry_cleanup(f.apps.clone(), &j.task_id)
        .await
        .unwrap();
    assert!(!retried.cleanup_pending);
    assert!(!source.exists());
    assert_eq!(
        fs::read(foreign.join("foreign")).unwrap(),
        b"foreign directory must remain"
    );
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn imported_direct_source_is_owned_and_complete_deletion_removes_every_import_path() {
    let f = fixture("zero-dependency").await;
    let task = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let imported = completed(&f).await;
    assert_eq!(
        imported.commit_state,
        CommitState::Committed,
        "{:?}",
        imported.task
    );
    f.manager.shutdown().await;
    let source = imported.source_path.unwrap();
    let preview = f.apps.delete_preview(&imported.app_id).await.unwrap();
    assert_eq!(preview["ownership"], "managed");
    assert_eq!(preview["preservedPaths"], json!([]));
    assert!(preview["deletePaths"]
        .as_array()
        .unwrap()
        .contains(&json!(source)));
    let input =
        serde_json::from_value(json!({"expectedRevision":preview["expectedRevision"]})).unwrap();
    f.apps.delete(&imported.app_id, input).await.unwrap();
    for path in [
        source,
        f.paths.data_root().join("node-apps").join(&imported.app_id),
        f.paths
            .data_root()
            .join("node-project-ownership")
            .join(&imported.app_id),
        f.paths
            .workspace_root()
            .join(".platform/source-publications")
            .join(&imported.app_id),
        f.paths
            .data_root()
            .join("onboarding/imports")
            .join(&task.task_id),
        f.paths
            .workspace_root()
            .join(".platform/tasks")
            .join(&task.task_id),
    ] {
        assert!(!path.exists(), "{}", path.display());
    }
    // The original author repository supplied by the disposable Git wrapper is
    // an external input, never inferred to belong to the imported project.
    assert!(f
        ._temporary
        .path()
        .join("disposable-import-author/.git")
        .is_dir());
    assert!(
        f.manager.status(&f.input.import_id).is_err(),
        "headless deletion must invalidate cached success before a native callback"
    );
    let rejected = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap_err();
    assert_eq!(rejected.details["accepted"], false);
    f.manager.forget_deleted_project(&imported.app_id);
    assert!(f.manager.status(&f.input.import_id).is_err());
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn prebuilt_runtime_files_do_not_execute_repository_build_scripts() {
    let f = fixture("prebuilt").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let result = completed(&f).await;
    assert_eq!(
        result.commit_state,
        CommitState::Committed,
        "{:?}",
        result.task
    );
    assert!(!f._temporary.path().join("must-never-run").exists());
    assert!(result
        .source_path
        .unwrap()
        .join("server/main.mjs")
        .is_file());
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
}
#[tokio::test]
async fn explicit_branch_is_persisted_and_a_missing_branch_rolls_back_the_same_import() {
    for branch in ["main", "missing-disposable-branch"] {
        let mut f = fixture("zero-dependency").await;
        f.input.branch = Some(branch.into());
        f.manager
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap();
        let result = completed(&f).await;
        if branch == "main" {
            assert_eq!(
                result.commit_state,
                CommitState::Committed,
                "{:?}",
                result.task
            );
            let registration: Value = serde_json::from_slice(
                &fs::read(
                    f.paths
                        .data_root()
                        .join("node-apps")
                        .join(&result.app_id)
                        .join("registration.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(registration["provenance"]["sourceBranch"], "main");
        } else {
            assert_eq!(result.commit_state, CommitState::NotCommitted);
            assert_eq!(
                result.task.error.as_ref().unwrap().code,
                "SOURCE_BRANCH_UNAVAILABLE"
            );
            assert!(!result.cleanup_pending);
        }
        f.manager.shutdown().await;
        f.tasks.shutdown().await;
    }
}

#[tokio::test]
async fn dependencies_are_prepared_in_the_published_source_and_private_npm_state_is_removed() {
    let f = fixture("locked-dependencies").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let result = completed(&f).await;
    assert_eq!(
        result.commit_state,
        CommitState::Committed,
        "{:?}",
        result.task
    );
    let source = result.source_path.unwrap();
    assert_eq!(
        fs::read(source.join("node_modules/disposable-import-marker")).unwrap(),
        b"in-place dependency fixture"
    );
    let arguments =
        fs::read_to_string(source.join("node_modules/disposable-npm-arguments")).unwrap();
    for flag in ["--omit=dev", "--ignore-scripts", "--no-audit", "--no-fund"] {
        assert!(arguments.lines().any(|line| line == flag), "{arguments}");
    }
    assert!(!f
        .paths
        .data_root()
        .join("onboarding/imports")
        .join(&result.task.task_id)
        .exists());
    assert!(!f
        .paths
        .data_root()
        .join("node-apps/.installations")
        .exists());
    assert!(!f._temporary.path().join("must-never-run").exists());
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn a_replaced_completed_source_cannot_block_startup_or_be_adopted() {
    let f = fixture("zero-dependency").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let done = completed(&f).await;
    assert_eq!(done.commit_state, CommitState::Committed);
    f.manager.shutdown().await;
    let source = done.source_path.unwrap();
    let held = f
        ._temporary
        .path()
        .join("held-disposable-completed-original");
    fs::rename(&source, &held).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("foreign"), "must remain untouched").unwrap();
    std::os::unix::fs::symlink("/not-a-file-to-inspect", source.join("must-not-follow")).unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    assert!(restarted.issues().is_empty());
    assert_eq!(
        restarted.status(&f.input.import_id).unwrap().app_id,
        done.app_id
    );
    assert_eq!(
        fs::read(source.join("foreign")).unwrap(),
        b"must remain untouched"
    );
    assert!(restarted.recover(f.apps.clone()).await.unwrap().is_empty());
    assert!(held.join("server/main.mjs").is_file());
    assert!(f.apps.list().await.is_ok());
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn definite_rejections_and_failed_acceptance_do_not_leave_an_owner() {
    let f = fixture("zero-dependency").await;
    let mut invalid = f.input.clone();
    invalid.import_id = "invalid".into();
    let issue = f
        .manager
        .start(f.apps.clone(), invalid, f.stop.subscribe())
        .await
        .unwrap_err();
    assert_eq!(issue.details["accepted"], false);
    assert_eq!(issue.details["operationOutcome"], "not_started");
    f.tasks.shutdown().await;
    let issue = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap_err();
    assert_eq!(issue.details["accepted"], false);
    assert_eq!(issue.details["operationOutcome"], "not_started");
    assert!(
        issue.details.get("acceptanceCleanupError").is_none(),
        "{issue:?}"
    );
    let owners = f.paths.data_root().join("node-project-ownership");
    if owners.is_dir() {
        assert!(fs::read_dir(owners).unwrap().next().is_none());
    }
}

#[tokio::test]
async fn a_bad_pending_journal_is_isolated_while_existing_projects_remain_available() {
    let f = fixture("zero-dependency").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let done = completed(&f).await;
    assert_eq!(done.commit_state, CommitState::Committed);
    f.manager.shutdown().await;
    let pending = interrupted_before_registration(&f);
    let journal = f.manager.journal_path(&pending).unwrap();
    fs::write(&journal, b"invalid disposable journal").unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    assert_eq!(restarted.issues().len(), 1);
    assert_eq!(
        restarted.status(&f.input.import_id).unwrap().app_id,
        done.app_id
    );
    assert!(f.apps.list().await.is_ok());
    assert!(restarted.require_migration_clear().is_err());
    assert!(restarted.recover(f.apps.clone()).await.is_err());
    assert_eq!(fs::read(journal).unwrap(), b"invalid disposable journal");
    assert!(pending.source_path.unwrap().is_dir());
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn startup_cleans_a_task_created_before_the_initial_journal_was_published() {
    let f = fixture("zero-dependency").await;
    let ownership = f.manager.ownership();
    let id = Uuid::new_v4().to_string();
    ownership.initialize(&id).unwrap();
    let task = f.tasks.create_node_operation(&id, "nodeGitImport").unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    assert!(restarted.require_migration_clear().is_err());
    assert!(restarted.recover(f.apps.clone()).await.unwrap().is_empty());
    assert!(!ownership.directory(&id).unwrap().exists());
    assert!(f.tasks.task(&task.task_id).is_err());
    assert!(!f
        .paths
        .workspace_root()
        .join(".platform/tasks")
        .join(task.task_id)
        .exists());
    assert!(restarted.require_migration_clear().is_ok());
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn startup_cleans_an_admission_interrupted_before_task_creation() {
    let f = fixture("zero-dependency").await;
    let ownership = f.manager.ownership();
    let id = Uuid::new_v4().to_string();
    ownership.initialize(&id).unwrap();
    let directory = ownership.directory(&id).unwrap();
    atomic_json(
        &directory.join("import-request.json"),
        &Admission {
            selected_root_identity: None,
            version: 1,
            input: f.input.clone(),
            app_id: id.clone(),
            workspace: f.paths.workspace_root().into(),
            workspace_identity: capture(f.paths.workspace_root()).unwrap(),
            owner_identity: capture(&directory).unwrap(),
        },
    )
    .unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    assert!(restarted.require_migration_clear().is_err());
    restarted.recover(f.apps.clone()).await.unwrap();
    assert!(!directory.exists());
    assert!(restarted.require_migration_clear().is_ok());
    f.tasks.shutdown().await;
}

async fn waiting_web(f: &Fixture) -> ImportSnapshot {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let snapshot = f.manager.status(&f.input.import_id).unwrap();
            assert!(!snapshot.task.status.is_final(), "{:?}", snapshot.task);
            if snapshot.task.status == TaskStatus::WaitingConfirmation {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn existing_web_entry_registers_without_build_or_node_and_restarts_deduplicate() {
    let f = fixture("web-prebuilt").await;
    let task = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let snapshot = completed(&f).await;
    assert_eq!(
        snapshot.commit_state,
        CommitState::Committed,
        "{:?}",
        snapshot.task
    );
    assert_eq!(snapshot.task.status, TaskStatus::Succeeded);
    assert_eq!(snapshot.runtime, Some(PandaRuntime::Web));
    assert!(snapshot.app_id.starts_with("prj_"));
    assert_eq!(
        snapshot.web_project_id.as_deref(),
        Some(snapshot.app_id.as_str())
    );
    assert_eq!(
        snapshot.task.project_id.as_deref(),
        Some(snapshot.app_id.as_str())
    );
    assert_eq!(snapshot.app.as_ref().unwrap()["ownership"], "platform");
    assert_eq!(
        snapshot.app.as_ref().unwrap()["gitSource"]["url"],
        f.input.url
    );
    assert_eq!(
        f.store
            .info(&snapshot.app_id)
            .await
            .unwrap()
            .git_source
            .as_ref()
            .unwrap()["url"],
        f.input.url
    );
    let root = snapshot.source_path.as_ref().unwrap();
    assert!(root.join(".git").is_dir());
    assert!(!root.join("disposable-ci-marker").exists());
    assert!(!root.join("node_modules").exists());
    assert!(!f._temporary.path().join("must-never-run").exists());
    let import_id = snapshot.task.result.as_ref().unwrap()["importOwnerId"]
        .as_str()
        .unwrap();
    assert!(!f.manager.ownership().directory(import_id).unwrap().exists());
    assert!(f.apps.list().await.unwrap().is_empty());
    f.manager.shutdown().await;
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    assert_eq!(
        restarted
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap()
            .task_id,
        task.task_id
    );
    assert_eq!(
        restarted.status(&f.input.import_id).unwrap().app_id,
        snapshot.app_id
    );
    assert!(restarted.recover(f.apps.clone()).await.unwrap().is_empty());
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn web_missing_entry_waits_persistently_and_builds_only_after_explicit_confirmation() {
    let f = fixture("web-build").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let waiting = waiting_web(&f).await;
    let root = waiting.source_path.as_ref().unwrap();
    let confirmation = waiting.build_confirmation.as_ref().unwrap();
    assert_eq!(confirmation.script, "build");
    assert_eq!(confirmation.source_dir, ".");
    assert_eq!(confirmation.source_path, *root);
    assert!(!f.store.is_registered(&waiting.app_id).unwrap());
    assert!(!root.join("dist").exists());
    assert!(!root.join("disposable-ci-marker").exists());
    let journal = f.manager.records.lock().unwrap()[&f.input.import_id].clone();
    let persisted: Journal =
        serde_json::from_slice(&fs::read(f.manager.journal_path(&journal).unwrap()).unwrap())
            .unwrap();
    assert_eq!(persisted.phase, "awaiting-build-confirmation");
    assert!(persisted.build_confirmation.is_some());
    assert_eq!(
        f.manager
            .confirm_build(&f.input.import_id, "stale")
            .unwrap_err()
            .code,
        "GIT_BUILD_CONFIRMATION_CHANGED"
    );
    f.manager
        .confirm_build(&f.input.import_id, &confirmation.revision)
        .unwrap();
    let completed = completed(&f).await;
    assert_eq!(
        completed.commit_state,
        CommitState::Committed,
        "{:?}",
        completed.task
    );
    assert_eq!(completed.source_path, waiting.source_path);
    assert!(root.join("disposable-ci-marker").is_file());
    assert_eq!(
        fs::read_to_string(root.join("dist/index.html")).unwrap(),
        "confirmed build page"
    );
    assert!(!f._temporary.path().join("must-never-run").exists());
    assert!(completed.build_confirmation.is_none());
    assert!(!f
        .paths
        .data_root()
        .join("onboarding/imports")
        .join(&completed.task.task_id)
        .exists());
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn cancelling_waiting_web_import_rolls_back_only_downloaded_source() {
    let f = fixture("web-build").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let waiting = waiting_web(&f).await;
    f.manager.cancel(&waiting.task.task_id).unwrap();
    let snapshot = completed(&f).await;
    assert_eq!(
        snapshot.commit_state,
        CommitState::NotCommitted,
        "{:?}",
        snapshot.task
    );
    assert!(!snapshot.cleanup_pending, "{:?}", snapshot.task);
    assert!(snapshot.build_confirmation.is_none());
    assert!(!waiting.source_path.unwrap().exists());
    assert!(!f.store.is_registered(&waiting.app_id).unwrap());
    assert!(snapshot.task.project_id.is_none());
    assert!(!f
        .paths
        .data_root()
        .join("web-apps")
        .join(&waiting.app_id)
        .exists());
    assert!(f
        ._temporary
        .path()
        .join("disposable-import-author/panda.json")
        .is_file());
    assert!(!f._temporary.path().join("must-never-run").exists());
    let owner = f.manager.records.lock().unwrap()[&f.input.import_id]
        .app_id
        .clone();
    assert!(!f.manager.ownership().directory(&owner).unwrap().exists());
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn failed_web_build_and_changed_manifest_are_not_registered_and_are_rolled_back() {
    for kind in ["web-build-fails", "web-build"] {
        let f = fixture(kind).await;
        f.manager
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap();
        let waiting = waiting_web(&f).await;
        if kind == "web-build" {
            let file = waiting.source_path.as_ref().unwrap().join("panda.json");
            let mut manifest: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
            manifest["development"]["build"]["script"] = "changed-script".into();
            atomic_json(&file, &manifest).unwrap();
        }
        f.manager
            .confirm_build(
                &f.input.import_id,
                &waiting.build_confirmation.unwrap().revision,
            )
            .unwrap();
        let snapshot = completed(&f).await;
        assert_eq!(
            snapshot.commit_state,
            CommitState::NotCommitted,
            "{kind}: {:?}",
            snapshot.task
        );
        assert!(!snapshot.cleanup_pending, "{kind}: {:?}", snapshot.task);
        assert!(!snapshot.source_path.unwrap().exists());
        assert!(!f.store.is_registered(&snapshot.app_id).unwrap());
        assert!(snapshot.task.project_id.is_none());
        if kind == "web-build" {
            assert_eq!(
                snapshot.task.error.unwrap().code,
                "GIT_BUILD_CONFIRMATION_CHANGED"
            );
        }
        f.manager.shutdown().await;
        f.tasks.shutdown().await;
    }
}

#[tokio::test]
async fn web_missing_entry_without_build_is_rejected_and_unregistered() {
    let f = fixture("web-no-build").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let snapshot = completed(&f).await;
    assert_eq!(
        snapshot.commit_state,
        CommitState::NotCommitted,
        "{:?}",
        snapshot.task
    );
    assert_eq!(snapshot.task.error.unwrap().code, "SOURCE_NEEDS_ADAPTATION");
    assert!(!snapshot.cleanup_pending);
    assert!(snapshot
        .source_path
        .as_ref()
        .is_none_or(|path| !path.exists()));
    assert!(!f._temporary.path().join("must-never-run").exists());
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn restarted_waiting_web_import_rolls_back_reservation_and_import_control() {
    let f = fixture("zero-dependency").await;
    let mut j = interrupted_before_registration(&f);
    let root = j.source_path.clone().unwrap();
    atomic_json(&root.join("panda.json"), &json!({"manifestVersion":1,"runtime":"web","app":{"name":"Disposable interrupted web"},"web":{"root":"dist","entry":"index.html"},"development":{"build":{"kind":"npm","script":"build"}}})).unwrap();
    j.runtime = Some(PandaRuntime::Web);
    j.manifest_digest = Some(format!(
        "{:x}",
        Sha256::digest(read_manifest(&root).unwrap())
    ));
    let web_id = format!("prj_{}", Uuid::parse_str(&j.app_id).unwrap().simple());
    j.web_project_id = Some(web_id.clone());
    j.phase = "web-reserving".into();
    f.manager.save(&j).unwrap();
    let identity = j.source_identity.as_ref().unwrap();
    f.store
        .reserve_managed_source(&web_id, &j.task_id, &root, identity)
        .unwrap();
    f.tasks.bind_web_operation(&j.task_id, &web_id).unwrap();
    // Simulate interruption after durable web reservation but before retiring
    // the import's original source rights: recovery must finish the transfer.
    j.phase = "awaiting-build-confirmation".into();
    f.manager.save(&j).unwrap();
    f.tasks.set_waiting_confirmation(&j.task_id).unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    let recovered = restarted.recover(f.apps.clone()).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].commit_state, CommitState::NotCommitted);
    assert!(!recovered[0].cleanup_pending, "{:?}", recovered[0].task);
    assert!(!root.exists());
    assert!(!f.store.is_registered(&web_id).unwrap());
    assert!(!f.paths.data_root().join("web-apps").join(&web_id).exists());
    assert!(!f.manager.ownership().directory(&j.app_id).unwrap().exists());
    assert!(f
        ._temporary
        .path()
        .join("disposable-import-author/panda.json")
        .is_file());
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn imported_web_public_delete_removes_all_owned_material_and_restart_cannot_revive_git_source(
) {
    delete_imported_web(false).await;
}
#[tokio::test]
async fn public_delete_also_removes_failed_import_retirement_control_and_its_intent() {
    delete_imported_web(true).await;
}
async fn delete_imported_web(retirement_failure: bool) {
    let f = fixture("web-prebuilt").await;
    f.manager
        .retirement_failure
        .store(retirement_failure, Ordering::Release);
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let imported = completed(&f).await;
    assert_eq!(
        imported.commit_state,
        CommitState::Committed,
        "{:?}",
        imported.task
    );
    let journal = f.manager.records.lock().unwrap()[&f.input.import_id].clone();
    let source = imported.source_path.as_ref().unwrap().clone();
    if retirement_failure {
        assert!(imported.cleanup_pending, "{:?}", imported.task);
        assert_eq!(
            imported.task.error.as_ref().unwrap().code,
            "DISPOSABLE_RETIREMENT_FAILURE"
        );
        assert!(f
            .manager
            .ownership()
            .directory(&journal.app_id)
            .unwrap()
            .exists());
        assert!(f
            .paths
            .workspace_root()
            .join(".platform/tasks")
            .join(&journal.task_id)
            .join("import-retirement.json")
            .is_file());
    }
    let original = f
        ._temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-import-author");
    let original_manifest = fs::read(original.join("panda.json")).unwrap();
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
    let service = crate::RuntimeService::start(f.paths.clone(), f.paths.acquire_lock().unwrap())
        .await
        .unwrap();
    let handle = service.handle();
    assert_eq!(
        handle.node_git_import_status(&f.input.import_id).unwrap()["appId"],
        imported.app_id
    );
    let unrelated_source = f
        ._temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-unrelated-user-web");
    fs::create_dir(&unrelated_source).unwrap();
    fs::write(
        unrelated_source.join("index.html"),
        "unrelated user source survives",
    )
    .unwrap();
    atomic_json(&unrelated_source.join("panda.json"), &json!({"manifestVersion":1,"runtime":"web","app":{"name":"Unrelated disposable web"},"web":{"root":".","entry":"index.html"}})).unwrap();
    let unrelated = service
        .store()
        .register_local(&unrelated_source)
        .await
        .unwrap();
    let info = service.store().info(&imported.app_id).await.unwrap();
    assert_eq!(info.git_source.as_ref().unwrap()["url"], f.input.url);
    let deleted = handle
        .project_delete(
            &info.id,
            json!({"expectedDocumentEpoch":info.document_epoch}),
        )
        .await
        .unwrap();
    let terminal = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let task = handle
                .task_for_project(deleted["taskId"].as_str().unwrap(), &info.id, "delete")
                .await
                .unwrap();
            if task["final"] == true {
                break task;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(terminal["status"], "succeeded", "{terminal}");
    assert!(!source.exists());
    for path in [
        f.paths.data_root().join("web-apps").join(&info.id),
        f.paths
            .data_root()
            .join("web-project-ownership")
            .join(&info.id),
        f.manager.ownership().directory(&journal.app_id).unwrap(),
        f.paths
            .data_root()
            .join("onboarding/imports")
            .join(&journal.task_id),
        f.paths
            .workspace_root()
            .join(".platform/sources")
            .join(&journal.source_id),
        f.paths
            .workspace_root()
            .join(".platform/source-publications")
            .join(&journal.app_id),
        f.paths
            .workspace_root()
            .join(".platform/tasks")
            .join(&journal.task_id),
        f.paths
            .workspace_root()
            .join(".platform/tasks")
            .join(deleted["taskId"].as_str().unwrap()),
    ] {
        assert!(!path.exists(), "owned path remains: {}", path.display());
    }
    assert!(handle.node_git_import_status(&f.input.import_id).is_err());
    let reopened = NodeGitImportManager::new(
        f.paths.clone(),
        service.store(),
        handle.node_task_manager(),
        Err(invalid("No tools after deletion")),
    )
    .unwrap();
    assert!(reopened.status(&f.input.import_id).is_err());
    assert!(reopened.recover(f.apps.clone()).await.unwrap().is_empty());
    assert_eq!(
        service.store().info(&unrelated.id).await.unwrap().name,
        "Unrelated disposable web"
    );
    assert_eq!(
        fs::read_to_string(unrelated_source.join("index.html")).unwrap(),
        "unrelated user source survives"
    );
    assert_eq!(
        fs::read(original.join("panda.json")).unwrap(),
        original_manifest
    );
    assert!(original.join(".git").is_dir());
    service.shutdown().await.unwrap();
}
