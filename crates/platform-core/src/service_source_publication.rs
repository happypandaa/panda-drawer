use super::*;
use crate::projects::ProjectTaskLease;

struct PublicationScope {
    _active: Active,
    _publication: ProjectTaskLease,
    _source: ProjectTaskLease,
    _installation: ProjectTaskLease,
    _development: ProjectTaskLease,
}
struct BuildInfoScope {
    _active: Active,
    _source: ProjectTaskLease,
}
fn publication_invalid(message: &str) -> ApiError {
    ApiError::new("SOURCE_PUBLICATION_INVALID", message)
}
fn acquired_identity(
    task: &TaskRecord,
    expected_task: &str,
) -> std::result::Result<(String, String), ApiError> {
    if task.task_id != expected_task
        || task.task_type != "sourceAcquire"
        || task.status != TaskStatus::Succeeded
    {
        return Err(publication_invalid("本次下载尚未完成，未读取或发布源码。"));
    }
    let result = task
        .result
        .as_ref()
        .ok_or_else(|| publication_invalid("下载结果缺失。"))?;
    let id = result["sourceId"]
        .as_str()
        .filter(|id| {
            id.starts_with("src_")
                && id.len() == 36
                && id[4..].bytes().all(|b| b.is_ascii_hexdigit())
        })
        .ok_or_else(|| publication_invalid("无法核对下载身份。"))?;
    let digest = result["contentDigest"]
        .as_str()
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| publication_invalid("下载摘要缺失。"))?;
    Ok((id.into(), digest.into()))
}
fn installed_generation(
    detail: &Value,
    app_id: &str,
    source_id: &str,
    digest: &str,
) -> std::result::Result<String, ApiError> {
    if detail["id"] != app_id
        || detail["installation"]["sourceId"] != source_id
        || detail["installation"]["sourceDigest"] != digest
        || detail["installation"]["kind"] != "managed"
    {
        return Err(publication_invalid("本次源码与已添加应用不匹配。"));
    }
    detail["installation"]["generation"]
        .as_str()
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| publication_invalid("已添加的程序版本无法核对。"))
}

