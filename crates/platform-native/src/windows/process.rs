//! Suspended creation, Job ownership and anonymous stdio pipes. No shell,
//! Ctrl+Break, persisted-PID signalling or detached process ownership is used.
use super::{checked, native_path, wide};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    time::{Duration, SystemTime},
};
use windows_sys::Win32::{
    Foundation::{
        GetLastError, SetHandleInformation, ERROR_INVALID_PARAMETER, FILETIME, HANDLE,
        HANDLE_FLAG_INHERIT, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Security::SECURITY_ATTRIBUTES,
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
            TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Pipes::CreatePipe,
        SystemInformation::GetTickCount64,
        Threading::{
            CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess, GetProcessTimes,
            InitializeProcThreadAttributeList, OpenProcess, QueryFullProcessImageNameW,
            ResumeThread, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
            CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
            EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESTDHANDLES, STARTUPINFOEXW,
        },
    },
};

mod environment;

/// Resolve only the caller's explicitly declared inheritance policy and
/// overrides. This never changes the platform process's own environment.
pub fn resolve_environment(
    inherit: bool,
    changes: &[(OsString, Option<OsString>)],
) -> io::Result<Vec<(OsString, OsString)>> {
    environment::resolve(inherit, changes)
}

fn owned(raw: HANDLE) -> io::Result<OwnedHandle> {
    if raw.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
}
struct Pipes {
    parent: File,
    child: OwnedHandle,
}
fn pipe(parent_writes: bool) -> io::Result<Pipes> {
    let mut read = null_mut();
    let mut write = null_mut();
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    unsafe {
        checked(CreatePipe(&mut read, &mut write, &security, 0))?;
    }
    let read = owned(read)?;
    let write = owned(write)?;
    let (parent, child) = if parent_writes {
        (write, read)
    } else {
        (read, write)
    };
    unsafe {
        checked(SetHandleInformation(
            parent.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            0,
        ))?;
    }
    Ok(Pipes {
        parent: File::from(parent),
        child,
    })
}

struct Attributes {
    storage: Vec<usize>,
    initialized: bool,
}
impl Attributes {
    fn new(handles: &[HANDLE]) -> io::Result<Self> {
        let mut length = 0;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut length);
        }
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut value = Self {
            storage: vec![0usize; length.div_ceil(size_of::<usize>())],
            initialized: false,
        };
        unsafe {
            checked(InitializeProcThreadAttributeList(
                value.pointer(),
                1,
                0,
                &mut length,
            ))?;
            value.initialized = true;
            checked(UpdateProcThreadAttribute(
                value.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                size_of_val(handles),
                null_mut(),
                null(),
            ))?;
        }
        Ok(value)
    }
    fn pointer(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                DeleteProcThreadAttributeList(self.pointer());
            }
        }
    }
}

fn quoted(argument: &OsStr) -> io::Result<Vec<u16>> {
    let bytes = wide(argument)?;
    let mut result = vec![34];
    let mut slashes = 0;
    for value in bytes.into_iter().take_while(|value| *value != 0) {
        if value == 92 {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n(
            92,
            if value == 34 {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        slashes = 0;
        result.push(value);
    }
    result.extend(std::iter::repeat_n(92, slashes * 2));
    result.push(34);
    Ok(result)
}
fn command_line(program: &Path, arguments: &[OsString]) -> io::Result<Vec<u16>> {
    let mut result = quoted(program.as_os_str())?;
    for argument in arguments {
        result.push(32);
        result.extend(quoted(argument)?);
    }
    result.push(0);
    Ok(result)
}
pub struct Spawn {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub current_directory: PathBuf,
    /// The caller supplies its existing isolated environment policy.
    pub environment: Vec<(OsString, OsString)>,
}
pub struct OwnedProcess {
    // Field order matters: closing the last Job handle terminates descendants
    // before the remaining root/pipe handles are released.
    job: OwnedHandle,
    process: OwnedHandle,
    thread: Option<OwnedHandle>,
    job_assigned: bool,
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
    pub pid: u32,
}

/// A process was created, so the error retains its actual resource owner. An
/// error message, termination request or timeout is never a closure receipt.
/// New runtime callers use the staged methods and keep the owner directly.
pub struct SpawnSetupFailure {
    cause: io::Error,
    owner: OwnedProcess,
}
impl SpawnSetupFailure {
    pub fn into_parts(self) -> (io::Error, OwnedProcess) {
        (self.cause, self.owner)
    }
    pub fn owner(&self) -> &OwnedProcess {
        &self.owner
    }
}
impl std::fmt::Debug for SpawnSetupFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpawnSetupFailure")
            .field("cause", &self.cause)
            .field("pid", &self.owner.pid)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for SpawnSetupFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Created process setup failed; resource closure outcome is unknown: {}",
            self.cause
        )
    }
}
impl std::error::Error for SpawnSetupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        // An assignment failure can leave a never-resumed root outside the Job.
        // This best-effort request uses only the retained creation handle. Drop
        // is not a wait, a joined-I/O receipt or permission to clean its files.
        if !self.job_assigned {
            unsafe {
                TerminateProcess(self.process.as_raw_handle(), 1);
            }
        }
    }
}
impl OwnedProcess {
    pub fn spawn(request: Spawn) -> io::Result<Self> {
        let mut process = Self::create_suspended(request)?;
        if let Err(cause) = process.assign_job().and_then(|()| process.resume()) {
            return Err(process.setup_failure(cause));
        }
        Ok(process)
    }

