//! Network requests are mocked here; these tests exercise the real management
//! boundary, selected executable, operation lifetime, and shutdown drain.
use super::*;
use crate::git_remote::GitRemoteCheckInput;
use std::os::unix::fs::PermissionsExt;

fn mock_git(root: &std::path::Path, body: &str) -> String {
    let executable = root.join("disposable-git");
    std::fs::write(
        &executable,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'git version 2.56.0\\n'; exit 0; fi\n{body}\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    executable.to_string_lossy().into_owned()
}

async fn wait_for_process_record(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(3), async {
        // Opening a file precedes its write. Wait for both complete record lines
        // so parallel test load cannot expose a partially written marker.
        while !std::fs::read_to_string(path)
            .is_ok_and(|contents| contents.lines().count() == 2 && contents.ends_with('\n'))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("mock process did not start");
}

#[tokio::test]
async fn remote_check_auth_validation_and_read_only_result_are_separate_from_doctor() {
    let (temp, paths, service, client) = tests::start().await;
    let project = tests::create(&service, "Disposable public HTTPS access check").await;
    let cookie = tests::connect(&service, &client, &project.id).await;
    let endpoint = format!(
        "{}/api/v1/environment/git/remote-check",
        service.info().base_url
    );
    let token = paths.read_control_token().unwrap();
    let marker = temp.path().join("remote-was-invoked");
    let executable = mock_git(temp.path(), &format!(
        "printf '%s' \"$PWD\" > '{}'\nprintf 'ref: refs/heads/main\\tHEAD\\n0123456789abcdef0123456789abcdef01234567\\tHEAD\\n'",
        marker.display()
    ));
    service
        .handle()
        .set_git_path(Some(executable.clone()))
        .await
        .unwrap();
    let settings = paths.data_root().join("git-toolchain.json");
    let settings_before = std::fs::read(&settings).unwrap();
    let node_settings = paths.data_root().join("toolchain.json");
    let node_bytes = b"{\"schemaVersion\":1,\"nodePath\":null}\n";
    std::fs::write(&node_settings, node_bytes).unwrap();
    let source = project
        .project_root
        .join("app/disposable-preserved-source.txt");
    std::fs::write(&source, "preserve this source").unwrap();

    for response in [
        client
            .post(&endpoint)
            .json(&json!({"url":"https://github.com/git/git.git"}))
            .send()
            .await
            .unwrap(),
        client
            .post(&endpoint)
            .header("cookie", &cookie)
            .json(&json!({"url":"https://github.com/git/git.git"}))
            .send()
            .await
            .unwrap(),
    ] {
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        client
            .post(&endpoint)
            .bearer_auth(&token)
            .header("origin", &service.info().base_url)
            .json(&json!({"url":"https://github.com/git/git.git"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    for input in [
        json!({}),
        json!({"url":3}),
        json!({"url":"https://github.com/git/git.git","clone":true}),
        json!({"url":"https://sensitive-test-token@github.com/git/git.git"}),
        json!({"url":"https://github.com/git/git.git?token=sensitive-test-token"}),
        json!({"url":"git@github.com:git/git.git"}),
    ] {
        let response: Value = client
            .post(&endpoint)
            .bearer_auth(&token)
            .json(&input)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["ok"], false);
        assert!(!response.to_string().contains("sensitive-test-token"));
    }
    assert!(
        !marker.exists(),
        "unauthorized or invalid input executed Git"
    );
    let response: Value = client
        .post(&endpoint)
        .bearer_auth(&token)
        .json(&json!({"url":"https://github.com/git/git"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["data"]["url"], "https://github.com/git/git.git");
    assert_eq!(response["data"]["state"], "readable");
    assert_eq!(response["data"]["transport"], "public-https");
    assert_eq!(response["data"]["head"]["symbolicRef"], "refs/heads/main");
    assert_eq!(
        response["data"]["git"]["path"],
        std::fs::canonicalize(executable)
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    let private_cwd = std::fs::read_to_string(&marker).unwrap();
    assert!(
        !std::path::Path::new(&private_cwd).exists(),
        "private cwd remained after completion"
    );
    assert_eq!(
        service.handle().git_environment().await.unwrap()["authentication"]["state"],
        "not-checked"
    );
    assert_eq!(std::fs::read(&settings).unwrap(), settings_before);
    assert_eq!(std::fs::read(&node_settings).unwrap(), node_bytes);
    assert_eq!(
        std::fs::read_to_string(&source).unwrap(),
        "preserve this source"
    );
    assert!(!project.project_root.join(".git").exists());
    assert!(!project.project_root.join("app/.git").exists());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_remote_check_is_single_flight_and_shutdown_cancels_and_drains_it() {
    let (temp, _paths, service, _client) = tests::start().await;
    let started = temp.path().join("started");
    let executable = mock_git(
        temp.path(),
        &format!(
            "printf '%s\\n%s\\n' \"$$\" \"$PWD\" > '{}'\nwhile :; do /bin/sleep 1; done",
            started.display()
        ),
    );
    service
        .handle()
        .set_git_path(Some(executable))
        .await
        .unwrap();
    let handle = service.handle();
    let running_handle = handle.clone();
    let running = tokio::spawn(async move {
        running_handle
            .check_git_remote(GitRemoteCheckInput {
                url: "https://gitee.com/mirrors/git.git".into(),
            })
            .await
    });
    wait_for_process_record(&started).await;
    let error = handle
        .check_git_remote(GitRemoteCheckInput {
            url: "https://github.com/git/git.git".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "GIT_REMOTE_CHECK_BUSY");
    let evidence = std::fs::read_to_string(&started).unwrap();
    let mut lines = evidence.lines();
    let pid: i32 = lines.next().unwrap().parse().unwrap();
    let cwd = std::path::PathBuf::from(lines.next().unwrap());
    tokio::time::timeout(Duration::from_secs(3), service.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        running.await.unwrap().unwrap_err().code,
        "GIT_REMOTE_CHECK_CANCELLED"
    );
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "direct process survived shutdown"
    );
    assert!(!cwd.exists(), "private cwd remained after cancellation");
    let error = handle
        .check_git_remote(GitRemoteCheckInput {
            url: "https://github.com/git/git.git".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "SERVICE_STOPPING");
}
