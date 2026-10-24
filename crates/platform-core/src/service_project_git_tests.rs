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

/// Real HTTP management authorization, task dispatch and the selected real Git.
/// This is a disposable runtime service, not native desktop acceptance.
#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable development-git payload"]
async fn official_git_data_save_http_uses_real_sql_and_requires_management_auth() {
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
    let (_temp, paths, service, client) = tests::start().await;
    service
        .handle()
        .set_git_path(Some(tool_root.join("bin/git").to_string_lossy().into()))
        .await
        .unwrap();
    let project = tests::create(&service, "Disposable HTTP SQLite Git data save").await;
    let base = format!("projects/{}/git", project.id);
    let token = paths.read_control_token().unwrap();
    for (method, tail) in [
        (Method::POST, "data-saves"),
        (Method::GET, "data-save-recovery"),
        (Method::POST, "data-save-finalizations"),
    ] {
        let mut request = client.request(
            method,
            format!("{}/api/v1/{base}/{tail}", service.info().base_url),
        );
        if tail != "data-save-recovery" {
            request = request.json(&json!({}));
        }
        assert_eq!(
            request.send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(!project.project_root.join("version-data").exists());
    assert!(!project.project_root.join(".git").exists());
    let response = client
        .get(format!(
            "{}/api/v1/{base}/data-save-recovery?unexpected=true",
            service.info().base_url
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let invalid: Value = response.json().await.unwrap();
    assert_eq!(invalid["ok"], false);
    assert_eq!(invalid["error"]["code"], "INVALID_INPUT");
    let status = git_http(&service, &client, Method::GET, &base, None, StatusCode::OK).await;
    let task = git_http(
        &service,
        &client,
        Method::POST,
        &format!("{base}/initialize"),
        Some(json!({"initialBranch":"main","expectedSnapshot":status["snapshotDigest"]})),
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(
        git_http_task(&service, &client, &task).await["status"],
        "succeeded"
    );
    let db = format!("projects/{}/db/execute", project.id);
    for sql in [
        "CREATE TABLE notes(body TEXT)",
        "INSERT INTO notes VALUES('artificial HTTP record remains live')",
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
        project.project_root.join("data/attachments/完整.txt"),
        "HTTP人工附件\n完整第二行",
    )
    .unwrap();
    let status = git_http(&service, &client, Method::GET, &base, None, StatusCode::OK).await;
    assert_eq!(status["database"]["canSave"], true);
    let task=git_http(&service,&client,Method::POST,&format!("{base}/data-saves"),Some(json!({"expectedHead":status["repository"]["head"]["oid"],"expectedSnapshot":status["snapshotDigest"],"message":"Artificial HTTP source and SQLite data version","author":{"name":"HTTP data fixture","email":"http-data@example.invalid"}})),StatusCode::ACCEPTED).await;
    assert_eq!(task["type"], "gitDataSave");
    let saved = git_http_task(&service, &client, &task).await;
    assert_eq!(saved["status"], "succeeded", "{saved}");
    assert_eq!(saved["result"]["operation"], "save-data");
    assert_eq!(saved["result"]["database"]["state"], "included");
    let read = git_http(
        &service,
        &client,
        Method::POST,
        &db,
        Some(json!({"sql":"SELECT body FROM notes"})),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        read["rows"][0]["body"],
        "artificial HTTP record remains live"
    );
    let status = git_http(&service, &client, Method::GET, &base, None, StatusCode::OK).await;
    assert_eq!(status["database"]["canSave"], true);
    assert_eq!(
        status["database"]["currentSavedSnapshot"]["attachmentCount"],
        1
    );
    assert_eq!(
        git_http(
            &service,
            &client,
            Method::GET,
            &format!("{base}/data-save-recovery"),
            None,
            StatusCode::OK
        )
        .await["state"],
        "clear"
    );
    assert_eq!(
        service
            .handle()
            .project_git_data_save_status(&project.id)
            .await
            .unwrap()["state"],
        "clear"
    );
    let snapshot =
        rusqlite::Connection::open(project.project_root.join("version-data/database.sqlite"))
            .unwrap();
    assert_eq!(
        snapshot
            .query_row("SELECT body FROM notes", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "artificial HTTP record remains live"
    );
    drop(snapshot);
    assert_eq!(
        std::fs::read_to_string(
            project
                .project_root
                .join("version-data/attachments/完整.txt")
        )
        .unwrap(),
        "HTTP人工附件\n完整第二行"
    );
    service.shutdown().await.unwrap();
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
    assert_eq!(status["canInitialize"], true, "{status}");
    let task = git_http(
        &service,
        &client,
        Method::POST,
        &format!("{base}/initialize"),
        Some(json!({"initialBranch":"main","expectedSnapshot":status["snapshotDigest"]})),
        StatusCode::ACCEPTED,
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
        let task = git_http(&service, &client, Method::POST, &format!("{base}/saves"), Some(json!({"expectedHead":status["repository"]["head"]["oid"],"expectedSnapshot":status["snapshotDigest"],"message":message,"author":{"name":"Disposable HTTP","email":"disposable@example.invalid"}})), StatusCode::ACCEPTED).await;
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
            .project_git_create_branch(
                &project.id,
                crate::project_git::GitCreateBranchInput {
                    name: "codex/disposable".into(),
                    expected_head: "a".repeat(40),
                    expected_branch: "refs/heads/main".into(),
                    expected_snapshot: "a".repeat(64),
                }
            )
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
    assert_eq!(
        handle
            .project_git_initialize(
                &project.id,
                crate::project_git::GitInitInput {
                    expected_snapshot: "a".repeat(64),
                    initial_branch: "main".into(),
                }
            )
            .await
            .unwrap_err()
            .code,
        "SERVICE_STOPPING"
    );
}
