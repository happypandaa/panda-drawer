//! Artificial Git protocol and real child/pipe/File regressions. These do not
//! prove public network compatibility and never touch user repositories.
use super::tests::{fake_git, final_task, fixture};
use super::*;
use crate::owned_process::ProcessPhase;

fn pipeline(paths: &RuntimePaths) -> PathBuf {
    fake_git(
        paths,
        &format!(
            r#"
op=''
for argument do
 case "$argument" in clone|rev-parse|fsck|ls-tree|cat-file|read-tree) op="$argument";; esac
 last="$argument"
done
case "$op" in
 clone) /bin/mkdir "$last";;
 rev-parse) printf '%s\n' '{oid}';;
 ls-tree) printf '100644 blob %s 6\tindex.html\000' '{oid}';;
 cat-file) while IFS= read -r oid; do printf '%s blob 6\nhello\n\n' "$oid"; done;;
 fsck|read-tree) exit 0;;
 *) exit 128;;
esac
"#,
            oid = "1".repeat(40)
        ),
    )
}
fn input() -> AcquireSourceInput {
    AcquireSourceInput {
        url: "https://github.com/example/disposable-owned-source".into(),
        branch: None,
        name: None,
    }
}
async fn pending(store: &ProjectStore) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if store.processes().snapshot().iter().any(|p| {
                p.component == "source-acquisition/materialize"
                    && p.phase == ProcessPhase::CleanupPending
            }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn source_progress(manager: &SourceAcquisitionManager, phase: &str) -> SourceCandidate {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let candidate = manager.list().unwrap().pop().unwrap();
            if candidate.phase == phase {
                return candidate;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn actual_source_pipeline_publishes_only_after_all_process_io_is_closed() {
    let (_temporary, paths, store, tasks, manager) = fixture();
    pipeline(&paths);
    let (stop, _) = watch::channel(false);
    let task = manager.submit(input(), stop.subscribe()).await.unwrap();
    let done = final_task(&tasks, &task.task_id).await;
    assert!(done.error.is_none(), "{:?}", done.error);
    manager.shutdown().await;
    let candidate = manager.list().unwrap().pop().unwrap();
    assert_eq!(candidate.status, "ready");
    assert_eq!(
        fs::read(candidate.source_path.unwrap().join("index.html")).unwrap(),
        b"hello\n"
    );
    assert!(manager.active.lock().unwrap().is_empty());
    assert!(!store.has_active_tasks());
    assert!(store.list().await.unwrap().is_empty());
    let states = store.processes().snapshot();
    assert!(states
        .iter()
        .any(|p| p.component == "source-acquisition/materialize"));
    assert!(states.iter().all(|p| p.phase == ProcessPhase::Closed
        && p.leader_reaped
        && p.group_exited
        && p.io_joined));
    assert!(store.processes().shutdown().await.is_empty());
    assert!(store
        .processes()
        .snapshot()
        .iter()
        .all(|p| p.resource_closed()));
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn cancellation_keeps_real_file_writer_source_busy_and_lease_until_actual_closed() {
    let (_temporary, paths, store, tasks, manager) = fixture();
    pipeline(&paths);
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    *manager.hooks.writer.lock().unwrap() = Some((entered, blocked));
    let (stop, _) = watch::channel(false);
    let task = manager.submit(input(), stop.subscribe()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    let before = manager.list().unwrap().pop().unwrap();
    let slot = before.evidence_path;
    let file = slot.join(".acquiring/repository/index.html");
    assert!(file.is_file());
    manager.cancel(&task.task_id).unwrap();
    pending(&store).await;
    let candidate = source_progress(&manager, "process-cleanup-pending").await;
    assert_eq!(candidate.status, "acquiring");
    assert!(!tasks.task(&task.task_id).unwrap().status.is_final());
    assert!(manager.active.lock().unwrap().contains_key(&task.task_id));
    assert!(store.has_active_tasks());
    assert!(!slot.join("repository").exists());
    assert!(candidate.source_path.is_none());
    assert_eq!(
        manager
            .submit(input(), stop.subscribe())
            .await
            .unwrap_err()
            .code,
        "SOURCE_ACQUISITION_BUSY"
    );
    let closing = manager.clone();
    let shutdown = tokio::spawn(async move { closing.shutdown().await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !shutdown.is_finished(),
        "manager must wait for the actual blocking file writer"
    );
    // Process cleanup may report Pending while its owned writer and G continue.
    let issues = store.processes().shutdown().await;
    assert!(issues.iter().any(|i| i.code == "PROCESS_CLEANUP_PENDING"));
    assert!(store.has_active_tasks());
    release.send(()).unwrap();
    let done = final_task(&tasks, &task.task_id).await;
    assert!(matches!(
        done.error.unwrap().code.as_str(),
        "SOURCE_ACQUISITION_CANCELLED" | "SOURCE_ACQUISITION_INTERRUPTED"
    ));
    assert!(tokio::time::timeout(Duration::from_secs(5), shutdown)
        .await
        .unwrap()
        .unwrap()
        .is_empty());
    assert!(!store.has_active_tasks());
    assert!(manager.active.lock().unwrap().is_empty());
    assert!(!slot.join("repository").exists());
    assert!(file.exists(), "partial candidate file is retained evidence");
    assert!(store.processes().shutdown().await.is_empty());
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn actual_materializer_panic_reports_unknown_and_retains_g_source_lease_and_slot() {
    let (_temporary, paths, store, tasks, manager) = fixture();
    pipeline(&paths);
    manager.hooks.panic_writer.store(true, Ordering::Release);
    let (stop, _) = watch::channel(false);
    let task = manager.submit(input(), stop.subscribe()).await.unwrap();
    let done = final_task(&tasks, &task.task_id).await;
    let error = done.error.unwrap();
    assert_eq!(error.details["operationOutcome"], "unknown");
    assert_eq!(error.details["processResourceClosed"], false);
    assert!(error.details.get("not_committed").is_none());
    assert!(
        store.has_active_tasks(),
        "registry G must hold the actual task lease"
    );
    assert!(manager.active.lock().unwrap().contains_key(&task.task_id));
    let candidate = manager.list().unwrap().pop().unwrap();
    assert_eq!(candidate.status, "failed");
    assert_eq!(candidate.phase, "process-unconfirmed");
    assert!(candidate.source_path.is_none());
    assert!(candidate
        .evidence_path
        .join(".acquiring/repository/index.html")
        .exists());
    assert!(!candidate.evidence_path.join("repository").exists());
    let retained = fs::read(candidate.evidence_path.join("source.json")).unwrap();
    assert_eq!(
        manager
            .submit(input(), stop.subscribe())
            .await
            .unwrap_err()
            .code,
        "SOURCE_ACQUISITION_BUSY"
    );
    assert!(
        manager.shutdown().await.is_empty(),
        "manager business runner may finish; registry retains resource uncertainty"
    );
    let issues = store.processes().shutdown().await;
    assert!(issues.iter().any(|i| i.code == "PROCESS_IO_JOIN_FAILED"));
    assert!(store
        .processes()
        .snapshot()
        .iter()
        .any(|p| p.phase == ProcessPhase::Unconfirmed && !p.resource_closed()));
    assert_eq!(
        fs::read(candidate.evidence_path.join("source.json")).unwrap(),
        retained
    );
    assert!(store.has_active_tasks());
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn lost_source_runner_does_not_drop_actual_writer_or_registry_scope() {
    let (_temporary, paths, store, tasks, manager) = fixture();
    pipeline(&paths);
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    *manager.hooks.writer.lock().unwrap() = Some((entered, blocked));
    let (stop, _) = watch::channel(false);
    let task = manager.submit(input(), stop.subscribe()).await.unwrap();
    ready.await.unwrap();
    let slot = manager.list().unwrap().pop().unwrap().evidence_path;
    // Deterministic fault injection into the real owned business runner. Keep
    // its JoinHandle registered so shutdown still observes this lost owner.
    manager.runners.lock().unwrap()[0].abort();
    pending(&store).await;
    assert!(store.has_active_tasks());
    assert!(manager.active.lock().unwrap().contains_key(&task.task_id));
    assert!(!slot.join("repository").exists());
    assert_eq!(
        manager
            .submit(input(), stop.subscribe())
            .await
            .unwrap_err()
            .code,
        "SOURCE_ACQUISITION_BUSY"
    );
    let issues = manager.shutdown().await;
    assert!(issues.iter().any(|i| i.code == "SHUTDOWN_JOIN_FAILED"));
    assert!(store
        .processes()
        .shutdown()
        .await
        .iter()
        .any(|i| i.code == "PROCESS_CLEANUP_PENDING"));
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while store.has_active_tasks() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(manager.active.lock().unwrap().is_empty());
    assert!(store.processes().shutdown().await.is_empty());
    assert!(slot.join(".acquiring/repository/index.html").exists());
    assert!(!slot.join("repository").exists());
    // The runner's absence cannot fabricate a business completion record.
    assert!(!tasks.task(&task.task_id).unwrap().status.is_final());
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn replaced_candidate_slot_is_not_published_or_written_by_error_cleanup() {
    let (_temporary, paths, store, tasks, manager) = fixture();
    pipeline(&paths);
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    *manager.hooks.writer.lock().unwrap() = Some((entered, blocked));
    let (stop, _) = watch::channel(false);
    let task = manager.submit(input(), stop.subscribe()).await.unwrap();
    ready.await.unwrap();
    let slot = manager.list().unwrap().pop().unwrap().evidence_path;
    let saved = slot.with_file_name(format!(
        "artificial-preserved-{}",
        slot.file_name().unwrap().to_str().unwrap()
    ));
    let original = fs::read(slot.join("source.json")).unwrap();
    fs::rename(&slot, &saved).unwrap();
    fs::create_dir(&slot).unwrap();
    let replacement = b"{\"untouched\":\"artificial replacement evidence\"}";
    fs::write(slot.join("source.json"), replacement).unwrap();
    release.send(()).unwrap();
    let done = final_task(&tasks, &task.task_id).await;
    let error = done.error.unwrap();
    assert_eq!(error.code, "SOURCE_SLOT_CHANGED");
    assert_eq!(error.details["operationOutcome"], "unknown");
    assert_eq!(error.details["candidateOwnershipConfirmed"], false);
    manager.shutdown().await;
    assert_eq!(fs::read(slot.join("source.json")).unwrap(), replacement);
    assert_eq!(fs::read(saved.join("source.json")).unwrap(), original);
    assert!(saved.join(".acquiring/repository/index.html").exists());
    assert!(!saved.join("repository").exists());
    assert!(!slot.join("repository").exists());
    assert!(store.processes().shutdown().await.is_empty());
    assert!(!store.has_active_tasks());
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn cancelled_version_resolution_keeps_source_scope_until_actual_probe_returns() {
    let (_temporary, paths, store, tasks, manager) = fixture();
    let marker = paths.data_root().join("artificial-probe-entered");
    let git = fake_git(&paths, "exit 128");
    fs::write(&git,format!("#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'entered\\n' > '{}'; /bin/sleep 0.3; printf 'git version 2.56.0\\n'; exit 0; fi\nexit 128\n",marker.display())).unwrap();
    let (stop, _) = watch::channel(false);
    let task = manager.submit(input(), stop.subscribe()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    manager.cancel(&task.task_id).unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!tasks.task(&task.task_id).unwrap().status.is_final());
    assert!(store.has_active_tasks());
    assert!(manager.active.lock().unwrap().contains_key(&task.task_id));
    let done = final_task(&tasks, &task.task_id).await;
    assert_eq!(done.error.unwrap().code, "SOURCE_ACQUISITION_CANCELLED");
    manager.shutdown().await;
    assert!(!store.has_active_tasks());
    let candidate = manager.list().unwrap().pop().unwrap();
    assert!(candidate.source_path.is_none());
    assert!(!candidate.evidence_path.join(".acquiring").exists());
    assert!(store.processes().shutdown().await.is_empty());
    tasks.shutdown().await;
    store.shutdown().await;
}
