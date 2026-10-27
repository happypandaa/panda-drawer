//! Public native-manager service calls with disposable source/data. The live
//! variant uses verified private Node/npm/Git; no user project or network API.
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use super::*;
use crate::{
    builds::atomic_json,
    node_app_configuration::SaveConfigurationRequest,
    node_apps::NodeApps,
    node_install_tasks::PrepareNodeInstallInput,
    project_git::{GitAuthor, GitInitInput, GitSaveInput, ProjectGitStatus},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

async fn finish(handle: &RuntimeHandle, task: &Value) -> Value {
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let next = handle.task(task["taskId"].as_str().unwrap()).await.unwrap();
            if next["final"] == true {
                assert_eq!(next["status"], "succeeded", "{next}");
                return next["result"].clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

fn source(root: &Path) -> PathBuf {
    let source = root.join("disposable-service-node-single-git");
    fs::create_dir_all(source.join("public")).unwrap();
    fs::create_dir(source.join("server")).unwrap();
    fs::write(
        source.join("public/index.html"),
        "disposable installed version one",
    )
    .unwrap();
    fs::write(source.join("server/main.mjs"), r#"import http from 'node:http';
import readline from 'node:readline';
import fs from 'node:fs';
const lines=readline.createInterface({input:process.stdin});
let server,token,instance;
for await(const line of lines){
 const msg=JSON.parse(line);
 if(msg.type==='initialize'){
  ({token,instanceId:instance}=msg);
  fs.writeFileSync(process.env.APP_DATA_DIR+'/disposable-launch-evidence','started');
  server=http.createServer((req,res)=>{
   if(req.headers.authorization!==`Bearer ${token}`){res.writeHead(401,{'content-length':0});return res.end();}
   const body=JSON.stringify({protocolVersion:1,instanceId:instance,status:'ok'});
   res.writeHead(200,{'content-type':'application/json','content-length':Buffer.byteLength(body)});res.end(body);
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  console.log(JSON.stringify({protocolVersion:1,type:'ready',instanceId:instance,host:'127.0.0.1',port:server.address().port}));
 }else if(msg.type==='ping'){
  console.log(JSON.stringify({protocolVersion:1,type:'pong',instanceId:instance,requestId:msg.requestId}));
 }else if(msg.type==='shutdown'){
  console.log(JSON.stringify({protocolVersion:1,type:'stopping',instanceId:instance}));
  await new Promise(resolve=>server.close(resolve));lines.close();break;
 }
}
"#).unwrap();
    atomic_json(&source.join("local-app.json"), &json!({
        "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
                "dataContract":{"id":"disposable.node-development-data","version":1},
        "app":{"id":"disposable.service.node-development","name":"可丢弃开发集成","version":"1.0.0"},
        "web":{"root":"public","entry":"index.html","routing":"files"},
        "backend":{"root":"server","entry":"main.mjs","nodeMajor":24,"protocolVersion":1},
        "development":{"sourceDir":".","outputDir":".","build":{"kind":"none"}},
        "configuration":{"version":1,"fields":[{"name":"LABEL","label":"标签","type":"string","required":false,"secret":false,"description":"可丢弃测试值","howToGet":["无需填写"]}]}
    })).unwrap();
    atomic_json(
        &source.join("package.json"),
        &json!({"name":"disposable-node-development","version":"1.0.0"}),
    )
    .unwrap();
    atomic_json(&source.join("package-lock.json"), &json!({"name":"disposable-node-development","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"disposable-node-development","version":"1.0.0"}}})).unwrap();
    source
}

async fn build_install(handle: &RuntimeHandle, apps: Arc<NodeApps>, id: &str) -> Value {
    let task = handle
        .node_development_build(apps.clone(), id.into())
        .await
        .unwrap();
    let built = finish(handle, &task).await;
    assert_eq!(built["installed"], false);
    let ready = &built["source"];
    let task = handle
        .node_install_prepare(
            apps.clone(),
            PrepareNodeInstallInput {
                build_source: false,
                source_id: ready["sourceId"].as_str().unwrap().into(),
                expected_source_digest: ready["contentDigest"].as_str().unwrap().into(),
                target_app_id: Some(id.into()),
            },
        )
        .await
        .unwrap();
    let prepared = finish(handle, &task).await;
    let candidate_id = prepared["candidate"]["candidateId"].as_str().unwrap();
    let applied = handle.node_install_apply(apps, candidate_id).await.unwrap();
    finish(handle, &applied).await
}

async fn save(handle: &RuntimeHandle, apps: &NodeApps, id: &str, message: &str) -> Value {
    let status: ProjectGitStatus = serde_json::from_value(
        handle
            .node_development_git(apps, id, NodeGitAction::Status)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(status.can_save, "{status:?}");
    handle
        .node_development_git(
            apps,
            id,
            NodeGitAction::Save(GitSaveInput {
                expected_head: status.repository.head.and_then(|head| head.oid),
                expected_snapshot: status.snapshot_digest,
                message: message.into(),
                author: GitAuthor {
                    name: "Disposable integration".into(),
                    email: "disposable@example.invalid".into(),
                },
            }),
        )
        .await
        .unwrap()
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[tokio::test]
#[ignore = "uses the explicit packaged private Node/npm/Git payloads; creates only disposable source/data"]
async fn pinned_tools_single_repository_build_update_and_runtime_roundtrip() {
    let resources = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src-tauri/resources")
        .canonicalize()
        .unwrap();
    let runtime_node = std::env::var_os("NODE_RUNTIME_TRIAL_TEST_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| resources.join("node-runtime/bin/node"));
    let development_node = std::env::var_os("NODE_DEVELOPMENT_TEST_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| resources.join("development-node/bin/node"));
    let git = std::env::var_os("LOCAL_PROJECTS_TEST_GIT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| resources.join("development-git"))
        .join("bin/git");
    let (temporary, paths, service, _client) = tests::start().await;
    let handle = service.handle();
    handle
        .set_node_path(Some(development_node.to_string_lossy().into()))
        .await
        .unwrap();
    handle
        .set_git_path(Some(git.to_string_lossy().into()))
        .await
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let source = source(&root);
    let apps = Arc::new(NodeApps::new(paths.data_root().into(), runtime_node).unwrap());
    let registered = apps.register(source.clone()).await.unwrap();
    let id = registered["id"].as_str().unwrap();
    let configuration: SaveConfigurationRequest = serde_json::from_value(
        json!({"revision":0,"values":{"LABEL":"retained disposable label"},"secrets":{}}),
    )
    .unwrap();
    apps.save(id, configuration).await.unwrap();
    let slot = paths.data_root().join("node-apps").join(id);
    fs::write(
        slot.join("data/disposable-business.txt"),
        b"retained business file",
    )
    .unwrap();
    let status: ProjectGitStatus = serde_json::from_value(
        handle
            .node_development_git(&apps, id, NodeGitAction::Status)
            .await
            .unwrap(),
    )
    .unwrap();
    handle
        .node_development_git(
            &apps,
            id,
            NodeGitAction::Initialize(GitInitInput {
                expected_snapshot: status.snapshot_digest,
                initial_branch: "main".into(),
            }),
        )
        .await
        .unwrap();
    let initial = save(&handle, &apps, id, "Save disposable initial program").await;
    let initial_head = initial["status"]["repository"]["head"]["oid"]
        .as_str()
        .unwrap();
    let external_handoff = handle.node_development_handoff(&apps, id).await.unwrap();
    assert_eq!(
        external_handoff["projectRoot"],
        source.to_string_lossy().as_ref()
    );
    assert!(external_handoff["text"]
        .as_str()
        .unwrap()
        .contains("直接运行此文件夹"));
    let installed = build_install(&handle, apps.clone(), id).await;
    assert_eq!(installed["appId"], id);
    let detail = apps.configuration(id).await.unwrap();
    assert_eq!(
        detail["configuration"]["values"]["LABEL"],
        "retained disposable label"
    );
    let registration: Value =
        serde_json::from_slice(&fs::read(slot.join("registration.json")).unwrap()).unwrap();
    assert_eq!(registration["provenance"]["sourceCommit"], initial_head);
    assert_eq!(
        registration["provenance"]["buildSourceDigest"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    let program = PathBuf::from(detail["sourcePath"].as_str().unwrap());
    assert_ne!(program, source);
    assert!(!program.join(".git").exists());
    assert!(source.join(".git").is_dir());
    let binding = handle.node_development_info(&apps, id).await.unwrap();
    assert_eq!(binding["sourceRoot"], json!(source));
    assert_eq!(binding["runtimeIsSource"], false);
    handle.node_development_handoff(&apps, id).await.unwrap();
    assert_eq!(apps.start(id).await.unwrap()["state"], "running");
    assert_eq!(apps.stop(id).await.unwrap()["state"], "stopped");
    fs::write(
        source.join("public/index.html"),
        "disposable source version two",
    )
    .unwrap();
    let saved = save(&handle, &apps, id, "Save disposable program update").await;
    assert_ne!(saved["status"]["repository"]["head"]["oid"], initial_head);
    assert_eq!(
        fs::read(program.join("public/index.html")).unwrap(),
        b"disposable installed version one"
    );
    fs::write(
        source.join("public/index.html"),
        "disposable uncommitted source update",
    )
    .unwrap();
    let updated = build_install(&handle, apps.clone(), id).await;
    assert_eq!(updated["appId"], id);
    let next = apps.configuration(id).await.unwrap();
    assert_ne!(
        next["installation"]["generation"],
        detail["installation"]["generation"]
    );
    assert_eq!(
        fs::read(PathBuf::from(next["sourcePath"].as_str().unwrap()).join("public/index.html"))
            .unwrap(),
        b"disposable uncommitted source update"
    );
    let next_registration: Value =
        serde_json::from_slice(&fs::read(slot.join("registration.json")).unwrap()).unwrap();
    assert_eq!(
        next_registration["provenance"]["sourceCommit"],
        saved["status"]["repository"]["head"]["oid"]
    );
    assert_ne!(
        next_registration["provenance"]["buildSourceDigest"],
        registration["provenance"]["buildSourceDigest"]
    );
    assert_eq!(
        fs::read(slot.join("data/disposable-business.txt")).unwrap(),
        b"retained business file"
    );
    assert_eq!(apps.start(id).await.unwrap()["state"], "running");
    assert_eq!(apps.restart(id).await.unwrap()["state"], "running");
    assert_eq!(apps.stop(id).await.unwrap()["state"], "stopped");
    assert!(!slot.join("active.json").exists());
    let versions = handle
        .node_development_git(&apps, id, NodeGitAction::Versions(10))
        .await
        .unwrap();
    assert_eq!(versions["versions"].as_array().unwrap().len(), 2);
    Box::pin(verify_source_restore_roundtrip(
        &handle,
        &apps,
        id,
        &source,
        &slot,
        initial_head,
        &next,
    ))
    .await;
    apps.stop_all().await.unwrap();
    service.shutdown().await.unwrap();
}

async fn verify_source_restore_roundtrip(
    handle: &RuntimeHandle,
    apps: &NodeApps,
    id: &str,
    source: &Path,
    slot: &Path,
    initial_head: &str,
    next: &Value,
) {
    let before_restore = Box::pin(save(
        handle,
        apps,
        id,
        "Save disposable source before explicit restore",
    ))
    .await;
    assert_eq!(apps.start(id).await.unwrap()["state"], "running");
    assert_eq!(
        handle
            .node_development_git(
                apps,
                id,
                NodeGitAction::RestorePreview(
                    crate::node_development_git::NodeRestorePreviewInput {
                        target_commit: initial_head.into()
                    }
                )
            )
            .await
            .unwrap_err()
            .code,
        "NODE_SOURCE_RESTORE_RUNTIME_ACTIVE"
    );
    assert_eq!(apps.stop(id).await.unwrap()["state"], "stopped");
    let retained_registration = fs::read(slot.join("registration.json")).unwrap();
    let retained_configuration = fs::read(slot.join("configuration/configuration.json")).unwrap();
    let retained_index = fs::read(source.join(".git/index")).unwrap();
    let retained_head = fs::read(source.join(".git/HEAD")).unwrap();
    let retained_branch = fs::read(source.join(".git/refs/heads/main")).unwrap();
    let running_program = PathBuf::from(next["sourcePath"].as_str().unwrap());
    let preview = handle
        .node_development_git(
            apps,
            id,
            NodeGitAction::RestorePreview(crate::node_development_git::NodeRestorePreviewInput {
                target_commit: initial_head.into(),
            }),
        )
        .await
        .unwrap();
    let applied = handle
        .node_development_git(
            apps,
            id,
            NodeGitAction::RestoreApply(crate::node_development_git::NodeRestoreApplyInput {
                target_commit: initial_head.into(),
                expected_snapshot: preview["snapshotDigest"].as_str().unwrap().into(),
                expected_plan_digest: preview["planDigest"].as_str().unwrap().into(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(applied["state"], "completed");
    assert_eq!(
        fs::read(source.join("public/index.html")).unwrap(),
        b"disposable installed version one"
    );
    assert_eq!(
        fs::read(running_program.join("public/index.html")).unwrap(),
        b"disposable uncommitted source update"
    );
    assert_eq!(
        fs::read(slot.join("registration.json")).unwrap(),
        retained_registration
    );
    assert_eq!(
        fs::read(slot.join("configuration/configuration.json")).unwrap(),
        retained_configuration
    );
    assert_eq!(
        fs::read(slot.join("data/disposable-business.txt")).unwrap(),
        b"retained business file"
    );
    assert_eq!(fs::read(source.join(".git/index")).unwrap(), retained_index);
    assert_eq!(fs::read(source.join(".git/HEAD")).unwrap(), retained_head);
    assert_eq!(
        fs::read(source.join(".git/refs/heads/main")).unwrap(),
        retained_branch
    );
    let after_restore = handle
        .node_development_git(apps, id, NodeGitAction::Status)
        .await
        .unwrap();
    assert_eq!(
        after_restore["repository"]["head"],
        before_restore["status"]["repository"]["head"]
    );
    assert_eq!(after_restore["changes"].as_array().unwrap().len(), 1);
    assert_eq!(after_restore["canSave"], true);
    let final_detail = apps.configuration(id).await.unwrap();
    assert_eq!(
        final_detail["installation"]["generation"],
        next["installation"]["generation"]
    );
}

async fn final_record(handle: &RuntimeHandle, task: &Value) -> Value {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let next = handle.task(task["taskId"].as_str().unwrap()).await.unwrap();
            if next["final"] == true {
                return next;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

async fn artificial_toolchain(handle: &RuntimeHandle, root: &Path, build_body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let bin = root.join("disposable-service-development-tools/bin");
    fs::create_dir_all(&bin).unwrap();
    let node = bin.join("node");
    fs::write(&node,"#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'v24.22.0\\n'; exit 0; fi\nexec /bin/sh \"$@\"\n").unwrap();
    fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(bin.join("npm"),format!("set -eu\nif [ \"$1\" = --version ]; then printf '11.9.0\\n'; exit 0; fi\nif [ \"$1\" = ci ]; then mkdir -p node_modules; exit 0; fi\n{build_body}\n")).unwrap();
    handle
        .set_node_path(Some(node.to_string_lossy().into()))
        .await
        .unwrap();
}

fn configure_build(source: &Path) {
    let path = source.join("local-app.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["development"] =
        json!({"sourceDir":".","build":{"kind":"npm","script":"build"},"outputDir":"release"});
    atomic_json(&path, &manifest).unwrap();
    let path = source.join("package.json");
    let mut package: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    package["scripts"] = json!({"build":"node build.mjs"});
    atomic_json(&path, &package).unwrap();
}

async fn wait_build_marker(paths: &RuntimePaths, task: &Value) -> PathBuf {
    let root = paths
        .data_root()
        .join("node-development-builds")
        .join(task["taskId"].as_str().unwrap())
        .join("repository");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if root.join("node_modules/disposable-pid").is_file() {
                return root;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

/// Exercises the real service cancellation path with an artificial npm child
/// group. It does not prove Node application or browser compatibility.
#[tokio::test]
async fn cancelling_service_build_drains_group_and_releases_instance_without_publication() {
    let (temporary, paths, service, _client) = tests::start().await;
    let handle = service.handle();
    let root = temporary.path().canonicalize().unwrap();
    artificial_toolchain(
        &handle,
        &root,
        "printf '%s' \"$$\" > node_modules/disposable-pid\n/bin/sleep 60 & wait",
    )
    .await;
    let source = source(&root);
    configure_build(&source);
    let apps = Arc::new(
        NodeApps::new(
            paths.data_root().into(),
            root.join("never-start-artificial-runtime"),
        )
        .unwrap(),
    );
    let registered = apps.register(source.clone()).await.unwrap();
    let id = registered["id"].as_str().unwrap();
    let slot = paths.data_root().join("node-apps").join(id);
    apps.save(id,serde_json::from_value(json!({"revision":0,"values":{"LABEL":"retained development failure config"},"secrets":{}})).unwrap()).await.unwrap();
    let configuration = fs::read(slot.join("configuration/configuration.json")).unwrap();
    let registration = fs::read(slot.join("registration.json")).unwrap();
    fs::write(
        slot.join("data/disposable-retained"),
        b"retained business state",
    )
    .unwrap();
    let task = handle
        .node_development_build(apps.clone(), id.into())
        .await
        .unwrap();
    let operation = wait_build_marker(&paths, &task).await;
    let pid: i32 = fs::read_to_string(operation.join("node_modules/disposable-pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        apps.start(id).await.unwrap_err().code,
        "NODE_DEVELOPMENT_BUSY"
    );
    assert_eq!(
        apps.install_target_snapshot(Some(id))
            .await
            .unwrap_err()
            .code,
        "NODE_DEVELOPMENT_BUSY"
    );
    assert_eq!(
        apps.bind_development_source(id, source.clone())
            .await
            .unwrap_err()
            .code,
        "NODE_DEVELOPMENT_BUSY"
    );
    assert_eq!(
        handle.node_install_require_idle().unwrap_err().code,
        "NODE_DEVELOPMENT_BUSY"
    );
    assert_eq!(
        apps.delete_preview(id).await.unwrap_err().code,
        "NODE_DEVELOPMENT_BUSY"
    );
    assert_eq!(apps.list().await.unwrap().len(), 1);
    apps.stop_runtime(id).await.unwrap();
    handle
        .node_development_cancel(task["taskId"].as_str().unwrap())
        .unwrap();
    let completed = final_record(&handle, &task).await;
    assert_eq!(
        completed["error"]["code"], "NODE_DEVELOPMENT_CANCELLED",
        "{completed}"
    );
    assert_ne!(unsafe { libc::kill(-pid, 0) }, 0);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    assert_eq!(
        fs::read(slot.join("registration.json")).unwrap(),
        registration
    );
    assert_eq!(
        fs::read(slot.join("configuration/configuration.json")).unwrap(),
        configuration
    );
    assert_eq!(
        fs::read(slot.join("data/disposable-retained")).unwrap(),
        b"retained business state"
    );
    assert!(!slot.join("generations").exists());
    assert!(!source.join("node_modules").exists());
    apps.development_build_source_lease(id)
        .await
        .unwrap()
        .validate()
        .unwrap();
    apps.stop_all().await.unwrap();
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn source_changed_during_service_build_rejects_stale_output_without_publication() {
    let (temporary, paths, service, _client) = tests::start().await;
    let handle = service.handle();
    let root = temporary.path().canonicalize().unwrap();
    artificial_toolchain(&handle,&root,"printf '%s' \"$$\" > node_modules/disposable-pid\nwhile [ ! -f node_modules/disposable-continue ]; do /bin/sleep 0.05; done\nmkdir release\ncp -R public server package.json package-lock.json local-app.json release/").await;
    let source = source(&root);
    configure_build(&source);
    let apps = Arc::new(
        NodeApps::new(
            paths.data_root().into(),
            root.join("never-start-artificial-runtime"),
        )
        .unwrap(),
    );
    let registered = apps.register(source.clone()).await.unwrap();
    let id = registered["id"].as_str().unwrap();
    let slot = paths.data_root().join("node-apps").join(id);
    apps.save(id,serde_json::from_value(json!({"revision":0,"values":{"LABEL":"retained development failure config"},"secrets":{}})).unwrap()).await.unwrap();
    let configuration = fs::read(slot.join("configuration/configuration.json")).unwrap();
    let registration = fs::read(slot.join("registration.json")).unwrap();
    fs::write(
        slot.join("data/disposable-retained"),
        b"retained during stale build",
    )
    .unwrap();
    let task = handle
        .node_development_build(apps.clone(), id.into())
        .await
        .unwrap();
    let operation = wait_build_marker(&paths, &task).await;
    fs::write(
        source.join("public/index.html"),
        "disposable author edit during build",
    )
    .unwrap();
    fs::write(
        operation.join("node_modules/disposable-continue"),
        b"continue disposable subprocess",
    )
    .unwrap();
    let completed = final_record(&handle, &task).await;
    assert_eq!(
        completed["error"]["code"], "NODE_DEVELOPMENT_CHANGED",
        "{completed}"
    );
    assert_eq!(
        fs::read(slot.join("registration.json")).unwrap(),
        registration
    );
    assert_eq!(
        fs::read(slot.join("configuration/configuration.json")).unwrap(),
        configuration
    );
    assert_eq!(
        fs::read(slot.join("data/disposable-retained")).unwrap(),
        b"retained during stale build"
    );
    assert!(!slot.join("generations").exists());
    assert_eq!(
        fs::read(source.join("public/index.html")).unwrap(),
        b"disposable author edit during build"
    );
    assert!(
        operation.parent().unwrap().exists(),
        "failed build evidence stays retained"
    );
    apps.development_build_source_lease(id)
        .await
        .unwrap()
        .validate()
        .unwrap();
    apps.stop_all().await.unwrap();
    service.shutdown().await.unwrap();
}
