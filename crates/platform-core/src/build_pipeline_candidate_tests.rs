use super::*;

fn assert_candidate_removed(project: &ProjectHandle, task_id: &str) {
    let task_root = project.root.join(".platform/builds").join(task_id);
    // Successful publication now removes its entire owned temporary root.
    if !task_root.exists() {
        return;
    }
    let record: Value =
        serde_json::from_slice(&fs::read(task_root.join("candidate-v1.json")).unwrap()).unwrap();
    assert_eq!(record["state"], "removed");
    assert!(!task_root.join(record["path"].as_str().unwrap()).exists());
}

#[tokio::test]
async fn successful_build_and_failed_install_never_modify_existing_dependencies_or_source() {
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
    let project = project(&store, "Candidate dependency preservation").await;
    fixture(
        &project,
        &format!(
            r#"
const assert=require('assert');
assert(!require('fs').existsSync('.git'));
assert(process.cwd().includes(require('path').sep+'candidate-v1-'));
assert.equal(require('tiny'), 'candidate local package');
require('fs').writeFileSync('node_modules/new-build-file', 'only the candidate');
{BUILD}
"#
        ),
    );
    let app = project.root.join("app");
    fs::write(
        app.join("vendor/tiny/index.js"),
        "module.exports='candidate local package'",
    )
    .unwrap();
    fs::create_dir_all(app.join("node_modules/live-dependency")).unwrap();
    fs::write(
        app.join("node_modules/live-dependency/index.js"),
        "running dependency",
    )
    .unwrap();
    fs::write(
        app.join("node_modules/.package-lock.json"),
        "old installed lock",
    )
    .unwrap();
    fs::write(app.join(".hg"), "excluded version-control material").unwrap();
    let old_record = b"legacy dependency record must not even be parsed";
    fs::write(project.root.join(".platform/dependencies.json"), old_record).unwrap();
    let before_dependencies = tree_digest(&app.join("node_modules"), false).unwrap();
    let before_input = input_digest(&project).unwrap();
    let before_data = tree_digest(&project.root.join("data"), false).unwrap();
    let manager =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into()).unwrap();
    let task = manager.submit(&project.manifest.id).await.unwrap();
    let done = wait(&manager, &task.task_id).await;
    assert_eq!(done.status, TaskStatus::Succeeded, "{done:?}");
    assert_candidate_removed(&project, &task.task_id);
    assert_eq!(input_digest(&project).unwrap(), before_input);
    assert_eq!(
        tree_digest(&app.join("node_modules"), false).unwrap(),
        before_dependencies
    );
    let previous = fs::read(project.root.join("dist/index.html")).unwrap();
    let mut package: Value =
        serde_json::from_slice(&fs::read(app.join("package.json")).unwrap()).unwrap();
    package["scripts"]["postinstall"] = json!("node fail-install.cjs");
    atomic_json(&app.join("package.json"), &package).unwrap();
    fs::write(app.join("fail-install.cjs"), "require('fs').writeFileSync('node_modules/candidate-only','failed install');process.exit(42)").unwrap();
    let before_failed_input = input_digest(&project).unwrap();
    let failed = manager.submit(&project.manifest.id).await.unwrap();
    let done = wait(&manager, &failed.task_id).await;
    assert_eq!(done.status, TaskStatus::Failed, "{done:?}");
    assert_eq!(done.error.unwrap().code, "BUILD_FAILED");
    assert_candidate_removed(&project, &failed.task_id);
    assert_eq!(input_digest(&project).unwrap(), before_failed_input);
    assert_eq!(
        tree_digest(&app.join("node_modules"), false).unwrap(),
        before_dependencies
    );
    assert_eq!(
        tree_digest(&project.root.join("data"), false).unwrap(),
        before_data
    );
    assert_eq!(
        fs::read(project.root.join("dist/index.html")).unwrap(),
        previous
    );
    assert_eq!(
        fs::read(project.root.join(".platform/dependencies.json")).unwrap(),
        old_record
    );
    assert_eq!(
        fs::read(app.join(".hg")).unwrap(),
        b"excluded version-control material"
    );
    manager.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn real_npm_installs_internal_workspaces_and_nested_local_dependencies_in_candidate() {
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
    let project = project(&store, "Candidate workspace").await;
    fixture(
        &project,
        &format!("require('assert').equal(require('workspace-one'),'tiny value');\n{BUILD}"),
    );
    let app = project.root.join("app");
    fs::write(
        app.join("vendor/tiny/index.js"),
        "module.exports='tiny value'",
    )
    .unwrap();
    ensure_dir(&app.join("packages/one")).unwrap();
    atomic_json(&app.join("packages/one/package.json"), &json!({"name":"workspace-one","version":"1.0.0","dependencies":{"tiny":"file:../../vendor/tiny"}})).unwrap();
    fs::write(
        app.join("packages/one/index.js"),
        "module.exports=require('tiny')",
    )
    .unwrap();
    let mut package: Value =
        serde_json::from_slice(&fs::read(app.join("package.json")).unwrap()).unwrap();
    package["workspaces"] = json!(["packages/*"]);
    atomic_json(&app.join("package.json"), &package).unwrap();
    let mut lock: Value =
        serde_json::from_slice(&fs::read(app.join("package-lock.json")).unwrap()).unwrap();
    lock["packages"][""]["workspaces"] = package["workspaces"].clone();
    lock["packages"]["node_modules/workspace-one"] = json!({"resolved":"packages/one","link":true});
    lock["packages"]["packages/one"] = json!({"name":"workspace-one","version":"1.0.0","dependencies":{"tiny":"file:../../vendor/tiny"}});
    atomic_json(&app.join("package-lock.json"), &lock).unwrap();
    let before = input_digest(&project).unwrap();
    let manager =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into()).unwrap();
    let task = manager.submit(&project.manifest.id).await.unwrap();
    let done = wait(&manager, &task.task_id).await;
    assert_eq!(done.status, TaskStatus::Succeeded, "{done:?}");
    assert_candidate_removed(&project, &task.task_id);
    assert_eq!(input_digest(&project).unwrap(), before);
    assert!(!app.join("node_modules").exists());
    manager.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn editing_original_during_candidate_build_prevents_publication_and_preserves_editor_change()
{
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
    let project = project(&store, "Candidate input edit").await;
    let marker = paths.data_root().join("build-started");
    let script = format!(
        "require('fs').writeFileSync({},'started');setTimeout(()=>{{{BUILD}}},700);",
        serde_json::to_string(&marker).unwrap()
    );
    fixture(&project, &script);
    ensure_dir(&project.root.join("dist")).unwrap();
    fs::write(
        project.root.join("dist/index.html"),
        "previous working page",
    )
    .unwrap();
    let previous = fs::read(project.root.join("dist/index.html")).unwrap();
    let manager =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into()).unwrap();
    let task = manager.submit(&project.manifest.id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let edit = project.root.join("app/editor-change.txt");
    fs::write(&edit, "keep this editor change").unwrap();
    let done = wait(&manager, &task.task_id).await;
    assert_eq!(done.status, TaskStatus::Failed, "{done:?}");
    assert_eq!(done.error.unwrap().code, "BUILD_INPUT_CHANGED");
    assert_eq!(fs::read(edit).unwrap(), b"keep this editor change");
    assert_eq!(
        fs::read(project.root.join("dist/index.html")).unwrap(),
        previous
    );
    assert_candidate_removed(&project, &task.task_id);
    manager.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn restart_retains_interrupted_candidate_evidence_instead_of_resuming_or_deleting_it() {
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
    let project = project(&store, "Retained candidate evidence").await;
    let task_id = format!("task_{}", Uuid::new_v4().simple());
    let task_root = project.root.join(".platform/builds").join(&task_id);
    let candidate = task_root.join("candidate-v1-interrupted");
    ensure_dir(&candidate.join("app/node_modules")).unwrap();
    fs::write(
        candidate.join("app/node_modules/evidence"),
        "interrupted install",
    )
    .unwrap();
    atomic_json(&task_root.join("candidate-v1.json"), &json!({"schemaVersion":1,"kind":"disposable-build-candidate","path":"candidate-v1-interrupted","state":"retained"})).unwrap();
    atomic_json(
        &paths
            .workspace_root()
            .join(".platform/tasks")
            .join(&task_id)
            .join("task.json"),
        &TaskRecord {
            task_id: task_id.clone(),
            project_id: Some(project.manifest.id.clone()),
            task_type: "build".into(),
            status: TaskStatus::Running,
            phase: "dependencies".into(),
            result: None,
            error: None,
        },
    )
    .unwrap();
    let manager = BuildManager::new(paths, store.clone(), "http://127.0.0.1:1234".into()).unwrap();
    assert_eq!(
        manager.task(&task_id).unwrap().status,
        TaskStatus::Interrupted
    );
    assert_eq!(
        fs::read(candidate.join("app/node_modules/evidence")).unwrap(),
        b"interrupted install"
    );
    let marker: Value =
        serde_json::from_slice(&fs::read(task_root.join("candidate-v1.json")).unwrap()).unwrap();
    assert_eq!(marker["state"], "retained");
    manager.shutdown().await;
    store.shutdown().await;
}