    fn setup_failure(self, cause: io::Error) -> io::Error {
        // Preserve the child even when termination itself fails. Callers can
        // recover this owner from the error; no ignored wait can turn this
        // into the pre-CreateProcess failure case.
        let _ = self.terminate(1);
        io::Error::other(SpawnSetupFailure { cause, owner: self })
    }

    /// Create a root suspended with all of its native owners retained. Every
    /// Err from this method precedes successful CreateProcessW. The caller must
    /// assign the Job, register its actual owner/pumps, then resume explicitly.
    pub fn create_suspended(request: Spawn) -> io::Result<Self> {
        let program = native_path(&request.program)?;
        // A filesystem verbatim prefix changes root-relative paths inside a
        // child (for example /etc becomes a UNC path). Match std's cwd rule;
        // the executable still uses the file-operation path representation.
        let directory = super::normalized_path(&request.current_directory)?;
        let directory = super::api_paths::conventional_path(&directory)?;
        let directory = wide(directory.as_os_str())?;
        let mut arguments = command_line(&request.program, &request.arguments)?;
        let environment = environment::block(&request.environment)?;
        let input = pipe(true)?;
        let output = pipe(false)?;
        let errors = pipe(false)?;
        let job = owned(unsafe { CreateJobObjectW(null(), null()) })?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            checked(SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&limits) as u32,
            ))?;
        }
        let handles = [
            input.child.as_raw_handle(),
            output.child.as_raw_handle(),
            errors.child.as_raw_handle(),
        ];
        let mut attributes = Attributes::new(&handles)?;
        let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = handles[0];
        startup.StartupInfo.hStdOutput = handles[1];
        startup.StartupInfo.hStdError = handles[2];
        startup.lpAttributeList = attributes.pointer();
        let mut created: PROCESS_INFORMATION = unsafe { zeroed() };
        unsafe {
            checked(CreateProcessW(
                program.as_ptr(),
                arguments.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_SUSPENDED
                    | CREATE_UNICODE_ENVIRONMENT
                    | EXTENDED_STARTUPINFO_PRESENT
                    | CREATE_NO_WINDOW,
                environment.as_ptr().cast(),
                directory.as_ptr(),
                &startup.StartupInfo,
                &mut created,
            ))?;
        }
        // Successful CreateProcessW returns valid process and primary-thread
        // handles. From this point there is no fallible setup before the full
        // owner is returned, so a created root cannot be mistaken for no child.
        let process = unsafe { OwnedHandle::from_raw_handle(created.hProcess) };
        let thread = unsafe { OwnedHandle::from_raw_handle(created.hThread) };
        // Only the child owns these ends after CreateProcess. Keeping one parent
        // copy would prevent EOF and could leave deletion waiting forever.
        drop(input.child);
        drop(output.child);
        drop(errors.child);
        Ok(Self {
            job,
            process,
            thread: Some(thread),
            job_assigned: false,
            stdin: Some(input.parent),
            stdout: Some(output.parent),
            stderr: Some(errors.parent),
            pid: created.dwProcessId,
        })
    }

    pub fn assign_job(&mut self) -> io::Result<()> {
        if self.job_assigned {
            return Ok(());
        }
        unsafe {
            checked(AssignProcessToJobObject(
                self.job.as_raw_handle(),
                self.process.as_raw_handle(),
            ))?;
        }
        self.job_assigned = true;
        Ok(())
    }

    /// Failure leaves the actual suspended owner in the caller's custody.
    pub fn resume(&mut self) -> io::Result<()> {
        if !self.job_assigned {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cannot resume a process before assigning its owned Job",
            ));
        }
        let thread = self.thread.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Process was already resumed")
        })?;
        let previous = unsafe { ResumeThread(thread.as_raw_handle()) };
        if previous == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        if previous != 1 {
            return Err(io::Error::other(
                "Owned primary thread did not have its original suspend count",
            ));
        }
        self.thread.take();
        Ok(())
    }
    pub fn identity(&self) -> io::Result<ProcessIdentity> {
        read_identity(self.pid, &self.process)
    }
    pub fn exit_code(&self) -> io::Result<Option<u32>> {
        exit_code(&self.process)
    }
    pub fn wait(&self, timeout: Duration) -> io::Result<Option<u32>> {
        let milliseconds = timeout.as_millis().min((u32::MAX - 1) as u128) as u32;
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), milliseconds) } {
            WAIT_OBJECT_0 => self.exit_code(),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }
    /// The caller first writes its normal protocol shutdown message and waits.
    /// After that grace period this kills the complete owned Job, never a PID.
    pub fn terminate(&self, exit: u32) -> io::Result<()> {
        unsafe {
            if self.job_assigned {
                checked(TerminateJobObject(self.job.as_raw_handle(), exit))
            } else {
                // The only unassigned root is still suspended. Its real
                // creation handle authorizes this request, never a saved PID.
                checked(TerminateProcess(self.process.as_raw_handle(), exit))
            }
        }
    }
    pub fn active_processes(&self) -> io::Result<u32> {
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
        unsafe {
            checked(QueryInformationJobObject(
                self.job.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of_val(&info) as u32,
                null_mut(),
            ))?;
        }
        Ok(info.ActiveProcesses)
    }
    pub fn wait_empty(&self, timeout: Duration) -> io::Result<bool> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.active_processes()? == 0 {
                return Ok(true);
            }
            if std::time::Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