impl RuntimeHandle {
    pub async fn source_import_publish(
        &self,
        apps: Arc<crate::node_apps::NodeApps>,
        source_task_id: String,
        app_id: String,
    ) -> std::result::Result<Value, ApiError> {
        if Uuid::parse_str(&app_id)
            .map(|id| id.to_string() != app_id)
            .unwrap_or(true)
        {
            return Err(publication_invalid("应用标识无效。"));
        }
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let active = Active(state.clone());
        self.node_install_require_idle()?;
        let task = state.builds.task(&source_task_id)?;
        let (source_id, digest) = acquired_identity(&task, &source_task_id)?;
        let lease_id = format!("source-publication:{app_id}");
        let scope = PublicationScope {
            _active: active,
            _publication: state
                .store
                .reserve_transient_operation(&lease_id, "source-publication")?,
            _source: state
                .store
                .reserve_task(&format!("source:{source_id}"), &lease_id)?,
            _installation: state.store.reserve_transient_operation(
                &format!("node-install:{app_id}"),
                "source-publication",
            )?,
            _development: state.store.reserve_transient_operation(
                &format!("node-development:{app_id}"),
                "source-publication",
            )?,
        };
        // Verify the installed target before creating any author directory. The
        // final binding repeats this comparison atomically under NodeApps' lock.
        let generation = installed_generation(
            &apps.configuration(&app_id).await?,
            &app_id,
            &source_id,
            &digest,
        )?;
        let manager = state
            .source_acquisition
            .as_ref()
            .map_err(Clone::clone)?
            .clone();
        let copy_app = app_id.clone();
        let copy_source = source_id.clone();
        let copy_digest = digest.clone();
        let (path, _scope) =
            tokio::task::spawn_blocking(move || -> std::result::Result<_, ApiError> {
                // Actual leases and Active live in the blocking owner, even if a
                // core API caller drops its future while the copy is still writing.
                let scope = scope;
                let source = manager.verify_ready_leased(&copy_source, &copy_digest, &lease_id)?;
                if source.task_id != source_task_id {
                    return Err(publication_invalid("源码任务身份不符。"));
                }
                #[cfg(test)]
                pause_publication_test(&copy_app);
                let path = manager.publish_author_source(&source, &copy_app, &lease_id)?;
                Ok((path, scope))
            })
            .await
            .map_err(|_| {
                ApiError::new(
                    "SOURCE_PUBLICATION_INTERRUPTED",
                    "源码准备未完成，请保留现场并核对后重试。",
                )
                .outcome("unknown")
            })??;
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "源码已准备，平台正在退出；重新打开后核对同一次添加。",
            ));
        }
        apps.set_source_workspace(state.info.workspace_root.clone())?;
        apps.bind_imported_development_source(
            &app_id,
            path.clone(),
            &source_id,
            &digest,
            &generation,
        )
        .await?;
        Ok(
            json!({"published":true,"sourcePath":path,"projectDirectory":state.info.workspace_root.join("project-sources")}),
        )
    }

    pub async fn source_import_build_info(
        &self,
        source_task_id: &str,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "平台正在退出。"));
        }
        let task = state.builds.task(source_task_id)?;
        let (source_id, digest) = acquired_identity(&task, source_task_id)?;
        let lease_id = format!("source-build-info:{source_task_id}");
        let scope = BuildInfoScope {
            _active: active,
            _source: state
                .store
                .reserve_task(&format!("source:{source_id}"), &lease_id)?,
        };
        let manager = state
            .source_acquisition
            .as_ref()
            .map_err(Clone::clone)?
            .clone();
        let expected_task = source_task_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let _scope = scope;
            let source = manager.verify_ready_leased(&source_id, &digest, &lease_id)?;
            if source.task_id != expected_task
                || source.status != "ready"
                || source
                    .inspection
                    .as_ref()
                    .is_none_or(|i| i.kind != "node-service-source" || !i.requires_build)
            {
                return Err(ApiError::new(
                    "SOURCE_BUILD_INVALID",
                    "此下载没有可核对的声明构建。",
                ));
            }
            let root = source
                .source_path
                .ok_or_else(|| ApiError::new("SOURCE_BUILD_INVALID", "源码位置不可用。"))?;
            let directory = crate::files::root(&root)?;
            let read = |path: &str, max: u64| -> std::result::Result<Vec<u8>, ApiError> {
                let file = crate::files::read(&directory, path)?;
                let mut bytes = Vec::new();
                file.take(max + 1).read_to_end(&mut bytes)?;
                if bytes.len() as u64 > max {
                    return Err(ApiError::new(
                        "SOURCE_BUILD_INVALID",
                        "构建说明超过读取预算。",
                    ));
                }
                Ok(bytes)
            };
            let manifest =
                crate::node_app_manifest::TrialManifest::parse(&read("local-app.json", 64 * 1024)?)
                    .map_err(|_| {
                        ApiError::new("SOURCE_BUILD_INVALID", "源码中的应用声明无法核对。")
                    })?;
            let Some(development) = manifest.development else {
                return Err(ApiError::new("SOURCE_BUILD_INVALID", "没有声明构建脚本。"));
            };
            let crate::node_app_manifest::NodeDevelopmentBuild::Npm { script } = development.build
            else {
                return Err(ApiError::new("SOURCE_BUILD_INVALID", "此项目不需要构建。"));
            };
            let package: Value = serde_json::from_slice(&read("package.json", 256 * 1024)?)?;
            let content = package["scripts"][&script]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| ApiError::new("SOURCE_BUILD_INVALID", "找不到声明的构建脚本内容。"))?
                .to_owned();
            manager.verify_ready_leased(&source_id, &digest, &lease_id)?;
            Ok(json!({"scriptName":script,"script":content,"sourceDigest":digest}))
        })
        .await
        .map_err(|_| ApiError::new("SOURCE_BUILD_INVALID", "构建说明读取未完成。"))?
    }
}

#[cfg(test)]
type PublicationPause = (String, oneshot::Sender<()>, std::sync::mpsc::Receiver<()>);
#[cfg(test)]
static PUBLICATION_PAUSE: Mutex<Option<PublicationPause>> = Mutex::new(None);
#[cfg(test)]
fn pause_publication_test(app_id: &str) {
    let pause = {
        let mut slot = PUBLICATION_PAUSE.lock().unwrap();
        if slot.as_ref().is_some_and(|value| value.0 == app_id) {
            slot.take()
        } else {
            None
        }
    };
    if let Some((_, entered, release)) = pause {
        let _ = entered.send(());
        let _ = release.recv();
    }
}

#[cfg(test)]
mod tests {
    // Real service/installation/receipt lifetime, with synthetic source records
    // and an offline fake npm. No network Git, application launch or user data.
    use super::*;
    use crate::{builds::atomic_json, source_acquisition::CaptureLocalSourceInput};
    use std::{fs, os::unix::fs::PermissionsExt};

