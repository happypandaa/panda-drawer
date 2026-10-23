//! Explicit, real Radio acceptance flow. This test has network/package-install
//! effects and moves one newly created disposable application slot to macOS
//! Trash. It never reads credentials or mutates the author's original source.
use super::*;
use crate::{
    node_app_configuration::SaveConfigurationRequest,
    node_apps::{DeleteNodeAppInput, NodeApps},
    node_install_tasks::PrepareNodeInstallInput,
    service::NodeGitAction,
    source_acquisition::{AcquireSourceInput, CaptureLocalSourceInput},
    RuntimeHandle, RuntimeService,
};

const SDK_PAYLOADS: &[&str] = &[
    "node_modules/node-windows/bin/winsw/winsw.exe",
    "node_modules/node-windows/bin/sudowin/sudo.exe",
    "node_modules/node-windows/bin/sudowin/Sudowin.Common.dll",
    "node_modules/@neteasecloudmusicapienhanced/api/module/song_url_v1.js",
    "node_modules/@unblockneteasemusic/server/nw.js",
];

async fn start_disposable_radio(apps: &NodeApps, id: &str, evidence: &Path, label: &str) {
    let detail = apps.configuration(id).await.unwrap();
    assert_eq!(detail["configuration"]["canStart"], false);
    // Synthetic credential and a reserved invalid domain: startup/health only.
    // No real account key, song request, AI or speech request is used.
    let input: SaveConfigurationRequest = serde_json::from_value(json!({
        "revision":detail["configuration"]["revision"],
        "values":{"ANTHROPIC_BASE_URL":"https://example.invalid/anthropic", "TTS_PROVIDER":"none"},
        "secrets":{"ANTHROPIC_API_KEY":{"action":"replace", "value":"disposable-test-only-not-a-real-key"}}
    })).unwrap();
    apps.save(id, input).await.unwrap();
    let running = apps.start(id).await.unwrap();
    assert_eq!(running["state"], "running", "{running}");
    apps.stop(id).await.unwrap();
    assert_eq!(apps.configuration(id).await.unwrap()["state"], "stopped");
    let reopened = apps.start(id).await.unwrap();
    assert_eq!(reopened["state"], "running", "{reopened}");
    apps.stop(id).await.unwrap();
    atomic_json(
        &evidence.join(format!("{label}-startup.json")),
        &json!({
            "appId":id,"startedHealthy":true,"stopped":true,"reopenedHealthy":true,
            "syntheticCredentialOnly":true,"aiAndVoiceRequests":false
        }),
    )
    .unwrap();
}

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("explicit {name} is required"))
}

