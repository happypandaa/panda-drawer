//! Windows Job/anonymous-pipe adaptation. The shared registry owns every pump,
//! session, drain and driver; only real handle observations authorize closure.
use super::{
    command::Environment, failure, group::ExitObservation, not_started, ErasedRecord, IoOwner,
    IoPlan, IoTask, OwnedCommand, OwnedProcessRegistry, ProcessOptions, ProcessPhase, ProcessPipes,
    ProcessRun, ProcessSnapshot, Record, Resources,
};
use crate::ApiError;
use platform_native::windows::process::{OwnedProcess as NativeProcess, Spawn};
use std::{
    io,
    os::windows::process::ExitStatusExt,
    process::ExitStatus,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    runtime::Handle,
    sync::{oneshot, watch},
};

type Shared = Arc<Mutex<Option<NativeProcess>>>;

#[cfg(test)]
std::thread_local! {
    static FAIL_NEXT_SETUP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(super) fn fail_next_setup_for_fixture() {
    FAIL_NEXT_SETUP.with(|failure| failure.set(true));
}

pub(super) struct Child {
    process: Shared,
}
impl Child {
    pub(super) fn wait(&mut self) -> io::Result<ExitStatus> {
        let mut slot = self.process.lock().unwrap_or_else(|p| p.into_inner());
        let process = slot
            .as_ref()
            .ok_or_else(|| io::Error::other("Created process owner is missing"))?;
        let exit = process
            .wait(Duration::ZERO)?
            .ok_or_else(|| io::Error::other("Owned root has not actually exited"))?;
        if process.active_processes()? != 0 {
            return Err(io::Error::other(
                "Owned Job still contains active processes",
            ));
        }
        // The real root and complete Job are now observed closed. Pipe handles
        // already belong to their separately joined threads, never this Drop.
        slot.take();
        Ok(ExitStatus::from_raw(exit))
    }
}

pub(super) struct ProcessGroup {
    process: Shared,
    armed: bool,
}
impl ProcessGroup {
    fn with_process<R>(
        &self,
        code: &str,
        action: impl FnOnce(&NativeProcess) -> io::Result<R>,
    ) -> Result<R, ApiError> {
        if !self.armed {
            return Err(failure(
                "PROCESS_OWNERSHIP_RELEASED",
                "Created process ownership has been released",
            ));
        }
        let slot = self.process.lock().unwrap_or_else(|p| p.into_inner());
        let owner = slot
            .as_ref()
            .ok_or_else(|| failure("PROCESS_CHILD_MISSING", "Created process owner is missing"))?;
        action(owner).map_err(|error| {
            failure(code, "Could not observe or stop the retained Windows Job")
                .detail("errno", serde_json::json!(error.raw_os_error()))
        })
    }

    pub(super) fn observe(&mut self) -> Result<Option<ExitObservation>, ApiError> {
        self.with_process("PROCESS_OBSERVATION_FAILED", NativeProcess::exit_code)
            .map(|exit| {
                exit.map(|code| ExitObservation {
                    code: i32::try_from(code).ok(),
                })
            })
    }

    pub(super) fn has_descendants(&mut self) -> Result<bool, ApiError> {
        self.with_process("PROCESS_GROUP_OBSERVATION_FAILED", |owner| {
            owner.active_processes().map(|count| count != 0)
        })
    }

    pub(super) fn request_stop(&mut self) -> Result<(), ApiError> {
        // The shared driver already broadcast the actual protocol stop watch.
        // Windows has no generic TERM. Keep the grace interval for that real
        // protocol request; this observation makes no termination/EOF claim.
        self.observe().map(|_| ())
    }

    pub(super) fn force_stop(&mut self) -> Result<(), ApiError> {
        self.with_process("PROCESS_SIGNAL_FAILED", |owner| owner.terminate(137))
    }

    pub(super) fn disarm_before_reap(&mut self) {
        self.armed = false;
    }
}

#[path = "windows/pipe_bridge.rs"]
pub(crate) mod pipe_bridge;
pub(super) use pipe_bridge::PipeThread;
use pipe_bridge::PreparedPumps;

impl PreparedPumps {
    fn register<T>(&mut self) -> Vec<IoOwner<T>> {
        self.take_workers()
            .into_iter()
            .map(|(name, handle)| IoOwner {
                name,
                handle: Some(IoTask::Pipe(handle)),
                joined: false,
            })
            .collect()
    }
}

pub(crate) fn request(command: OwnedCommand) -> Result<Spawn, ApiError> {
    let inherit = match command.environment {
        Environment::Inherit => true,
        Environment::Clear => false,
        Environment::Unspecified => {
            return Err(not_started(
                "PROCESS_ENVIRONMENT_UNSPECIFIED",
                "Native process environment policy must be declared before spawn",
            ))
        }
    };
    let command = command.command.as_std();
    let changes = command
        .get_envs()
        .map(|(key, value)| (key.to_owned(), value.map(ToOwned::to_owned)))
        .collect::<Vec<_>>();
    Ok(Spawn {
        program: command.get_program().into(),
        arguments: command.get_args().map(ToOwned::to_owned).collect(),
        current_directory: command
            .get_current_dir()
            .map(ToOwned::to_owned)
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)?,
        environment: platform_native::windows::process::resolve_environment(inherit, &changes)?,
    })
}

