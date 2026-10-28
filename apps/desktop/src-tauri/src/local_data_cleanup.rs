//! The manager owns one explicit cleanup. Losing an IPC waiter never cancels
//! accepted shutdown/deletion work, and normal Quit cannot race the deletion.
use super::{desktop_shutdown, ApiError, AppState};
use platform_core::local_cleanup::LocalCleanupPlan;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{atomic::Ordering, Mutex};
use tauri::Manager;

const CONFIRMATION: &str = "删除全部本机数据";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SubmitInput {
    plan_id: String,
    first_confirmation: bool,
    confirmation: String,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Report {
    operation_id: Option<String>,
    status: Status,
    message: String,
    remaining_paths: Vec<String>,
    error: Option<String>,
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    #[default]
    Idle,
    Stopping,
    Deleting,
    Completed,
    Blocked,
    Failed,
}
impl Status {
    fn can_quit(self) -> bool {
        matches!(
            self,
            Self::Idle | Self::Completed | Self::Blocked | Self::Failed
        )
    }
}
impl Report {
    fn finish(&mut self, result: Result<Value, ApiError>) {
        match result {
            Ok(_) => {
                self.status = Status::Completed;
                self.message = "本软件管理的本机项目数据和代码已清理。外部原始源码及清理范围之外的独立导出文件保留。请退出软件后卸载。".into();
                self.remaining_paths.clear();
            }
            Err(error) => {
                self.status = Status::Failed;
                self.message = if error.details["operationOutcome"] == "not_started" {
                    "清理范围或关闭条件未通过核对，没有开始删除。请退出后检查，再重新确认。"
                } else {
                    "清理未全部完成，部分内容可能已经永久删除。请核对剩余目录和错误信息，不会自动重试。"
                }.into();
                if let Some(paths) = error.details["remainingPaths"].as_array() {
                    self.remaining_paths = paths
                        .iter()
                        .filter_map(|path| path.as_str().map(str::to_owned))
                        .collect();
                }
                self.error = Some(format!("{}: {}", error.code, error.message));
            }
        }
    }
}
#[derive(Default)]
struct Inner {
    plan: Option<LocalCleanupPlan>,
    report: Report,
    quit_requested: bool,
    worker: Option<tokio::task::JoinHandle<()>>,
}
#[derive(Default)]
pub(super) struct LocalDataCleanup(Mutex<Inner>);
impl LocalDataCleanup {
    fn install(&self, plan: LocalCleanupPlan) -> Result<Value, ApiError> {
        let mut inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if inner.report.status != Status::Idle {
            return Err(ApiError::new(
                "LOCAL_CLEANUP_ALREADY_STARTED",
                "清理已经受理，请查看原操作结果",
            ));
        }
        let preview = plan.preview();
        inner.plan = Some(plan);
        Ok(preview)
    }
    pub fn active(&self) -> bool {
        self.status().status != Status::Idle
    }
    pub fn status(&self) -> Report {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .report
            .clone()
    }
    /// Normal menu/keyboard Quit uses the same terminal rules as the button.
    /// During deletion it must not let Tauri exit beneath the owned worker.
    pub fn intercept_quit(&self, app: &tauri::AppHandle) -> bool {
        let state = app.state::<AppState>();
        let mut inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if inner.report.status == Status::Idle {
            return false;
        }
        if !inner.report.status.can_quit() {
            drop(inner);
            super::show_manager(app);
            return true;
        }
        inner.quit_requested = true;
        inner.plan = None;
        drop(inner);
        if state.desktop_shutdown.status().status == desktop_shutdown::Status::ReadyForNativeExit {
            state.exit_ready.store(true, Ordering::Release);
            app.exit(0);
            return true;
        }
        false
    }
    fn accept(&self, input: SubmitInput, state: &AppState) -> Result<Report, ApiError> {
        let mut inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if inner.report.status != Status::Idle {
            return Err(ApiError::new(
                "LOCAL_CLEANUP_ALREADY_STARTED",
                "清理已经受理，不会重复执行",
            ));
        }
        let plan = inner.plan.as_ref().ok_or_else(|| {
            ApiError::new(
                "LOCAL_CLEANUP_PREVIEW_REQUIRED",
                "请重新读取清理范围并完成两次确认",
            )
        })?;
        if plan.id() != input.plan_id
            || !input.first_confirmation
            || input.confirmation != CONFIRMATION
        {
            return Err(ApiError::new(
                "LOCAL_CLEANUP_CONFIRMATION_REQUIRED",
                "清理范围或确认内容不匹配，请重新核对",
            ));
        }
        let preview = plan.preview();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        if preview["expiresAt"]
            .as_u64()
            .is_none_or(|expires| u128::from(expires) <= now)
        {
            return Err(ApiError::new(
                "LOCAL_CLEANUP_PLAN_EXPIRED",
                "清理预览已过期，请重新确认",
            ));
        }
        state
            .shutting_down
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                ApiError::new(
                    "SERVICE_STOPPING",
                    "软件已经开始退出，请重新启动后核对清理范围",
                )
            })?;
        inner.report = Report {
            operation_id: Some(input.plan_id),
            status: Status::Stopping,
            message: "正在关闭业务窗口、后台服务和文件工作；尚未开始删除。".into(),
            remaining_paths: preview["deletePaths"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| item["path"].as_str().map(str::to_owned))
                .collect(),
            error: None,
        };
        Ok(inner.report.clone())
    }
    pub fn shutdown_changed(&self, report: &desktop_shutdown::Report) -> bool {
        let mut inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if inner.report.status == Status::Idle || inner.quit_requested {
            return false;
        }
        if report.status == desktop_shutdown::Status::Blocked
            && inner.report.status == Status::Stopping
        {
            inner.report.status = Status::Blocked;
            inner.report.message =
                "尚未确认所有窗口、服务和文件工作已停止，没有开始删除。请退出后检查保留现场。"
                    .into();
            inner.report.error = Some(
                report
                    .issues
                    .iter()
                    .map(|issue| issue.code.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        true
    }
    /// Called only after the actual four shutdown jobs have been observed. This
    /// consumes the plan once; serialized Ready alone cannot authorize deletion.
    pub fn drained(&self, app: &tauri::AppHandle) -> bool {
        let state = app.state::<AppState>();
        let mut inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if inner.report.status == Status::Idle || inner.quit_requested {
            return false;
        }
        if inner.report.status != Status::Stopping {
            return true;
        }
        if !state.desktop_shutdown.cleanup_ready() {
            inner.report.status = Status::Blocked;
            inner.report.message = "关闭过程仍有未解决的问题，没有开始删除。".into();
            inner.report.error = Some("LOCAL_CLEANUP_DRAIN_UNCONFIRMED".into());
            return true;
        }
        let Some(plan) = inner.plan.take() else {
            inner.report.status = Status::Failed;
            inner.report.message = "清理计划不可用，没有开始删除。".into();
            return true;
        };
        inner.report.status = Status::Deleting;
        inner.report.message = "正在永久清理本机项目内容，请勿强制退出软件。".into();
        let app = app.clone();
        let runtime = state.runtime.clone();
        inner.worker = Some(tauri::async_runtime::handle().inner().spawn(async move {
            let result = execute_native_then_disk(&app, plan, runtime).await;
            let state = app.state::<AppState>();
            state
                .local_data_cleanup
                .0
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .report
                .finish(result);
        }));
        true
    }
}

async fn execute_native_then_disk(
    app: &tauri::AppHandle,
    plan: LocalCleanupPlan,
    runtime: std::sync::Arc<platform_core::RuntimeService>,
) -> Result<Value, ApiError> {
    // Verify writer closure and the exact user-reviewed scope before erasing
    // native data, then repeat disk verification after the OS callbacks.
    let (plan, runtime) = tauri::async_runtime::spawn_blocking(move || {
        plan.verify_before_native_cleanup(&runtime)?;
        Ok::<_, ApiError>((plan, runtime))
    })
    .await
    .map_err(|_| {
        ApiError::new(
            "LOCAL_CLEANUP_PREFLIGHT_UNKNOWN",
            "未能确认清理前置条件，没有开始删除。",
        )
        .outcome("not_started")
    })??;
    let ledger = app.state::<AppState>().handle.webview_stores();
    let mut native_started = false;
    let native_result = async {
        for record in ledger.records()? {
            native_started = true;
            super::native_stores::purge_identifier(app, record.identifier).await?;
            ledger.forget_confirmed(record.kind, &record.id)?;
        }
        native_started = true;
        super::native_stores::clear_identifier_and_verify(
            app,
            platform_core::webview_storage::manager_identifier(),
        )
        .await?;
        Ok::<(), ApiError>(())
    }
    .await;
    if let Err(error) = native_result {
        return Err(error.outcome(if native_started {
            "delete_incomplete"
        } else {
            "not_started"
        }));
    }
    tauri::async_runtime::spawn_blocking(move || plan.execute_after_shutdown(&runtime))
        .await
        .map_err(|_| {
            ApiError::new(
                "LOCAL_CLEANUP_WORKER_UNKNOWN",
                "清理工作意外结束，部分数据可能已经删除。",
            )
            .outcome("unknown")
        })?
        .map_err(|error| error.outcome("delete_incomplete"))
}

#[tauri::command]
pub(super) async fn local_data_cleanup_preview(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<Value, ApiError> {
    super::require_manager(&window)?;
    let lease = app.state::<AppState>().handle.io_lifetime().admit()?;
    lease
        .run(async move {
            let runtime = app.state::<AppState>().runtime.clone();
            let plan = tauri::async_runtime::spawn_blocking(move || {
                let paths = platform_core::RuntimePaths::discover().map_err(|error| {
                    ApiError::new("LOCAL_CLEANUP_PREVIEW_FAILED", error.to_string())
                })?;
                LocalCleanupPlan::prepare(&paths, &runtime)
            })
            .await
            .map_err(|_| ApiError::new("LOCAL_CLEANUP_PREVIEW_FAILED", "无法完整读取清理范围"))??;
            let state = app.state::<AppState>();
            state.handle.io_lifetime().check_open()?;
            state.local_data_cleanup.install(plan)
        })
        .await?
}
#[tauri::command]
pub(super) fn local_data_cleanup_start(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    input: SubmitInput,
) -> Result<Report, ApiError> {
    super::require_manager(&window)?;
    let state = app.state::<AppState>();
    let _lease = state.handle.io_lifetime().admit()?;
    let report = state.local_data_cleanup.accept(input, &state)?;
    super::desktop_shutdown_native::request_for_cleanup(&app);
    Ok(report)
}
#[tauri::command]
pub(super) fn local_data_cleanup_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Report, ApiError> {
    super::require_manager(&window)?;
    Ok(state.local_data_cleanup.status())
}
#[tauri::command]
pub(super) fn local_data_cleanup_quit(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), ApiError> {
    super::require_manager(&window)?;
    let state = app.state::<AppState>();
    if !state.local_data_cleanup.status().status.can_quit() {
        return Err(ApiError::new(
            "LOCAL_CLEANUP_PENDING",
            "请等待清理的最终结果后退出",
        ));
    }
    super::desktop_shutdown_native::request(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pending_cleanup_cannot_be_quit_or_reinterpreted_as_finished() {
        assert!(!Status::Stopping.can_quit());
        assert!(!Status::Deleting.can_quit());
        for status in [
            Status::Idle,
            Status::Completed,
            Status::Blocked,
            Status::Failed,
        ] {
            assert!(status.can_quit());
        }
    }

    #[test]
    fn partial_failure_reports_actual_remaining_paths_without_a_receipt() {
        let mut report = Report {
            remaining_paths: vec![
                "/disposable/already-removed".into(),
                "/disposable/remaining".into(),
            ],
            ..Report::default()
        };
        report.finish(Err(ApiError::new(
            "LOCAL_CLEANUP_INCOMPLETE",
            "controlled failure",
        )
        .outcome("delete_incomplete")
        .detail("remainingPaths", json!(["/disposable/remaining"]))
        .detail("receiptPath", "/disposable/.local-cleanup/receipt.json")));
        assert!(report.status == Status::Failed);
        assert_eq!(report.remaining_paths, ["/disposable/remaining"]);
        assert!(serde_json::to_value(&report)
            .unwrap()
            .get("receiptPath")
            .is_none());
        assert!(report.message.contains("部分内容可能已经永久删除"));
    }

    #[test]
    fn rejected_preflight_never_claims_partial_deletion_or_success() {
        let mut report = Report::default();
        report.finish(Err(ApiError::new(
            "LOCAL_CLEANUP_CHANGED",
            "controlled mismatch",
        )
        .outcome("not_started")));
        assert!(report.status == Status::Failed);
        assert!(report.message.contains("没有开始删除"));
        assert!(!report.message.contains("部分内容可能已经"));
    }

    #[test]
    fn completed_cleanup_clears_pending_paths_without_a_receipt() {
        let mut report = Report {
            remaining_paths: vec!["/disposable/owned".into()],
            ..Report::default()
        };
        report.finish(Ok(
            json!({"operationOutcome":"deleted", "receiptPath":"/disposable/receipt.json"}),
        ));
        assert!(report.status == Status::Completed);
        assert!(report.remaining_paths.is_empty());
        assert!(serde_json::to_value(&report)
            .unwrap()
            .get("receiptPath")
            .is_none());
    }
}