fn exit_code(process: &OwnedHandle) -> io::Result<Option<u32>> {
    match unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } {
        WAIT_TIMEOUT => return Ok(None),
        WAIT_OBJECT_0 => (),
        _ => return Err(io::Error::last_os_error()),
    }
    let mut exit = 0;
    unsafe {
        checked(GetExitCodeProcess(process.as_raw_handle(), &mut exit))?;
    }
    // Checking the handle's signalled state above distinguishes a live process
    // from a process that actually exited with STILL_ACTIVE (259).
    Ok(Some(exit))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub creation_100ns: u64,
    pub executable: PathBuf,
}
fn read_identity(pid: u32, handle: &OwnedHandle) -> io::Result<ProcessIdentity> {
    let mut created: FILETIME = unsafe { zeroed() };
    let mut exited: FILETIME = unsafe { zeroed() };
    let mut kernel: FILETIME = unsafe { zeroed() };
    let mut user: FILETIME = unsafe { zeroed() };
    unsafe {
        checked(GetProcessTimes(
            handle.as_raw_handle(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        ))?;
    }
    let mut path = vec![0u16; 32768];
    let mut length = path.len() as u32;
    unsafe {
        checked(QueryFullProcessImageNameW(
            handle.as_raw_handle(),
            0,
            path.as_mut_ptr(),
            &mut length,
        ))?;
    }
    path.truncate(length as usize);
    Ok(ProcessIdentity {
        pid,
        creation_100ns: (u64::from(created.dwHighDateTime) << 32)
            | u64::from(created.dwLowDateTime),
        executable: PathBuf::from(OsString::from_wide(&path)),
    })
}
fn open_observable(pid: u32) -> io::Result<Option<OwnedHandle>> {
    if pid <= 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid process identity",
        ));
    }
    let raw = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if raw.is_null() {
        return if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER {
            Ok(None)
        } else {
            Err(io::Error::last_os_error())
        };
    }
    Ok(Some(owned(raw)?))
}

