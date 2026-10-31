//! Shared Windows pipe workers. Dropped consumers never replace actual EOF.
use crate::{
    owned_process::{failure, not_started, ProcessPipes},
    ApiError,
};
use platform_native::windows::process::OwnedProcess as NativeProcess;
use std::{
    fs::File,
    io::{self, Read, Write},
    sync::mpsc,
    thread::{self, JoinHandle},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, DuplexStream},
    runtime::Handle,
    sync::watch,
};

pub(crate) struct PipeThread {
    handle: Option<JoinHandle<io::Result<()>>>,
}
impl PipeThread {
    pub(crate) fn is_finished(&self) -> bool {
        self.handle.as_ref().is_some_and(JoinHandle::is_finished)
    }
    pub(crate) fn join(&mut self) -> Result<Result<(), ApiError>, ()> {
        let Some(handle) = self.handle.take() else {
            return Err(());
        };
        handle
            .join()
            .map(|value| value.map_err(Into::into))
            .map_err(|_| ())
    }
}

pub(crate) struct PreparedPumps {
    senders: Vec<Option<mpsc::Sender<File>>>,
    threads: Vec<(String, PipeThread)>,
    pipes: Option<ProcessPipes>,
}
impl PreparedPumps {
    pub(crate) fn new(runtime: &Handle, stop: watch::Receiver<bool>) -> Result<Self, ApiError> {
        let (stdin, input_pump) = tokio::io::duplex(64 * 1024);
        let (stdout, output_pump) = tokio::io::duplex(64 * 1024);
        let (stderr, errors_pump) = tokio::io::duplex(64 * 1024);
        let mut prepared = Self {
            senders: Vec::new(),
            threads: Vec::new(),
            pipes: Some(ProcessPipes {
                stdin,
                stdout,
                stderr,
                stop,
            }),
        };
        for (name, stream, input) in [
            ("stdin-pump", input_pump, true),
            ("stdout-pump", output_pump, false),
            ("stderr-pump", errors_pump, false),
        ] {
            let (file_sender, file_receiver) = mpsc::channel::<File>();
            let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
            let runtime = runtime.clone();
            let handle = thread::Builder::new()
                .name(format!("panda-owned-{name}"))
                .spawn(move || {
                    let _ = ready_sender.send(());
                    let Ok(file) = file_receiver.recv() else {
                        return Ok(());
                    };
                    if input {
                        pump_input(&runtime, stream, file)
                    } else {
                        pump_output(&runtime, file, stream)
                    }
                })
                .map_err(|_| {
                    not_started(
                        "PROCESS_STDIO_UNAVAILABLE",
                        "Could not create an owned pipe worker before spawn",
                    )
                })?;
            prepared.senders.push(Some(file_sender));
            prepared.threads.push((
                name.into(),
                PipeThread {
                    handle: Some(handle),
                },
            ));
            ready_receiver.recv().map_err(|_| {
                not_started(
                    "PROCESS_STDIO_UNAVAILABLE",
                    "Owned pipe worker did not become ready before spawn",
                )
            })?;
        }
        Ok(prepared)
    }

    pub(crate) fn take_workers(&mut self) -> Vec<(String, PipeThread)> {
        std::mem::take(&mut self.threads)
    }

    pub(crate) fn take_pipes(&mut self) -> ProcessPipes {
        self.pipes.take().expect("new prepared pipe facades")
    }

    pub(crate) fn transfer(&mut self, owner: &mut NativeProcess) -> Result<(), ApiError> {
        for (sender, slot) in
            self.senders
                .iter_mut()
                .zip([&mut owner.stdin, &mut owner.stdout, &mut owner.stderr])
        {
            let file = slot.take().ok_or_else(|| {
                failure(
                    "PROCESS_PIPE_MISSING",
                    "Created process pipe owner is missing",
                )
            })?;
            let sender = sender.take().ok_or_else(|| {
                failure(
                    "PROCESS_PIPE_MISSING",
                    "Created process pipe handoff is missing",
                )
            })?;
            if let Err(mpsc::SendError(file)) = sender.send(file) {
                // Keep an untransferred native pipe in the real process owner.
                // The original worker handle is still registered separately.
                *slot = Some(file);
                return Err(failure(
                    "PROCESS_PIPE_HANDOFF_FAILED",
                    "Created process pipe worker was lost",
                ));
            }
        }
        Ok(())
    }
}
impl Drop for PreparedPumps {
    fn drop(&mut self) {
        self.senders.clear();
        // Before process creation, ready workers only wait for their channel.
        // Once a child exists their handles have already moved to the registry.
        for (_, mut worker) in self.threads.drain(..) {
            if let Some(handle) = worker.handle.take() {
                let _ = handle.join();
            }
        }
    }
}

fn pump_input(runtime: &Handle, mut producer: DuplexStream, mut file: File) -> io::Result<()> {
    let mut bytes = [0u8; 8192];
    loop {
        let count = runtime.block_on(producer.read(&mut bytes))?;
        if count == 0 {
            return Ok(()); // Dropping the actual File sends real stdin EOF.
        }
        match file.write_all(&bytes[..count]) {
            Ok(()) => (),
            // The peer may exit or close stdin while bytes are queued. This
            // closes only this actual pipe worker; process/Job/other readers
            // still require their separate observations and real joins.
            Err(error) if input_disconnected(&error) => return Ok(()),
            Err(error) => return Err(error),
        }
    }
}

fn input_disconnected(error: &io::Error) -> bool {
    // WriteFile documents ERROR_BROKEN_PIPE for a closed anonymous-pipe read
    // handle. WinError also defines 232 as a closing pipe and 233 as no peer:
    // https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-writefile
    // https://learn.microsoft.com/windows/win32/debug/system-error-codes--0-499-
    error.kind() == io::ErrorKind::BrokenPipe
        || matches!(error.raw_os_error(), Some(109 | 232 | 233))
}

fn pump_output(runtime: &Handle, mut file: File, mut consumer: DuplexStream) -> io::Result<()> {
    let mut bytes = [0u8; 8192];
    let mut consuming = true;
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            return Ok(());
        }
        if consuming
            && runtime
                .block_on(consumer.write_all(&bytes[..count]))
                .is_err()
        {
            consuming = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_stdin_peer_disconnect_is_an_expected_write_termination() {
        for code in [109, 232, 233] {
            assert!(input_disconnected(&io::Error::from_raw_os_error(code)));
        }
        assert!(input_disconnected(&io::Error::from(
            io::ErrorKind::BrokenPipe
        )));
        for code in [5, 6, 13, 29, 87, 995] {
            assert!(!input_disconnected(&io::Error::from_raw_os_error(code)));
        }
        for kind in [
            io::ErrorKind::WriteZero,
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::Other,
        ] {
            assert!(!input_disconnected(&io::Error::from(kind)));
        }
    }
}
