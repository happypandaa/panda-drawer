//! A browser may outlive a crashed host. A durable session is published before
//! engine creation and is retired only after actual native family/IO closure.
//! Drop deliberately keeps it; a new host must observe a real cleanup engine's
//! Env5 exit rather than treating an absent in-memory owner as closed.
use super::*;
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Write, sync::Arc, time::Duration};

const NAME: &str = "active-browser.json";
const MAX_BYTES: u64 = 4096;

fn pending() -> ApiError {
    ApiError::new(
        "WEBVIEW_PROFILE_SESSION_RETAINED",
        "原项目浏览器会话尚未确认结束，已保留存储和原凭据。",
    )
    .outcome("unknown")
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    version: u32,
    kind: ProjectKind,
    id: String,
    identifier: [u8; 16],
    directory_identity: PersistedIdentity,
    file_identity: PersistedIdentity,
    token: String,
    boot_id: u32,
}

/// This guard grants no PID-based process action. Its only retirement API has
/// the same trusted-caller precondition as StoreLedger::forget_confirmed.
pub struct ProfileSession {
    profile: Arc<BoundProfile>,
    identity: native::FileIdentity,
    bytes: Vec<u8>,
    file: Option<File>,
    retiring: bool,
    closed: bool,
}

impl BoundProfile {
    pub fn begin_browser_session(self: &Arc<Self>) -> Result<ProfileSession> {
        self.verify()?;
        if self.recover_browser_session()?.is_some() {
            return Err(pending());
        }
        let path = self.directory.join(NAME);
        let temporary = self
            .directory
            .join(format!(".browser-preparing-{}", uuid::Uuid::new_v4()));
        let mut file = native::create_private_file_for_publication(&temporary)?;
        let identity = native::identity(&file)?;
        let record = Record {
            version: 1,
            kind: self.kind,
            id: self.id.clone(),
            identifier: self.identifier,
            directory_identity: self.identity.clone(),
            file_identity: PersistedIdentity::from_file(&file).map_err(|_| pending())?,
            token: uuid::Uuid::new_v4().to_string(),
            boot_id: platform_native::windows::boot_session::current_boot_id()?,
        };
        let bytes = serde_json::to_vec(&record)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(pending());
        }
        file.write_all(&bytes)?;
        file.sync_all()?;
        // Native rename success is publication. A failure never deletes an
        // unknown destination or hides a partially published preparation.
        native::rename_private_file_handle(&file, &path, false)?;
        drop(file);
        let file = native::open_read_file(&path)?;
        if !identity.same_object(&native::identity(&file)?) {
            return Err(pending());
        }
        self.verify()?;
        Ok(ProfileSession {
            profile: self.clone(),
            identity,
            bytes,
            file: Some(file),
            retiring: false,
            closed: false,
        })
    }

    /// Retain the exact immutable original session for a trusted invisible
    /// cleanup browser. This never declares that the old process is stopped.
    pub fn recover_browser_session(self: &Arc<Self>) -> Result<Option<ProfileSession>> {
        self.verify()?;
        let path = self.directory.join(NAME);
        let mut file = match native::open_read_file(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(pending()),
        };
        native::verify_private(&file).map_err(|_| pending())?;
        if file.metadata()?.len() > MAX_BYTES {
            return Err(pending());
        }
        let identity = native::identity(&file)?;
        let mut bytes = vec![];
        (&mut file).take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(pending());
        }
        let record: Record = serde_json::from_slice(&bytes).map_err(|_| pending())?;
        if record.version != 1
            || record.kind != self.kind
            || record.id != self.id
            || record.identifier != self.identifier
            || !uuid::Uuid::parse_str(&record.token)
                .is_ok_and(|token| !token.is_nil() && token.to_string() == record.token)
            || !record
                .directory_identity
                .matches_open_runtime(&self.scope.file)
                .map_err(|_| pending())?
            || !record
                .file_identity
                .matches_open_runtime(&file)
                .map_err(|_| pending())?
        {
            return Err(pending());
        }
        Ok(Some(ProfileSession {
            profile: self.clone(),
            identity,
            bytes,
            file: Some(file),
            retiring: false,
            closed: false,
        }))
    }
}

