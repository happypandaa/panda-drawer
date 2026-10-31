//! Fixed registration publication for explicit data restore. Live slot exchange
//! is owned by the transaction module. Pending access deliberately does not
//! construct a normal ProjectHandle, whose database may be displaced.
use super::*;
use crate::{control::Freeze, data_restore_guard::DataRestoreRecoveryPermit, files};
use sha2::{Digest, Sha256};
use tokio::sync::{watch, OwnedRwLockWriteGuard};
use unicode_normalization::UnicodeNormalization;

#[path = "data_restore_registration_io.rs"]
mod file_io;

const MAX_RECORD: u64 = 8 * 1024 * 1024;
const MAX_DATABASE: u64 = 256_000_000;
const APPLY_SLOT: &str = "registration-apply.json";
const UNDO_SLOT: &str = "registration-undo.json";

pub(crate) struct DataRestoreRegistrationPreparation<'a> {
    pub staging: &'a Path,
    pub candidate_database: &'a Path,
    pub apply_epoch: &'a str,
    pub undo_epoch: &'a str,
}

pub(crate) fn validate_data_restore_layout(manifest: &ProjectManifest) -> Result<(), ApiError> {
    let slots = [
        manifest.database.clone(),
        format!("{}-wal", manifest.database),
        format!("{}-shm", manifest.database),
        format!("{}-journal", manifest.database),
        manifest.attachments_dir.clone(),
    ];
    let mut normalized = std::collections::BTreeMap::new();
    let mut folded = Vec::new();
    for slot in &slots {
        let parts = files::parts(slot)?;
        let mut prefix = String::new();
        for part in parts {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let key = prefix
                .nfd()
                .flat_map(char::to_lowercase)
                .collect::<String>();
            if normalized
                .insert(key, prefix.clone())
                .is_some_and(|previous| previous != prefix)
            {
                return Err(invalid());
            }
        }
        folded.push(slot.nfd().flat_map(char::to_lowercase).collect::<String>());
    }
    for (index, left) in folded.iter().enumerate() {
        for right in folded.iter().skip(index + 1) {
            if left == right
                || left.starts_with(&format!("{right}/"))
                || right.starts_with(&format!("{left}/"))
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

pub(crate) use crate::data_restore_proof_io::FileProof;

/// New restore evidence accepts only the exact stable identity object. The
/// global identity reader intentionally still supports older project records.
pub(crate) mod strict_identity {
    use crate::file_identity::PersistedIdentity;
    use serde::{Deserialize, Deserializer};

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Identity {
        device: u64,
        inode: u64,
        volume_uuid: Option<String>,
        #[serde(default)]
        windows_file_id: Option<[u8; 16]>,
    }
    impl From<Identity> for PersistedIdentity {
        fn from(value: Identity) -> Self {
            Self {
                device: value.device,
                inode: value.inode,
                volume_uuid: value.volume_uuid,
                windows_file_id: value.windows_file_id,
            }
        }
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PersistedIdentity, D::Error> {
        Identity::deserialize(deserializer).map(Into::into)
    }
    pub(crate) fn optional<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PersistedIdentity>, D::Error> {
        Option::<Identity>::deserialize(deserializer).map(|identity| identity.map(Into::into))
    }
}
impl FileProof {
    pub(crate) fn verify(&self, path: &Path, maximum: u64) -> Result<(), ApiError> {
        if self.matches(&read_proof(path, maximum, false)?.0) {
            Ok(())
        } else {
            Err(changed())
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistrationImage {
    proof: FileProof,
    document: String,
}
impl RegistrationImage {
    fn registration(&self) -> Result<Registration, ApiError> {
        if self.document.len() as u64 != self.proof.bytes
            || self.proof.bytes > MAX_RECORD
            || digest(self.document.as_bytes()) != self.proof.sha256
            || self.proof.identity.is_legacy()
        {
            return Err(invalid());
        }
        read_registration(self.document.as_bytes())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DataRestoreRegistrationPlan {
    before: RegistrationImage,
    apply: RegistrationImage,
    undo: RegistrationImage,
    source_database: FileProof,
    target_database: FileProof,
    manifest_sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DataRestoreDirection {
    Continue,
    Undo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DataRestoreEpochState {
    Before,
    Applied,
    Undone,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DataRestoreEpochCommit {
    pub document_epoch: String,
    pub state: DataRestoreEpochState,
    pub already_committed: bool,
}

impl DataRestoreRegistrationPlan {
    /// Windows-only exchange material is counted and recognized by exact plan
    /// binding. Unix has no extra namespace between its atomic swap states.
    pub(crate) fn exchange_metadata(
        &self,
        workspace: &Path,
        id: &str,
        operation: &Path,
        operation_identity: &FileIdentity,
    ) -> Result<Vec<(String, u64)>, ApiError> {
        Ok(file_io::exchange_metadata(self, workspace, id, operation, operation_identity)?.entries)
    }
    #[cfg(windows)]
    pub(crate) fn visible_state_at(
        &self,
        workspace: &Path,
        id: &str,
        operation: &Path,
    ) -> Result<DataRestoreEpochState, ApiError> {
        self.verify_root(workspace, id)?;
        if let Some(state) = file_io::pending_state(self, workspace, id, operation)? {
            return Ok(state);
        }
        self.visible_state(workspace, id)
    }
    pub(crate) fn epoch(&self, direction: DataRestoreDirection) -> Result<String, ApiError> {
        Ok(if direction == DataRestoreDirection::Continue {
            self.apply.registration()?
        } else {
            self.undo.registration()?
        }
        .document_epoch)
    }
    pub(crate) fn visible_state(
        &self,
        workspace: &Path,
        id: &str,
    ) -> Result<DataRestoreEpochState, ApiError> {
        let observed = read_proof(&registration_path(workspace, id), MAX_RECORD, false)?.0;
        if self.before.proof.matches(&observed) {
            Ok(DataRestoreEpochState::Before)
        } else if self.apply.proof.matches(&observed) {
            Ok(DataRestoreEpochState::Applied)
        } else if self.undo.proof.matches(&observed) {
            Ok(DataRestoreEpochState::Undone)
        } else {
            Err(changed())
        }
    }
    pub(crate) fn validate_shape(&self, id: &str) -> Result<(), ApiError> {
        let before = self.before.registration()?;
        let apply = self.apply.registration()?;
        let undo = self.undo.registration()?;
        let epochs = [
            &before.document_epoch,
            &apply.document_epoch,
            &undo.document_epoch,
        ];
        if epochs.iter().any(|value| {
            Uuid::parse_str(value)
                .map_or(true, |epoch| epoch.is_nil() || epoch.to_string() != **value)
        }) || epochs[0] == epochs[1]
            || epochs[0] == epochs[2]
            || epochs[1] == epochs[2]
            || !valid_digest(&self.manifest_sha256)
            || self.source_database.identity.is_legacy()
            || self.target_database.identity.is_legacy()
        {
            return Err(invalid());
        }
        for registration in [&before, &apply, &undo] {
            if registration.schema_version != 1
                || registration.id != id
                || registration.project_root != before.project_root
                || registration.directory_name != before.directory_name
                || registration.database_path != before.database_path
                || !registration
                    .root_identity
                    .matches_captured(&before.root_identity)
                || registration.root_identity.is_legacy()
            {
                return Err(invalid());
            }
        }
        if !before
            .database_identity
            .matches_captured(&self.source_database.identity)
            || !undo
                .database_identity
                .matches_captured(&self.source_database.identity)
            || !apply
                .database_identity
                .matches_captured(&self.target_database.identity)
        {
            return Err(invalid());
        }
        for proof in [&self.source_database, &self.target_database] {
            if proof.bytes > MAX_DATABASE || !valid_digest(&proof.sha256) {
                return Err(invalid());
            }
        }
        Ok(())
    }
    /// Root/manifest checks work while the database is absent and never open it.
    pub(crate) fn verify_root(&self, workspace: &Path, id: &str) -> Result<PathBuf, ApiError> {
        self.validate_shape(id)?;
        let registration = self.before.registration()?;
        let root = registration_root(workspace, id, &registration)?;
        if !registration
            .root_identity
            .matches_path(&root)
            .map_err(|_| changed())?
        {
            return Err(changed());
        }
        let (_, bytes) = read_proof(&root.join("project.json"), MAX_RECORD, true)?;
        if digest(&bytes) != self.manifest_sha256 {
            return Err(changed());
        }
        let manifest: ProjectManifest = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        validate_manifest(&manifest, id, &root)?;
        validate_data_restore_layout(&manifest)?;
        if manifest.database != registration.database_path {
            return Err(changed());
        }
        Ok(root)
    }
    pub(crate) fn epoch_state(
        &self,
        workspace: &Path,
        id: &str,
        operation: &Path,
    ) -> Result<DataRestoreEpochState, ApiError> {
        self.verify_root(workspace, id)?;
        #[cfg(windows)]
        if let Some(state) = file_io::pending_state(self, workspace, id, operation)? {
            return Ok(state);
        }
        let live = read_proof(&registration_path(workspace, id), MAX_RECORD, false)?.0;
        let apply = read_proof(&operation.join(APPLY_SLOT), MAX_RECORD, false)?.0;
        let undo = read_proof(&operation.join(UNDO_SLOT), MAX_RECORD, false)?.0;
        if self.before.proof.matches(&live)
            && self.apply.proof.matches(&apply)
            && self.undo.proof.matches(&undo)
        {
            Ok(DataRestoreEpochState::Before)
        } else if self.apply.proof.matches(&live)
            && self.before.proof.matches(&apply)
            && self.undo.proof.matches(&undo)
        {
            Ok(DataRestoreEpochState::Applied)
        } else if self.undo.proof.matches(&live)
            && self.before.proof.matches(&undo)
            && self.apply.proof.matches(&apply)
        {
            Ok(DataRestoreEpochState::Undone)
        } else {
            Err(changed())
        }
    }
}

pub(crate) struct DataRestoreRecoveryAccess {
    workspace: PathBuf,
    id: String,
    root: PathBuf,
    operation: PathBuf,
    #[cfg(windows)]
    operation_identity: FileIdentity,
    plan: DataRestoreRegistrationPlan,
    permit: DataRestoreRecoveryPermit,
    gate: Arc<RwLock<()>>,
    control: Arc<crate::control::ProjectControl>,
    _lease: ProjectTaskLease,
}
pub(crate) struct DataRestoreExclusiveAccess {
    access: DataRestoreRecoveryAccess,
    _gate: OwnedRwLockWriteGuard<()>,
    _freeze: Freeze,
}
impl DataRestoreRecoveryAccess {
    pub(crate) fn with_lock(
        self,
        gate: OwnedRwLockWriteGuard<()>,
        freeze: Freeze,
    ) -> Result<DataRestoreExclusiveAccess, ApiError> {
        if !Arc::ptr_eq(OwnedRwLockWriteGuard::rwlock(&gate), &self.gate) {
            return Err(invalid());
        }
        self.verify()?;
        #[cfg(windows)]
        {
            file_io::stabilize(&self)?;
            self.verify()?;
        }
        Ok(DataRestoreExclusiveAccess {
            access: self,
            _gate: gate,
            _freeze: freeze,
        })
    }
    fn verify(&self) -> Result<(), ApiError> {
        self.permit.revalidate(&self.workspace, &self.id)?;
        self.plan.verify_root(&self.workspace, &self.id)?;
        self.plan
            .epoch_state(&self.workspace, &self.id, &self.operation)?;
        Ok(())
    }
    pub(crate) async fn lock(
        self,
        mut stop: watch::Receiver<bool>,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<DataRestoreExclusiveAccess, ApiError> {
        self.verify()?;
        if *stop.borrow()
            || *cancel.borrow()
            || stop.has_changed().is_err()
            || cancel.has_changed().is_err()
        {
            return Err(stopped());
        }
        let control = self.control.get();
        if control.mode != "ready" || control.recovery_required {
            return Err(changed());
        }
        let freeze = Freeze::new(self.control.clone());
        let gate = tokio::select! { biased;
            _ = stop.changed() => return Err(stopped()),
            _ = cancel.changed() => return Err(stopped()),
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => return Err(ApiError::new("PROJECT_DATA_RESTORE_BUSY","业务尚未排空；原现场保持不变。")),
            gate = self.gate.clone().write_owned() => gate,
        };
        self.verify()?;
        #[cfg(windows)]
        {
            file_io::stabilize(&self)?;
            self.verify()?;
        }
        Ok(DataRestoreExclusiveAccess {
            access: self,
            _gate: gate,
            _freeze: freeze,
        })
    }
}

impl ProjectStore {
    /// Prepare only two private registration images. The caller owns the task
    /// lease and has quiesced/closed SQLite before recording the raw DB proof.
    pub(crate) fn prepare_data_restore_registration(
        &self,
        id: &str,
        lease: &ProjectTaskLease,
        gate: &OwnedRwLockWriteGuard<()>,
        input: DataRestoreRegistrationPreparation<'_>,
    ) -> Result<DataRestoreRegistrationPlan, ApiError> {
        let DataRestoreRegistrationPreparation {
            staging,
            candidate_database,
            apply_epoch,
            undo_epoch,
        } = input;
        self.verify_data_restore_lease(id, lease)?;
        self.check_development_available(id)?;
        if !Arc::ptr_eq(OwnedRwLockWriteGuard::rwlock(gate), &self.gate(id)) {
            return Err(invalid());
        }
        let project = self.project(id)?;
        validate_data_restore_layout(&project.manifest)?;
        let control = project.control.get();
        if control.mode != "ready" || control.recovery_required {
            return Err(changed());
        }
        if !staging.starts_with(self.workspace.join(".platform"))
            || staging.starts_with(&project.root)
            || !candidate_database.starts_with(staging)
        {
            return Err(invalid());
        }
        let _ = files::root(staging)?;
        let record = registration_path(&self.workspace, id);
        let (proof, bytes) = read_proof(&record, MAX_RECORD, true)?;
        let before = RegistrationImage {
            proof,
            document: String::from_utf8(bytes).map_err(|_| invalid())?,
        };
        let source = before.registration()?;
        if [apply_epoch, undo_epoch].into_iter().any(|value| {
            Uuid::parse_str(value)
                .map_or(true, |epoch| epoch.is_nil() || epoch.to_string() != value)
        }) || apply_epoch == undo_epoch
            || apply_epoch == source.document_epoch
            || undo_epoch == source.document_epoch
        {
            return Err(invalid());
        }
        let source_database = read_proof(
            &project.root.join(&project.manifest.database),
            MAX_DATABASE,
            false,
        )?
        .0;
        let target_database = read_proof(candidate_database, MAX_DATABASE, false)?.0;
        if !file_io::same_volume(&source_database.identity, &target_database.identity) {
            return Err(invalid());
        }
        let mut next = source.clone();
        next.database_identity = target_database.identity.clone();
        next.document_epoch = apply_epoch.into();
        let apply = create_image(&staging.join(APPLY_SLOT), &next)?;
        let mut reversed = source;
        reversed.document_epoch = undo_epoch.into();
        let undo = create_image(&staging.join(UNDO_SLOT), &reversed)?;
        let plan = DataRestoreRegistrationPlan {
            before,
            apply,
            undo,
            source_database,
            target_database,
            manifest_sha256: read_proof(&project.root.join("project.json"), MAX_RECORD, false)?
                .0
                .sha256,
        };
        plan.validate_shape(id)?;
        plan.before.proof.verify(&record, MAX_RECORD)?;
        project.check()?;
        file_io::sync_directory(staging)?;
        Ok(plan)
    }
    fn verify_data_restore_lease(
        &self,
        id: &str,
        lease: &ProjectTaskLease,
    ) -> Result<(), ApiError> {
        if lease.id != id || !Arc::ptr_eq(&lease.active, &self.active_tasks) {
            return Err(invalid());
        }
        Ok(())
    }
    pub(crate) fn reserve_data_restore_recovery_task(
        &self,
        id: &str,
        task: &str,
        permit: &DataRestoreRecoveryPermit,
    ) -> Result<ProjectTaskLease, ApiError> {
        self.check_not_deleting(id)?;
        let lease = self.reserve_operation(id, ProjectOperation::Task(task.into()))?;
        crate::source_restore_guard::check_development_available(&self.workspace, id)?;
        crate::project_git::check_data_operations_available(&self.workspace, id)?;
        permit.revalidate(&self.workspace, id)?;
        Ok(lease)
    }
    pub(crate) fn data_restore_recovery_access(
        &self,
        id: &str,
        lease: ProjectTaskLease,
        permit: DataRestoreRecoveryPermit,
    ) -> Result<DataRestoreRecoveryAccess, ApiError> {
        self.accepting()?;
        self.check_not_deleting(id)?;
        self.require_active_identity(id)?;
        self.verify_data_restore_lease(id, &lease)?;
        let (operation, header) = permit.retained(&self.workspace, id)?;
        let root = header.registration_plan.verify_root(&self.workspace, id)?;
        header
            .registration_plan
            .epoch_state(&self.workspace, id, &operation)?;
        let control = file_io::recovery_control(
            self,
            id,
            &operation,
            &header.operation_identity,
            &header.registration_plan,
        )?;
        Ok(DataRestoreRecoveryAccess {
            workspace: self.workspace.clone(),
            id: id.into(),
            root,
            operation,
            #[cfg(windows)]
            operation_identity: header.operation_identity,
            plan: header.registration_plan,
            permit,
            gate: self.gate(id),
            control,
            _lease: lease,
        })
    }
}

impl DataRestoreExclusiveAccess {
    pub(crate) fn root(&self) -> &Path {
        &self.access.root
    }
    pub(crate) fn operation(&self) -> &Path {
        &self.access.operation
    }
    pub(crate) fn epoch_state(&self) -> Result<DataRestoreEpochState, ApiError> {
        self.access.plan.epoch_state(
            &self.access.workspace,
            &self.access.id,
            &self.access.operation,
        )
    }
    /// The only publication here is registration metadata. A future transaction
    /// must install and verify the appropriate DB before asking for its epoch.
    pub(crate) fn publish_epoch(
        &self,
        direction: DataRestoreDirection,
    ) -> Result<DataRestoreEpochCommit, ApiError> {
        self.publish_epoch_with(direction, || Ok(()), || Ok(()))
    }
    fn publish_epoch_with(
        &self,
        direction: DataRestoreDirection,
        before_swap: impl FnOnce() -> Result<(), ApiError>,
        after_swap: impl FnOnce() -> Result<(), ApiError>,
    ) -> Result<DataRestoreEpochCommit, ApiError> {
        self.access.verify()?;
        #[cfg(windows)]
        {
            file_io::stabilize(&self.access)?;
            self.access.verify()?;
        }
        let plan = &self.access.plan;
        let wanted = if direction == DataRestoreDirection::Continue {
            DataRestoreEpochState::Applied
        } else {
            DataRestoreEpochState::Undone
        };
        let result = (|| {
            let source = plan.before.registration()?;
            let database = checked_relative(&self.access.root, &source.database_path, false)?;
            let proof = if direction == DataRestoreDirection::Continue {
                &plan.target_database
            } else {
                &plan.source_database
            };
            proof.verify(&database, MAX_DATABASE)?;
            let state = plan.epoch_state(
                &self.access.workspace,
                &self.access.id,
                &self.access.operation,
            )?;
            let already_committed = state == wanted;
            if state != DataRestoreEpochState::Before && !already_committed {
                return Err(changed());
            }
            let record = registration_path(&self.access.workspace, &self.access.id);
            #[cfg(not(windows))]
            let slot = self
                .access
                .operation
                .join(if direction == DataRestoreDirection::Continue {
                    APPLY_SLOT
                } else {
                    UNDO_SLOT
                });
            if !already_committed {
                before_swap()?;
                #[cfg(not(windows))]
                swap_preserving(&record, &slot)?;
                #[cfg(windows)]
                file_io::exchange(&self.access, direction)?;
                after_swap()?;
            }
            if plan.epoch_state(
                &self.access.workspace,
                &self.access.id,
                &self.access.operation,
            )? != wanted
            {
                return Err(changed());
            }
            file_io::sync_directory(record.parent().unwrap())?;
            file_io::sync_directory(&self.access.operation)?;
            let registration = if wanted == DataRestoreEpochState::Applied {
                plan.apply.registration()?
            } else {
                plan.undo.registration()?
            };
            Ok(DataRestoreEpochCommit {
                document_epoch: registration.document_epoch,
                state: wanted,
                already_committed,
            })
        })();
        result.map_err(|error: ApiError| {
            let committed = if direction == DataRestoreDirection::Continue {
                &plan.apply
            } else {
                &plan.undo
            };
            let record = registration_path(&self.access.workspace, &self.access.id);
            error
                .outcome(if committed.proof.verify(&record, MAX_RECORD).is_ok() {
                    "committed"
                } else {
                    "unknown"
                })
                .detail("pending", true)
                .detail("businessBlocked", true)
        })
    }
}

fn create_image(path: &Path, registration: &Registration) -> Result<RegistrationImage, ApiError> {
    let bytes = serde_json::to_vec_pretty(registration)?;
    let parent = files::root(path.parent().ok_or_else(invalid)?)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?;
    let mut file = files::create(&parent, name)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    let proof = read_proof(path, MAX_RECORD, false)?.0;
    Ok(RegistrationImage {
        proof,
        document: String::from_utf8(bytes).map_err(|_| invalid())?,
    })
}
fn read_registration(bytes: &[u8]) -> Result<Registration, ApiError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let object = value.as_object().ok_or_else(invalid)?;
    if object.keys().any(|key| {
        ![
            "schemaVersion",
            "id",
            "projectRoot",
            "directoryName",
            "rootIdentity",
            "databasePath",
            "databaseIdentity",
            "documentEpoch",
        ]
        .contains(&key.as_str())
    }) {
        return Err(invalid());
    }
    for key in ["rootIdentity", "databaseIdentity"] {
        let identity = object
            .get(key)
            .and_then(Value::as_object)
            .ok_or_else(invalid)?;
        if identity
            .keys()
            .any(|key| !file_io::identity_key_allowed(key))
        {
            return Err(invalid());
        }
    }
    serde_json::from_value(value).map_err(|_| invalid())
}
pub(crate) fn read_proof(
    path: &Path,
    maximum: u64,
    keep: bool,
) -> Result<(FileProof, Vec<u8>), ApiError> {
    crate::data_restore_proof_io::read_proof(path, maximum, keep, invalid, changed)
}
#[cfg(target_os = "macos")]
fn swap_preserving(from: &Path, to: &Path) -> Result<(), ApiError> {
    use std::{
        ffi::CString,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
    };
    let left = files::root(from.parent().ok_or_else(invalid)?)?;
    let right = files::root(to.parent().ok_or_else(invalid)?)?;
    if left.metadata()?.dev() != right.metadata()?.dev() {
        return Err(invalid());
    }
    let left_name =
        CString::new(from.file_name().ok_or_else(invalid)?.as_bytes()).map_err(|_| invalid())?;
    let right_name =
        CString::new(to.file_name().ok_or_else(invalid)?.as_bytes()).map_err(|_| invalid())?;
    if unsafe {
        libc::renameatx_np(
            left.as_raw_fd(),
            left_name.as_ptr(),
            right.as_raw_fd(),
            right_name.as_ptr(),
            libc::RENAME_SWAP,
        )
    } != 0
    {
        return Err(ApiError::new(
            "PROJECT_DATA_RESTORE_REGISTRATION_MOVE_FAILED",
            "注册代次发布未完成；现有记录及原现场均保留。",
        ));
    }
    Ok(())
}
#[cfg(not(any(target_os = "macos", windows)))]
fn swap_preserving(_from: &Path, _to: &Path) -> Result<(), ApiError> {
    Err(ApiError::new(
        "UNSUPPORTED_PLATFORM",
        "数据取回注册事务目前仅验证macOS。",
    ))
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn invalid() -> ApiError {
    ApiError::new(
        "PROJECT_DATA_RESTORE_INVALID",
        "数据取回注册证据无法核验；现有记录和数据保持保全。",
    )
}
fn changed() -> ApiError {
    ApiError::new(
        "PROJECT_DATA_RESTORE_CHANGED",
        "数据取回身份、注册或候选记录已变化；保留现场并停止操作。",
    )
}
fn stopped() -> ApiError {
    ApiError::new(
        "PROJECT_DATA_RESTORE_CANCELLED",
        "等待业务排空已停止；原现场保持不变。",
    )
}

#[cfg(all(test, unix))]
#[path = "project_data_restore_registration_tests.rs"]
mod tests;
