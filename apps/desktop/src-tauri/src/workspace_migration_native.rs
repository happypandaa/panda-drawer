//! Reversible native receipts for the public workspace migration workflow.
use super::{
    business_window_lifetime::WorkspaceWindowPause, engine_downloads::EngineMigrationPause,
    AppState,
};
use platform_core::{node_apps::NodeWorkspacePause, ApiError, WorkspaceMigrationStage};
use std::sync::Arc;
use tauri::Manager;

struct NativePause {
    _windows: WorkspaceWindowPause,
    engines: EngineMigrationPause,
    nodes: NodeWorkspacePause,
}
pub(super) fn install(app: &tauri::AppHandle) {
    let owner = app.clone();
    let pause: Arc<tokio::sync::Mutex<Option<NativePause>>> = Arc::default();
    app.state::<AppState>()
        .handle
        .set_workspace_migration_hook(Arc::new(move |notice| {
            let app = owner.clone();
            let pause = pause.clone();
            Box::pin(async move {
                let mut current = pause.lock().await;
                let state = app.state::<AppState>();
                match notice.stage {
                    WorkspaceMigrationStage::Prepare => {
                        if current.is_some() {
                            return Err(ApiError::new(
                                "WORKSPACE_MIGRATING",
                                "原生工作区暂停尚未完成。",
                            ));
                        }
                        let windows =
                            state.window_lifetimes.pause_workspace().map_err(|issue| {
                                ApiError::new(issue.code, "项目窗口正在收尾，工作区暂不能切换。")
                            })?;
                        let engines = state.engine_downloads.pause_workspace().await?;
                        // Set this before stopping a backend, including an abort:
                        // the monitor must preserve the old document and its draft.
                        state.window_lifetimes.obsolete_workspace(Some("node"));
                        let nodes = state.node_apps.begin_workspace_migration().await?;
                        *current = Some(NativePause {
                            _windows: windows,
                            engines,
                            nodes,
                        });
                        Ok(())
                    }
                    WorkspaceMigrationStage::Reopened => {
                        let receipt = current.as_ref().ok_or_else(|| {
                            ApiError::new(
                                "WORKSPACE_NATIVE_RECEIPT_MISSING",
                                "尚未取得旧原生窗口和后台的停止收据。",
                            )
                        })?;
                        receipt.nodes.validate_reopened(&notice.destination_path)?;
                        state.window_lifetimes.obsolete_workspace(None);
                        receipt.engines.commit();
                        drop(current.take());
                        Ok(())
                    }
                    WorkspaceMigrationStage::Aborted => {
                        state.window_lifetimes.obsolete_workspace(Some("node"));
                        drop(current.take());
                        Ok(())
                    }
                    WorkspaceMigrationStage::Startup => {
                        if let Some(receipt) = current.as_ref() {
                            // A previous live switch installed this runtime but did
                            // not yet release its exact old-resource receipts.
                            receipt.nodes.validate_reopened(&notice.destination_path)?;
                            state.window_lifetimes.obsolete_workspace(None);
                            receipt.engines.commit();
                            drop(current.take());
                            return Ok(());
                        }
                        // A retry must not stop applications that were started in
                        // the selected workspace after a former recovery failure.
                        state
                            .node_apps
                            .verify_previous_workspace_drained(&notice.source_path)
                            .await
                    }
                }
            })
        }));
}
