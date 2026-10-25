use std::{
    collections::HashSet,
    fmt, io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::{Mutex, OnceLock},
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WitnessErrorKind {
    InvalidIdentity,
    InvalidNonce,
    NonceAlreadyUsed,
    NonceCapacityReached,
    NotActualParent,
    ProcessUnavailable,
    ProcessIdentityChanged,
    ProcessAlreadyExited,
    QueueCreationFailed,
    CloseOnExecFailed,
    RegistrationRejected,
    UnexpectedKernelEvent,
    KernelObservationFailed,
    InvalidWaitBudget,
    WitnessFailed,
    EvidenceAlreadyConsumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WitnessError {
    pub kind: WitnessErrorKind,
    pub os_error: Option<i32>,
}
impl WitnessError {
    fn new(kind: WitnessErrorKind) -> Self {
        Self {
            kind,
            os_error: None,
        }
    }
    fn os(kind: WitnessErrorKind) -> Self {
        Self {
            kind,
            os_error: io::Error::last_os_error().raw_os_error(),
        }
    }
}
impl fmt::Display for WitnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "parent-exit witness {:?} (OS {:?})",
            self.kind, self.os_error
        )
    }
}
impl std::error::Error for WitnessError {}
type Result<T> = std::result::Result<T, WitnessError>;

/// Expected identity from the parent's trusted launch handshake. PID alone is
/// never sufficient. The start fields use Darwin proc_bsdinfo's sec/usec pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessIdentity {
    pid: i32,
    start_seconds: u64,
    start_microseconds: u64,
    effective_uid: u32,
}
impl ProcessIdentity {
    pub fn from_parts(
        pid: i32,
        start_seconds: u64,
        start_microseconds: u64,
        effective_uid: u32,
    ) -> Result<Self> {
        if pid <= 1 || start_seconds == 0 || start_microseconds >= 1_000_000 {
            return Err(WitnessError::new(WitnessErrorKind::InvalidIdentity));
        }
        Ok(Self {
            pid,
            start_seconds,
            start_microseconds,
            effective_uid,
        })
    }
    pub fn current() -> Result<Self> {
        // SAFETY: getpid has no preconditions and does not modify resources.
        let pid = unsafe { libc::getpid() };
        process_identity(pid)
    }
    pub fn pid(&self) -> i32 {
        self.pid
    }
    pub fn start_seconds(&self) -> u64 {
        self.start_seconds
    }
    pub fn start_microseconds(&self) -> u64 {
        self.start_microseconds
    }
    pub fn effective_uid(&self) -> u32 {
        self.effective_uid
    }
}

/// Format-checked nonce; this type does not generate randomness or attest its
/// entropy. A future trusted launcher must supply its own unpredictable value.
#[derive(Debug)]
pub struct SessionNonce(String);
impl SessionNonce {
    pub fn parse(value: &str) -> Result<Self> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || value.bytes().all(|b| b == b'0')
        {
            return Err(WitnessError::new(WitnessErrorKind::InvalidNonce));
        }
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

static USED_NONCES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
fn claim_nonce(nonce: &SessionNonce) -> Result<()> {
    let mut used = USED_NONCES
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| WitnessError::new(WitnessErrorKind::WitnessFailed))?;
    if used.contains(nonce.as_str()) {
        return Err(WitnessError::new(WitnessErrorKind::NonceAlreadyUsed));
    }
    if used.len() >= 256 {
        return Err(WitnessError::new(WitnessErrorKind::NonceCapacityReached));
    }
    used.insert(nonce.0.clone());
    Ok(())
}

fn process_identity(pid: i32) -> Result<ProcessIdentity> {
    // SAFETY: an all-zero proc_bsdinfo is valid output storage. libproc gets a
    // correctly sized writable buffer; arg=1 includes zombies for explicit rejection.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info);
    let count = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            1,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size as i32,
        )
    };
    if count != size as i32 {
        return Err(WitnessError::os(WitnessErrorKind::ProcessUnavailable));
    }
    if info.pbi_pid != pid as u32 {
        return Err(WitnessError::new(WitnessErrorKind::ProcessIdentityChanged));
    }
    if info.pbi_status == libc::SZOMB {
        return Err(WitnessError::new(WitnessErrorKind::ProcessAlreadyExited));
    }
    ProcessIdentity::from_parts(
        pid,
        info.pbi_start_tvsec,
        info.pbi_start_tvusec,
        info.pbi_uid,
    )
}

