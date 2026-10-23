//! Windows process ownership. No process executes before it belongs to our job.
//! The unnamed, non-inherited job is closed by the kernel on host termination.
use platform_project_format::ApiError;
type ProcessResult<T> = Result<T, ApiError>;
fn fail(code: &'static str, message: &'static str) -> ApiError {
    ApiError::new(code, message)
}
#[derive(Clone, Copy, Debug)]
pub struct ExitObservation {
    pub code: Option<i32>,
}
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::File,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    path::Path,
    process::ExitStatus,
    ptr::{null, null_mut},
    sync::{Arc, Mutex},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Security::SECURITY_ATTRIBUTES,
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
            TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Pipes::CreatePipe,
        SystemInformation::GetWindowsDirectoryW,
        Threading::{
            CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
            InitializeProcThreadAttributeList, ResumeThread, TerminateProcess,
            UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED,
            CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESTDHANDLES, STARTUPINFOEXW,
        },
    },
};

fn raw(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle().cast()
}
fn owned(handle: HANDLE) -> ProcessResult<OwnedHandle> {
    if handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(fail(
            "JOB_PROCESS_FAILED",
            "Could not create an owned process resource",
        ));
    }
    // Each successful native creation transfers one fresh handle to this owner.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.cast()) })
}
fn wide(value: &OsStr) -> ProcessResult<Vec<u16>> {
    let mut value: Vec<_> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(fail(
            "JOB_PATH_INVALID",
            "Process strings cannot contain NUL",
        ));
    }
    value.push(0);
    Ok(value)
}

/// Quote one argument for the Windows CRT parser, including trailing slashes.
fn quote(value: &OsStr) -> ProcessResult<Vec<u16>> {
    let input = wide(value)?;
    let mut result = vec![b'"' as u16];
    let mut slashes = 0;
    for &unit in &input[..input.len() - 1] {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n(
            b'\\' as u16,
            slashes * if unit == b'"' as u16 { 2 } else { 1 },
        ));
        if unit == b'"' as u16 {
            result.push(b'\\' as u16);
        }
        result.push(unit);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    result.push(b'"' as u16);
    Ok(result)
}

