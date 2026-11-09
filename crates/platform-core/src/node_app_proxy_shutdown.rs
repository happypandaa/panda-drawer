//! Owns per-proxy work through real completion. No child-process or installer permit.
use super::*;
use crate::{
    io_lifetime::{IoLease, IoLifetime},
    shutdown::{ShutdownIssue, ShutdownReport, ShutdownStatus},
};
use futures_util::FutureExt;
use std::{
    collections::VecDeque,
    future::Future,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::sync::oneshot;

#[cfg(test)]
type Pause = (oneshot::Sender<()>, std::sync::mpsc::Receiver<()>);
#[cfg(test)]
#[derive(Default)]
pub(super) struct Hooks {
    pub open: Mutex<Option<Pause>>,
    pub read: Mutex<Option<Pause>>,
    pub panic_read: std::sync::atomic::AtomicBool,
    pub panic_open: std::sync::atomic::AtomicBool,
    pub panic_serve: std::sync::atomic::AtomicBool,
    pub panic_driver: std::sync::atomic::AtomicBool,
    pub driver_closed: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    pub socket_write_budget: Mutex<Option<usize>>,
    pub state: Mutex<std::sync::Weak<ProxyState>>,
}

pub(super) struct ProxyShutdown {
    pub lifetime: IoLifetime,
    #[cfg(test)]
    pub authorization: Arc<Mutex<Authorization>>,
    pub stopping: watch::Sender<bool>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    draining: tokio::sync::Mutex<VecDeque<JoinHandle<()>>>,
    completion: Arc<Completion>,
    server: tokio::sync::Mutex<Option<JoinHandle<io::Result<()>>>>,
    issues: Mutex<Vec<ShutdownIssue>>,
    coordinator: Mutex<Option<JoinHandle<()>>>,
    report: watch::Sender<ShutdownReport>,
    #[cfg(test)]
    pub hooks: Hooks,
}
struct Completion {
    done: AtomicBool,
    changed: tokio::sync::Notify,
}
impl ProxyShutdown {
    pub fn new() -> Arc<Self> {
        let (stopping, _) = watch::channel(false);
        let (report, _) = watch::channel(ShutdownReport::default());
        Arc::new(Self {
            lifetime: IoLifetime::default(),
            #[cfg(test)]
            authorization: Arc::new(Mutex::new(Authorization {
                bootstrap: None,
                session: Uuid::new_v4().simple().to_string(),
            })),
            stopping,
            workers: Mutex::new(Vec::new()),
            draining: tokio::sync::Mutex::new(VecDeque::new()),
            completion: Arc::new(Completion {
                done: AtomicBool::new(false),
                changed: tokio::sync::Notify::new(),
            }),
            server: tokio::sync::Mutex::new(None),
            issues: Mutex::new(Vec::new()),
            coordinator: Mutex::new(None),
            report,
            #[cfg(test)]
            hooks: Hooks::default(),
        })
    }
    fn spawn_locked<T, F>(&self, workers: &mut Vec<JoinHandle<()>>, work: F) -> oneshot::Receiver<T>
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        let (send, receive) = oneshot::channel();
        workers.push(tokio::spawn(async move {
            let result = work.await;
            // A lost receiver is not completion proof; the worker remains
            // registered through resource destruction and actual JoinHandle join.
            let _ = send.send(result);
        }));
        receive
    }
    pub fn request<T, F, W>(self: &Arc<Self>, work: W) -> Result<oneshot::Receiver<T>, ApiError>
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
        W: FnOnce(IoLease) -> F,
    {
        let mut workers = self.workers.lock().unwrap_or_else(|p| p.into_inner());
        let lease = self.lifetime.admit()?;
        Ok(self.spawn_locked(&mut workers, work(lease)))
    }
    // Only already-admitted work may derive an internal worker. It retains the
    // original lease; it never attempts another admission after seal.
    pub fn work<T, F>(self: &Arc<Self>, work: F) -> oneshot::Receiver<T>
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        let mut workers = self.workers.lock().unwrap_or_else(|p| p.into_inner());
        self.spawn_locked(&mut workers, work)
    }
    #[cfg(test)]
    pub fn blocking<T, F>(self: &Arc<Self>, work: F) -> oneshot::Receiver<Result<T, ApiError>>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let mut workers = self.workers.lock().unwrap_or_else(|p| p.into_inner());
        let blocking = tokio::task::spawn_blocking(work);
        let owner = self.clone();
        self.spawn_locked(&mut workers, async move {
            match blocking.await {
                Ok(value) => Ok(value),
                Err(_) => {
                    owner.record_worker_failure();
                    Err(proxy_error(
                        "NODE_APP_PROXY_WORKER_FAILED",
                        "应用连接的文件工作未正常结束，现场和句柄已保留。",
                    ))
                }
            }
        })
    }
    fn record_worker_failure(&self) {
        self.issues
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(ShutdownIssue::new(
                "nodeAppProxy",
                "NODE_APP_PROXY_WORKER_JOIN_FAILED",
                "Owned proxy work did not finish normally; resource shutdown remains unconfirmed",
                false,
                false,
            ));
    }
    #[cfg(test)]
    pub fn install(
        self: &Arc<Self>,
        listener: TcpListener,
        router: Router,
    ) -> Result<(), ApiError> {
        let _submission = self.workers.lock().unwrap_or_else(|p| p.into_inner());
        self.lifetime.check_open()?;
        let mut slot = self
            .server
            .try_lock()
            .map_err(|_| proxy_error("NODE_APP_PROXY_UNAVAILABLE", "应用连接正在关闭。"))?;
        if slot.is_some() {
            return Err(proxy_error(
                "NODE_APP_PROXY_UNAVAILABLE",
                "应用连接已登记。",
            ));
        }
        let owner = self.clone();
        let mut stop = self.stopping.subscribe();
        *slot = Some(tokio::spawn(async move {
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    if !*stop.borrow() {
                        let _ = stop.changed().await;
                    }
                })
                .await;
            #[cfg(test)]
            if owner
                .hooks
                .panic_serve
                .load(std::sync::atomic::Ordering::Acquire)
            {
                panic!("controlled artificial proxy serve panic");
            }
            drop(owner);
            result
        }));
        Ok(())
    }
    pub fn request_stop(&self) {
        let _submission = self.workers.lock().unwrap_or_else(|p| p.into_inner());
        self.lifetime.seal();
        self.stopping.send_replace(true);
        #[cfg(test)]
        {
            let mut authorization = self.authorization.lock().unwrap_or_else(|p| p.into_inner());
            authorization.bootstrap = None;
            authorization.session.clear();
        }
    }
    #[cfg(test)]
    pub async fn server_for_test(&self) -> bool {
        self.server.lock().await.is_none()
    }
    pub fn status(&self) -> ShutdownReport {
        self.report.borrow().clone()
    }
    pub fn begin(self: &Arc<Self>) {
        self.request_stop();
        let mut coordinator = self.coordinator.lock().unwrap_or_else(|p| p.into_inner());
        if coordinator.is_some() || self.status().status == ShutdownStatus::Blocked {
            return;
        }
        self.report.send_replace(ShutdownReport {
            status: ShutdownStatus::Pending,
            phase: "proxy-workers".into(),
            attempt: 1,
            resource_closed: false,
            issues: Vec::new(),
        });
        let owner = self.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                let completion = self.completion.clone();
                *coordinator = Some(runtime.spawn(async move {
                    owner.run_owned().await;
                    completion.done.store(true, Ordering::Release);
                    completion.changed.notify_waiters();
                }));
            }
            Err(_) => {
                self.report.send_replace(ShutdownReport {
                    status: ShutdownStatus::Blocked,
                    phase: "proxy-unknown".into(),
                    attempt: 1,
                    resource_closed: false,
                    issues: vec![ShutdownIssue::new(
                        "nodeAppProxy",
                        "NODE_APP_PROXY_RUNTIME_UNAVAILABLE",
                        "No executor can prove proxy shutdown; owner retained",
                        false,
                        false,
                    )],
                });
                let _ = Arc::into_raw(owner);
            }
        }
    }
    async fn run_owned(self: Arc<Self>) {
        let result = std::panic::AssertUnwindSafe(self.drain())
            .catch_unwind()
            .await;
        let report = match result {
            Ok(report) => report,
            Err(_) => ShutdownReport {
                status: ShutdownStatus::Blocked,
                phase: "proxy-unknown".into(),
                attempt: 1,
                resource_closed: false,
                issues: vec![ShutdownIssue::new(
                    "nodeAppProxy",
                    "NODE_APP_PROXY_COORDINATOR_PANICKED",
                    "Proxy drain panicked; owner and unfinished resources retained",
                    false,
                    false,
                )],
            },
        };
        let blocked = !report.is_complete();
        self.report.send_replace(report);
        if blocked {
            std::future::pending::<()>().await;
        }
    }
    pub async fn wait(self: &Arc<Self>) -> Result<(), ApiError> {
        let mut report = self.report.subscribe();
        self.begin();
        loop {
            let status = report.borrow_and_update().clone();
            if status.is_complete() {
                loop {
                    let done = self.completion.changed.notified();
                    if self.completion.done.load(Ordering::Acquire) {
                        break;
                    }
                    done.await;
                }
                return Ok(());
            }
            if status.status == ShutdownStatus::Blocked {
                return Err(proxy_error(
                    "NODE_APP_PROXY_DRAIN_BLOCKED",
                    "应用连接尚未确认完整关闭，已保留受管句柄。",
                )
                .detail("shutdownIssues", serde_json::json!(status.issues)));
            }
            if report.changed().await.is_err() {
                return Err(proxy_error(
                    "NODE_APP_PROXY_DRAIN_BLOCKED",
                    "无法取得应用连接关闭结果，句柄已保留。",
                ));
            }
        }
    }
    async fn drain(&self) -> ShutdownReport {
        let mut draining = self.draining.lock().await;
        loop {
            draining.extend(std::mem::take(
                &mut *self.workers.lock().unwrap_or_else(|p| p.into_inner()),
            ));
            if draining.is_empty() {
                self.lifetime.drain().await;
                if self
                    .workers
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .is_empty()
                {
                    break;
                }
                continue;
            }
            while let Some(worker) = draining.front_mut() {
                if worker.await.is_err() {
                    self.record_worker_failure();
                }
                // The owner retains every unobserved handle until the actual
                // join result is available, including coordinator unwinding.
                draining.pop_front();
            }
        }
        drop(draining);
        self.issues
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .extend(self.lifetime.shutdown_issues());
        let mut server = self.server.lock().await;
        if let Some(server) = server.as_mut() {
            let issue = match server.await {
                Ok(Ok(())) => None,
                Ok(Err(_)) => Some(ShutdownIssue::new(
                    "nodeAppProxy",
                    "NODE_APP_PROXY_SERVE_FAILED",
                    "Proxy server failed; connection shutdown remains unconfirmed",
                    false,
                    false,
                )),
                Err(_) => Some(ShutdownIssue::new(
                    "nodeAppProxy",
                    "NODE_APP_PROXY_SERVE_JOIN_FAILED",
                    "Proxy server did not finish normally; connection shutdown remains unconfirmed",
                    false,
                    false,
                )),
            };
            if let Some(issue) = issue {
                self.issues
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(issue);
            }
        }
        *server = None;
        let issues = self
            .issues
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        ShutdownReport {
            status: if issues.is_empty() {
                ShutdownStatus::Complete
            } else {
                ShutdownStatus::Blocked
            },
            phase: if issues.is_empty() {
                "proxy-complete"
            } else {
                "proxy-blocked"
            }
            .into(),
            attempt: 1,
            resource_closed: issues.iter().all(|issue| issue.resource_closed),
            issues,
        }
    }
}
