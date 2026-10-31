//! Project-local browser profiles. Mac keeps its existing data-store identity;
//! Windows binds the actual directory and an environment-owned exit receipt.
use platform_core::{project_cleanup::ProjectKind, webview_storage::StoreLedger, ApiError};
use tauri::{AppHandle, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Wry};

#[cfg(windows)]
mod windows;
type ProjectBuilder<'a> = WebviewWindowBuilder<'a, Wry, AppHandle>;

#[derive(Clone)]
pub(super) struct Profile {
    #[cfg(windows)]
    binding: windows::Binding,
    #[cfg(not(windows))]
    identifier: [u8; 16],
}
impl Profile {
    pub(super) fn prepare(
        app: &AppHandle,
        ledger: &StoreLedger,
        kind: ProjectKind,
        id: &str,
        label: &str,
        identifier: [u8; 16],
    ) -> Result<Self, ApiError> {
        #[cfg(windows)]
        {
            let _ = identifier;
            Ok(Self {
                binding: windows::Binding::prepare(app, ledger, kind, id, label)?,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = (app, ledger, kind, id, label);
            Ok(Self { identifier })
        }
    }
    pub(super) fn initial_url(&self, launch: Url) -> WebviewUrl {
        #[cfg(windows)]
        {
            let _ = launch;
            WebviewUrl::External(Url::parse("about:blank").unwrap())
        }
        #[cfg(not(windows))]
        {
            WebviewUrl::External(launch)
        }
    }
    pub(super) fn configure<'a>(&self, builder: ProjectBuilder<'a>) -> ProjectBuilder<'a> {
        #[cfg(windows)]
        {
            builder
                .data_directory(self.binding.directory().to_path_buf())
                .visible(false)
        }
        #[cfg(not(windows))]
        {
            builder.data_store_identifier(self.identifier)
        }
    }
    pub(super) fn reserve(&self, app: &AppHandle, generation: u64) -> Result<(), ApiError> {
        #[cfg(windows)]
        {
            self.binding.reserve(app, generation)
        }
        #[cfg(not(windows))]
        {
            let _ = (app, generation);
            Ok(())
        }
    }
    pub(super) fn hold_resource(
        &self,
        receipt: &super::business_window_lifetime::Receipt,
        resource: impl Send + Sync + 'static,
    ) -> Result<(), ApiError> {
        #[cfg(windows)]
        {
            let _ = receipt;
            self.binding.hold_resource(resource)
        }
        #[cfg(not(windows))]
        {
            receipt
                .hold_resource(resource)
                .map_err(|issue| ApiError::new(issue.code, "项目窗口已关闭，未创建新窗口"))
        }
    }
    pub(super) fn navigation(&self, original: impl FnOnce() -> bool) -> bool {
        #[cfg(windows)]
        if !self.binding.document_allowed() {
            return false;
        }
        original()
    }
    pub(super) fn bind_window(&self, window: &WebviewWindow) -> Result<(), ApiError> {
        #[cfg(windows)]
        {
            self.binding.bind_window(window)
        }
        #[cfg(not(windows))]
        {
            let _ = window;
            Ok(())
        }
    }
    pub(super) fn creation_failed(&self) {
        #[cfg(windows)]
        self.binding.creation_failed();
    }
    pub(super) fn activate(
        &self,
        window: &WebviewWindow,
        launch: Url,
        admitted: impl FnOnce() -> Result<(), ApiError>,
    ) -> Result<(), ApiError> {
        #[cfg(windows)]
        {
            admitted()?;
            if !self.binding.document_allowed() {
                return Err(ApiError::new(
                    "WEBVIEW_PROFILE_PROCESS_UNKNOWN",
                    "项目浏览器进程尚未核实，项目页面尚未加载。",
                ));
            }
            window
                .navigate(launch)
                .and_then(|()| window.show())
                .map_err(|cause| ApiError::new("WINDOW_OPEN_FAILED", cause.to_string()))
        }
        #[cfg(not(windows))]
        {
            let _ = (window, launch, admitted);
            Ok(())
        }
    }
    pub(super) fn abort_start(&self, app: &AppHandle, window: &WebviewWindow, generation: u64) {
        #[cfg(windows)]
        if let Err(issue) = before_destroy(app, window, generation).and_then(|()| {
            window
                .destroy()
                .map_err(|cause| ApiError::new("PROJECT_WINDOW_DESTROY_FAILED", cause.to_string()))
        }) {
            eprintln!("Project browser startup owner retained: {}", issue.code);
        }
        #[cfg(not(windows))]
        let _ = (app, window, generation);
    }
}

pub(super) fn before_destroy(
    app: &AppHandle,
    window: &WebviewWindow,
    generation: u64,
) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        if window.label().starts_with("project-") {
            windows::before_destroy(app, window, generation)
        } else {
            Ok(())
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (app, window, generation);
        Ok(())
    }
}
pub(super) async fn wait_closed(
    app: &AppHandle,
    label: &str,
    generation: Option<u64>,
) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        if let Some(generation) = generation.filter(|_| label.starts_with("project-")) {
            windows::wait_closed(app, label, generation).await?;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (app, label, generation);
        Ok(())
    }
}

pub(super) fn window_destroyed(app: &AppHandle, label: &str, generation: u64) {
    #[cfg(windows)]
    if label.starts_with("project-") {
        windows::window_destroyed(app, label, generation);
    }
    #[cfg(not(windows))]
    let _ = (app, label, generation);
}

pub(super) async fn destroy_for_shutdown(
    app: &AppHandle,
    window: WebviewWindow,
    generation: u64,
) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        let (send, receive) = tokio::sync::oneshot::channel();
        let owner = app.clone();
        app.run_on_main_thread(move || {
            let result = before_destroy(&owner, &window, generation).and_then(|()| {
                window.destroy().map_err(|cause| {
                    ApiError::new("PROJECT_WINDOW_DESTROY_FAILED", cause.to_string())
                })
            });
            let _ = send.send(result);
        })
        .map_err(|_| {
            ApiError::new(
                "PROJECT_WINDOW_UI_DISPATCH_FAILED",
                "无法派发项目窗口关闭。",
            )
        })?;
        receive
            .await
            .map_err(|_| ApiError::new("PROJECT_WINDOW_UI_REPLY_LOST", "项目窗口关闭回执缺失。"))?
    }
    #[cfg(not(windows))]
    {
        let _ = (app, generation);
        window
            .destroy()
            .map_err(|cause| ApiError::new("PROJECT_WINDOW_DESTROY_FAILED", cause.to_string()))
    }
}

pub(super) async fn drain_shutdown(app: &AppHandle) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        windows::drain_shutdown(app).await
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Ok(())
    }
}
