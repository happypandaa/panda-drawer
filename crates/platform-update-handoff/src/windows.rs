//! Windows hands the coordinator's one-use permit to a copy of this executable,
//! through pipes inherited only by that child. The helper pins the live parent
//! process before acknowledging registration and waits on the same kernel object.
use platform_native::windows::files;
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    marker::PhantomData,
    os::windows::{
        ffi::OsStringExt,
        fs::OpenOptionsExt,
        io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;
use windows_sys::{
    Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation},
    Win32::{
        Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ},
        System::{
            Pipes::{GetNamedPipeServerProcessId, PeekNamedPipe},
            Threading::{
                GetCurrentProcess, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
                CREATE_NO_WINDOW, PROCESS_BASIC_INFORMATION, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            },
        },
    },
};

const HELPER: &str = "updater-helper.exe";
const HELPER_ARGUMENT: &str = "--platform-update-helper";
const SESSION_PREFIX: &str = "handoff-";
const MAX_PLAN_BYTES: usize = 4096;
const HELPER_BUDGET: Duration = Duration::from_secs(60);

fn rejected(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, reason)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionBinding {
    parent: u32,
    nonce: Uuid,
}

pub trait DrainProof {
    fn binding(&self) -> &SessionBinding;
}

pub fn current_parent_matches(binding: &SessionBinding) -> io::Result<()> {
    if binding.parent == std::process::id() {
        Ok(())
    } else {
        Err(rejected("session belongs to a different process"))
    }
}

pub struct SessionDirectory {
    binding: SessionBinding,
    path: PathBuf,
    pin: files::PinnedDirectory,
}