impl ProfileSession {
    pub fn is_finished(&self) -> bool {
        self.closed
    }
    /// Call only after real native browser-family exit, Window Destroyed and
    /// resource closure. A simulated PID or a task terminal state is insufficient.
    pub fn finish_after_native_close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.profile.verify()?;
        let path = self.profile.directory.join(NAME);
        if !self.retiring {
            let mut actual = native::open_read_file(&path).map_err(|_| pending())?;
            let identity = native::identity(&actual)?;
            let mut bytes = vec![];
            (&mut actual).take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
            if !self.identity.same_object(&identity) || bytes != self.bytes {
                return Err(pending());
            }
            drop(actual);
            // Close our original publication/read HANDLE before observing a
            // delete-pending name. Other readers still prevent success.
            drop(self.file.take());
            native::remove_file(&path, &self.identity)?;
            self.retiring = true;
        }
        for milliseconds in [0, 20, 40, 80, 160, 320, 640, 1000] {
            if milliseconds != 0 {
                std::thread::sleep(Duration::from_millis(milliseconds));
            }
            self.profile.verify()?;
            match std::fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.closed = true;
                    return Ok(());
                }
                Ok(_) => {
                    if let Ok(actual) = native::open_no_reparse(&path) {
                        if !self.identity.same_object(&actual.identity) {
                            return Err(pending());
                        }
                    }
                }
                Err(error) if matches!(error.raw_os_error(), Some(5 | 32 | 33)) => (),
                Err(_) => return Err(pending()),
            }
        }
        Err(pending())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    fn fixture(kind: ProjectKind) -> (tempfile::TempDir, StoreLedger, Arc<BoundProfile>) {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-browser-session-")
            .tempdir()
            .unwrap();
        let ledger = StoreLedger::new(temporary.path().canonicalize().unwrap());
        let id = "0a9bb0b8-ec9d-5f0a-b0ce-dd1d2d925168";
        ledger.record(kind, id).unwrap();
        let profile = Arc::new(ledger.bind_profile(kind, id).unwrap());
        (temporary, ledger, profile)
    }

    #[test]
    fn actual_app_session_is_immutable_private_and_survives_dropped_owner() {
        for kind in [ProjectKind::App] {
            let (_temporary, _ledger, profile) = fixture(kind);
            let session = profile.begin_browser_session().unwrap();
            let path = profile.directory().join(NAME);
            let bytes = std::fs::read(&path).unwrap();
            let record: Record = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(record.kind, kind);
            assert_eq!(record.identifier, profile.identifier());
            assert_eq!(
                record.boot_id,
                platform_native::windows::boot_session::current_boot_id().unwrap()
            );
            native::verify_private(&native::open_read_file(&path).unwrap()).unwrap();
            drop(session);
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert!(profile.begin_browser_session().is_err());
            let mut recovered = profile.recover_browser_session().unwrap().unwrap();
            assert!(!recovered.is_finished());
            // There is no engine in this file-lifecycle unit fixture. The
            // separate desktop fixture supplies actual Env5/Destroyed proof.
            recovered.finish_after_native_close().unwrap();
            assert!(recovered.is_finished());
            assert!(!path.exists());
            recovered.finish_after_native_close().unwrap();
        }
    }

    #[test]
    fn held_real_marker_reader_preserves_session_until_retry_and_actual_absence() {
        for share in [0x1 | 0x2, 0x1 | 0x2 | 0x4] {
            let (_temporary, _ledger, profile) = fixture(ProjectKind::App);
            let mut session = profile.begin_browser_session().unwrap();
            let path = profile.directory().join(NAME);
            let reader = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(share)
                .open(&path)
                .unwrap();
            assert!(session.finish_after_native_close().is_err());
            assert!(!session.is_finished());
            drop(reader);
            session.finish_after_native_close().unwrap();
            assert!(!path.exists());
        }
    }

    #[test]
    fn unknown_or_replaced_session_is_preserved_without_claiming_closure() {
        for malformed in [false, true] {
            let (_temporary, _ledger, profile) = fixture(ProjectKind::App);
            let session = profile.begin_browser_session().unwrap();
            let path = profile.directory().join(NAME);
            let original = std::fs::read(&path).unwrap();
            let identity = native::open_no_reparse(&path).unwrap().identity;
            drop(session);
            native::remove_file(&path, &identity).unwrap();
            let mut replacement = native::create_private_file(&path).unwrap();
            let bytes: &[u8] = if malformed { b"{unknown" } else { &original };
            replacement.write_all(bytes).unwrap();
            replacement.sync_all().unwrap();
            drop(replacement);
            assert!(profile.recover_browser_session().is_err());
            assert!(profile.begin_browser_session().is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
}
