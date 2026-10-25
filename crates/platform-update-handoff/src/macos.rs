mod ipc;
mod store;
use ipc::{Message, Reader, WireBinding};
use platform_update_witness::{ParentExitWitness, ProcessIdentity, SessionNonce, WaitOutcome};
use std::{
    collections::HashMap,
    fs::File,
    marker::PhantomData,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    process::{Child, Command},
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex, OnceLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
pub use store::SessionDirectory;

const HANDOFF_FD: RawFd = 3;
const DIRECTORY_FD: RawFd = 4;
const SESSION_BUDGET: Duration = Duration::from_secs(30);
const MAX_OWNERS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBinding {
    parent: ProcessIdentity,
    nonce: String,
}
impl SessionBinding {
    pub fn parent(&self) -> ProcessIdentity {
        self.parent
    }
    pub fn nonce(&self) -> &str {
        &self.nonce
    }
}
/// Implementations are trusted adapters. This open trait alone cannot attest
/// application shutdown; fix P at launch to the adapter's unforgeable type.
pub trait DrainProof {
    fn binding(&self) -> &SessionBinding;
}
pub fn current_parent_matches(binding: &SessionBinding) -> Result<()> {
    if ProcessIdentity::current().map_err(|_| Error::WitnessRejected)? == binding.parent {
        Ok(())
    } else {
        Err(Error::BindingMismatch)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Io,
    InvalidFrame,
    EndOfPipe,
    InvalidDirectory,
    ClaimExists,
    WitnessRejected,
    Timeout,
    Protocol,
    BindingMismatch,
    Capacity,
    OwnerUnknown,
    HandoffCommitUnknown,
}
type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerState {
    Retained,
    Reaped {
        child_success: bool,
        install_authorized: bool,
    },
    Unknown,
}
struct Owner {
    handle: Option<JoinHandle<OwnerState>>,
    state: OwnerState,
}
static OWNERS: OnceLock<Mutex<HashMap<String, Owner>>> = OnceLock::new();
fn owners() -> &'static Mutex<HashMap<String, Owner>> {
    OWNERS.get_or_init(Mutex::default)
}
/// Reaped means actual Child::wait AND the dedicated worker JoinHandle joined.
/// A lost API waiter leaves the worker/Child retained here, outside core drain.
pub fn owner_state(binding: &SessionBinding) -> OwnerState {
    let Ok(mut owners) = owners().lock() else {
        return OwnerState::Unknown;
    };
    let Some(owner) = owners.get_mut(binding.nonce()) else {
        return OwnerState::Unknown;
    };
    if owner.handle.as_ref().is_some_and(|h| h.is_finished()) {
        owner.state = owner
            .handle
            .take()
            .map(|h| h.join().unwrap_or(OwnerState::Unknown))
            .unwrap_or(OwnerState::Unknown);
    }
    owner.state.clone()
}
enum Control {
    Drain,
    Cancel,
    LeaveArmed,
}
enum Event {
    Registered,
    Accepted,
    Armed,
    Failed(Error),
}
/// P is fixed when launching, not chosen by acknowledge_drained. The helper is
/// explicitly supplied by the trusted caller; code-signature admission is future work.
pub struct ParentEndpoint<P: DrainProof> {
    binding: SessionBinding,
    control: Sender<Control>,
    events: Receiver<Event>,
    registered: bool,
    accepted: bool,
    cancel_on_drop: bool,
    _proof: PhantomData<fn(P)>,
}
#[derive(Debug)]
pub struct AwaitingParentExit {
    binding: SessionBinding,
}
impl AwaitingParentExit {
    pub fn binding(&self) -> &SessionBinding {
        &self.binding
    }
    pub fn install_authorized(&self) -> bool {
        false
    }
}
#[derive(Debug)]
pub struct ArmedForParentExit {
    binding: SessionBinding,
}
impl ArmedForParentExit {
    pub fn binding(&self) -> &SessionBinding {
        &self.binding
    }
    pub fn install_authorized(&self) -> bool {
        false
    }
}
impl<P: DrainProof> ParentEndpoint<P> {
    pub fn launch(session: SessionDirectory, mut command: Command) -> Result<Self> {
        current_parent_matches(&session.binding)?;
        let (parent, child) = UnixStream::pair().map_err(|_| Error::Io)?;
        parent.set_nonblocking(true).map_err(|_| Error::Io)?;
        // Pin both sources above the destination slots; pre_exec contains only
        // async-signal-safe dup2/fcntl and cannot collide with fd 3/4.
        let child = duplicate_above_slots(child.as_raw_fd())?;
        let directory = duplicate_above_slots(session.directory.as_raw_fd())?;
        unsafe {
            command.pre_exec(move || {
                for (source, target) in [
                    (child.as_raw_fd(), HANDOFF_FD),
                    (directory.as_raw_fd(), DIRECTORY_FD),
                ] {
                    if libc::dup2(source, target) < 0 || libc::fcntl(target, libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let binding = session.binding.clone();
        let (control, commands) = mpsc::channel();
        let (send, events) = mpsc::channel();
        let mut registry = owners().lock().map_err(|_| Error::OwnerUnknown)?;
        if registry.len() >= MAX_OWNERS || registry.contains_key(binding.nonce()) {
            return Err(Error::Capacity);
        }
        let worker_binding = binding.clone();
        let handle = thread::Builder::new()
            .name("update-handoff-owner".into())
            .spawn(move || run_owner(command, parent, session, worker_binding, commands, send))
            .map_err(|_| Error::Io)?;
        registry.insert(
            binding.nonce.clone(),
            Owner {
                handle: Some(handle),
                state: OwnerState::Retained,
            },
        );
        Ok(Self {
            binding,
            control,
            events,
            registered: false,
            accepted: false,
            cancel_on_drop: true,
            _proof: PhantomData,
        })
    }
    pub fn binding(&self) -> &SessionBinding {
        &self.binding
    }
    pub fn wait_registered(&mut self, budget: Duration) -> Result<()> {
        valid_budget(budget)?;
        if self.registered {
            return Err(Error::Protocol);
        }
        match self.events.recv_timeout(budget) {
            Ok(Event::Registered) => {
                self.registered = true;
                Ok(())
            }
            Ok(Event::Failed(error)) => Err(error),
            _ => Err(Error::Timeout),
        }
    }
    pub fn acknowledge_drained(
        &mut self,
        proof: P,
        budget: Duration,
    ) -> Result<AwaitingParentExit> {
        valid_budget(budget)?;
        if !self.registered || self.accepted {
            return Err(Error::Protocol);
        }
        if proof.binding() != &self.binding {
            return Err(Error::BindingMismatch);
        }
        self.control
            .send(Control::Drain)
            .map_err(|_| Error::OwnerUnknown)?;
        match self.events.recv_timeout(budget) {
            Ok(Event::Accepted) => {
                self.accepted = true;
                Ok(AwaitingParentExit {
                    binding: self.binding.clone(),
                })
            }
            Ok(Event::Failed(error)) => Err(error),
            _ => Err(Error::Timeout),
        }
    }
    pub fn cancel_and_wait(self, budget: Duration) -> OwnerState {
        let _ = self.control.send(Control::Cancel);
        if valid_budget(budget).is_err() {
            return OwnerState::Retained;
        }
        let deadline = Instant::now() + budget;
        loop {
            let state = owner_state(&self.binding);
            if state != OwnerState::Retained || Instant::now() >= deadline {
                return state;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    /// Explicit handoff commit; only matching durable Armed ack yields a token.
    /// After Arm is sent, timeout/lost ack is HandoffCommitUnknown: cancellation
    /// cannot be promised. The worker still owns Child::wait. No install permit.
    pub fn leave_witness_armed(
        mut self,
        accepted: AwaitingParentExit,
        budget: Duration,
    ) -> Result<ArmedForParentExit> {
        valid_budget(budget)?;
        if !self.accepted || accepted.binding != self.binding {
            return Err(Error::BindingMismatch);
        }
        self.control
            .send(Control::LeaveArmed)
            .map_err(|_| Error::OwnerUnknown)?;
        match self.events.recv_timeout(budget) {
            Ok(Event::Armed) => {
                self.cancel_on_drop = false;
                Ok(ArmedForParentExit {
                    binding: self.binding.clone(),
                })
            }
            _ => Err(Error::HandoffCommitUnknown),
        }
    }
}
impl<P: DrainProof> Drop for ParentEndpoint<P> {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            let _ = self.control.send(Control::Cancel);
        }
    }
}
fn valid_budget(budget: Duration) -> Result<()> {
    if budget.is_zero() || budget > Duration::from_secs(60) {
        Err(Error::Timeout)
    } else {
        Ok(())
    }
}
fn duplicate_above_slots(fd: RawFd) -> Result<File> {
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10) };
    if duplicate < 0 {
        return Err(Error::Io);
    }
    Ok(unsafe { File::from_raw_fd(duplicate) })
}
// Last-resort unwinding also keeps the actual Child and waits it. No numeric
// PID signal is sent anywhere. The trusted helper has its own bounded lifetime.
struct ChildOwner(Option<Child>);
impl ChildOwner {
    fn wait(&mut self) -> OwnerState {
        loop {
            match self.0.as_mut().map(Child::wait) {
                Some(Ok(status)) => {
                    self.0.take();
                    return OwnerState::Reaped {
                        child_success: status.success(),
                        install_authorized: false,
                    };
                }
                // Unknown wait results cannot release the actual Child owner.
                // Public waits stay bounded and report Retained meanwhile.
                Some(Err(_)) => thread::sleep(Duration::from_millis(25)),
                None => return OwnerState::Unknown,
            }
        }
    }
}
impl Drop for ChildOwner {
    fn drop(&mut self) {
        if self.0.is_some() {
            let _ = self.wait();
        }
    }
}
fn run_owner(
    mut command: Command,
    mut pipe: UnixStream,
    _session: SessionDirectory,
    binding: SessionBinding,
    commands: Receiver<Control>,
    events: Sender<Event>,
) -> OwnerState {
    let child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            let _ = events.send(Event::Failed(Error::Io));
            return OwnerState::Unknown;
        }
    };
    // The pre_exec closure owns duplicate child-side descriptors; release them
    // in the parent now so EOF is not artificially held open.
    drop(command);
    let mut child = ChildOwner(Some(child));
    let outcome = owner_protocol(&mut pipe, &binding, &commands, &events);
    drop(commands);
    if let Err(error) = outcome {
        let _ = events.send(Event::Failed(error));
    }
    if outcome != Ok(true) {
        let _ = ipc::write(&mut pipe, &Message::Cancel {}, Duration::from_millis(100));
    }
    drop(pipe);
    child.wait()
}
fn owner_protocol(
    pipe: &mut UnixStream,
    binding: &SessionBinding,
    commands: &Receiver<Control>,
    events: &Sender<Event>,
) -> Result<bool> {
    ipc::write(
        pipe,
        &Message::Hello {
            binding: WireBinding::from(binding),
        },
        SESSION_BUDGET,
    )?;
    let mut reader = Reader::default();
    let deadline = Instant::now() + SESSION_BUDGET;
    let mut registered = false;
    let mut drain_sent = false;
    let mut accepted = false;
    let mut arm_sent = false;
    loop {
        match commands.try_recv() {
            Ok(Control::Cancel) | Err(mpsc::TryRecvError::Disconnected) => return Ok(false),
            Ok(Control::LeaveArmed) => {
                if !accepted || arm_sent {
                    return Err(Error::Protocol);
                }
                arm_sent = true;
                ipc::write(
                    pipe,
                    &Message::Arm {
                        binding: WireBinding::from(binding),
                    },
                    SESSION_BUDGET,
                )?;
            }
            Ok(Control::Drain) => {
                if !registered || drain_sent {
                    return Err(Error::Protocol);
                }
                drain_sent = true;
                ipc::write(
                    pipe,
                    &Message::Drain {
                        binding: WireBinding::from(binding),
                    },
                    SESSION_BUDGET,
                )?;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        match reader.next(pipe)? {
            Some(Message::Registered { binding: received })
                if !registered && received.matches(binding) =>
            {
                registered = true;
                events
                    .send(Event::Registered)
                    .map_err(|_| Error::OwnerUnknown)?;
            }
            Some(Message::Accepted { binding: received })
                if registered && drain_sent && !accepted && received.matches(binding) =>
            {
                accepted = true;
                events
                    .send(Event::Accepted)
                    .map_err(|_| Error::OwnerUnknown)?;
            }
            Some(Message::Armed { binding: received })
                if accepted && arm_sent && received.matches(binding) =>
            {
                events.send(Event::Armed).map_err(|_| Error::OwnerUnknown)?;
                return Ok(true);
            }
            Some(_) => return Err(Error::Protocol),
            None => thread::sleep(Duration::from_millis(5)),
        }
    }
}
/// Only the explicitly inherited descriptors are accepted; no PID, root path,
/// receipt or nonce can be supplied from argv/env/old ledger records.
/// # Safety
/// Caller exclusively owns valid launcher-inherited fd 3/4, invokes this exactly
/// once, and has not wrapped either descriptor in another owning Rust object.
pub unsafe fn helper_from_inherited_descriptors() -> Result<()> {
    // Transfer both descriptors immediately, so every validation failure closes
    // our inherited resources. Valid, exclusive ownership is the unsafe contract.
    let socket = unsafe { OwnedFd::from_raw_fd(HANDOFF_FD) };
    let directory = unsafe { OwnedFd::from_raw_fd(DIRECTORY_FD) };
    let mut socket_type: libc::c_int = 0;
    let mut length = std::mem::size_of_val(&socket_type) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&mut socket_type as *mut libc::c_int).cast(),
            &mut length,
        )
    } != 0
        || socket_type != libc::SOCK_STREAM
    {
        return Err(Error::Protocol);
    }
    let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut address_length = std::mem::size_of_val(&address) as libc::socklen_t;
    if unsafe {
        libc::getsockname(
            socket.as_raw_fd(),
            (&mut address as *mut libc::sockaddr_storage).cast(),
            &mut address_length,
        )
    } != 0
        || address.ss_family != libc::AF_UNIX as libc::sa_family_t
    {
        return Err(Error::Protocol);
    }
    for fd in [socket.as_raw_fd(), directory.as_raw_fd()] {
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(Error::Io);
        }
    }
    let mut pipe = UnixStream::from(socket);
    let directory = File::from(directory);
    pipe.set_nonblocking(true).map_err(|_| Error::Io)?;
    store::validate_directory(&directory)?;
    let mut reader = Reader::default();
    let binding = match receive(&mut reader, &mut pipe, Instant::now() + SESSION_BUDGET)? {
        Message::Hello { binding } => binding.checked()?,
        _ => return Err(Error::Protocol),
    };
    store::claim(&directory, &binding)?;
    let nonce = SessionNonce::parse(binding.nonce()).map_err(|_| Error::Protocol)?;
    let mut witness =
        ParentExitWitness::register(binding.parent, nonce).map_err(|_| Error::WitnessRejected)?;
    ipc::write(
        &mut pipe,
        &Message::Registered {
            binding: WireBinding::from(&binding),
        },
        SESSION_BUDGET,
    )?;
    match receive(&mut reader, &mut pipe, Instant::now() + SESSION_BUDGET)? {
        Message::Drain { binding: received } if received.matches(&binding) => {}
        Message::Cancel {} => return Ok(()),
        _ => return Err(Error::Protocol),
    }
    store::record(
        &directory,
        "drain-accepted.json",
        &binding,
        "parent-declared-clean-drain",
    )?;
    ipc::write(
        &mut pipe,
        &Message::Accepted {
            binding: WireBinding::from(&binding),
        },
        SESSION_BUDGET,
    )?;
    match receive(&mut reader, &mut pipe, Instant::now() + SESSION_BUDGET)? {
        Message::Arm { binding: received } if received.matches(&binding) => {}
        Message::Cancel {} => return Ok(()),
        _ => return Err(Error::Protocol),
    }
    // Commit point. A lost Armed reply cannot make this safely retractable.
    store::record(
        &directory,
        "witness-armed.json",
        &binding,
        "parent-explicitly-committed-handoff",
    )?;
    ipc::write(
        &mut pipe,
        &Message::Armed {
            binding: WireBinding::from(&binding),
        },
        SESSION_BUDGET,
    )?;
    let deadline = Instant::now() + SESSION_BUDGET;
    let mut eof = false;
    let mut exit_evidence = None;
    loop {
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        if !eof {
            match reader.next(&mut pipe) {
                Ok(Some(Message::Cancel {})) => return Ok(()),
                Ok(Some(_)) => return Err(Error::Protocol),
                Ok(None) => {}
                Err(Error::EndOfPipe) => {
                    reader.ensure_empty()?;
                    eof = true;
                }
                Err(error) => return Err(error),
            }
        }
        if exit_evidence.is_none() {
            match witness
                .wait(Duration::from_millis(10))
                .map_err(|_| Error::WitnessRejected)?
            {
                WaitOutcome::Pending => {}
                WaitOutcome::ParentExited(evidence) => exit_evidence = Some(evidence),
            }
        }
        if eof {
            if let Some(evidence) = &exit_evidence {
                if evidence.parent() != binding.parent() || evidence.nonce() != binding.nonce() {
                    return Err(Error::BindingMismatch);
                }
                return store::record(
                    &directory,
                    "observed-parent-exit.json",
                    &binding,
                    "declared-drain-and-kernel-parent-exit",
                );
            }
        }
        if exit_evidence.is_some() {
            thread::sleep(Duration::from_millis(5));
        }
    }
}
fn receive(reader: &mut Reader, pipe: &mut UnixStream, deadline: Instant) -> Result<Message> {
    loop {
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        if let Some(message) = reader.next(pipe)? {
            return Ok(message);
        }
        thread::sleep(Duration::from_millis(5));
    }
}