async fn completed(handle: &RuntimeHandle, initial: Value, evidence: &Path, name: &str) -> Value {
    let id = initial["taskId"].as_str().expect("task ID").to_owned();
    let final_task = tokio::time::timeout(Duration::from_secs(1200), async {
        let mut phase = String::new();
        loop {
            let task = handle.task(&id).await.unwrap();
            if task["phase"].as_str().unwrap_or("") != phase {
                phase = task["phase"].as_str().unwrap_or("").into();
                eprintln!("Radio disposable {name}: {phase}");
            }
            if task["final"] == true {
                break task;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("bounded Radio acceptance task");
    atomic_json(&evidence.join(format!("{name}.json")), &final_task).unwrap();
    assert_eq!(final_task["status"], "succeeded", "{final_task}");
    final_task["result"].clone()
}

async fn install(
    handle: &RuntimeHandle,
    apps: Arc<NodeApps>,
    source: &Value,
    target: Option<String>,
    evidence: &Path,
    label: &str,
) -> Value {
    let task = handle
        .node_install_prepare(
            apps.clone(),
            PrepareNodeInstallInput {
                build_source: source["inspection"]["requiresBuild"]
                    .as_bool()
                    .unwrap_or(false)
                    && source["buildSourceDigest"].is_null(),
                source_id: source["sourceId"].as_str().expect("ready source ID").into(),
                expected_source_digest: source["contentDigest"]
                    .as_str()
                    .expect("ready source digest")
                    .into(),
                target_app_id: target,
            },
        )
        .await
        .unwrap();
    let candidate = completed(handle, task, evidence, &format!("{label}-prepare")).await;
    let task = handle
        .node_install_apply(
            apps,
            candidate["candidate"]["candidateId"]
                .as_str()
                .expect("prepared candidate ID"),
        )
        .await
        .unwrap();
    completed(handle, task, evidence, &format!("{label}-apply")).await
}

/// Required variables:
/// PLATFORM_RADIO_REMOTE=https://github.com/happypandaa/radio-bob.git
/// PLATFORM_RADIO_BRANCH=codex/tts-providers
/// PLATFORM_RADIO_COMMIT=<reviewed full 40-character remote HEAD>
/// PLATFORM_RADIO_SOURCE=<clean disposable Git checkout below .test-artifacts>
/// PLATFORM_RADIO_SYSTEM_TRASH=1
/// Optional PLATFORM_RADIO_EVIDENCE_ROOT must also be below .test-artifacts.
/// For a private Radio repository, PLATFORM_RADIO_AUTHENTICATED_GIT may explicitly
/// select /usr/bin/git to use this machine's existing Git login, then exercise
/// the public local-source capture flow. This is not platform Git authentication
/// and is never an automatic fallback from anonymous source acquisition.
/// All app/runtime/evidence files are retained. The Trash result is never purged.
#[tokio::test]
#[ignore = "explicit real Radio network/npm/macOS Trash acceptance; see required PLATFORM_RADIO_* variables"]
async fn radio_source_git_build_update_system_trash_and_remote_reinstall() {
    let remote = required_env("PLATFORM_RADIO_REMOTE");
    assert_eq!(remote, "https://github.com/happypandaa/radio-bob.git");
    let branch = required_env("PLATFORM_RADIO_BRANCH");
    assert!(super::super::validate_branch_name(&branch).is_ok());
    let commit = required_env("PLATFORM_RADIO_COMMIT");
    assert!(valid_oid(&commit));
    assert_eq!(required_env("PLATFORM_RADIO_SYSTEM_TRASH"), "1");
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let artifacts = repository.join(".test-artifacts");
    fs::create_dir_all(&artifacts).unwrap();
    let artifacts = artifacts.canonicalize().unwrap();
    let source = PathBuf::from(required_env("PLATFORM_RADIO_SOURCE"))
        .canonicalize()
        .unwrap();
    assert!(
        source.starts_with(&artifacts),
        "Radio source must be a disposable checkout in .test-artifacts"
    );
    assert!(
        source
            .components()
            .any(|part| part.as_os_str().to_string_lossy().starts_with("disposable")),
        "checkout name must identify a disposable source"
    );
    node_root_identity(&source).unwrap();
    let base = std::env::var_os("PLATFORM_RADIO_EVIDENCE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| artifacts.join("radio-development-e2e"));
    fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    assert!(base.starts_with(&artifacts));
    check_no_symlinks(&base, false).unwrap();
    let evidence = base.join(format!("disposable-radio-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&evidence).unwrap();
    let paths = RuntimePaths::for_test_at(evidence.join("platform-data")).unwrap();
    let resources = repository.join("apps/desktop/src-tauri/resources");
    let environment = GitEnvironmentManager::new(
        paths.data_root().to_path_buf(),
        Some(resources.join("development-git")),
    );
    let git = NodeGit::new(environment);
    let preview = git.status(&source).await.unwrap();
    atomic_json(&evidence.join("git-status.json"), &preview).unwrap();
    assert_eq!(
        head_oid(&preview),
        Some(commit.as_str()),
        "checkout must match reviewed remote commit: {preview:?}"
    );
    assert!(
        preview.changes.is_empty(),
        "disposable source must start clean"
    );
    assert!(
        preview.can_save,
        "Node Git scope must support the real Radio repository: {preview:?}"
    );
    atomic_json(&evidence.join("git-status.json"), &preview).unwrap();
    atomic_json(
        &evidence.join("git-history.json"),
        &git.versions(&source, 10).await.unwrap(),
    )
    .unwrap();
    let source_manifest = read_file(&source.join("local-app.json"), MAX_METADATA).unwrap();
    let source_manifest: Value = serde_json::from_slice(&source_manifest).unwrap();
    // Legacy direct-JavaScript manifests use the implicit no-build declaration.
    let service = RuntimeService::start_with_resources(
        paths.clone(),
        paths.acquire_lock().unwrap(),
        resources.clone(),
        std::env::current_exe().unwrap().canonicalize().unwrap(),
    )
    .await
    .unwrap();
    let handle = service.handle();
    let apps = Arc::new(
        NodeApps::new(
            paths.data_root().to_path_buf(),
            resources.join("node-runtime/bin/node"),
        )
        .unwrap(),
    );
    let initial = apps.register(source.clone()).await.unwrap();
    let app_id = initial["id"].as_str().unwrap().to_owned();
    let info = handle.node_development_info(&apps, &app_id).await.unwrap();
    assert_eq!(info["sourceRoot"], json!(source));
    let view = handle
        .node_development_git(&apps, &app_id, NodeGitAction::Status)
        .await
        .unwrap();
    assert_eq!(view["repository"]["head"]["oid"], commit);
    let build = handle
        .node_development_build(apps.clone(), app_id.clone())
        .await
        .unwrap();
    let built = completed(&handle, build, &evidence, "build-source").await;
    assert_eq!(built["sourceCommit"], commit);
    assert_eq!(built["installed"], false);
    let updated = install(
        &handle,
        apps.clone(),
        &built["source"],
        Some(app_id.clone()),
        &evidence,
        "update",
    )
    .await;
    atomic_json(&evidence.join("update-result.json"), &updated).unwrap();
    let details = apps.configuration(&app_id).await.unwrap();
    assert_eq!(details["id"], app_id);
    assert_eq!(details["installation"]["kind"], "managed");
    let program = PathBuf::from(details["sourcePath"].as_str().unwrap());
    assert_ne!(program, source);
    assert!(!program.join(".git").exists());
    let payloads: Vec<_> = SDK_PAYLOADS
        .iter()
        .map(|path| {
            let bytes = read_file(&program.join(path), MAX_BLOB).unwrap();
            assert!(!bytes.is_empty(), "complete SDK payload: {path}");
            (path, bytes)
        })
        .collect();
    let runtime_entry = program.join("server/managed.mjs");
    let before = read_file(&runtime_entry, MAX_BLOB).unwrap();
    let source_entry = source.join("server/managed.mjs");
    let mut changed = read_file(&source_entry, MAX_BLOB).unwrap();
    changed.extend_from_slice(
        b"\n// Disposable acceptance: runtime must remain independent of source edits.\n",
    );
    fs::write(&source_entry, &changed).unwrap();
    assert_eq!(read_file(&runtime_entry, MAX_BLOB).unwrap(), before);
    assert_eq!(
        apps.configuration(&app_id).await.unwrap()["installation"]["generation"],
        details["installation"]["generation"]
    );
    start_disposable_radio(&apps, &app_id, &evidence, "updated").await;
    atomic_json(&evidence.join("runtime-isolation.json"),&json!({"appId":app_id,"sourceRoot":source,"runtimeRoot":program,"sourceEditPreserved":true,"runtimeUnchanged":true,"started":true,"syntheticCredentialOnly":true})).unwrap();
    let marker = paths
        .data_root()
        .join("node-apps")
        .join(&app_id)
        .join("data/disposable-keep-proof.txt");
    fs::write(
        &marker,
        b"Disposable Radio data must move with its whole application slot.\n",
    )
    .unwrap();
    let deletion = apps.delete_preview(&app_id).await.unwrap();
    assert_eq!(deletion["canDelete"], true, "{deletion}");
    let deleted = apps
        .delete(
            &app_id,
            DeleteNodeAppInput {
                preview_id: deletion["previewId"].as_str().unwrap().into(),
            },
        )
        .await
        .unwrap();
    atomic_json(&evidence.join("system-trash-result.json"), &deleted).unwrap();
    assert_eq!(deleted["operationOutcome"], "system_trashed");
    let trashed = PathBuf::from(deleted["systemTrashPath"].as_str().unwrap());
    assert_eq!(
        read_file(&trashed.join("data/disposable-keep-proof.txt"), MAX_BLOB).unwrap(),
        b"Disposable Radio data must move with its whole application slot.\n"
    );
    assert!(source_entry.exists());
    assert!(!paths.data_root().join("node-apps").join(&app_id).exists());
    assert!(apps.list().await.unwrap().is_empty());
    let download_mode;
    let acquired =
        if let Some(authenticated_git) = std::env::var_os("PLATFORM_RADIO_AUTHENTICATED_GIT") {
            assert_eq!(PathBuf::from(&authenticated_git), Path::new("/usr/bin/git"));
            download_mode = "existing-local-git-login-then-platform-source-capture";
            let downloaded = evidence.join("disposable-authenticated-radio-redownload");
            let hooks = evidence.join("empty-download-hooks");
            fs::create_dir(&hooks).unwrap();
            let mut command = tokio::process::Command::new(authenticated_git);
            command
                .arg("-c")
                .arg(format!("core.hooksPath={}", hooks.display()))
                .args([
                    "-c",
                    "core.fsmonitor=false",
                    "clone",
                    "--depth=1",
                    "--single-branch",
                    "--no-tags",
                    "--no-recurse-submodules",
                    "--branch",
                ])
                .arg(&branch)
                .arg("--")
                .arg(&remote)
                .arg(&downloaded)
                .current_dir(&evidence)
                .env("GIT_TERMINAL_PROMPT", "0")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            assert!(
                tokio::time::timeout(Duration::from_secs(180), command.status())
                    .await
                    .expect("bounded authenticated Git download")
                    .unwrap()
                    .success(),
                "existing local Git login must support the reviewed private repository"
            );
            let downloaded_status = git.status(&downloaded).await.unwrap();
            assert_eq!(head_oid(&downloaded_status), Some(commit.as_str()));
            assert!(downloaded_status.changes.is_empty());
            atomic_json(
                &evidence.join("authenticated-download.json"),
                &json!({
                    "mode":download_mode,"branch":branch,"commit":commit,
                    "platformPrivateAuthenticationImplemented":false
                }),
            )
            .unwrap();
            handle
                .source_capture(CaptureLocalSourceInput {
                    source_path: downloaded,
                    name: Some("Disposable Radio fresh Git download".into()),
                })
                .await
                .unwrap()
        } else {
            download_mode = "platform-anonymous-public-git-download";
            handle
                .source_acquire(AcquireSourceInput {
                    url: remote.clone(),
                    branch: Some(branch.clone()),
                    name: Some("Disposable Radio reviewed remote re-download".into()),
                })
                .await
                .unwrap()
        };
    let remote_source = completed(&handle, acquired, &evidence, "remote-acquire").await;
    if download_mode == "platform-anonymous-public-git-download" {
        assert_eq!(remote_source["branch"], branch);
        assert_eq!(
            remote_source["commit"], commit,
            "remote moved; review a new commit before repeating"
        );
    }
    let reinstalled = install(
        &handle,
        apps.clone(),
        &remote_source,
        None,
        &evidence,
        "remote-reinstall",
    )
    .await;
    atomic_json(&evidence.join("remote-reinstall-result.json"), &reinstalled).unwrap();
    let listed = apps.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_ne!(listed[0]["id"], app_id);
    assert_eq!(listed[0]["installation"]["kind"], "managed");
    let fresh = PathBuf::from(listed[0]["sourcePath"].as_str().unwrap());
    let fresh_manifest: Value =
        serde_json::from_slice(&read_file(&fresh.join("local-app.json"), MAX_METADATA).unwrap())
            .unwrap();
    assert_eq!(
        fresh_manifest["development"],
        source_manifest["development"]
    );
    assert_eq!(fresh_manifest["app"]["id"], source_manifest["app"]["id"]);
    assert_eq!(
        read_file(&fresh.join("server/managed.mjs"), MAX_BLOB).unwrap(),
        before
    );
    assert!(!fresh.join(".git").exists());
    for (path, bytes) in payloads {
        assert_eq!(
            read_file(&fresh.join(path), MAX_BLOB).unwrap(),
            bytes,
            "complete SDK resource must survive Git reinstallation: {path}"
        );
    }
    start_disposable_radio(
        &apps,
        listed[0]["id"].as_str().unwrap(),
        &evidence,
        "reinstalled",
    )
    .await;
    apps.stop_all().await.unwrap();
    service.shutdown().await.unwrap();
    atomic_json(&evidence.join("completed.json"),&json!({"remote":remote,"branch":branch,"reviewedCommit":commit,"downloadMode":download_mode,"evidenceRoot":evidence,"sourceRoot":source,"systemTrashPath":trashed,"originalAppId":app_id,"reinstalledAppId":listed[0]["id"],"networkAcquisitionVerified":true,"physicalSystemTrashVerified":true,"completeSdkPayloadsVerified":SDK_PAYLOADS,"startupAndRestartVerified":true,"aiAndVoiceTested":false,"windowsRuntimeTested":false})).unwrap();
    eprintln!(
        "Radio disposable acceptance evidence retained: {}",
        evidence.display()
    );
}