impl SessionDirectory {
    pub fn create_in(parent: &File) -> io::Result<Self> {
        let (root, pin) = files::directory_scope(parent)?;
        files::verify_private(&pin.file)?;
        let nonce = Uuid::new_v4();
        let path = root.join(format!("{SESSION_PREFIX}{nonce}"));
        files::create_private_directory(&path)?;
        let pin = files::pin_directory(&path)?;
        files::verify_private(&pin.file)?;
        Ok(Self {
            binding: SessionBinding {
                parent: std::process::id(),
                nonce,
            },
            path,
            pin,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn directory(&self) -> &File {
        &self.pin.file
    }
    pub fn binding(&self) -> &SessionBinding {
        &self.binding
    }
}

fn session_nonce(path: &Path) -> io::Result<Uuid> {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .and_then(|s| s.strip_prefix(SESSION_PREFIX))
        .ok_or_else(|| rejected("invalid handoff session location"))?;
    let nonce = Uuid::parse_str(name).map_err(|_| rejected("invalid handoff session UUID"))?;
    if name != nonce.to_string() {
        return Err(rejected("session name is not canonical"));
    }
    Ok(nonce)
}

// Reopen the inspected, single-link file without write or delete sharing. Keep
// this handle through CreateProcess so verification covers the actual image.
fn lock_image(path: &Path) -> io::Result<File> {
    let checked = files::open_no_reparse(path)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let identity = files::identity(&file)?;
    if identity.directory || identity.links != 1 || !identity.same_object(&checked.identity) {
        return Err(rejected(
            "updater image changed or is not an independent file",
        ));
    }
    Ok(file)
}

fn match_bytes(mut left: &File, mut right: &File) -> io::Result<()> {
    let size = left.metadata()?.len();
    if size == 0 || size != right.metadata()?.len() {
        return Err(rejected("updater image lengths differ"));
    }
    let mut a = vec![0; 64 * 1024];
    let mut b = vec![0; a.len()];
    let mut remaining = size;
    while remaining > 0 {
        let count = remaining.min(a.len() as u64) as usize;
        left.read_exact(&mut a[..count])?;
        right.read_exact(&mut b[..count])?;
        if a[..count] != b[..count] {
            return Err(rejected("updater image bytes differ"));
        }
        remaining -= count as u64;
    }
    Ok(())
}

fn stage_helper(session: &SessionDirectory) -> io::Result<(PathBuf, File)> {
    use std::io::Seek;
    let exe = std::env::current_exe()?;
    let mut original = lock_image(&exe)?;
    let destination = session.path.join(HELPER);
    let mut created = files::create_private_file(&destination)?;
    let identity = files::identity(&created)?;
    io::copy(&mut original, &mut created)?;
    created.sync_all()?;
    drop(created);
    let pinned = lock_image(&destination)?;
    if !identity.same_object(&files::identity(&pinned)?) {
        return Err(rejected("staged helper changed after copying"));
    }
    original.rewind()?;
    match_bytes(&original, &pinned)?;
    Ok((destination, pinned))
}

fn receive<R: Read + AsRawHandle>(
    reader: &mut R,
    mut buffer: &mut [u8],
    budget: Duration,
) -> io::Result<()> {
    if budget.is_zero() || budget > HELPER_BUDGET {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let deadline = Instant::now() + budget;
    while !buffer.is_empty() {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let mut available = 0;
        if unsafe {
            PeekNamedPipe(
                reader.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if available == 0 {
            thread::sleep(Duration::from_millis(10));
        } else {
            let count = (available as usize).min(buffer.len());
            let (next, remainder) = buffer.split_at_mut(count);
            reader.read_exact(next)?;
            buffer = remainder;
        }
    }
    Ok(())
}

pub struct ParentEndpoint<P: DrainProof> {
    session: SessionDirectory,
    control: ChildStdin,
    reply: ChildStdout,
    registered: bool,
    proof: PhantomData<fn(P)>,
}

impl<P: DrainProof> ParentEndpoint<P> {
    /// Carry the exact user-confirmed plan over the child-only channel. The
    /// helper retains these bytes in memory across the old process's exit.
    pub fn launch_with_plan(session: SessionDirectory, plan: &[u8]) -> io::Result<Self> {
        if plan.len() > MAX_PLAN_BYTES {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        current_parent_matches(session.binding())?;
        let (helper, _pin) = stage_helper(&session)?;
        let mut child = Command::new(helper)
            .arg(HELPER_ARGUMENT)
            .current_dir(&session.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        let mut control = child
            .stdin
            .take()
            .ok_or_else(|| rejected("missing helper control pipe"))?;
        let reply = child
            .stdout
            .take()
            .ok_or_else(|| rejected("missing helper reply pipe"))?;
        thread::Builder::new()
            .name("update-helper-wait".into())
            .spawn(move || {
                let _ = child.wait();
            })?;
        control.write_all(session.binding.nonce.as_bytes())?;
        control.write_all(&(plan.len() as u16).to_le_bytes())?;
        control.write_all(plan)?;
        Ok(Self {
            session,
            control,
            reply,
            registered: false,
            proof: PhantomData,
        })
    }

    pub fn binding(&self) -> &SessionBinding {
        self.session.binding()
    }

    pub fn wait_registered(&mut self, budget: Duration) -> io::Result<()> {
        if self.registered {
            return Err(rejected("helper already registered"));
        }
        let mut nonce = [0; 16];
        receive(&mut self.reply, &mut nonce, budget)?;
        if &nonce != self.binding().nonce.as_bytes() {
            return Err(rejected(
                "registration reply belongs to a different session",
            ));
        }
        self.registered = true;
        Ok(())
    }

    pub fn commit(mut self, proof: P, budget: Duration) -> io::Result<()> {
        if !self.registered
            || proof.binding() != self.binding()
            || budget.is_zero()
            || budget > HELPER_BUDGET
        {
            return Err(rejected(
                "receipt does not match the registered handoff or its budget",
            ));
        }
        // From the first attempted write the child may have accepted the permit.
        // A lost acknowledgement must not authorize a second installation.
        let unknown = |_| {
            io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "update handoff outcome unknown",
            )
        };
        self.control.write_all(b"I").map_err(unknown)?;
        let mut ack = [0];
        receive(&mut self.reply, &mut ack, budget).map_err(unknown)?;
        if ack != *b"A" {
            return Err(unknown(rejected("unexpected handoff acknowledgement")));
        }
        Ok(())
    }
}

fn parent_process(std_input: HANDLE) -> io::Result<OwnedHandle> {
    let mut info = PROCESS_BASIC_INFORMATION::default();
    let len = std::mem::size_of_val(&info) as u32;
    let status = unsafe {
        NtQueryInformationProcess(
            GetCurrentProcess(),
            ProcessBasicInformation,
            (&mut info as *mut PROCESS_BASIC_INFORMATION).cast(),
            len,
            std::ptr::null_mut(),
        )
    };
    if status < 0 {
        return Err(rejected("cannot identify the original parent process"));
    }
    let pid = u32::try_from(info.InheritedFromUniqueProcessId)
        .map_err(|_| rejected("invalid original parent process identifier"))?;
    let mut server = 0;
    // std::process creates both connected pipe ends in the launcher. Ensure the
    // process controlling our inherited input is also our original parent.
    if unsafe { GetNamedPipeServerProcessId(std_input, &mut server) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if server != pid || pid == 0 {
        return Err(rejected("pipe owner is not the original parent"));
    }
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    let owner = unsafe { OwnedHandle::from_raw_handle(handle) };
    if unsafe { WaitForSingleObject(owner.as_raw_handle(), 0) } != WAIT_TIMEOUT {
        return Err(rejected("original parent is no longer running"));
    }
    Ok(owner)
}

fn executable(process: &OwnedHandle) -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 32768];
    let mut len = buffer.len() as u32;
    if unsafe {
        QueryFullProcessImageNameW(
            process.as_raw_handle(),
            PROCESS_NAME_WIN32,
            buffer.as_mut_ptr(),
            &mut len,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(OsString::from_wide(&buffer[..len as usize]).into())
}

pub struct ObservedParentExit {
    pin: files::PinnedDirectory,
    executable: PathBuf,
    plan: Vec<u8>,
}

impl ObservedParentExit {
    pub fn directory(&self) -> &File {
        &self.pin.file
    }
    pub fn plan(&self) -> &[u8] {
        &self.plan
    }

    pub fn parent_executable(&self) -> &Path {
        &self.executable
    }
}

pub fn observe_parent_exit() -> io::Result<ObservedParentExit> {
    observe_parent_exit_with(|_, _, _| Ok(())).map(|(observed, ())| observed)
}

/// The install adapter acquires its startup-exclusion guard before the
/// registration reply. It remains owned by the helper through kernel exit.
pub fn observe_parent_exit_with<T>(
    register: impl FnOnce(&File, &Path, &[u8]) -> io::Result<T>,
) -> io::Result<(ObservedParentExit, T)> {
    let current = std::env::current_exe()?;
    if current.file_name() != Some(OsStr::new(HELPER)) {
        return Err(rejected("unexpected helper image name"));
    }
    let path = current
        .parent()
        .ok_or_else(|| rejected("invalid handoff session location"))?;
    let nonce = session_nonce(path)?;
    let pin = files::pin_directory(path)?;
    files::verify_private(&pin.file)?;
    // Use unbuffered handles. A buffered stdin can consume the following
    // frame before PeekNamedPipe sees it, and GUI stdout may be line buffered.
    let mut input = File::from(io::stdin().as_handle().try_clone_to_owned()?);
    let mut output = File::from(io::stdout().as_handle().try_clone_to_owned()?);
    let parent = parent_process(input.as_raw_handle())?;
    let executable = executable(&parent)?;
    if executable.file_name() == Some(OsStr::new(HELPER)) {
        return Err(rejected("a helper cannot authorize another helper"));
    }
    match_bytes(&lock_image(&current)?, &lock_image(&executable)?)?;
    let mut announced = [0; 16];
    receive(&mut input, &mut announced, HELPER_BUDGET)?;
    if &announced != nonce.as_bytes() {
        return Err(rejected("pipe session does not match helper directory"));
    }
    let mut length = [0; 2];
    receive(&mut input, &mut length, HELPER_BUDGET)?;
    let length = u16::from_le_bytes(length) as usize;
    if length > MAX_PLAN_BYTES {
        return Err(rejected("handoff plan is larger than permitted"));
    }
    let mut plan = vec![0; length];
    if length > 0 {
        receive(&mut input, &mut plan, HELPER_BUDGET)?;
    }
    let guard = register(&pin.file, &executable, &plan)?;
    output.write_all(nonce.as_bytes())?;
    output.flush()?;
    let mut commit = [0];
    receive(&mut input, &mut commit, HELPER_BUDGET)?;
    if commit != *b"I" {
        return Err(rejected("unexpected handoff control message"));
    }
    output.write_all(b"A")?;
    output.flush()?;
    match unsafe { WaitForSingleObject(parent.as_raw_handle(), HELPER_BUDGET.as_millis() as u32) } {
        WAIT_OBJECT_0 => Ok((
            ObservedParentExit {
                pin,
                executable,
                plan,
            },
            guard,
        )),
        WAIT_TIMEOUT => Err(io::ErrorKind::TimedOut.into()),
        _ => Err(io::Error::last_os_error()),
    }
}

// The helper image is still mapped at the time it restarts the desktop. On a
// later startup retire only finished private sessions. Never recursively delete
// a directory: unrecognized files, including unfinished plans, keep it intact.
pub fn cleanup_sessions(root: &File) -> io::Result<()> {
    let (root, guard) = files::directory_scope(root)?;
    files::verify_private(&guard.file)?;
    for entry in fs::read_dir(&root)?.flatten() {
        let path = entry.path();
        if session_nonce(&path).is_err() {
            continue;
        }
        let Ok(directory) = files::pin_directory(&path) else {
            continue;
        };
        if files::verify_private(&directory.file).is_err() {
            continue;
        }
        let Ok(mut contents) = fs::read_dir(&path) else {
            continue;
        };
        match (contents.next(), contents.next()) {
            (Some(Ok(item)), None) if item.file_name() == HELPER => {
                let Ok(file) = files::open_read_file(&item.path()) else {
                    continue;
                };
                if files::verify_private(&file).is_err() {
                    continue;
                }
                let identity = files::identity(&file)?;
                drop(file);
                if files::remove_file(&item.path(), &identity).is_err() {
                    continue;
                }
            }
            (None, None) => (),
            _ => continue,
        }
        drop(contents);
        drop(directory);
        let _ = fs::remove_dir(&path);
    }
    Ok(())
}
