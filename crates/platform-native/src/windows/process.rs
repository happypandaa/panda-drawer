//! Suspended creation, Job ownership and anonymous stdio pipes. No shell,
//! Ctrl+Break, persisted-PID signalling or detached process ownership is used.
use super::{checked, native_path, wide};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
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
fn environment_block(values: &[(OsString, OsString)]) -> io::Result<Vec<u16>> {
    let mut sorted: Vec<_> = values.iter().collect();
    sorted.sort_by_key(|(name, _)| name.to_string_lossy().to_uppercase());
    let mut result = Vec::new();
    let mut previous: Option<String> = None;
    for (name, value) in sorted {
        let key = name.to_string_lossy().to_uppercase();
        if name.is_empty()
            || name.encode_wide().any(|c| c == 61 || c == 0)
            || previous.as_ref() == Some(&key)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid or duplicate environment key",
            ));
        }
        result.extend(wide(name)?.into_iter().take_while(|c| *c != 0));
        result.push(61);
        result.extend(wide(value)?);
        previous = Some(key);
    }
    if result.is_empty() {
        result.push(0);
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
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
    pub pid: u32,
}
impl OwnedProcess {
    pub fn spawn(request: Spawn) -> io::Result<Self> {
        let program = native_path(&request.program)?;
        let directory = native_path(&request.current_directory)?;
        let mut arguments = command_line(&request.program, &request.arguments)?;
        let environment = environment_block(&request.environment)?;
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
        let process = owned(created.hProcess)?;
        let thread = owned(created.hThread)?;
        unsafe {
            if AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) == 0 {
                let error = io::Error::last_os_error();
                TerminateProcess(process.as_raw_handle(), 1);
                WaitForSingleObject(process.as_raw_handle(), 5000);
                return Err(error);
            }
            if ResumeThread(thread.as_raw_handle()) == u32::MAX {
                let error = io::Error::last_os_error();
                TerminateJobObject(job.as_raw_handle(), 1);
                WaitForSingleObject(process.as_raw_handle(), 5000);
                return Err(error);
            }
        }
        drop(thread);
        // Only the child owns these ends after CreateProcess. Keeping one parent
        // copy would prevent EOF and could leave deletion waiting forever.
        drop(input.child);
        drop(output.child);
        drop(errors.child);
        Ok(Self {
            job,
            process,
            stdin: Some(input.parent),
            stdout: Some(output.parent),
            stderr: Some(errors.parent),
            pid: created.dwProcessId,
        })
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
        unsafe { checked(TerminateJobObject(self.job.as_raw_handle(), exit)) }
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
    use std::io::{BufRead, BufReader, Read, Write};
    fn request(mode: &str, directory: &Path) -> Spawn {
        let mut environment: Vec<_> = std::env::vars_os()
            .filter(|(name, _)| name != "PANDA_NATIVE_FIXTURE")
            .collect();
        environment.push(("PANDA_NATIVE_FIXTURE".into(), mode.into()));
        Spawn {
            program: std::env::current_exe().unwrap(),
            arguments: vec![
                "--ignored".into(),
                "--exact".into(),
                "windows::process::tests::child_fixture".into(),
                "--nocapture".into(),
            ],
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
                let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--ignored",
                        "--exact",
                        "windows::process::tests::child_fixture",
                        "--nocapture",
                    ])
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
            _ => panic!("Unknown disposable fixture mode"),
        }
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
            process::{Child, Command, Stdio},
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
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "windows::process::tests::child_fixture",
                    "--nocapture",
                ])
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
            environment_block(&[("Path".into(), "a".into()), ("PATH".into(), "b".into())]).is_err()
        );
    }
}
