//! Retain a service that started before desktop/Node setup failed. This uses
//! exactly the normal-Quit owner, without inventing business windows or Node.
use super::desktop_shutdown::{Backend, CoreState, Issue, Report};
use platform_core::RuntimeService;
use std::{future::Future, pin::Pin, sync::Arc};
type Work<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
pub(super) struct CoreOnlyBackend {
    runtime: Arc<RuntimeService>,
    changed: Box<dyn Fn(&Report) + Send + Sync>,
    ready: Box<dyn Fn() + Send + Sync>,
}
impl CoreOnlyBackend {
    pub fn new(
        runtime: Arc<RuntimeService>,
        changed: impl Fn(&Report) + Send + Sync + 'static,
        ready: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            runtime,
            changed: Box::new(changed),
            ready: Box::new(ready),
        }
    }
}
pub(super) fn core_state(report: platform_core::shutdown::ShutdownReport) -> CoreState {
    CoreState {
        closed: report.is_complete(),
        blocked: report.status == platform_core::shutdown::ShutdownStatus::Blocked,
        issues: report
            .issues
            .into_iter()
            .map(|i| Issue::new(&i.code, i.resource_closed, i.retryable))
            .collect(),
    }
}
pub(super) fn confirm_core(runtime: Arc<RuntimeService>) -> Work<Result<(), Issue>> {
    Box::pin(async move {
        let report = runtime.quiesce().await;
        if report.is_complete() {
            Ok(())
        } else {
            Err(Issue::new(
                "DESKTOP_CORE_NOT_QUIESCED",
                false,
                report.issues.iter().all(|i| i.retryable),
            ))
        }
    })
}
impl Backend for CoreOnlyBackend {
    fn start_core(&self) {
        self.runtime.begin_quiesce();
    }
    fn core_state(&self) -> CoreState {
        core_state(self.runtime.shutdown_status())
    }
    fn confirm_core(&self) -> Work<Result<(), Issue>> {
        confirm_core(self.runtime.clone())
    }
    fn windows(&self) -> Work<Result<(), Issue>> {
        Box::pin(async { Ok(()) })
    }
    fn nodes(&self) -> Work<Result<(), Issue>> {
        Box::pin(async { Ok(()) })
    }
    fn sessions(&self) -> Work<Result<(), Issue>> {
        Box::pin(async { Ok(()) })
    }
    fn changed(&self, report: &Report) {
        (self.changed)(report);
    }
    fn ready(&self) {
        (self.ready)();
    }
}