pub(super) fn start<T: Send + 'static, G: Send + 'static>(
    registry: &OwnedProcessRegistry,
    command: OwnedCommand,
    options: ProcessOptions,
    keep_alive: G,
    make_io: impl FnOnce(ProcessPipes) -> IoPlan<T> + Send + 'static,
) -> Result<ProcessRun<T>, ApiError> {
    let runtime = Handle::try_current().map_err(|_| {
        not_started(
            "PROCESS_EXECUTOR_UNAVAILABLE",
            "No executor can own the process drain",
        )
    })?;
    let mut state = registry.0.state.lock().unwrap_or_else(|p| p.into_inner());
    if state.sealed {
        return Err(not_started(
            "SERVICE_STOPPING",
            "Process admission is sealed",
        ));
    }
    #[cfg(test)]
    if let Some((entered, release)) = state.submission_pause.take() {
        entered.send(()).expect("fixture receives admission signal");
        release.recv().expect("fixture releases admission");
    }
    let request = request(command)?;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(tokio::time::sleep(Duration::ZERO));
    }))
    .map_err(|_| {
        not_started(
            "PROCESS_EXECUTOR_UNAVAILABLE",
            "No timer executor can own the process drain",
        )
    })?;
    let (cancel, stop) = watch::channel(false);
    let mut pumps = PreparedPumps::new(&runtime, stop)?;
    let mut writer_guard = registry
        .1
        .as_ref()
        .map(|owner| owner.begin(&options.component))
        .transpose()?;
    let mut owner = match NativeProcess::create_suspended(request) {
        Ok(owner) => owner,
        Err(_) => {
            if let Some(guard) = &mut writer_guard {
                guard.finish()?;
            }
            return Err(not_started(
                "PROCESS_SPAWN_FAILED",
                "Could not create the owned command",
            ));
        }
    };
    let mut setup_error = owner.assign_job().err().map(|_| {
        failure(
            "PROCESS_JOB_ASSIGN_FAILED",
            "Created process could not join its owned Job",
        )
    });
    #[cfg(test)]
    if FAIL_NEXT_SETUP.with(|failure| failure.replace(false)) {
        setup_error.get_or_insert_with(|| {
            failure(
                "PROCESS_FIXTURE_SETUP_FAILURE",
                "Controlled failure after real suspended creation and Job assignment",
            )
        });
    }
    let pid = owner.pid;
    let shared = Arc::new(Mutex::new(Some(owner)));
    let (reply, receiver) = oneshot::channel();
    let record = Arc::new(Record {
        registry: Arc::downgrade(&registry.0),
        meta: Mutex::new(ProcessSnapshot {
            id: uuid::Uuid::new_v4().to_string(),
            component: options.component.clone(),
            pid,
            phase: ProcessPhase::Running,
            leader_reaped: false,
            group_exited: false,
            io_joined: false,
            driver_joined: false,
            issues: vec![],
            resolved_issues: vec![],
        }),
        resources: tokio::sync::Mutex::new(Resources {
            child: Some(Child {
                process: shared.clone(),
            }),
            group: ProcessGroup {
                process: shared.clone(),
                armed: true,
            },
            io: pumps.register(),
            output: None,
            exit: None,
            keep_alive: Some(keep_alive),
            writer_guard,
        }),
        driver: tokio::sync::Mutex::new(None),
        reply: Mutex::new(Some(reply)),
        cancel,
        options,
    });
    let erased: Arc<dyn ErasedRecord> = record.clone();
    state.records.insert(record.snapshot().id, erased.clone());
    {
        let mut slot = shared.lock().unwrap_or_else(|p| p.into_inner());
        if let Err(error) = pumps.transfer(slot.as_mut().expect("new created owner")) {
            setup_error.get_or_insert(error);
        }
    }
    let pipes = pumps.take_pipes();
    let (started, start_result) = oneshot::channel();
    let driver_owner = record.clone();
    *record.driver.try_lock().expect("new driver slot") = Some(runtime.spawn(async move {
        match start_result.await {
            Ok(Ok(())) => driver_owner.run(pipes, make_io).await,
            Ok(Err(error)) => driver_owner.run_failed_start(pipes, error).await,
            Err(_) => {
                driver_owner
                    .run_failed_start(
                        pipes,
                        failure(
                            "PROCESS_START_RECEIPT_LOST",
                            "Created process start receipt was lost",
                        ),
                    )
                    .await
            }
        }
    }));
    if setup_error.is_none() {
        let mut slot = shared.lock().unwrap_or_else(|p| p.into_inner());
        if slot.as_mut().expect("new created owner").resume().is_err() {
            setup_error = Some(failure(
                "PROCESS_RESUME_FAILED",
                "Created process could not be resumed",
            ));
        }
    }
    if started.send(setup_error.map_or(Ok(()), Err)).is_err() {
        let slot = shared.lock().unwrap_or_else(|p| p.into_inner());
        let _ = slot
            .as_ref()
            .expect("created owner retained")
            .terminate(137);
        record.issue(
            "PROCESS_DRIVER_START_FAILED",
            "Created process driver was lost; owner retained",
            false,
        );
        record.update(|meta| meta.phase = ProcessPhase::Unconfirmed);
        record.send(Err(failure(
            "PROCESS_DRIVER_START_FAILED",
            "Created process driver was lost; owner retained",
        )));
    }
    drop(state);
    Ok(ProcessRun {
        reply: receiver,
        owner: erased,
        received: false,
    })
}
