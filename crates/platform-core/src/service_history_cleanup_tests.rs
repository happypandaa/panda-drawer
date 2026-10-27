use super::*;

async fn final_task(service: &RuntimeService, id: &str) -> Value {
    for _ in 0..1000 {
        let value = service.handle().task(id).await.unwrap();
        if value["final"] == true {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("History cleanup HTTP fixture timed out");
}

#[tokio::test]
async fn history_cleanup_http_requires_admin_confirmation_and_preserves_other_projects_on_restart()
{
    let (_temp, paths, service, client) = tests::start().await;
    let keep = tests::create(&service, "Keep current project").await;
    let marker = keep.project_root.join("README.md");
    let before = std::fs::read(&marker).unwrap();
    // An unused, finished source copy is the disposable cleanup target.
    let source = format!("src_{}", uuid::Uuid::new_v4().simple());
    let slot = paths
        .workspace_root()
        .join(".platform/sources")
        .join(&source);
    std::fs::create_dir_all(slot.join("repository")).unwrap();
    std::fs::write(slot.join("repository/app.js"), "disposable source copy").unwrap();
    crate::builds::atomic_json(
        &slot.join("source.json"),
        &json!({"schemaVersion":1,"sourceId":source,"name":"Disposable source","status":"ready"}),
    )
    .unwrap();
    let base = service.info().base_url;
    let token = service.shared.snapshot().token.clone();
    let endpoint = format!("{base}/api/v1/workspace/cleanup-plans");
    let denied = client
        .post(&endpoint)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert!(!denied.status().is_success());
    let cookie = tests::connect(&service, &client, &keep.id).await;
    let denied = client
        .post(&endpoint)
        .header("Cookie", cookie)
        .header("Origin", &base)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert!(!denied.status().is_success());

    let preview = client
        .post(&endpoint)
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::ACCEPTED);
    let preview: Value = preview.json().await.unwrap();
    let ready = final_task(&service, preview["data"]["taskId"].as_str().unwrap()).await;
    assert_eq!(ready["status"], "succeeded", "{ready}");
    let plan = &ready["result"];
    let item = plan["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == source)
        .unwrap();
    assert_eq!(item["canClean"], true, "{item}");
    let payload = PathBuf::from(item["path"].as_str().unwrap());
    let input =
        json!({"planId":plan["planId"],"itemIds":[item["id"]],"confirmation":"永久清理所选内容"});
    let cleanup_url = format!("{base}/api/v1/workspace/cleanups");
    let mut arbitrary_path = input.clone();
    arbitrary_path["path"] = json!(keep.project_root);
    let invalid = client
        .post(&cleanup_url)
        .bearer_auth(&token)
        .json(&arbitrary_path)
        .send()
        .await
        .unwrap();
    assert!(!invalid.status().is_success());
    assert!(payload.is_dir());
    let mut missing_confirmation = input.clone();
    missing_confirmation["confirmation"] = "".into();
    assert!(!client
        .post(&cleanup_url)
        .bearer_auth(&token)
        .json(&missing_confirmation)
        .send()
        .await
        .unwrap()
        .status()
        .is_success());

    let accepted = client
        .post(&cleanup_url)
        .bearer_auth(&token)
        .json(&input)
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let accepted: Value = accepted.json().await.unwrap();
    let task_id = accepted["data"]["taskId"].as_str().unwrap().to_owned();
    let cleaned = final_task(&service, &task_id).await;
    assert_eq!(cleaned["status"], "succeeded", "{cleaned}");
    assert_eq!(cleaned["result"]["completed"], true);
    assert!(
        cleaned["result"]["items"][0]["removed"]["removedLogicalBytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(!payload.exists());
    assert_eq!(std::fs::read(&marker).unwrap(), before);
    assert!(keep.project_root.join("data/main.sqlite").is_file());
    let duplicate = client
        .post(&cleanup_url)
        .bearer_auth(&token)
        .json(&input)
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::ACCEPTED);
    let duplicate: Value = duplicate.json().await.unwrap();
    assert_eq!(duplicate["data"]["taskId"], task_id);
    let unconsumed = service.handle().workspace_cleanup_preview().await.unwrap();
    let unconsumed = final_task(&service, unconsumed["taskId"].as_str().unwrap()).await;
    service.shutdown().await.unwrap();

    let lock = paths.acquire_lock().unwrap();
    let restarted = RuntimeService::start(paths.clone(), lock).await.unwrap();
    let current = restarted.handle().workspace_info().await.unwrap();
    assert!(current["cleanupTask"].is_null());
    assert_eq!(
        restarted
            .handle()
            .workspace_cleanup(input)
            .await
            .unwrap_err()
            .code,
        "CLEANUP_PLAN_EXPIRED"
    );
    assert!(!paths.data_root().join("cleanup/plans").exists());
    assert!(!paths.data_root().join("cleanup/receipts").exists());
    let fresh = restarted
        .handle()
        .workspace_cleanup_preview()
        .await
        .unwrap();
    let fresh = final_task(&restarted, fresh["taskId"].as_str().unwrap()).await;
    assert_eq!(fresh["status"], "succeeded");
    assert_ne!(fresh["result"]["planId"], plan["planId"]);
    let stale = json!({"planId":unconsumed["result"]["planId"],"itemIds":["unlisted"],"confirmation":"永久清理所选内容"});
    assert_eq!(
        restarted
            .handle()
            .workspace_cleanup(stale)
            .await
            .unwrap_err()
            .code,
        "CLEANUP_PLAN_EXPIRED"
    );
    assert_eq!(std::fs::read(&marker).unwrap(), before);
    assert!(!payload.exists());
    restarted.shutdown().await.unwrap();
}
