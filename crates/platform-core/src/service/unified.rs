//! Application lifecycle and user confirmation shared by native UI and CLI.
use super::*;
use futures_util::future::BoxFuture;

pub type NativeAppReload =
    Arc<dyn Fn(Value) -> BoxFuture<'static, std::result::Result<(), ApiError>> + Send + Sync>;
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagementRequest {
    pub request_id: String,
    pub requester: String,
    pub action: String,
    pub impact: String,
    pub expires_at: u64,
}
pub(super) struct PendingConfirmation {
    expires: Instant,
    answer: oneshot::Sender<bool>,
}

impl RuntimeHandle {
    pub fn set_app_reload_hook(&self, hook: NativeAppReload) {
        *self.shared.app_reload_hook.write().unwrap() = Some(hook);
    }
    async fn reload_notice(&self, notice: Value) -> std::result::Result<(), ApiError> {
        let hook = self.shared.app_reload_hook.read().unwrap().clone();
        if let Some(hook) = hook {
            hook(notice).await?;
        }
        Ok(())
    }
    pub async fn reload_app(&self, id: &str) -> std::result::Result<Value, ApiError> {
        let gate = self
            .shared
            .reload_gates
            .lock()
            .unwrap()
            .entry(id.into())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _reload = gate.lock().await;
        self.check_native_ready()?;
        let state = self.current();
        state.store.check_workspace_available()?;
        state.store.control(id)?.check(false, None, false)?;
        let _operation = state.store.reserve_transient_operation(id, "reload")?;
        let project = match state.store.project(id) {
            Ok(project) => project,
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "INVALID_PROJECT_MANIFEST" | "INVALID_MANIFEST" | "APP_MANIFEST_INVALID"
                ) =>
            {
                let result = json!({"state":"invalidManifest","appId":id,"message":error.message,"error":error});
                self.reload_notice(json!({"phase":"invalidManifest","appId":id,"result":result}))
                    .await?;
                return Ok(result);
            }
            Err(error) => return Err(error),
        };
        match state.store.require_web_configuration_ready(&project) {
            Ok(()) => {}
            Err(error) if error.code == "WEB_CONFIGURATION_REQUIRED" => {
                let result = json!({"state":"needsConfiguration","appId":id,"message":error.message,"error":error});
                self.reload_notice(
                    json!({"phase":"needsConfiguration","appId":id,"result":result}),
                )
                .await?;
                return Ok(result);
            }
            Err(error) => return Err(error),
        }
        let was_open = self.web_status(id)["state"] == "running";
        let switched: std::result::Result<Value, ApiError> = async {
            self.reload_notice(json!({"phase":"prepare","appId":id})).await?;
            self.close_web(id).await?;
            let launch = self.launch(id).await?;
            // Cleanup is idempotent and also retries a prior post-start failure.
            // Accepted hasBackend may already be false after such an attempt.
            let removed_secrets = if !project.manifest.has_backend() {
                crate::web_app_configuration::store(&state.store.project(id)?)?.remove_all_secrets()
                    .map_err(|error|ApiError::new("APP_SECRET_CLEANUP_FAILED",error.to_string()))?
            } else {vec![]};
            let result=json!({"state":if was_open {"reloaded"} else {"opened"},"appId":id,"launch":launch,"removedSecrets":removed_secrets,"message":if removed_secrets.is_empty(){String::new()}else{format!("后台已移除，已删除密钥：{}",removed_secrets.join("、"))}});
            self.reload_notice(json!({"phase":"ready","appId":id,"result":result,"launch":result["launch"]})).await?;
            Ok(result)
        }.await;
        match switched {
            Ok(result) => Ok(result),
            Err(error) => {
                let mut error = error.detail(
                    "logPath",
                    json!(project.private_dir.join("logs/current.log")),
                );
                if let Err(close) = self.close_web(id).await {
                    error = error.detail("closeError", json!(close));
                }
                if let Some(manager) = &state.web_runtime {
                    manager.mark_failed(id, error.clone());
                }
                if let Err(notice) = self
                    .reload_notice(json!({"phase":"failed","appId":id,"error":error}))
                    .await
                {
                    error = error.detail("noticeError", json!(notice));
                }
                Err(error)
            }
        }
    }
    pub fn app_logs(&self, id: &str) -> std::result::Result<Value, ApiError> {
        let store = self.store();
        let accepted = store.accepted_has_backend(id)?;
        let has_backend = store
            .project(id)
            .map(|project| project.manifest.has_backend())
            .unwrap_or(accepted);
        let private = store.private_dir(id)?;
        let path = private.join("logs/current.log");
        let previous = private.join("logs/previous.log");
        let lines = if path.try_exists()? {
            let bytes = crate::independent_file_io::read_file(
                &path,
                crate::node_runtime_log::LOG_LIMIT as u64,
            )?;
            String::from_utf8_lossy(&bytes)
                .lines()
                .rev()
                .take(100)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        } else {
            vec![]
        };
        Ok(
            json!({"appId":id,"hasBackend":has_backend,"path":path,"currentLog":path,"current":path,"previousLog":previous,"previous":previous,"lines":lines}),
        )
    }
    pub fn subscribe_management_requests(&self) -> broadcast::Receiver<ManagementRequest> {
        self.shared.management_requests.subscribe()
    }
    pub fn resolve_management_request(
        &self,
        request_id: &str,
        approved: bool,
    ) -> std::result::Result<Value, ApiError> {
        self.check_native_ready()?;
        let pending = self
            .shared
            .management_pending
            .lock()
            .unwrap()
            .remove(request_id)
            .ok_or_else(|| {
                ApiError::new(
                    "MANAGEMENT_CONFIRMATION_EXPIRED",
                    "这个请求已结束或已过期。",
                )
            })?;
        if pending.expires <= Instant::now() {
            return Err(ApiError::new(
                "MANAGEMENT_CONFIRMATION_EXPIRED",
                "这个请求已过期，未执行。",
            ));
        }
        pending.answer.send(approved).map_err(|_| {
            ApiError::new("MANAGEMENT_REQUEST_CANCELLED", "请求方已停止等待，未执行。")
        })?;
        Ok(json!({"requestId":request_id,"approved":approved}))
    }
    pub(super) async fn confirm_management(
        &self,
        action: &str,
        impact: String,
    ) -> std::result::Result<Option<Value>, ApiError> {
        self.check_native_ready()?;
        let request_id = Uuid::new_v4().to_string();
        let (answer, receiver) = oneshot::channel();
        {
            let mut pending = self.shared.management_pending.lock().unwrap();
            pending.retain(|_, value| value.expires > Instant::now() && !value.answer.is_closed());
            if pending.len() >= 16 {
                return Err(ApiError::new(
                    "MANAGEMENT_CONFIRMATION_BUSY",
                    "等待确认的请求过多，请稍后重试。",
                ));
            }
            pending.insert(
                request_id.clone(),
                PendingConfirmation {
                    expires: Instant::now() + Duration::from_secs(120),
                    answer,
                },
            );
        }
        let request = ManagementRequest {
            request_id: request_id.clone(),
            requester: "平台命令行（本机 AI 或终端）".into(),
            action: action.into(),
            impact,
            expires_at: millis() + 120_000,
        };
        if self.shared.management_requests.send(request).is_err() {
            self.shared
                .management_pending
                .lock()
                .unwrap()
                .remove(&request_id);
            return Err(ApiError::new(
                "MANAGEMENT_CONFIRMATION_UNAVAILABLE",
                "请打开平台主窗口后再运行此命令。",
            ));
        }
        let result = match tokio::time::timeout(Duration::from_secs(120), receiver).await {
            Ok(Ok(true)) => None,
            Ok(Ok(false)) => Some(
                json!({"state":"declined","requestId":request_id,"message":"用户拒绝，未执行。"}),
            ),
            _ => Some(
                json!({"state":"timeout","requestId":request_id,"message":"两分钟内未确认，未执行。"}),
            ),
        };
        self.shared
            .management_pending
            .lock()
            .unwrap()
            .remove(&request_id);
        Ok(result)
    }
}

#[cfg(test)]
#[path = "unified/tests.rs"]
mod tests;