fn environment(values: &BTreeMap<OsString, OsString>) -> ProcessResult<Vec<u16>> {
    let mut entries = Vec::new();
    let mut names = std::collections::HashSet::new();
    for (name, value) in values {
        let name_text = name
            .to_str()
            .ok_or_else(|| fail("JOB_ENV_INVALID", "Invalid environment name"))?;
        if name_text.is_empty()
            || !name_text.is_ascii()
            || name_text.contains(['=', '\0'])
            || !names.insert(name_text.to_ascii_uppercase())
        {
            return Err(fail(
                "JOB_ENV_INVALID",
                "Duplicate or invalid environment name",
            ));
        }
        let mut entry = wide(name)?;
        entry.pop();
        entry.push(b'=' as u16);
        entry.extend(wide(value)?);
        entries.push((name_text.to_ascii_uppercase(), entry));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut block: Vec<u16> = entries.into_iter().flat_map(|(_, value)| value).collect();
    if block.len() > 32_766 {
        return Err(fail(
            "JOB_ENV_INVALID",
            "Environment exceeds its Windows budget",
        ));
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

pub fn windows_directory() -> ProcessResult<OsString> {
    let mut buffer = vec![0u16; 32_768];
    let count = unsafe { GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if count == 0 || count >= buffer.len() {
        return Err(fail(
            "JOB_ENV_INVALID",
            "Windows system directory is unavailable",
        ));
    }
    use std::os::windows::ffi::OsStringExt;
    Ok(OsString::from_wide(&buffer[..count]))
}

struct Attributes {
    storage: Vec<usize>,
}
impl Attributes {
    fn new(handles: &[HANDLE]) -> ProcessResult<Self> {
        let mut bytes = 0;
        unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes) };
        if bytes == 0 {
            return Err(fail(
                "JOB_PROCESS_FAILED",
                "Process inheritance sizing failed",
            ));
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        if unsafe {
            InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 1, 0, &mut bytes)
        } == 0
        {
            return Err(fail(
                "JOB_PROCESS_FAILED",
                "Process inheritance initialization failed",
            ));
        }
        let mut result = Self { storage };
        if unsafe {
            UpdateProcThreadAttribute(
                result.storage.as_mut_ptr().cast(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(handles),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(fail(
                "JOB_PROCESS_FAILED",
                "Process inheritance allowlist failed",
            ));
        }
        Ok(result)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.storage.as_mut_ptr().cast()) };
    }
}

fn pipe(parent_reads: bool) -> ProcessResult<(OwnedHandle, OwnedHandle)> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let (mut read, mut write) = (null_mut(), null_mut());
    if unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } == 0 {
        return Err(fail("JOB_PROCESS_FAILED", "Could not create process pipes"));
    }
    let (read, write) = (owned(read)?, owned(write)?);
    let (parent, child) = if parent_reads {
        (read, write)
    } else {
        (write, read)
    };
    if unsafe { SetHandleInformation(raw(&parent), HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(fail(
            "JOB_PROCESS_FAILED",
            "Could not protect parent pipe inheritance",
        ));
    }
    Ok((parent, child))
}

struct Suspended {
    process: Option<OwnedHandle>,
    thread: OwnedHandle,
}
impl Drop for Suspended {
    fn drop(&mut self) {
        if let Some(process) = &self.process {
            // This process was created by this exact call and has not been
            // transferred to its long-lived owner. No PID lookup is involved.
            unsafe {
                TerminateProcess(raw(process), 1);
                WaitForSingleObject(raw(process), 5000);
            }
        }
    }
}

pub struct JobOwnership {
    job: OwnedHandle,
    process: OwnedHandle,
    pid: u32,
    armed: Mutex<bool>,
}
impl JobOwnership {
    fn observe_handle(&self) -> ProcessResult<Option<ExitObservation>> {
        match unsafe { WaitForSingleObject(raw(&self.process), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(raw(&self.process), &mut code) } == 0 {
                    return Err(fail(
                        "JOB_PROCESS_OWNERSHIP_LOST",
                        "Could not inspect the owned process handle",
                    ));
                }
                Ok(Some(ExitObservation {
                    code: Some(code as i32),
                }))
            }
            _ => Err(fail(
                "JOB_PROCESS_OWNERSHIP_LOST",
                "Owned process wait failed",
            )),
        }
    }
    pub fn active_processes(&self) -> ProcessResult<u32> {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        if unsafe {
            QueryInformationJobObject(
                raw(&self.job),
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                null_mut(),
            )
        } == 0
        {
            return Err(fail(
                "JOB_GROUP_UNCONFIRMED",
                "Could not inspect the owned job",
            ));
        }
        Ok(info.ActiveProcesses)
    }
    /// Clone the exact leader HANDLE for observation without a PID lookup.
    pub fn leader_handle(&self) -> std::io::Result<OwnedHandle> {
        self.process.try_clone()
    }
    pub fn observe(&self) -> ProcessResult<Option<ExitObservation>> {
        let armed = self.armed.lock().unwrap_or_else(|p| p.into_inner());
        if !*armed {
            return Err(fail(
                "JOB_OWNERSHIP_RELEASED",
                "Process ownership has been released",
            ));
        }
        self.observe_handle()
    }
    pub fn terminate(&self) -> ProcessResult<()> {
        let armed = self.armed.lock().unwrap_or_else(|p| p.into_inner());
        if !*armed {
            return Err(fail(
                "JOB_OWNERSHIP_RELEASED",
                "Process ownership has been released",
            ));
        }
        // Forced cleanup targets this exact job HANDLE, including descendants
        // after its leader exits. Protocol-specific graceful shutdown is external.
        if unsafe { TerminateJobObject(raw(&self.job), 1) } == 0 {
            return Err(fail(
                "JOB_SIGNAL_FAILED",
                "Could not terminate the owned job",
            ));
        }
        Ok(())
    }
    pub fn has_descendants(&self) -> ProcessResult<bool> {
        let armed = self.armed.lock().unwrap_or_else(|p| p.into_inner());
        if !*armed {
            return Err(fail(
                "JOB_OWNERSHIP_RELEASED",
                "Process ownership has been released",
            ));
        }
        let leader = u32::from(self.observe_handle()?.is_none());
        Ok(self.active_processes()? > leader)
    }
    pub fn disarm_before_reap(&self) {
        *self.armed.lock().unwrap_or_else(|p| p.into_inner()) = false;
    }
}