fn queue() -> Result<OwnedFd> {
    // SAFETY: kqueue creates a fresh descriptor, exclusively owned below.
    let fd = unsafe { libc::kqueue() };
    if fd < 0 {
        return Err(WitnessError::os(WitnessErrorKind::QueueCreationFailed));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: this is our live descriptor; no other object can close/reuse it.
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(WitnessError::os(WitnessErrorKind::CloseOnExecFailed));
    }
    Ok(fd)
}

fn register_kernel(fd: &OwnedFd, pid: i32) -> Result<()> {
    let change = libc::kevent {
        ident: pid as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT | libc::EV_RECEIPT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let zero = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let mut receipt: libc::kevent = unsafe { std::mem::zeroed() };
    // SAFETY: all pointers reference the exact initialized input/output storage
    // required by kevent. EV_RECEIPT reports whether this registration succeeded.
    let count = unsafe { libc::kevent(fd.as_raw_fd(), &change, 1, &mut receipt, 1, &zero) };
    if count < 0 {
        return Err(WitnessError::os(WitnessErrorKind::RegistrationRejected));
    }
    if count != 1
        || receipt.ident != pid as libc::uintptr_t
        || receipt.filter != libc::EVFILT_PROC
        || receipt.flags & libc::EV_ERROR == 0
        || receipt.data != 0
    {
        return Err(WitnessError {
            kind: WitnessErrorKind::RegistrationRejected,
            os_error: (receipt.data != 0).then_some(receipt.data as i32),
        });
    }
    Ok(())
}

/// Borrow this only after successful registration to send a launch handshake.
/// It does not prove the parent exited, and is not an installation permit.
#[derive(Debug)]
pub struct RegistrationReceipt {
    parent: ProcessIdentity,
    nonce: SessionNonce,
}
impl RegistrationReceipt {
    pub fn parent(&self) -> ProcessIdentity {
        self.parent
    }
    pub fn nonce(&self) -> &str {
        self.nonce.as_str()
    }
}

/// Opaque, non-Clone and non-deserializable evidence from one kernel event.
/// It says nothing about the exit being graceful or any other resource closing.
///
/// ```compile_fail
/// use platform_update_witness::ParentExitEvidence;
/// fn cannot_replay_by_cloning(evidence: ParentExitEvidence) {
///     let _copy = evidence.clone();
/// }
/// ```
///
/// ```compile_fail
/// use platform_update_witness::{ParentExitEvidence, ProcessIdentity, SessionNonce};
/// fn cannot_forge_from_launch_fields(parent: ProcessIdentity, nonce: SessionNonce) {
///     let _forged = ParentExitEvidence { parent, nonce };
/// }
/// ```
#[derive(Debug)]
pub struct ParentExitEvidence {
    parent: ProcessIdentity,
    nonce: SessionNonce,
}
impl ParentExitEvidence {
    pub fn parent(&self) -> ProcessIdentity {
        self.parent
    }
    pub fn nonce(&self) -> &str {
        self.nonce.as_str()
    }
}

#[derive(Debug)]
pub enum WaitOutcome {
    Pending,
    ParentExited(ParentExitEvidence),
}

/// Owns the same dedicated queue across every Pending wait. Dropping it simply
/// cancels observation; it never signals a process or manufactures evidence.
#[derive(Debug)]
pub struct ParentExitWitness {
    queue: OwnedFd,
    receipt: Option<RegistrationReceipt>,
    failed: bool,
}
impl ParentExitWitness {
    pub fn register(expected: ProcessIdentity, nonce: SessionNonce) -> Result<Self> {
        // Burn even failed attempts, preventing same-process replay or two
        // concurrent registrations claiming one session. No cross-restart ledger.
        claim_nonce(&nonce)?;
        // SAFETY: these identity syscalls have no pointer arguments.
        if unsafe { libc::getppid() } != expected.pid {
            return Err(WitnessError::new(WitnessErrorKind::NotActualParent));
        }
        if unsafe { libc::geteuid() } != expected.effective_uid
            || process_identity(expected.pid)? != expected
        {
            return Err(WitnessError::new(WitnessErrorKind::ProcessIdentityChanged));
        }
        let queue = queue()?;
        register_kernel(&queue, expected.pid)?;
        // A lost/reused/exited parent during registration is rejected, never
        // inferred from ESRCH. The intended parent waits for this handshake.
        if process_identity(expected.pid)? != expected || unsafe { libc::getppid() } != expected.pid
        {
            return Err(WitnessError::new(WitnessErrorKind::ProcessIdentityChanged));
        }
        Ok(Self {
            queue,
            receipt: Some(RegistrationReceipt {
                parent: expected,
                nonce,
            }),
            failed: false,
        })
    }
    pub fn registration(&self) -> Result<&RegistrationReceipt> {
        if self.failed {
            return Err(WitnessError::new(WitnessErrorKind::WitnessFailed));
        }
        self.receipt
            .as_ref()
            .ok_or_else(|| WitnessError::new(WitnessErrorKind::EvidenceAlreadyConsumed))
    }
    pub fn wait(&mut self, budget: Duration) -> Result<WaitOutcome> {
        if budget > Duration::from_secs(60) {
            return Err(WitnessError::new(WitnessErrorKind::InvalidWaitBudget));
        }
        let expected = self.registration()?.parent;
        let timeout = libc::timespec {
            tv_sec: budget.as_secs() as libc::time_t,
            tv_nsec: budget.subsec_nanos() as libc::c_long,
        };
        let mut event: libc::kevent = unsafe { std::mem::zeroed() };
        // SAFETY: no changes are submitted and the dedicated live descriptor
        // can return only events registered above. Output and timeout are sized.
        let count = unsafe {
            libc::kevent(
                self.queue.as_raw_fd(),
                std::ptr::null(),
                0,
                &mut event,
                1,
                &timeout,
            )
        };
        if count < 0 {
            if io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                return Ok(WaitOutcome::Pending);
            }
            self.failed = true;
            return Err(WitnessError::os(WitnessErrorKind::KernelObservationFailed));
        }
        if count == 0 {
            return Ok(WaitOutcome::Pending);
        }
        if count != 1
            || event.ident != expected.pid as libc::uintptr_t
            || event.filter != libc::EVFILT_PROC
            || event.flags & libc::EV_ERROR != 0
            || event.fflags & libc::NOTE_EXIT == 0
        {
            self.failed = true;
            return Err(WitnessError::new(WitnessErrorKind::UnexpectedKernelEvent));
        }
        let receipt = self
            .receipt
            .take()
            .expect("registration checked before the kernel wait");
        Ok(WaitOutcome::ParentExited(ParentExitEvidence {
            parent: receipt.parent,
            nonce: receipt.nonce,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_identity_and_nonce_reject_invalid_inputs() {
        for (pid, sec, usec) in [
            (0, 1, 0),
            (1, 1, 0),
            (-1, 1, 0),
            (2, 0, 0),
            (2, 1, 1_000_000),
        ] {
            assert!(ProcessIdentity::from_parts(pid, sec, usec, 501).is_err());
        }
        for value in [
            "".into(),
            "a".repeat(63),
            "A".repeat(64),
            "0".repeat(64),
            "g".repeat(64),
        ] {
            assert!(SessionNonce::parse(&value).is_err());
        }
    }
    #[test]
    fn actual_queue_is_close_on_exec_and_bad_kernel_registration_is_rejected() {
        let fd = queue().unwrap();
        assert_ne!(
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        assert_eq!(
            register_kernel(&fd, i32::MAX).unwrap_err().kind,
            WitnessErrorKind::RegistrationRejected
        );
    }
    #[test]
    fn failed_attempt_burns_nonce_and_never_observes_a_non_parent() {
        let own = ProcessIdentity::current().unwrap();
        let nonce = "b".repeat(64);
        assert_eq!(
            ParentExitWitness::register(own, SessionNonce::parse(&nonce).unwrap())
                .unwrap_err()
                .kind,
            WitnessErrorKind::NotActualParent
        );
        assert_eq!(
            ParentExitWitness::register(own, SessionNonce::parse(&nonce).unwrap())
                .unwrap_err()
                .kind,
            WitnessErrorKind::NonceAlreadyUsed
        );
    }

    #[test]
    fn concurrent_claims_cannot_register_one_nonce_twice() {
        let contenders: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| claim_nonce(&SessionNonce::parse(&"c".repeat(64)).unwrap()))
            })
            .collect();
        let results: Vec<_> = contenders.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|error| error.kind == WitnessErrorKind::NonceAlreadyUsed));
    }
}
