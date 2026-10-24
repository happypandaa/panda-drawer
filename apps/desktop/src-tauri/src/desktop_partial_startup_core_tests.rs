//! Verification harness included only by platform-core's private test fixture
//! entry. RuntimePaths has intentionally no public custom-root override; this
//! source uses its cfg(test) TempDir factory, never the actual development root.
#[path = "desktop_partial_startup.rs"]
mod desktop_partial_startup;
#[path = "desktop_shutdown.rs"]
mod desktop_shutdown;
#[cfg(test)]
mod tests {
    use super::{
        desktop_partial_startup::CoreOnlyBackend,
        desktop_shutdown::{DesktopShutdown, Status},
    };
    use crate::{RuntimePaths, RuntimeService};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    async fn until(check: impl Fn() -> bool) {
        tokio::time::timeout(std::time::Duration::from_secs(4), async {
            while !check() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn partial_startup_with_real_runtime_waits_for_actual_io_after_losing_quit_caller() {
        let (_temp, paths) = RuntimePaths::for_test();
        let runtime = Arc::new(
            RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
                .await
                .unwrap(),
        );
        let io = runtime.handle().io_lifetime();
        let path = paths.data_root().join("artificial-partial-startup-file");
        std::fs::write(&path, b"artificial preserved content").unwrap();
        let held = io
            .admit()
            .unwrap()
            .hold(std::fs::File::open(&path).unwrap());
        let ready = Arc::new(AtomicBool::new(false));
        let signal = ready.clone();
        let backend = Arc::new(CoreOnlyBackend::new(
            runtime.clone(),
            |_| {},
            move || {
                signal.store(true, Ordering::Release);
            },
        ));
        let owner = Arc::new(DesktopShutdown::default());
        owner.request(backend, Some(tokio::runtime::Handle::current()), || {
            Ok((Box::new(()), Box::new(())))
        });
        let weak = Arc::downgrade(&runtime);
        let weak_owner = Arc::downgrade(&owner);
        drop(runtime);
        drop(owner);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!ready.load(Ordering::Acquire));
        assert!(weak.upgrade().is_some());
        assert_eq!(
            weak_owner.upgrade().unwrap().status().status,
            Status::Pending
        );
        assert!(paths.acquire_lock().is_err());
        drop(held);
        until(|| ready.load(Ordering::Acquire)).await;
        assert!(!paths.connection_path().exists());
        assert!(!paths.token_path().exists());
        assert_eq!(
            std::fs::read(path).unwrap(),
            b"artificial preserved content"
        );
    }
    #[tokio::test]
    async fn partial_startup_real_core_blocked_keeps_instance_and_never_allows_normal_exit() {
        let (_temp, paths) = RuntimePaths::for_test();
        let runtime = Arc::new(
            RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
                .await
                .unwrap(),
        );
        let mut foreign = paths.read_discovery().unwrap();
        foreign.instance_id = "00000000-0000-4000-8000-000000000099".into();
        let bytes = serde_json::to_vec(&foreign).unwrap();
        std::fs::write(paths.connection_path(), &bytes).unwrap();
        let ready = Arc::new(AtomicBool::new(false));
        let signal = ready.clone();
        let backend = Arc::new(CoreOnlyBackend::new(
            runtime.clone(),
            |_| {},
            move || {
                signal.store(true, Ordering::Release);
            },
        ));
        let owner = Arc::new(DesktopShutdown::default());
        owner.request(backend, Some(tokio::runtime::Handle::current()), || {
            Ok((Box::new(()), Box::new(())))
        });
        until(|| owner.status().status == Status::Blocked).await;
        let report = owner.status();
        assert!(report
            .issues
            .iter()
            .any(|i| !i.resource_closed && !i.retryable));
        owner.request(
            Arc::new(CoreOnlyBackend::new(
                runtime.clone(),
                |_| {},
                || panic!("blocked runtime cannot grant ready"),
            )),
            Some(tokio::runtime::Handle::current()),
            || panic!("sticky partial failure must not readmit"),
        );
        assert_eq!(owner.status(), report);
        let weak = Arc::downgrade(&runtime);
        let weak_owner = Arc::downgrade(&owner);
        drop(runtime);
        drop(owner);
        assert!(!ready.load(Ordering::Acquire));
        assert!(weak.upgrade().is_some());
        assert_eq!(
            weak_owner.upgrade().unwrap().status().status,
            Status::Blocked
        );
        assert!(paths.acquire_lock().is_err());
        assert_eq!(std::fs::read(paths.connection_path()).unwrap(), bytes);
        assert!(paths.token_path().exists());
    }
}