/// Observation only. The caller must already own and retain a direct child to
/// use this as a capture; opening a PID never grants ownership or termination.
pub fn capture_live(pid: u32) -> io::Result<Option<ProcessIdentity>> {
    let Some(handle) = open_observable(pid)? else {
        return Ok(None);
    };
    if exit_code(&handle)?.is_some() {
        return Ok(None);
    }
    let identity = read_identity(pid, &handle)?;
    if exit_code(&handle)?.is_some() {
        return Ok(None);
    }
    Ok(Some(identity))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessObservation {
    Matching,
    Stopped,
    Different,
}

pub fn observation(identity: &ProcessIdentity) -> io::Result<ProcessObservation> {
    let Some(handle) = open_observable(identity.pid)? else {
        return Ok(ProcessObservation::Stopped);
    };
    if exit_code(&handle)?.is_some() {
        return Ok(ProcessObservation::Stopped);
    }
    Ok(if read_identity(identity.pid, &handle)? == *identity {
        ProcessObservation::Matching
    } else {
        ProcessObservation::Different
    })
}

pub fn observe(identity: &ProcessIdentity) -> io::Result<bool> {
    Ok(observation(identity)? == ProcessObservation::Matching)
}
/// This follows the port standard's uptime observation; callers must allow for
/// timer granularity and must not turn this estimate into a fabricated UUID.
pub fn boot_time() -> io::Result<SystemTime> {
    SystemTime::now()
        .checked_sub(Duration::from_millis(unsafe { GetTickCount64() }))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid boot time observation"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        os::windows::ffi::OsStrExt,
    };
    fn fixture_arguments() -> [&'static str; 4] {
        [
            "--ignored",
            "--exact",
            "windows::process::tests::child_fixture",
            "--nocapture",
        ]
    }
    fn fixture_command() -> std::process::Command {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        // Piped libtest fixtures have no terminal. Do not search host terminfo
        // locations before entering the fixture. Product inheritance is intact.
        command.args(fixture_arguments()).env_remove("TERM");
        command
    }
    fn raw_directory_observations() -> (Vec<u16>, Vec<u16>) {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentDirectoryW(length: u32, buffer: *mut u16) -> u32;
        }
        let mut current = vec![0; 32768];
        let count = unsafe { GetCurrentDirectoryW(current.len() as u32, current.as_mut_ptr()) };
        assert!(count != 0 && (count as usize) < current.len());
        current.truncate(count as usize);
        let input = wide(OsStr::new("/panda-disposable-root-relative")).unwrap();
        let mut absolute = vec![0; 32768];
        let count = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetFullPathNameW(
                input.as_ptr(),
                absolute.len() as u32,
                absolute.as_mut_ptr(),
                null_mut(),
            )
        };
        assert!(count != 0 && (count as usize) < absolute.len());
        absolute.truncate(count as usize);
        (current, absolute)
    }
    fn request(mode: &str, directory: &Path) -> Spawn {
        let mut environment: Vec<_> = std::env::vars_os()
            .filter(|(name, _)| {
                name != "PANDA_NATIVE_FIXTURE"
                    && !name.to_string_lossy().eq_ignore_ascii_case("TERM")
            })
            .collect();
        environment.push(("PANDA_NATIVE_FIXTURE".into(), mode.into()));
        Spawn {
            program: std::env::current_exe().unwrap(),
            arguments: fixture_arguments()
                .into_iter()
                .map(OsString::from)
                .collect(),
            current_directory: directory.into(),
            environment,
        }
    }
    #[test]
    #[ignore = "entry point for disposable native child-process fixtures"]
    fn child_fixture() {
        match std::env::var("PANDA_NATIVE_FIXTURE").unwrap().as_str() {
            "streams" => {
                let mut error = std::io::stderr().lock();
                error.write_all(&vec![b'e'; 200_000]).unwrap();
                error.flush().unwrap();
                drop(error);
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).unwrap();
                println!("ECHO:{}", line.trim());
            }
            "family" => {
                let mut child = fixture_command()
                    .env("PANDA_NATIVE_FIXTURE", "sleep")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap();
                println!("DESCENDANT:{}", child.id());
                std::io::stdout().flush().unwrap();
                child.wait().unwrap();
            }
            "host" => {
                let mut child =
                    OwnedProcess::spawn(request("family", &std::env::current_dir().unwrap()))
                        .unwrap();
                let mut reader = BufReader::new(child.stdout.take().unwrap());
                let descendant = loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if let Some(pid) = line.trim().strip_prefix("DESCENDANT:") {
                        break pid.parse::<u32>().unwrap();
                    }
                };
                println!("HOST_ROOT:{}", child.pid);
                println!("HOST_DESCENDANT:{descendant}");
                std::io::stdout().flush().unwrap();
                loop {
                    std::thread::sleep(Duration::from_secs(60));
                }
            }
            "sleep" => loop {
                std::thread::sleep(Duration::from_secs(60));
            },
            "exit259" => std::process::exit(259),
            "raw-cwd" => {
                let (directory, absolute) = raw_directory_observations();
                println!("PANDA_RAW_CWD:{directory:?}");
                println!("PANDA_RAW_ROOT_RELATIVE:{absolute:?}");
                std::process::exit(0);
            }
            "raw-env" => {
                let keys = environment::non_ascii_fixture_values();
                // Only synthetic fixture keys are emitted, even if this entry
                // point is accidentally invoked outside the cleared test plan.
                for (key, value) in std::env::vars_os()
                    .filter(|(key, _)| keys.iter().any(|(expected, _)| expected == key))
                {
                    println!(
                        "PANDA_ENV:{:?}={:?}",
                        key.encode_wide().collect::<Vec<_>>(),
                        value.encode_wide().collect::<Vec<_>>()
                    );
                }
                std::process::exit(0);
            }
            "marker" => {
                std::fs::write(std::env::var_os("PANDA_NATIVE_MARKER").unwrap(), b"ran").unwrap();
            }
            _ => panic!("Unknown disposable fixture mode"),
        }
    }

    #[test]
    fn native_child_cwd_matches_std_raw_resolution_and_long_path_outcomes() {
        let root = tempfile::Builder::new()
            .prefix("panda-cwd-中文 space-")
            .tempdir()
            .unwrap();
        let mut directory = root.path().to_path_buf();
        for _ in 0..5 {
            directory.push("中文 child directory with spaces and a sufficiently long name");
        }
        std::fs::create_dir_all(&directory).unwrap();
        assert!(directory.as_os_str().encode_wide().count() > 260);
        let mut successful_short_directories = 0;
        for selected in [
            root.path().to_path_buf(),
            root.path().canonicalize().unwrap(),
            directory.clone(),
            directory.canonicalize().unwrap(),
        ] {
            let expected = fixture_command()
                .env("PANDA_NATIVE_FIXTURE", "raw-cwd")
                .current_dir(&selected)
                .output();
            let actual = OwnedProcess::create_suspended(request("raw-cwd", &selected));
            let mut created = match (expected, actual) {
                (Err(expected), Err(actual)) => {
                    // Some Windows hosts reject long cwd values even though
                    // ordinary file operations support the same directory.
                    // Prove the same pre-create OS outcome; do not add a limit
                    // or call this a successful long-directory process test.
                    assert!(selected.as_os_str().encode_wide().count() > 260);
                    assert!(matches!(expected.raw_os_error(), Some(206 | 267)));
                    assert_eq!(actual.raw_os_error(), expected.raw_os_error());
                    eprintln!("actual long cwd unsupported equally by std/native: {actual}");
                    continue;
                }
                (Ok(expected), Ok(mut process)) => {
                    assert!(expected.status.success(), "{expected:?}");
                    process.assign_job().unwrap();
                    process.resume().unwrap();
                    (expected, process)
                }
                (expected, Err(actual)) => {
                    panic!("std/native cwd creation differed: std={expected:?}, native={actual}")
                }
                (Err(expected), Ok(process)) => {
                    // Even an unexpected success retains the real suspended
                    // owner through actual termination and Job/root receipts.
                    process.terminate(137).unwrap();
                    assert_eq!(process.wait(Duration::from_secs(5)).unwrap(), Some(137));
                    assert!(process.wait_empty(Duration::from_secs(5)).unwrap());
                    panic!("std rejected cwd but native created a child: {expected}")
                }
            };
            let (expected, ref mut process) = created;
            drop(process.stdin.take());
            let mut stdout = process.stdout.take().unwrap();
            let output = std::thread::spawn(move || {
                let mut bytes = Vec::new();
                stdout.read_to_end(&mut bytes).unwrap();
                bytes
            });
            let mut stderr = process.stderr.take().unwrap();
            let errors = std::thread::spawn(move || {
                let mut bytes = Vec::new();
                stderr.read_to_end(&mut bytes).unwrap();
                bytes
            });
            assert_eq!(process.wait(Duration::from_secs(5)).unwrap(), Some(0));
            assert!(process.wait_empty(Duration::from_secs(5)).unwrap());
            assert!(errors.join().unwrap().is_empty());
            let actual = String::from_utf8(output.join().unwrap()).unwrap();
            let expected = String::from_utf8(expected.stdout).unwrap();
            for marker in ["PANDA_RAW_CWD:", "PANDA_RAW_ROOT_RELATIVE:"] {
                let observation = |value: &str| {
                    value
                        .lines()
                        .find_map(|line| line.find(marker).map(|start| line[start..].to_owned()))
                        .expect("child returned the actual Win32 path observation")
                };
                assert_eq!(observation(&actual), observation(&expected), "{selected:?}");
            }
            if selected.as_os_str().encode_wide().count() <= 260 {
                successful_short_directories += 1;
            }
        }
        assert_eq!(successful_short_directories, 2);
    }

    #[test]
    fn native_child_environment_matches_std_for_hidden_and_non_ascii_utf16_entries() {
        let directory = tempfile::Builder::new()
            .prefix("panda-disposable-env-中文 space-")
            .tempdir()
            .unwrap();
        let values = environment::non_ascii_fixture_values();
        let expected = fixture_command()
            .env_clear()
            .envs(values.iter().map(|(key, value)| (key, value)))
            .env("PANDA_NATIVE_FIXTURE", "raw-env")
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(expected.status.success(), "{expected:?}");
        let mut plan = request("raw-env", directory.path());
        plan.environment = values.clone();
        plan.environment
            .push(("PANDA_NATIVE_FIXTURE".into(), "raw-env".into()));
        let mut process = OwnedProcess::spawn(plan).unwrap();
        drop(process.stdin.take());
        let mut stdout = process.stdout.take().unwrap();
        let output = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let mut stderr = process.stderr.take().unwrap();
        let errors = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).unwrap();
            bytes
        });
        assert_eq!(process.wait(Duration::from_secs(5)).unwrap(), Some(0));
        assert!(process.wait_empty(Duration::from_secs(5)).unwrap());
        assert!(errors.join().unwrap().is_empty());
        let observations = |bytes| {
            String::from_utf8(bytes)
                .unwrap()
                .lines()
                .filter_map(|line| {
                    line.find("PANDA_ENV:")
                        .map(|start| line[start..].to_owned())
                })
                .collect::<Vec<_>>()
        };
        let expected = observations(expected.stdout);
        let actual = observations(output.join().unwrap());
        assert_eq!(expected.len(), values.len());
        assert_eq!(actual, expected);
    }

    #[test]
    fn suspended_owner_is_registered_before_any_child_code_and_then_resumes() {
        let temp = tempfile::Builder::new()
            .prefix("panda-staged-中文 space-")
            .tempdir()
            .unwrap();
        let directory = temp.path().canonicalize().unwrap();
        let marker = directory.join("child-ran");
        let mut plan = request("marker", &directory);
        plan.environment
            .push(("PANDA_NATIVE_MARKER".into(), marker.clone().into()));
        let mut process = OwnedProcess::create_suspended(plan).unwrap();
        assert!(!marker.exists());
        assert_eq!(process.wait(Duration::from_millis(20)).unwrap(), None);
        assert_eq!(process.active_processes().unwrap(), 0);
        assert!(!marker.exists());
        assert!(process.resume().is_err());
        assert_eq!(process.exit_code().unwrap(), None);
        process.assign_job().unwrap();
        assert!(process.active_processes().unwrap() >= 1);
        process.resume().unwrap();
        drop(process.stdin.take());
        assert_eq!(process.wait(Duration::from_secs(5)).unwrap(), Some(0));
        assert_eq!(std::fs::read(marker).unwrap(), b"ran");
        assert!(process.wait_empty(Duration::from_secs(5)).unwrap());
    }

    #[test]
    fn post_creation_setup_error_keeps_the_real_owner_until_explicit_receipts() {
        let temp = tempfile::Builder::new()
            .prefix("panda-staged-failure-")
            .tempdir()
            .unwrap();
        let directory = temp.path().canonicalize().unwrap();
        let mut process = OwnedProcess::create_suspended(request("sleep", &directory)).unwrap();
        let identity = process.identity().unwrap();
        let cause = process.resume().unwrap_err();
        assert!(observe(&identity).unwrap());
        let failure = process.setup_failure(cause);
        assert!(failure.to_string().contains("outcome is unknown"));
        let failure = failure
            .into_inner()
            .unwrap()
            .downcast::<SpawnSetupFailure>()
            .unwrap();
        assert_eq!(failure.owner().pid, identity.pid);
        let (_, mut process) = failure.into_parts();
        drop(process.stdin.take());
        let mut output = Vec::new();
        process
            .stdout
            .take()
            .unwrap()
            .read_to_end(&mut output)
            .unwrap();
        let mut errors = Vec::new();
        process
            .stderr
            .take()
            .unwrap()
            .read_to_end(&mut errors)
            .unwrap();
        assert_eq!(process.wait(Duration::from_secs(5)).unwrap(), Some(1));
        assert!(process.wait_empty(Duration::from_secs(5)).unwrap());
        assert!(!observe(&identity).unwrap());
        assert!(output.is_empty());
        assert!(errors.is_empty());

        let mut missing = request("sleep", &directory);
        missing.program = directory.join("does-not-exist.exe");
        let error = OwnedProcess::create_suspended(missing).err().unwrap();
        assert!(error
            .get_ref()
            .and_then(|value| value.downcast_ref::<SpawnSetupFailure>())
            .is_none());
    }

    #[test]
    fn observed_exit_259_is_not_reported_as_still_running() {
        let temp = tempfile::Builder::new()
            .prefix("panda-exit259-")
            .tempdir()
            .unwrap();
        let process =
            OwnedProcess::spawn(request("exit259", &temp.path().canonicalize().unwrap())).unwrap();
        assert_eq!(process.wait(Duration::from_secs(5)).unwrap(), Some(259));
        assert_eq!(process.exit_code().unwrap(), Some(259));
        assert!(process.wait_empty(Duration::from_secs(5)).unwrap());
    }

    #[test]
    #[ignore = "requires PANDA_NATIVE_TEST_NODE pointing to the verified official Windows Node"]
    fn official_node_runs_only_after_the_staged_owner_is_ready() {
        let node = std::env::var_os("PANDA_NATIVE_TEST_NODE")
            .expect("explicit verified official Node path is required");
        let temp = tempfile::Builder::new()
            .prefix("panda-node-staged-中文 space-")
            .tempdir()
            .unwrap();
        let directory = temp.path().canonicalize().unwrap();
        let marker = directory.join("official-node-ran");
        let mut process = OwnedProcess::create_suspended(Spawn {
            program: node.into(),
            arguments: vec![
                "-e".into(),
                "require('node:fs').writeFileSync(process.argv[1], process.version); process.stdout.write('官方 Node 中文'); process.stderr.write('e'.repeat(200000));".into(),
                marker.clone().into(),
            ],
            current_directory: directory,
            environment: std::env::vars_os().collect(),
        })
        .unwrap();
        assert_eq!(process.wait(Duration::from_millis(20)).unwrap(), None);
        assert!(!marker.exists());
        process.assign_job().unwrap();
        let mut stderr = process.stderr.take().unwrap();
        let drain = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).unwrap();
            bytes
        });
        process.resume().unwrap();
        drop(process.stdin.take());
        let mut output = String::new();
        process
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert_eq!(process.wait(Duration::from_secs(5)).unwrap(), Some(0));
        assert!(process.wait_empty(Duration::from_secs(5)).unwrap());
        assert_eq!(drain.join().unwrap(), vec![b'e'; 200000]);
        assert_eq!(output, "官方 Node 中文");
        assert!(std::fs::read_to_string(marker).unwrap().starts_with("v24."));
    }
    #[test]
    fn real_pipes_drain_large_stderr_and_preserve_protocol_stdin() {
        let temp = tempfile::Builder::new()
            .prefix("panda-native-中文 space-")
            .tempdir()
            .unwrap();
        let mut process =
            OwnedProcess::spawn(request("streams", &temp.path().canonicalize().unwrap())).unwrap();
        let identity = process.identity().unwrap();
        assert!(observe(&identity).unwrap());
        let errors = process.stderr.take().unwrap();
        let drain = std::thread::spawn(move || {
            let mut result = Vec::new();
            let mut errors = errors;
            errors.read_to_end(&mut result).unwrap();
            result
        });
        writeln!(process.stdin.as_mut().unwrap(), "disposable-中文 echo").unwrap();
        drop(process.stdin.take());
        let mut output = String::new();
        process
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert_eq!(process.wait(Duration::from_secs(10)).unwrap(), Some(0));
        assert_eq!(drain.join().unwrap().len(), 200_000);
        assert!(output.contains("ECHO:disposable-中文 echo"));
        assert!(!observe(&identity).unwrap());
        // Console helpers can outlive the root momentarily. A root exit is not
        // the receipt that the complete Job has stopped; finish its owned tail.
        process.terminate(0).unwrap();
        assert!(process.wait_empty(Duration::from_secs(5)).unwrap());
    }
    #[test]
    fn dropping_job_ends_the_real_child_and_its_descendant() {
        let temp = tempfile::Builder::new()
            .prefix("panda-native-中文 space-")
            .tempdir()
            .unwrap();
        let mut process =
            OwnedProcess::spawn(request("family", &temp.path().canonicalize().unwrap())).unwrap();
        let root = process.identity().unwrap();
        let root_handle = process.process.try_clone().unwrap();
        let mut reader = BufReader::new(process.stdout.take().unwrap());
        let descendant = loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if let Some(pid) = line.trim().strip_prefix("DESCENDANT:") {
                break pid.parse().unwrap();
            }
        };
        let handle = owned(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                descendant,
            )
        })
        .unwrap();
        let child = read_identity(descendant, &handle).unwrap();
        assert!(observe(&child).unwrap());
        assert!(observe(&root).unwrap());
        // Windows can add a console host; the Job must contain at least the
        // real root and child, rather than pretending its size is always two.
        assert!(process.active_processes().unwrap() >= 2);
        drop(process);
        assert_eq!(
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 5000) },
            WAIT_OBJECT_0
        );
        assert_eq!(
            unsafe { WaitForSingleObject(root_handle.as_raw_handle(), 5000) },
            WAIT_OBJECT_0
        );
        assert!(exit_code(&handle).unwrap().is_some());
        assert!(exit_code(&root_handle).unwrap().is_some());
    }
    #[test]
    fn identity_mismatch_does_not_authorize_any_pid_operation() {
        let temp = tempfile::Builder::new()
            .prefix("panda-native-中文 space-")
            .tempdir()
            .unwrap();
        let process =
            OwnedProcess::spawn(request("sleep", &temp.path().canonicalize().unwrap())).unwrap();
        let identity = process.identity().unwrap();
        let mut changed = identity.clone();
        changed.creation_100ns += 1;
        assert!(!observe(&changed).unwrap());
        assert!(observe(&identity).unwrap());
        process.terminate(37).unwrap();
        assert_eq!(process.wait(Duration::from_secs(5)).unwrap(), Some(37));
        assert!(boot_time().unwrap() < SystemTime::now());
    }
    #[test]
    fn abrupt_host_termination_closes_its_job_and_ends_the_entire_family() {
        use std::{
            os::windows::process::CommandExt,
            process::{Child, Stdio},
        };
        struct Host(Child);
        impl Drop for Host {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let temp = tempfile::Builder::new()
            .prefix("panda-native-中文 space-")
            .tempdir()
            .unwrap();
        let mut host = Host(
            fixture_command()
                .env("PANDA_NATIVE_FIXTURE", "host")
                .current_dir(temp.path())
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut reader = BufReader::new(host.0.stdout.take().unwrap());
        let mut children = Vec::new();
        while children.len() < 2 {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            let pid = line
                .trim()
                .strip_prefix("HOST_ROOT:")
                .or_else(|| line.trim().strip_prefix("HOST_DESCENDANT:"));
            if let Some(pid) = pid {
                let pid = pid.parse::<u32>().unwrap();
                children.push(
                    owned(unsafe {
                        OpenProcess(
                            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                            0,
                            pid,
                        )
                    })
                    .unwrap(),
                );
            }
        }
        // Equivalent native termination to ending this disposable host in Task
        // Manager. Neither the child nor Job destructor can run in that host.
        host.0.kill().unwrap();
        host.0.wait().unwrap();
        for handle in children {
            assert_eq!(
                unsafe { WaitForSingleObject(handle.as_raw_handle(), 5000) },
                WAIT_OBJECT_0
            );
            assert!(exit_code(&handle).unwrap().is_some());
        }
    }
    #[test]
    fn quotes_follow_the_windows_argv_protocol_without_a_shell() {
        assert_eq!(
            String::from_utf16(&quoted(OsStr::new("中文 space\\\"tail\\")).unwrap()).unwrap(),
            "\"中文 space\\\\\\\"tail\\\\\""
        );
        assert!(quoted(OsStr::new("bad\0argument")).is_err());
        assert!(
            environment::block(&[("Path".into(), "a".into()), ("PATH".into(), "b".into())])
                .is_err()
        );
    }
}
