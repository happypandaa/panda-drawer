use super::{ipc::WireBinding, Error, Result, SessionBinding};
use platform_update_witness::ProcessIdentity;
use serde::Serialize;
use std::{
    ffi::CString,
    fs::File,
    io::Write,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
};

pub struct SessionDirectory {
    pub(super) directory: File,
    pub(super) binding: SessionBinding,
}
impl SessionDirectory {
    /// parent is an explicitly opened private ledger root, never a guessed
    /// application data path. All evidence remains; there is no cleanup API.
    pub fn create_in(parent: &File) -> Result<Self> {
        validate_directory(parent)?;
        let identity = ProcessIdentity::current().map_err(|_| Error::WitnessRejected)?;
        let mut random = [0u8; 32];
        if unsafe { libc::getentropy(random.as_mut_ptr().cast(), random.len()) } != 0 {
            return Err(Error::Io);
        }
        if random.iter().all(|byte| *byte == 0) {
            return Err(Error::Io);
        }
        let nonce: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let name = CString::new(nonce.as_bytes()).map_err(|_| Error::Io)?;
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
            return Err(
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST) {
                    Error::ClaimExists
                } else {
                    Error::Io
                },
            );
        }
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(Error::Io);
        }
        let directory = unsafe { File::from_raw_fd(fd) };
        validate_directory(&directory)?;
        parent.sync_all().map_err(|_| Error::Io)?;
        Ok(Self {
            directory,
            binding: SessionBinding {
                parent: identity,
                nonce,
            },
        })
    }
    pub fn binding(&self) -> &SessionBinding {
        &self.binding
    }
}
pub(super) fn validate_directory(directory: &File) -> Result<()> {
    let metadata = directory.metadata().map_err(|_| Error::InvalidDirectory)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(Error::InvalidDirectory);
    }
    Ok(())
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Record<'a> {
    binding: WireBinding,
    state: &'a str,
    install_authorized: bool,
}
pub(super) fn claim(directory: &File, binding: &SessionBinding) -> Result<()> {
    record(directory, "claim.json", binding, "claimed")
}
pub(super) fn record(
    directory: &File,
    name: &str,
    binding: &SessionBinding,
    state: &str,
) -> Result<()> {
    validate_directory(directory)?;
    let name = CString::new(name).map_err(|_| Error::Io)?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST) {
                Error::ClaimExists
            } else {
                Error::Io
            },
        );
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    let bytes = serde_json::to_vec(&Record {
        binding: WireBinding::from(binding),
        state,
        install_authorized: false,
    })
    .map_err(|_| Error::InvalidFrame)?;
    file.write_all(&bytes).map_err(|_| Error::Io)?;
    file.sync_all().map_err(|_| Error::Io)?;
    directory.sync_all().map_err(|_| Error::Io)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_claim_is_permanent_and_never_reconstructed_from_old_ack_or_observation() {
        let root = tempfile::tempdir().unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let session = SessionDirectory::create_in(&File::open(root.path()).unwrap()).unwrap();
        claim(&session.directory, &session.binding).unwrap();
        record(
            &session.directory,
            "drain-accepted.json",
            &session.binding,
            "old",
        )
        .unwrap();
        record(
            &session.directory,
            "observed-parent-exit.json",
            &session.binding,
            "old",
        )
        .unwrap();
        assert_eq!(
            claim(&session.directory, &session.binding),
            Err(Error::ClaimExists)
        );
    }
    #[test]
    fn simultaneous_claims_have_exactly_one_durable_winner() {
        let root = tempfile::tempdir().unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let session = SessionDirectory::create_in(&File::open(root.path()).unwrap()).unwrap();
        let binding = session.binding.clone();
        let dir = session.directory.try_clone().unwrap();
        let other = std::thread::spawn(move || claim(&dir, &binding));
        let first = claim(&session.directory, &session.binding);
        let second = other.join().unwrap();
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        assert!(matches!(
            first.err().or(second.err()),
            Some(Error::ClaimExists)
        ));
    }
}
