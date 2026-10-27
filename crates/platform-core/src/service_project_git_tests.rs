//! Management boundary tests. These do not exercise a native user flow.
use super::*;
use std::os::unix::fs::PermissionsExt;

async fn git_http(
    service: &RuntimeService,
    client: &reqwest::Client,
    method: Method,
    path: &str,
    body: Option<Value>,
    expected: StatusCode,
) -> Value {
    let mut request = client
        .request(method, format!("{}/api/v1/{path}", service.info().base_url))
        .bearer_auth(&service.shared.snapshot().token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    assert_eq!(response.status(), expected);
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["ok"], true, "{value}");
    value["data"].clone()
}

/// Prepare disposable versions with standard Git; retired platform write
/// routes are never used to set up the retained HTTP restoration tests.
async fn git_fixture_write(service: &RuntimeService, path: &str, body: Option<Value>) -> Value {
    let state = service.handle().current();
    let manager = state.project_git.as_ref().unwrap();
    let id = path.split('/').nth(1).unwrap();
    let body = body.unwrap();
    let task = match path.rsplit('/').next().unwrap() {
        "initialize" => {
            manager
                .fixture_initialize(
                    id,
                    serde_json::from_value(body).unwrap(),
                    state.stop.subscribe(),
                )
                .await
        }
        "saves" => {
            manager
                .fixture_save(
                    id,
                    serde_json::from_value(body).unwrap(),
                    state.stop.subscribe(),
                )
                .await
        }
        "data-saves" => {
            manager
                .fixture_save_data(
                    id,
                    serde_json::from_value(body).unwrap(),
                    state.stop.subscribe(),
                )
                .await
        }
        _ => unreachable!(),
    }
    .unwrap();
    serde_json::to_value(task).unwrap()
}

