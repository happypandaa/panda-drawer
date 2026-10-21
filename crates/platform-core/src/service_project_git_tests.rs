//! Management boundary tests. These do not exercise a native user flow.
use super::*;
use std::os::unix::fs::PermissionsExt;

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
        client.get(&endpoint).header("cookie", cookie),
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
    for (suffix, body) in [
        (
            "initialize",
            json!({"expectedSnapshot":"bad","initialBranch":"main"}),
        ),
        (
            "saves",
            json!({"expectedSnapshot":snapshot,"message":"explicit null required","author":{"name":"Disposable","email":"sample@example.invalid"}}),
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