pub struct JobChild {
    ownership: Arc<JobOwnership>,
    pub stdin: Option<tokio::fs::File>,
    pub stdout: Option<tokio::fs::File>,
    pub stderr: Option<tokio::fs::File>,
}
impl JobChild {
    pub fn spawn(
        executable: &Path,
        args: &[OsString],
        cwd: &Path,
        values: &BTreeMap<OsString, OsString>,
    ) -> ProcessResult<Self> {
        Self::spawn_inner(executable, args, cwd, values, false)
    }
    fn spawn_inner(
        executable: &Path,
        args: &[OsString],
        cwd: &Path,
        values: &BTreeMap<OsString, OsString>,
        reject_assignment: bool,
    ) -> ProcessResult<Self> {
        if !executable.is_absolute() || !cwd.is_absolute() {
            return Err(fail("JOB_PATH_INVALID", "Process paths must be absolute"));
        }
        let application = wide(executable.as_os_str())?;
        let cwd = wide(cwd.as_os_str())?;
        let environment = environment(values)?;
        let mut command = quote(executable.as_os_str())?;
        for arg in args {
            command.push(b' ' as u16);
            command.extend(quote(arg)?);
        }
        command.push(0);
        if command.len() > 32_767 {
            return Err(fail(
                "JOB_PROCESS_FAILED",
                "Process command line exceeds its budget",
            ));
        }
        let job = owned(unsafe { CreateJobObjectW(null(), null()) })?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                raw(&job),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(fail(
                "JOB_PROCESS_FAILED",
                "Could not configure owned job cleanup",
            ));
        }
        let (input, child_input) = pipe(false)?;
        let (output, child_output) = pipe(true)?;
        let (error, child_error) = pipe(true)?;
        let handles = [raw(&child_input), raw(&child_output), raw(&child_error)];
        let mut attributes = Attributes::new(&handles)?;
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = handles[0];
        startup.StartupInfo.hStdOutput = handles[1];
        startup.StartupInfo.hStdError = handles[2];
        startup.lpAttributeList = attributes.storage.as_mut_ptr().cast();
        let mut info = PROCESS_INFORMATION::default();
        if unsafe {
            CreateProcessW(
                application.as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_SUSPENDED
                    | CREATE_NO_WINDOW
                    | CREATE_UNICODE_ENVIRONMENT
                    | EXTENDED_STARTUPINFO_PRESENT,
                environment.as_ptr().cast(),
                cwd.as_ptr(),
                &startup.StartupInfo,
                &mut info,
            )
        } == 0
        {
            return Err(fail(
                "JOB_PROCESS_FAILED",
                "Could not create the suspended private process",
            ));
        }
        // CreateProcess success supplies both fresh handles. Install cleanup
        // before assignment/resumption so every failure retains exact ownership.
        let mut created = Suspended {
            process: Some(unsafe { OwnedHandle::from_raw_handle(info.hProcess.cast()) }),
            thread: unsafe { OwnedHandle::from_raw_handle(info.hThread.cast()) },
        };
        // No application instruction has run. Assignment failure is fatal;
        // terminate only the freshly-created process HANDLE, never a numeric PID.
        if reject_assignment
            || unsafe {
                AssignProcessToJobObject(raw(&job), raw(created.process.as_ref().unwrap()))
            } == 0
        {
            return Err(fail(
                "JOB_PROCESS_FAILED",
                "Could not assign the suspended process to its job",
            ));
        }
        if unsafe { ResumeThread(raw(&created.thread)) } != 1 {
            unsafe { TerminateJobObject(raw(&job), 1) };
            return Err(fail(
                "JOB_PROCESS_FAILED",
                "Could not resume the owned process",
            ));
        }
        let process = created.process.take().unwrap();
        drop(created);
        drop((child_input, child_output, child_error));
        Ok(Self {
            ownership: Arc::new(JobOwnership {
                job,
                process,
                pid: info.dwProcessId,
                armed: Mutex::new(true),
            }),
            stdin: Some(tokio::fs::File::from_std(File::from(input))),
            stdout: Some(tokio::fs::File::from_std(File::from(output))),
            stderr: Some(tokio::fs::File::from_std(File::from(error))),
        })
    }
    pub fn id(&self) -> Option<u32> {
        Some(self.ownership.pid)
    }
    pub fn ownership(&self) -> Arc<JobOwnership> {
        self.ownership.clone()
    }
    pub async fn wait(&mut self) -> std::io::Result<ExitStatus> {
        loop {
            match self
                .ownership
                .observe_handle()
                .map_err(|e| std::io::Error::other(e.message))?
            {
                Some(exit) => return Ok(ExitStatus::from_raw(exit.code.unwrap_or(1) as u32)),
                None => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    }
    pub async fn wait_for_tree_exit(&self) {
        // Cleanup uncertainty retains the job and its caller's path guards.
        // A leader exit alone is insufficient, and a failed query is never zero.
        while self.ownership.active_processes().ok() != Some(0) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    pub fn wait_for_tree_exit_blocking(&self) {
        while self.ownership.active_processes().ok() != Some(0) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for JobChild {
    fn drop(&mut self) {
        // This guard also covers cancellation before the runtime is constructed.
        // KILL_ON_JOB_CLOSE is the kernel fallback if the host cannot run Drop.
        if self.ownership.active_processes().ok() != Some(0) {
            unsafe { TerminateJobObject(raw(&self.ownership.job), 1) };
        }
    }
}

/// Fault injection and encoding inspection for disposable process fixtures only.
#[cfg(any(test, feature = "test-fixtures"))]
#[doc(hidden)]
pub mod testing {
    use super::*;
    pub fn quote_argument(value: &OsStr) -> ProcessResult<Vec<u16>> {
        quote(value)
    }
    pub fn environment_block(values: &BTreeMap<OsString, OsString>) -> ProcessResult<Vec<u16>> {
        environment(values)
    }
    pub fn spawn_rejecting_assignment(
        executable: &Path,
        args: &[OsString],
        cwd: &Path,
        values: &BTreeMap<OsString, OsString>,
    ) -> ProcessResult<JobChild> {
        JobChild::spawn_inner(executable, args, cwd, values, true)
    }
}