async fn git_http_task(service: &RuntimeService, client: &reqwest::Client, task: &Value) -> Value {
    let path = format!("tasks/{}", task["taskId"].as_str().unwrap());
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let value = git_http(service, client, Method::GET, &path, None, StatusCode::OK).await;
            if value["final"] == true {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

/// A real loopback management/SDK boundary with an absent live database after
/// the first owned move. All source, SQL and attachment values are disposable.
#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable development-git payload"]
async fn official_git_data_restore_http_recovers_without_reopening_missing_database() {
    let tool_root =
        std::path::PathBuf::from(std::env::var("LOCAL_PROJECTS_TEST_GIT_ROOT").unwrap())
            .canonicalize()
            .unwrap();
    assert!(tool_root.starts_with(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.test-artifacts")
            .canonicalize()
            .unwrap()
    ));
    let (_temp, _paths, service, _client) = tests::start().await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    service
        .handle()
        .set_git_path(Some(tool_root.join("bin/git").to_string_lossy().into()))
        .await
        .unwrap();
    let project = tests::create(&service, "Disposable HTTP data restore recovery").await;
    let cookie = tests::connect(&service, &client, &project.id).await;
    let base = format!("projects/{}/git", project.id);
    let endpoint = format!("{}/api/v1/{base}", service.info().base_url);
    for (method, suffix) in [
        (Method::POST, "data-restores"),
        (Method::GET, "data-recovery"),
        (Method::POST, "data-recoveries"),
    ] {
        for with_cookie in [false, true] {
            let mut request = client.request(method.clone(), format!("{endpoint}/{suffix}"));
            if with_cookie {
                request = request.header(header::COOKIE, &cookie);
            }
            if method == Method::POST {
                request = request.json(&json!({}));
            }
            assert_eq!(
                request.send().await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
        }
    }
    let token = service.shared.snapshot().token.clone();
    let response = client
        .get(format!("{endpoint}/data-recovery?unexpected=true"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let invalid: Value = response.json().await.unwrap();
    assert_eq!(invalid["error"]["code"], "INVALID_INPUT");
    let status = git_http(&service, &client, Method::GET, &base, None, StatusCode::OK).await;
    let initialized = git_fixture_write(
        &service,
        &format!("{base}/initialize"),
        Some(json!({"expectedSnapshot":status["snapshotDigest"],"initialBranch":"main"})),
    )
    .await;
    assert_eq!(
        git_http_task(&service, &client, &initialized).await["status"],
        "succeeded"
    );
    let db = format!("projects/{}/db/execute", project.id);
    for sql in [
        "CREATE TABLE notes(body TEXT)",
        "INSERT INTO notes VALUES('target HTTP value')",
    ] {
        git_http(
            &service,
            &client,
            Method::POST,
            &db,
            Some(json!({"sql":sql})),
            StatusCode::OK,
        )
        .await;
    }
    let attachments = project.project_root.join("data/attachments");
    std::fs::write(
        attachments.join("complete.txt"),
        b"target HTTP attachment\nsecond line  ",
    )
    .unwrap();
    std::fs::create_dir(attachments.join("target-empty")).unwrap();
    let status = git_http(&service, &client, Method::GET, &base, None, StatusCode::OK).await;
    let saved = git_fixture_write(&service, &format!("{base}/data-saves"), Some(json!({"expectedHead":status["repository"]["head"]["oid"],"expectedSnapshot":status["snapshotDigest"],"message":"Disposable HTTP restore target","author":{"name":"HTTP restore fixture","email":"http-restore@example.invalid"}}))).await;
    let saved = git_http_task(&service, &client, &saved).await;
    assert_eq!(saved["status"], "succeeded", "{saved}");
    let target = saved["result"]["commit"].as_str().unwrap().to_owned();
    git_http(
        &service,
        &client,
        Method::POST,
        &db,
        Some(json!({"sql":"UPDATE notes SET body='before HTTP restore'"})),
        StatusCode::OK,
    )
    .await;
    std::fs::write(attachments.join("complete.txt"), b"before HTTP attachment").unwrap();
    std::fs::write(attachments.join("extra.txt"), b"before extra attachment").unwrap();
    let preview = git_http(
        &service,
        &client,
        Method::POST,
        &format!("{base}/data-restore-preview"),
        Some(json!({"targetCommit":target})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(preview["applyImplemented"], true);
    assert_eq!(preview["canApply"], true);
    let head = std::fs::read(project.project_root.join(".git/HEAD")).unwrap();
    let index = std::fs::read(project.project_root.join(".git/index")).unwrap();
    service
        .handle()
        .current()
        .project_git
        .as_ref()
        .unwrap()
        .test_data_restore_fault(4);
    let accepted = git_http(&service, &client, Method::POST, &format!("{base}/data-restores"),
        Some(json!({"targetCommit":target,"expectedHead":preview["head"]["oid"],"expectedBranch":preview["head"]["branch"],"expectedSnapshot":preview["snapshotDigest"],"expectedDocumentEpoch":preview["documentEpoch"],"expectedLiveDataDigest":preview["liveDataDigest"],"expectedTargetSnapshotDigest":preview["targetSnapshotDigest"],"expectedPlanDigest":preview["planDigest"]})),
        StatusCode::ACCEPTED).await;
    assert_eq!(accepted["type"], "gitDataRestoreApply");
    let failed = git_http_task(&service, &client, &accepted).await;
    assert_eq!(failed["status"], "failed", "{failed}");
    assert_eq!(
        failed["error"]["code"], "PROJECT_DATA_RESTORE_TEST_FAULT",
        "{failed}"
    );
    let database = project.project_root.join("data/main.sqlite");
    assert!(
        !database.exists(),
        "fault must leave the owned original displaced, without opening a replacement"
    );
    let pending = git_http(
        &service,
        &client,
        Method::GET,
        &format!("{base}/data-recovery"),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(pending["state"], "pending", "{pending}");
    assert_eq!(pending["businessBlocked"], true);
    assert_eq!(
        crate::workspace::validate_idle(&service.handle().current().paths)
            .unwrap_err()
            .code,
        "PROJECT_DATA_RESTORE_PENDING"
    );
    let operation = &pending["operations"][0];
    assert_eq!(operation["restoreTaskId"], accepted["taskId"]);
    assert_eq!(operation["originalRetained"], true);
    assert_eq!(operation["canContinue"], true);
    let info = git_http(
        &service,
        &client,
        Method::GET,
        &format!("projects/{}", project.id),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(info["manifest"].is_object(), "{info}");
    assert_eq!(
        info["projectRoot"],
        project.project_root.to_string_lossy().as_ref()
    );
    assert_eq!(info["buildInfo"]["dataRecoveryRequired"], true);
    let response = client
        .post(format!("{}/api/v1/{db}", service.info().base_url))
        .bearer_auth(&token)
        .json(&json!({"sql":"SELECT body FROM notes"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let blocked: Value = response.json().await.unwrap();
    assert_eq!(blocked["error"]["code"], "PROJECT_DATA_RESTORE_PENDING");
    assert!(
        !database.exists(),
        "blocked diagnostic/SQL reads must not create a blank live database"
    );
    let recovered = git_http(&service, &client, Method::POST, &format!("{base}/data-recoveries"),
        Some(json!({"restoreTaskId":operation["restoreTaskId"],"expectedPlanDigest":operation["planDigest"],"expectedJournalDigest":operation["journalDigest"],"action":"continue"})), StatusCode::ACCEPTED).await;
    assert_eq!(recovered["type"], "gitDataRestoreContinue");
    let recovered = git_http_task(&service, &client, &recovered).await;
    assert_eq!(recovered["status"], "succeeded", "{recovered}");
    assert_eq!(recovered["result"]["operationOutcome"], "committed");
    assert_eq!(recovered["result"]["dataState"], "applied");
    assert_eq!(recovered["result"]["pending"], false);
    assert_eq!(recovered["result"]["businessBlocked"], false);
    let clear = git_http(
        &service,
        &client,
        Method::GET,
        &format!("{base}/data-recovery"),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(clear["state"], "clear");
    assert_eq!(clear["businessBlocked"], false);
    let read = git_http(
        &service,
        &client,
        Method::POST,
        &db,
        Some(json!({"sql":"SELECT body FROM notes"})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(read["rows"][0]["body"], "target HTTP value");
    assert_eq!(
        std::fs::read(attachments.join("complete.txt")).unwrap(),
        b"target HTTP attachment\nsecond line  "
    );
    assert!(attachments.join("target-empty").is_dir());
    assert!(!attachments.join("extra.txt").exists());
    assert_eq!(
        std::fs::read(project.project_root.join(".git/HEAD")).unwrap(),
        head
    );
    assert_eq!(
        std::fs::read(project.project_root.join(".git/index")).unwrap(),
        index
    );
    let response = client
        .post(format!(
            "{}/p/{}/api/v1/db/execute",
            service.info().base_url,
            project.id
        ))
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, &service.info().base_url)
        .header("x-platform-document-epoch", &project.document_epoch)
        .json(&json!({"sql":"SELECT body FROM notes"}))
        .send()
        .await
        .unwrap();
    assert!(matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::CONFLICT
    ));
    let old_page: Value = response.json().await.unwrap();
    assert!(
        matches!(
            old_page["error"]["code"].as_str(),
            Some("INVALID_SESSION" | "PROJECT_RELOAD_REQUIRED")
        ),
        "{old_page}"
    );
    Box::pin(verify_completed_data_restore_public_migration(
        &service,
        &client,
        _temp.path(),
        &project.id,
    ))
    .await;
    service.shutdown().await.unwrap();
}

async fn verify_completed_data_restore_public_migration(
    service: &RuntimeService,
    client: &reqwest::Client,
    temporary: &std::path::Path,
    id: &str,
) {
    let db = format!("projects/{id}/db/execute");
    git_http(
        service,
        client,
        Method::POST,
        &db,
        Some(json!({"sql":"UPDATE notes SET body='ordinary value before migration'"})),
        StatusCode::OK,
    )
    .await;
    let destination = temporary
        .canonicalize()
        .unwrap()
        .join("completed-restore-migrated-workspace");
    std::fs::create_dir(&destination).unwrap();
    let migration = service
        .handle()
        .workspace_migrate(json!({"destinationPath":destination}))
        .await
        .unwrap();
    let migrated = git_http_task(service, client, &migration).await;
    assert_eq!(migrated["status"], "succeeded", "{migrated}");
    assert_eq!(service.handle().info().workspace_root, destination);
    let state = git_http(
        service,
        client,
        Method::GET,
        &format!("projects/{id}/git/data-recovery"),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(state["state"], "clear", "{state}");
    let result = git_http(
        service,
        client,
        Method::POST,
        &db,
        Some(json!({"sql":"SELECT body FROM notes"})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(result["rows"][0]["body"], "ordinary value before migration");
    git_http(
        service,
        client,
        Method::POST,
        &db,
        Some(json!({"sql":"UPDATE notes SET body='ordinary value after migration'"})),
        StatusCode::OK,
    )
    .await;
}

fn disposable_git_object(
    git: &std::path::Path,
    project: &std::path::Path,
    home: &std::path::Path,
    index: &std::path::Path,
    args: &[&str],
    input: &[u8],
) -> String {
    use std::io::Write as _;
    let mut child = std::process::Command::new(git)
        .current_dir(project)
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_INDEX_FILE", index)
        .env("GIT_AUTHOR_NAME", "Disposable fixture")
        .env("GIT_AUTHOR_EMAIL", "disposable@example.invalid")
        .env("GIT_COMMITTER_NAME", "Disposable fixture")
        .env("GIT_COMMITTER_EMAIL", "disposable@example.invalid")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "commit.gpgSign=false",
        ])
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "disposable Git fixture failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().into()
}

/// Public HTTP/RuntimeHandle preview uses real Git and SQLite; applying a data
/// version is deliberately unavailable in this step.
#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable development-git payload"]
async fn official_git_data_preview_http_is_authenticated_read_only_and_returns_200() {
    let tool_root =
        std::path::PathBuf::from(std::env::var("LOCAL_PROJECTS_TEST_GIT_ROOT").unwrap())
            .canonicalize()
            .unwrap();
    assert!(tool_root.starts_with(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.test-artifacts")
            .canonicalize()
            .unwrap()
    ));
    let (_temporary, _paths, service, client) = tests::start().await;
    service
        .handle()
        .set_git_path(Some(tool_root.join("bin/git").to_string_lossy().into()))
        .await
        .unwrap();
    let project = tests::create(&service, "Disposable HTTP data preview").await;
    let base = format!("projects/{}/git", project.id);
    let endpoint = format!(
        "{}/api/v1/{base}/data-restore-preview",
        service.info().base_url
    );
    let unauthenticated = client
        .post(&endpoint)
        .json(&json!({"targetCommit":"1111111111111111111111111111111111111111"}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert!(!project.project_root.join(".git").exists());
    let status = git_http(&service, &client, Method::GET, &base, None, StatusCode::OK).await;
    let task = git_fixture_write(
        &service,
        &format!("{base}/initialize"),
        Some(json!({"initialBranch":"main","expectedSnapshot":status["snapshotDigest"]})),
    )
    .await;
    assert_eq!(
        git_http_task(&service, &client, &task).await["status"],
        "succeeded"
    );
    let db = format!("projects/{}/db/execute", project.id);
    for sql in [
        "CREATE TABLE notes(body TEXT)",
        "INSERT INTO notes VALUES('saved HTTP data')",
    ] {
        git_http(
            &service,
            &client,
            Method::POST,
            &db,
            Some(json!({"sql":sql})),
            StatusCode::OK,
        )
        .await;
    }
    std::fs::write(
        project.project_root.join("data/attachments/note.txt"),
        "saved HTTP attachment",
    )
    .unwrap();
    let status = git_http(&service, &client, Method::GET, &base, None, StatusCode::OK).await;
    let task = git_fixture_write(&service, &format!("{base}/data-saves"), Some(json!({"expectedHead":status["repository"]["head"]["oid"],"expectedSnapshot":status["snapshotDigest"],"message":"disposable preview target","author":{"name":"HTTP preview fixture","email":"preview@example.invalid"}}))).await;
    let saved = git_http_task(&service, &client, &task).await;
    assert_eq!(saved["status"], "succeeded", "{saved}");
    let target_commit = saved["result"]["commit"].as_str().unwrap();
    git_http(
        &service,
        &client,
        Method::POST,
        &db,
        Some(json!({"sql":"INSERT INTO notes VALUES('unsaved HTTP data remains live')"})),
        StatusCode::OK,
    )
    .await;
    std::fs::write(
        project.project_root.join("data/attachments/note.txt"),
        "unsaved HTTP attachment remains live",
    )
    .unwrap();
    let index = std::fs::read(project.project_root.join(".git/index")).unwrap();
    let manifest = std::fs::read(project.project_root.join("version-data/snapshot.json")).unwrap();
    let preview = git_http(
        &service,
        &client,
        Method::POST,
        &format!("{base}/data-restore-preview"),
        Some(json!({"targetCommit":target_commit})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(preview["targetCommit"], target_commit);
    assert_eq!(preview["canApply"], cfg!(target_os = "macos"));
    assert_eq!(preview["applyImplemented"], cfg!(target_os = "macos"));
    assert_eq!(preview["databaseChanged"], true);
    assert_eq!(preview["issues"], json!([]));
    assert_eq!(
        preview["attachmentChanges"],
        json!([{"path":"note.txt","kind":"modify"}])
    );
    let direct = service
        .handle()
        .project_git_data_restore_preview(
            &project.id,
            crate::project_git::GitDataRestorePreviewInput {
                target_commit: target_commit.into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(preview["planDigest"], direct["planDigest"]);
    assert_eq!(
        std::fs::read(project.project_root.join(".git/index")).unwrap(),
        index
    );
    assert_eq!(
        std::fs::read(project.project_root.join("version-data/snapshot.json")).unwrap(),
        manifest
    );
    assert_eq!(
        std::fs::read_to_string(project.project_root.join("data/attachments/note.txt")).unwrap(),
        "unsaved HTTP attachment remains live"
    );
    let read = git_http(
        &service,
        &client,
        Method::POST,
        &db,
        Some(json!({"sql":"SELECT body FROM notes ORDER BY rowid"})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(read["rows"][1]["body"], "unsaved HTTP data remains live");
    for body in [
        json!({"targetCommit":"HEAD"}),
        json!({"targetCommit":target_commit,"apply":true}),
    ] {
        let invalid = client
            .post(&endpoint)
            .bearer_auth(&service.shared.snapshot().token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    }
    service.shutdown().await.unwrap();
}

/// Real public HTTP and a real Git executable; still not a native UI test.
#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable development-git payload"]
async fn official_git_source_restore_http_roundtrip_preserves_head_index_runtime_and_data() {
    let tool_root =
        std::path::PathBuf::from(std::env::var("LOCAL_PROJECTS_TEST_GIT_ROOT").unwrap())
            .canonicalize()
            .unwrap();
    let evidence = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.test-artifacts")
        .canonicalize()
        .unwrap();
    assert!(tool_root.starts_with(evidence));
    let (temp, _paths, service, client) = tests::start().await;
    service
        .handle()
        .set_git_path(Some(tool_root.join("bin/git").to_string_lossy().into()))
        .await
        .unwrap();
    let project = tests::create(&service, "Disposable HTTP source recovery").await;
    let source = project.project_root.join("docs/disposable-restore.txt");
    std::fs::write(&source, b"first saved source\n").unwrap();
    let base = format!("projects/{}/git", project.id);
    let status = git_http(&service, &client, Method::GET, &base, None, StatusCode::OK).await;
    assert_eq!(status["canInitialize"], false, "{status}");
    let task = git_fixture_write(
        &service,
        &format!("{base}/initialize"),
        Some(json!({"initialBranch":"main","expectedSnapshot":status["snapshotDigest"]})),
    )
    .await;
    let done = git_http_task(&service, &client, &task).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    let mut commits = Vec::new();
    for (contents, message) in [
        ("first saved source\n", "First disposable version"),
        ("second saved source\n", "Second disposable version"),
    ] {
        std::fs::write(&source, contents).unwrap();
        let status = git_http(&service, &client, Method::GET, &base, None, StatusCode::OK).await;
        let task = git_fixture_write(&service, &format!("{base}/saves"), Some(json!({"expectedHead":status["repository"]["head"]["oid"],"expectedSnapshot":status["snapshotDigest"],"message":message,"author":{"name":"Disposable HTTP","email":"disposable@example.invalid"}}))).await;
        let done = git_http_task(&service, &client, &task).await;
        assert_eq!(done["status"], "succeeded", "{done}");
        let status = git_http(&service, &client, Method::GET, &base, None, StatusCode::OK).await;
        commits.push(status["repository"]["head"]["oid"].clone());
    }
    let db = format!("projects/{}/db/execute", project.id);
    for sql in [
        "CREATE TABLE restore_check(value TEXT)",
        "INSERT INTO restore_check VALUES ('keep this artificial record')",
    ] {
        git_http(
            &service,
            &client,
            Method::POST,
            &db,
            Some(json!({"sql":sql})),
            StatusCode::OK,
        )
        .await;
    }
    let index = std::fs::read(project.project_root.join(".git/index")).unwrap();
    let output = std::fs::read(project.project_root.join("dist/index.html")).unwrap();
    // Construct unsafe historical trees using a separate temporary index. This
    // writes disposable Git objects only: HEAD, the real index and live files
    // never enter these states. The product must reject the whole target.
    let object_home = temp.path().join("git-object-home");
    std::fs::create_dir(&object_home).unwrap();
    let object_index = temp.path().join("object-fixture-index");
    let git = tool_root.join("bin/git");
    for (path, mode, contents, expected) in [
        (
            "project.json",
            "100644",
            "{\"different\":true}\n",
            "PROJECT_SOURCE_RESTORE_MANIFEST_CHANGED",
        ),
        (
            "data/disposable-protected.txt",
            "100644",
            "artificial protected content\n",
            "PROJECT_SOURCE_RESTORE_SCOPE_UNSUPPORTED",
        ),
        (
            "unknown-root-file.txt",
            "100644",
            "unknown source scope\n",
            "PROJECT_SOURCE_RESTORE_SCOPE_UNSUPPORTED",
        ),
        (
            "app/src/disposable-link",
            "120000",
            "../../outside-target",
            "PROJECT_SOURCE_RESTORE_TREE_UNSUPPORTED",
        ),
        (
            "app/src/disposable-submodule",
            "160000",
            "",
            "PROJECT_SOURCE_RESTORE_TREE_UNSUPPORTED",
        ),
        (
            ".gitattributes",
            "100644",
            "*.txt filter=untrusted\n",
            "PROJECT_SOURCE_RESTORE_ATTRIBUTES_UNSUPPORTED",
        ),
    ] {
        let run = |args: &[&str], bytes: &[u8]| {
            disposable_git_object(
                &git,
                &project.project_root,
                &object_home,
                &object_index,
                args,
                bytes,
            )
        };
        run(&["read-tree", "HEAD"], b"");
        let oid = if mode == "160000" {
            commits[1].as_str().unwrap().to_owned()
        } else {
            run(&["hash-object", "-w", "--stdin"], contents.as_bytes())
        };
        run(
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("{mode},{oid},{path}"),
            ],
            b"",
        );
        let tree = run(&["write-tree"], b"");
        let target = run(
            &["commit-tree", &tree, "-p", commits[1].as_str().unwrap()],
            b"Disposable unsupported historical target\n",
        );
        let response: Value = client
            .post(format!(
                "{}/api/v1/{base}/source-restore-preview",
                service.info().base_url
            ))
            .bearer_auth(&service.shared.snapshot().token)
            .json(&json!({"targetCommit":target}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["error"]["code"], expected, "{path}: {response}");
        assert_eq!(std::fs::read(&source).unwrap(), b"second saved source\n");
        assert_eq!(
            std::fs::read(project.project_root.join(".git/index")).unwrap(),
            index
        );
    }
    let cookie = tests::connect(&service, &client, &project.id).await;
    let preview = git_http(
        &service,
        &client,
        Method::POST,
        &format!("{base}/source-restore-preview"),
        Some(json!({"targetCommit":commits[0]})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(preview["canApply"], true, "{preview}");
    assert_eq!(preview["changes"].as_array().unwrap().len(), 1);
    let task = git_http(&service, &client, Method::POST, &format!("{base}/source-restores"), Some(json!({"targetCommit":preview["targetCommit"],"expectedHead":preview["head"]["oid"],"expectedBranch":preview["head"]["branch"],"expectedSnapshot":preview["snapshotDigest"],"expectedPlanDigest":preview["planDigest"]})), StatusCode::ACCEPTED).await;
    let done = git_http_task(&service, &client, &task).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(done["result"]["state"], "completed", "{done}");
    assert_eq!(std::fs::read(&source).unwrap(), b"first saved source\n");
    assert_eq!(
        std::fs::read(project.project_root.join(".git/index")).unwrap(),
        index
    );
    assert_eq!(
        std::fs::read(project.project_root.join("dist/index.html")).unwrap(),
        output
    );
    let status = git_http(&service, &client, Method::GET, &base, None, StatusCode::OK).await;
    assert_eq!(status["repository"]["head"]["oid"], commits[1]);
    assert_eq!(status["changes"].as_array().unwrap().len(), 1);
    let recovery = git_http(
        &service,
        &client,
        Method::GET,
        &format!("{base}/source-recovery"),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(recovery["state"], "clear", "{recovery}");
    let data = git_http(
        &service,
        &client,
        Method::POST,
        &db,
        Some(json!({"sql":"SELECT value FROM restore_check"})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(data["rows"][0]["value"], "keep this artificial record");
    let response = client
        .get(format!("{}/p/{}/", service.info().base_url, project.id))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn project_git_requires_manager_and_explicit_snapshot_and_head() {
    let (temp, _paths, service, client) = tests::start().await;
    let project = tests::create(&service, "Disposable Git route project").await;
    let other = tests::create(&service, "Disposable other Git project").await;
    let state = service.handle().current();
    let endpoint = format!(
        "{}/api/v1/projects/{}/git",
        service.info().base_url,
        project.id
    );
    let cookie = tests::connect(&service, &client, &project.id).await;
    for request in [
        client.get(&endpoint),
        client.get(&endpoint).header("cookie", cookie.clone()),
        client.get(format!("{endpoint}/versions?limit=50")),
        client
            .get(format!("{endpoint}/versions?limit=50"))
            .header("cookie", cookie.clone()),
        client.post(format!("{endpoint}/branches")).json(&json!({})),
        client
            .post(format!("{endpoint}/branches"))
            .header("cookie", cookie.clone())
            .json(&json!({})),
    ] {
        assert_eq!(
            request.send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for suffix in [
        "source-restore-preview",
        "source-restores",
        "source-recoveries",
    ] {
        for request in [
            client.post(format!("{endpoint}/{suffix}")).json(&json!({})),
            client
                .post(format!("{endpoint}/{suffix}"))
                .header("cookie", cookie.clone())
                .json(&json!({})),
        ] {
            assert_eq!(
                request.send().await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
        }
    }
    for request in [
        client.get(format!("{endpoint}/source-recovery")),
        client
            .get(format!("{endpoint}/source-recovery"))
            .header("cookie", cookie),
    ] {
        assert_eq!(
            request.send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let git = temp.path().join("disposable-git");
    std::fs::write(&git, "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'git version 2.56.0\\n'; exit 0; fi\ncase \" $* \" in *\" hash-object \"*) while IFS= read -r line; do printf \"1111111111111111111111111111111111111111\\n\"; done; exit 0;; esac\nexit 97\n").unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
    service
        .handle()
        .set_git_path(Some(git.to_string_lossy().into()))
        .await
        .unwrap();
    let response: Value = client
        .get(&endpoint)
        .bearer_auth(&state.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["data"]["repository"]["state"], "none");
    let snapshot = response["data"]["snapshotDigest"].as_str().unwrap();
    let recovery_query: Value = client
        .get(format!("{endpoint}/source-recovery?force=true"))
        .bearer_auth(&state.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(recovery_query["error"]["code"], "INVALID_INPUT");
    for query in [
        "limit=0",
        "limit=101",
        "limit=abc",
        "limit=1&revision=HEAD",
        "limit=1&limit=2",
    ] {
        let response: Value = client
            .get(format!("{endpoint}/versions?{query}"))
            .bearer_auth(&state.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["ok"], false, "{response}");
    }
    for (suffix, body) in [
        (
            "initialize",
            json!({"expectedSnapshot":"bad","initialBranch":"main"}),
        ),
        (
            "saves",
            json!({"expectedSnapshot":snapshot,"message":"explicit null required","author":{"name":"Disposable","email":"sample@example.invalid"}}),
        ),
        (
            "branches",
            json!({"name":"codex/disposable","expectedSnapshot":snapshot,"expectedHead":"a".repeat(40)}),
        ),
        (
            "source-restore-preview",
            json!({"targetCommit":"a".repeat(40),"force":true}),
        ),
        (
            "source-restores",
            json!({"targetCommit":"a".repeat(40),"expectedSnapshot":snapshot}),
        ),
        (
            "source-recoveries",
            json!({"restoreTaskId":"task_disposable","action":"continue","expectedPlanDigest":"a".repeat(64)}),
        ),
    ] {
        let response: Value = client
            .post(format!("{endpoint}/{suffix}"))
            .bearer_auth(&state.token)
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["ok"], false, "{response}");
    }
    assert!(!state.store.has_active_tasks());
    assert!(!project.project_root.join(".git").exists());
    let (task, lease) = state.builds.create_operation(&other.id, "gitSave").unwrap();
    let response: Value = client
        .post(format!("{endpoint}/tasks/{}/cancel", task.task_id))
        .bearer_auth(&state.token)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["error"]["code"], "PROJECT_GIT_INPUT_INVALID");
    state
        .builds
        .finish(&task.task_id, Ok(json!({"test":"no Git mutation"})))
        .unwrap();
    drop(lease);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn project_git_submissions_do_not_outlive_closed_runtime() {
    let (_temp, _paths, service, _client) = tests::start().await;
    let project = tests::create(&service, "Disposable shutdown Git project").await;
    let handle = service.handle();
    service.shutdown().await.unwrap();
    assert_eq!(
        handle
            .project_git_source_restore_status(&project.id)
            .await
            .unwrap_err()
            .code,
        "SERVICE_STOPPING"
    );
    assert_eq!(
        handle
            .project_git_source_restore_preview(
                &project.id,
                serde_json::from_value(json!({"targetCommit":"a".repeat(40)})).unwrap()
            )
            .await
            .unwrap_err()
            .code,
        "SERVICE_STOPPING"
    );
    assert_eq!(handle.project_git_source_restore_apply(&project.id,
        serde_json::from_value(json!({"targetCommit":"a".repeat(40),"expectedHead":"b".repeat(40),"expectedBranch":"refs/heads/main","expectedSnapshot":"c".repeat(64),"expectedPlanDigest":"d".repeat(64)})).unwrap()
    ).await.unwrap_err().code, "SERVICE_STOPPING");
    assert_eq!(handle.project_git_source_restore_recover(&project.id,
        serde_json::from_value(json!({"restoreTaskId":"task_disposable","expectedPlanDigest":"c".repeat(64),"expectedJournalDigest":"d".repeat(64),"action":"continue"})).unwrap()
    ).await.unwrap_err().code, "SERVICE_STOPPING");
    assert_eq!(
        handle
            .project_git_versions(&project.id, 50)
            .await
            .unwrap_err()
            .code,
        "SERVICE_STOPPING"
    );

    assert_eq!(
        handle
            .project_git_status(&project.id)
            .await
            .unwrap_err()
            .code,
        "SERVICE_STOPPING"
    );
}
