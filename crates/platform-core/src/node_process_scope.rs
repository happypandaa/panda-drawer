//! Current-phase Node owners, never a separate process registry or backup.
use crate::{owned_process::OwnedProcessRegistry, ApiError};
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub(crate) struct NodeProcessScope {
    pub processes: OwnedProcessRegistry,
    pub owner: Arc<dyn Send + Sync>,
    #[cfg(test)]
    fault: Arc<std::sync::Mutex<Option<NodeReaderFault>>>,
}
#[cfg(test)]
pub(crate) struct NodeReaderFault {
    pub component: String,
    pub step: String,
    pub entered: tokio::sync::oneshot::Sender<()>,
    pub release: Option<tokio::sync::oneshot::Receiver<()>>,
    pub panic: bool,
    pub writer: Option<PathBuf>,
}
struct OperationOwner {
    _parent: Arc<dyn Send + Sync>,
    _directory: File,
    _path: PathBuf,
}
impl NodeProcessScope {
    pub fn new(processes: OwnedProcessRegistry, owner: Arc<dyn Send + Sync>) -> Self {
        Self {
            processes,
            owner,
            #[cfg(test)]
            fault: Arc::new(std::sync::Mutex::new(None)),
        }
    }
    pub fn standalone() -> Self {
        Self::new(OwnedProcessRegistry::default(), Arc::new(()))
    }
    /// A tool resolver has no per-record closure receipt. Never treat a lost
    /// probe reply or pending owner as an ordinary missing-tool result.
    pub fn resolution_error(issue: ApiError) -> ApiError {
        if issue.code.starts_with("PROCESS_")
            && issue.details["operationOutcome"] != "not_started"
            && issue.details.get("processResourceClosed").is_none()
        {
            issue
                .detail("processResourceClosed", false)
                .outcome("unknown")
        } else {
            issue
        }
    }
    /// Operation directories are durable evidence and have no deleting Drop.
    /// Keep their actual verified directory FD and the parent phase owner.
    pub fn with_operation(&self, path: &Path) -> Result<Self, ApiError> {
        crate::projects::check_no_symlinks(path, false)?;
        let before = std::fs::symlink_metadata(path)?;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(path)?;
        let after = directory.metadata()?;
        if !before.is_dir() || before.dev() != after.dev() || before.ino() != after.ino() {
            return Err(ApiError::new(
                "NODE_PROCESS_OPERATION_CHANGED",
                "候选操作目录身份已变化，现场保留。",
            ));
        }
        let mut child = self.clone();
        child.owner = Arc::new(OperationOwner {
            _parent: self.owner.clone(),
            _directory: directory,
            _path: path.into(),
        });
        Ok(child)
    }
    #[cfg(test)]
    pub fn with_reader_fault(self, fault: NodeReaderFault) -> Self {
        *self.fault.lock().unwrap() = Some(fault);
        self
    }
    pub async fn finish_reader(
        &self,
        component: &str,
        step: &str,
        work: impl std::future::Future<Output = Result<(), ApiError>>,
    ) -> Result<(), ApiError> {
        work.await?;
        #[cfg(test)]
        {
            let fault = self
                .fault
                .lock()
                .unwrap()
                .take_if(|fault| fault.component == component && fault.step == step);
            if let Some(fault) = fault {
                let _ = fault.entered.send(());
                assert!(!fault.panic, "controlled disposable Node reader panic");
                if let Some(release) = fault.release {
                    let _ = release.await;
                }
                if let Some(path) = fault.writer {
                    use std::io::Write;
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .custom_flags(libc::O_NOFOLLOW)
                        .open(path)?;
                    file.write_all(b"actual owned Node writer completed\n")?;
                    file.sync_all()?;
                }
            }
        }
        let _ = (component, step);
        Ok(())
    }
}