    async fn final_task(handle: &RuntimeHandle, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let task = handle.task(id).await.unwrap();
                if task["final"] == true {
                    return task;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn idle(handle: &RuntimeHandle) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while handle.current().store.has_active_tasks() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    fn source_project(root: &std::path::Path, build: bool) {
        fs::create_dir_all(root.join("web")).unwrap();
        fs::create_dir_all(root.join("server")).unwrap();
        fs::write(
            root.join("web/index.html"),
            "disposable source publication page",
        )
        .unwrap();
        fs::write(root.join("server/main.mjs"), "throw Error('must not run');").unwrap();
        let mut manifest = json!({"packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":"disposable.source.publication","name":"可丢弃·源码发布集成","version":"1.0.0"},
            "web":{"root":"web","entry":"index.html","routing":"files"},
            "backend":{"root":"server","entry":"main.mjs","nodeMajor":24,"protocolVersion":1},
            "dataContract":{"id":"disposable.source.publication.data","version":1}});
        if build {
            manifest["development"] = json!({"sourceDir":".","build":{"kind":"npm","script":"build"},"outputDir":"release"});
        }
        atomic_json(&root.join("local-app.json"), &manifest).unwrap();
        atomic_json(&root.join("package.json"), &json!({"name":"disposable-source-publication","version":"1.0.0","scripts":{"build":"node build.mjs --disposable-only"}})).unwrap();
        atomic_json(&root.join("package-lock.json"), &json!({"name":"disposable-source-publication","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"disposable-source-publication","version":"1.0.0"}}})).unwrap();
        fs::write(
            root.join("build.mjs"),
            "throw Error('read-only review must not execute this');",
        )
        .unwrap();
    }
    async fn acquired_source(handle: &RuntimeHandle, root: PathBuf) -> (String, String) {
        let captured = handle
            .source_capture(CaptureLocalSourceInput {
                source_path: root,
                name: Some("disposable publication input".into()),
            })
            .await
            .unwrap();
        let captured = final_task(handle, captured["taskId"].as_str().unwrap()).await;
        assert_eq!(captured["status"], "succeeded", "{captured}");
        idle(handle).await;
        let id = captured["result"]["sourceId"].as_str().unwrap().to_owned();
        let state = handle.current();
        let manager = state.source_acquisition.as_ref().unwrap();
        let source = manager.info(&id).unwrap();
        let source_root = source.source_path.as_ref().unwrap();
        fs::create_dir(source_root.join(".git")).unwrap();
        fs::write(source_root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        // Convert this disposable captured fixture into a successful public
        // acquisition record without accessing any real remote repository.
        let task = state
            .builds
            .create_workspace_operation("sourceAcquire")
            .unwrap();
        let path = source.evidence_path.join("source.json");
        let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        record["provider"] = json!("github");
        record["url"] = json!("https://github.com/example/disposable-service-source.git");
        record["taskId"] = json!(task.task_id);
        record["capturePolicy"] = Value::Null;
        record["localSourcePath"] = Value::Null;
        atomic_json(&path, &record).unwrap();
        state
            .builds
            .finish(&task.task_id, Ok(json!(manager.info(&id).unwrap())))
            .unwrap();
        (task.task_id, id)
    }
    async fn installed() -> (
        tempfile::TempDir,
        RuntimePaths,
        RuntimeService,
        Arc<crate::node_apps::NodeApps>,
        String,
        String,
        String,
    ) {
        let (temp, paths, service, _client) = super::super::tests::start().await;
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("disposable-publication-author");
        source_project(&source, false);
        let bin = root.join("disposable-publication-tools/bin");
        fs::create_dir_all(&bin).unwrap();
        let node = bin.join("node");
        fs::write(&node,"#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'v24.22.0\\n'; exit 0; fi\nexec /bin/sh \"$@\"\n").unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(bin.join("npm"),"set -eu\nif [ \"$1\" = --version ]; then printf '11.9.0\\n'; exit 0; fi\nmkdir -p node_modules\nprintf fixture > node_modules/disposable-marker\n").unwrap();
        let handle = service.handle();
        handle
            .set_node_path(Some(node.to_string_lossy().into_owned()))
            .await
            .unwrap();
        let (task, source_id) = acquired_source(&handle, source).await;
        let nodes = Arc::new(
            crate::node_apps::NodeApps::new(
                paths.data_root().into(),
                root.join("must-not-run-fixed-node"),
            )
            .unwrap(),
        );
        let ready = handle
            .current()
            .source_acquisition
            .as_ref()
            .unwrap()
            .info(&source_id)
            .unwrap();
        let prepared = handle
            .node_install_prepare(
                nodes.clone(),
                crate::node_install_tasks::PrepareNodeInstallInput {
                    build_source: false,
                    source_id: source_id.clone(),
                    expected_source_digest: ready.content_digest.unwrap(),
                    target_app_id: None,
                },
            )
            .await
            .unwrap();
        let prepared = final_task(&handle, prepared["taskId"].as_str().unwrap()).await;
        assert_eq!(prepared["status"], "succeeded", "{prepared}");
        idle(&handle).await;
        let applied = handle
            .node_install_apply(
                nodes.clone(),
                prepared["result"]["candidate"]["candidateId"]
                    .as_str()
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(applied["status"], "succeeded", "{applied}");
        idle(&handle).await;
        let app = applied["result"]["appId"].as_str().unwrap().to_owned();
        (temp, paths, service, nodes, task, source_id, app)
    }

    #[tokio::test]
    async fn dropped_publication_future_retains_actual_leases_and_retries_the_same_published_source(
    ) {
        let (_temp, paths, service, nodes, task, source_id, app) = installed().await;
        let handle = service.handle();
        let state = handle.current();
        let app_root = paths.data_root().join("node-apps").join(&app);
        fs::write(
            app_root.join("data/disposable-record"),
            "business fixture preserved",
        )
        .unwrap();
        let (entered, waiting) = oneshot::channel();
        let (release, until_release) = std::sync::mpsc::channel();
        *PUBLICATION_PAUSE.lock().unwrap() = Some((app.clone(), entered, until_release));
        let worker = handle.clone();
        let worker_nodes = nodes.clone();
        let worker_task = task.clone();
        let worker_app = app.clone();
        let request = tokio::spawn(async move {
            worker
                .source_import_publish(worker_nodes, worker_task, worker_app)
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert!(state
            .store
            .active_task(&format!("source:{source_id}"))
            .is_some());
        assert!(state.active.load(Ordering::SeqCst) > 0);
        assert!(state
            .store
            .reserve_transient_operation(&format!("node-install:{app}"), "must-remain-busy")
            .is_err());
        release.send(()).unwrap();
        idle(&handle).await;
        let formal = paths
            .workspace_root()
            .join("project-sources/disposable-service-source");
        assert!(formal.join(".git/HEAD").is_file());
        assert!(!app_root.join("development-source.json").exists());
        let first = handle
            .source_import_publish(nodes.clone(), task.clone(), app.clone())
            .await
            .unwrap();
        let second = handle
            .source_import_publish(nodes.clone(), task, app)
            .await
            .unwrap();
        assert_eq!(first["sourcePath"], second["sourcePath"]);
        assert_eq!(first["sourcePath"], json!(formal));
        assert_eq!(
            fs::read_dir(paths.workspace_root().join("project-sources"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            fs::read(app_root.join("data/disposable-record")).unwrap(),
            b"business fixture preserved"
        );
        assert!(app_root.join("development-source.json").is_file());
        nodes.stop_all().await.unwrap();
        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_different_source_cannot_create_formal_source_for_an_installed_application() {
        let (_temp, paths, service, nodes, _task, _source_id, app) = installed().await;
        let handle = service.handle();
        let state = handle.current();
        let task = state
            .builds
            .create_workspace_operation("sourceAcquire")
            .unwrap();
        state.builds.finish(&task.task_id,Ok(json!({"sourceId":format!("src_{}",Uuid::new_v4().simple()),"contentDigest":"a".repeat(64)}))).unwrap();
        assert_eq!(
            handle
                .source_import_publish(nodes.clone(), task.task_id, app)
                .await
                .unwrap_err()
                .code,
            "SOURCE_PUBLICATION_INVALID"
        );
        assert!(!paths.workspace_root().join("project-sources").exists());
        assert!(!state.store.has_active_tasks());
        nodes.stop_all().await.unwrap();
        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn build_info_reads_only_declared_script_from_a_fresh_digest_verified_source() {
        let (temp, _paths, service, _client) = super::super::tests::start().await;
        let source = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-read-only-build-info");
        source_project(&source, true);
        let handle = service.handle();
        let (task, id) = acquired_source(&handle, source).await;
        let result = handle.source_import_build_info(&task).await.unwrap();
        assert_eq!(result["scriptName"], "build");
        assert_eq!(result["script"], "node build.mjs --disposable-only");
        let manager = handle
            .current()
            .source_acquisition
            .as_ref()
            .unwrap()
            .clone();
        let candidate = manager.info(&id).unwrap();
        let package = candidate.source_path.unwrap().join("package.json");
        fs::write(
            &package,
            "{\"scripts\":{\"build\":\"synthetic-unverified-command\"}}",
        )
        .unwrap();
        let error = handle.source_import_build_info(&task).await.unwrap_err();
        assert_eq!(error.code, "SOURCE_CHANGED");
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains("synthetic-unverified-command"));
        assert!(!handle.current().store.has_active_tasks());
        service.shutdown().await.unwrap();
    }
}
