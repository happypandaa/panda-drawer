//! Actual source-process and file-writer ownership. Import material is disposable
//! only after its writer closes; dropping a waiter never authorizes deletion.
use super::*;
use crate::{
    owned_process::{IoPlan, ProcessOptions, ProcessPhase, ProcessRun},
    projects::ProjectTaskLease,
};
#[cfg(test)]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

#[cfg(test)]
#[derive(Default)]
pub(super) struct SourceHooks {
    pub writer: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
    pub panic_writer: AtomicBool,
}

pub(super) struct SourceScope {
    _lease: ProjectTaskLease,
    tasks: Arc<BuildManager>,
    task: String,
    slot: PathBuf,
    anchor: fs::File,
}
impl SourceScope {
    pub fn new(
        lease: ProjectTaskLease,
        tasks: Arc<BuildManager>,
        task: String,
        slot: PathBuf,
        anchor: fs::File,
    ) -> Self {
        Self {
            _lease: lease,
            tasks,
            task,
            slot,
            anchor,
        }
    }
    pub fn check_identity(&self) -> Result<(), ApiError> {
        let changed = || {
            error(
                "SOURCE_SLOT_CHANGED",
                "源码候选目录身份发生变化，未发布；请保留原现场。",
            )
            .detail("candidateOwnershipConfirmed", false)
            .outcome("unknown")
        };
        check_no_symlinks(&self.slot, false).map_err(|_| changed())?;
        let current = fs::symlink_metadata(&self.slot).map_err(|_| changed())?;
        let owned = self.anchor.metadata().map_err(|_| changed())?;
        if !current.is_dir() || current.dev() != owned.dev() || current.ino() != owned.ino() {
            return Err(changed());
        }
        Ok(())
    }
    fn pending(&self, issue: &ApiError) {
        // Explicit progress reporting only. Drop never writes records or removes
        // files. A persistence failure cannot release the actual scope/lease.
        let _ = (|| -> Result<(), ApiError> {
            self.check_identity()?;
            let path = self.slot.join("source.json");
            let mut record: SourceRecord = serde_json::from_slice(&read_small(&path)?)?;
            if record.task_id != self.task || record.status != "acquiring" {
                return Err(invalid("源码获取状态已变化，保留关闭现场。"));
            }
            record.phase = "process-cleanup-pending".into();
            record.error = Some(issue.clone());
            atomic_json(&path, &record)?;
            self.tasks.phase(&self.task, "process-cleanup-pending")?;
            Ok(())
        })();
    }
}
pub(super) async fn resolve_source_git(
    environment: &GitEnvironmentManager,
    control: &RunControl,
) -> Result<GitToolchain, ApiError> {
    control.check()?;
    // Keep this distinct Arc so its last registry reference is observable without
    // counting unrelated clones of the source scope. Await resolution itself:
    // dropping its future could abandon managed verification's blocking I/O.
    let hold = Arc::new(control.scope.clone());
    let result = environment
        .with_process_registry(control.processes.clone())
        .resolve_with_keep_alive(hold.clone())
        .await;
    if let Err(issue) = &result {
        if issue.code == "PROCESS_CLEANUP_PENDING" {
            if let Some(scope) = &control.scope {
                scope.pending(issue);
            }
            while Arc::strong_count(&hold) > 1 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
    match result {
        Err(issue) if Arc::strong_count(&hold) > 1 => Err(issue
            .detail("processResourceClosed", false)
            .outcome("unknown")),
        result => {
            control.check()?;
            result.map_err(|issue| issue.detail("processResourceClosed", true))
        }
    }
}

async fn observed<T>(mut process: ProcessRun<T>, control: &mut RunControl) -> Result<T, ApiError> {
    // The borrowed reply is polled at most once to completion. Losing supervision
    // signals stop; it never drops the real child, readers or materializer.
    let mut replied = false;
    let reply = async {
        let result = (&mut process).await;
        replied = true;
        result
    };
    let supervised = supervise(control, reply).await;
    let (result, requested) = match supervised {
        Ok(output) => (Ok(output), None),
        Err(issue) if !replied => {
            process.cancel();
            ((&mut process).await, Some(issue))
        }
        Err(issue) => (Err(issue), None),
    };
    let mut reported = false;
    loop {
        let state = process.snapshot();
        match state.phase {
            ProcessPhase::Closed
                if state.leader_reaped
                    && state.group_exited
                    && state.io_joined
                    && state.issues.is_empty() =>
            {
                break
            }
            ProcessPhase::Unconfirmed => {
                let issue = result.err().unwrap_or_else(|| {
                    error(
                        "SOURCE_PROCESS_UNCONFIRMED",
                        "源码获取的进程或文件工作尚未确认关闭，已保留现场。",
                    )
                });
                return Err(issue
                    .detail("processId", state.id)
                    .detail("processResourceClosed", false)
                    .outcome("unknown"));
            }
            ProcessPhase::CleanupPending if !reported => {
                reported = true;
                if let Some(scope) = &control.scope {
                    scope.pending(
                        &error(
                            "PROCESS_CLEANUP_PENDING",
                            "源码获取仍在等待实际进程或文件工作结束，已保留占用和现场。",
                        )
                        .detail("processId", state.id)
                        .detail("processResourceClosed", false)
                        .outcome("unknown"),
                    );
                }
            }
            _ => (),
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Closed confirms leader/group/session/pumps. The shared registry separately
    // joins this final driver before granting its own resource-closed receipt.
    if let Some(issue) = requested {
        return Err(issue);
    }
    let output = result.map_err(|issue| {
        if matches!(
            issue.code.as_str(),
            "PROCESS_CANCELLED" | "PROCESS_WORK_TIMEOUT"
        ) {
            control.check().err().unwrap_or_else(process_failed)
        } else {
            issue
        }
        .detail("processResourceClosed", true)
    })?;
    if !output.status.success() {
        return Err(process_failed());
    }
    Ok(output.value)
}

pub(super) async fn execute(
    command: Command,
    control: &mut RunControl,
    maximum: usize,
) -> Result<Vec<u8>, ApiError> {
    control.check()?;
    let process = control.processes.start(
        command,
        ProcessOptions::new(
            "source-acquisition/execute",
            control.deadline.saturating_duration_since(Instant::now()),
        ),
        control.scope.clone(),
        move |pipes| {
            let crate::owned_process::ProcessPipes {
                stdin,
                stdout,
                stderr,
                ..
            } = pipes;
            drop(stdin);
            IoPlan::new(bounded(stdout, maximum, true)).drain("stderr", async move {
                bounded(stderr, MAX_OUTPUT, false).await.map(|_| ())
            })
        },
    )?;
    observed(process, control).await
}

pub(super) async fn materialize(
    tools: &GitToolchain,
    repository: &Path,
    home: &Path,
    template: &Path,
    entries: &[TreeEntry],
    control: &mut RunControl,
) -> Result<String, ApiError> {
    control.check()?;
    let mut command = isolated_command(tools, repository, home, template);
    command.args(["cat-file", "--batch"]);
    let repository = repository.to_path_buf();
    let entries = entries.to_vec();
    let writer_control = control.clone();
    let process = control.processes.start(
        command,
        ProcessOptions::new(
            "source-acquisition/materialize",
            control.deadline.saturating_duration_since(Instant::now()),
        ),
        control.scope.clone(),
        move |pipes| {
            let crate::owned_process::ProcessPipes {
                stdin,
                stdout,
                stderr,
                stop,
            } = pipes;
            IoPlan::new(async move {
                // This registered I/O session always waits for the real writer.
                // A blocking JoinError must become a registry-observed session
                // JoinError, rather than a harmless business failure receipt.
                match tokio::task::spawn_blocking(move || {
                    materialize_files(stdin, stdout, stop, repository, entries, writer_control)
                })
                .await
                {
                    Ok(result) => result,
                    Err(_) => panic!("source materializer did not join normally"),
                }
            })
            .drain("stderr", async move {
                bounded(stderr, MAX_OUTPUT, false).await.map(|_| ())
            })
        },
    )?;
    observed(process, control).await
}

fn materialize_files(
    mut input: tokio::io::DuplexStream,
    mut output: tokio::io::DuplexStream,
    stop: watch::Receiver<bool>,
    repository: PathBuf,
    entries: Vec<TreeEntry>,
    control: RunControl,
) -> Result<String, ApiError> {
    let handle = tokio::runtime::Handle::current();
    let check = || -> Result<(), ApiError> {
        control.check()?;
        if *stop.borrow() {
            return Err(cancelled());
        }
        Ok(())
    };
    let mut tree = Sha256::new();
    let mut buffer = vec![0u8; 65536];
    for entry in entries {
        check()?;
        handle
            .block_on(input.write_all(format!("{}\n", entry.oid).as_bytes()))
            .map_err(|_| process_failed())?;
        handle
            .block_on(input.flush())
            .map_err(|_| process_failed())?;
        let mut header = Vec::new();
        loop {
            check()?;
            if header.len() >= 160 {
                return Err(invalid("Git blob 响应头超限。"));
            }
            let byte = handle
                .block_on(output.read_u8())
                .map_err(|_| process_failed())?;
            if byte == b'\n' {
                break;
            }
            header.push(byte);
        }
        if header != format!("{} blob {}", entry.oid, entry.size).as_bytes() {
            return Err(invalid("Git blob 与已检查文件树不一致。"));
        }
        let path = repository.join(&entry.path);
        private_dir(path.parent().unwrap())?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(if entry.executable { 0o755 } else { 0o644 })
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        #[cfg(test)]
        {
            let pause = control.hooks.writer.lock().unwrap().take();
            if let Some((entered, release)) = pause {
                let _ = entered.send(());
                let _ = release.recv();
            }
            if control.hooks.panic_writer.load(Ordering::Acquire) {
                panic!("controlled artificial source materializer panic");
            }
        }
        let mut remaining = entry.size;
        let mut digest = Sha256::new();
        while remaining > 0 {
            check()?;
            let n = remaining.min(buffer.len() as u64) as usize;
            handle
                .block_on(output.read_exact(&mut buffer[..n]))
                .map_err(|_| process_failed())?;
            // Synchronous file writes/sync live in this owned blocking session;
            // no Tokio fs wrapper can outlive a dropped write future.
            file.write_all(&buffer[..n])?;
            digest.update(&buffer[..n]);
            remaining -= n as u64;
        }
        file.sync_all()?;
        check()?;
        if handle
            .block_on(output.read_u8())
            .map_err(|_| process_failed())?
            != b'\n'
        {
            return Err(invalid("Git blob 响应分隔无效。"));
        }
        tree.update(entry.path.as_bytes());
        tree.update([0]);
        tree.update(entry.size.to_le_bytes());
        tree.update([u8::from(entry.executable)]);
        tree.update(digest.finalize());
    }
    check()?;
    handle
        .block_on(input.shutdown())
        .map_err(|_| process_failed())?;
    drop(input);
    let mut trailing = [0u8; 1];
    if handle
        .block_on(output.read(&mut trailing))
        .map_err(|_| process_failed())?
        != 0
    {
        return Err(invalid("Git 返回了未请求的文件内容。"));
    }
    Ok(format!("{:x}", tree.finalize()))
}
