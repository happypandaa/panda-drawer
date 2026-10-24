//! Recoverable five-slot data exchange. The immutable plan and actual objects,
//! rather than progress labels, determine every permitted move and publication.
use super::*;
use crate::{
    data_restore_guard as guard,
    file_identity::PersistedIdentity,
    files,
    projects::data_restore_registration::{
        self as registration, DataRestoreDirection, DataRestoreEpochState,
        DataRestoreExclusiveAccess, DataRestoreRegistrationPlan, FileProof,
    },
};
use slot_files::{DirectoryProof, EntryProof};

pub(super) const POLICY: &str = "project-data-restore-v1";
const JOURNAL_MAX: u64 = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Slot {
    pub kind: String,
    pub path: String,
    pub before: Option<EntryProof>,
    pub after: Option<EntryProof>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Binding {
    pub git: DirectoryProof,
    pub metadata: BTreeMap<String, Option<EntryProof>>,
    pub roots: Vec<String>,
    pub source_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Plan {
    pub schema_version: u32,
    pub policy: String,
    pub project_id: String,
    pub task_id: String,
    pub target_commit: String,
    pub preview_plan_digest: String,
    #[serde(deserialize_with = "registration::strict_identity::deserialize")]
    pub operation_identity: PersistedIdentity,
    pub registration_plan: DataRestoreRegistrationPlan,
    pub manifest: ProjectManifest,
    pub parents: BTreeMap<String, DirectoryProof>,
    pub operation_parents: BTreeMap<String, DirectoryProof>,
    pub metadata_reserve: BTreeMap<String, FileProof>,
    #[serde(deserialize_with = "registration::strict_identity::deserialize")]
    pub journal_identity: PersistedIdentity,
    pub binding: Binding,
    pub current: version_data::DataSnapshotManifest,
    pub target: version_data::DataSnapshotManifest,
    pub live_data_digest: String,
    pub target_snapshot_digest: String,
    pub slots: Vec<Slot>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Journal {
    pub schema_version: u32,
    pub operation: String,
    pub project_id: String,
    pub task_id: String,
    pub phase: String,
    pub plan_digest: String,
    #[serde(deserialize_with = "registration::strict_identity::deserialize")]
    pub operation_identity: PersistedIdentity,
    pub registration_plan: DataRestoreRegistrationPlan,
    pub transaction_plan: Option<FileProof>,
    #[serde(deserialize_with = "registration::strict_identity::deserialize")]
    pub journal_identity: PersistedIdentity,
}
impl Journal {
    pub fn header(&self) -> guard::DataRestoreJournalHeader {
        guard::DataRestoreJournalHeader {
            schema_version: self.schema_version,
            operation: self.operation.clone(),
            project_id: self.project_id.clone(),
            task_id: self.task_id.clone(),
            phase: self.phase.clone(),
            plan_digest: self.plan_digest.clone(),
            operation_identity: self.operation_identity.clone(),
            registration_plan: self.registration_plan.clone(),
            transaction_plan: self.transaction_plan.clone(),
            journal_identity: Some(self.journal_identity.clone()),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Completion {
    pub schema_version: u32,
    pub policy: String,
    pub project_id: String,
    pub task_id: String,
    pub plan_sha256: String,
    pub journal_sha256: String,
    #[serde(deserialize_with = "registration::strict_identity::deserialize")]
    pub record_identity: PersistedIdentity,
    pub direction: DataRestoreDirection,
    pub document_epoch: String,
    pub terminal_summary: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Terminal {
    schema_version: u32,
    policy: String,
    project_id: String,
    task_id: String,
    plan_sha256: String,
    journal_sha256: String,
    completion_sha256: String,
    direction: DataRestoreDirection,
    document_epoch: String,
    terminal_summary: String,
    phase: String,
    #[serde(deserialize_with = "registration::strict_identity::deserialize")]
    record_identity: PersistedIdentity,
}
impl PartialEq for Completion {
    fn eq(&self, other: &Self) -> bool {
        matches!((serde_json::to_value(self),serde_json::to_value(other)),(Ok(left),Ok(right)) if left==right)
    }
}
impl Eq for Completion {}
impl PartialEq for Terminal {
    fn eq(&self, other: &Self) -> bool {
        matches!((serde_json::to_value(self),serde_json::to_value(other)),(Ok(left),Ok(right)) if left==right)
    }
}
impl Eq for Terminal {}
impl Terminal {
    fn new(
        completion: &Completion,
        completion_bytes: &[u8],
        record_identity: PersistedIdentity,
    ) -> Self {
        Self {
            schema_version: 1,
            policy: POLICY.into(),
            project_id: completion.project_id.clone(),
            task_id: completion.task_id.clone(),
            plan_sha256: completion.plan_sha256.clone(),
            journal_sha256: completion.journal_sha256.clone(),
            completion_sha256: sha256(completion_bytes),
            direction: completion.direction,
            document_epoch: completion.document_epoch.clone(),
            terminal_summary: completion.terminal_summary.clone(),
            phase: if completion.direction == DataRestoreDirection::Continue {
                "completed"
            } else {
                "undone"
            }
            .into(),
            record_identity,
        }
    }
}

impl Plan {
    pub fn validate(&self, header: &guard::DataRestoreJournalHeader) -> Result<(), ApiError> {
        if self.schema_version != 1
            || self.policy != POLICY
            || self.project_id != header.project_id
            || self.task_id != header.task_id
            || self.preview_plan_digest != header.plan_digest
            || !valid_oid(&self.target_commit)
            || self.target_commit.len() != 40
            || !self
                .operation_identity
                .matches_captured(&header.operation_identity)
            || serde_json::to_vec(&self.registration_plan)?
                != serde_json::to_vec(&header.registration_plan)?
            || self.slots.len() != 5
            || !valid_sha(&self.live_data_digest)
            || !valid_sha(&self.target_snapshot_digest)
        {
            return Err(invalid());
        }
        registration::validate_data_restore_layout(&self.manifest)?;
        if self.journal_identity.is_legacy()
            || header
                .journal_identity
                .as_ref()
                .is_none_or(|identity| !identity.matches_captured(&self.journal_identity))
        {
            return Err(invalid());
        }
        if self
            .metadata_reserve
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != ["completion", "terminal"]
        {
            return Err(invalid());
        }
        for proof in self.metadata_reserve.values() {
            if proof.identity.is_legacy()
                || proof.bytes != 0
                || proof.sha256 != sha256(&[])
                || proof.mode & 0o777 != 0o600
            {
                return Err(invalid());
            }
        }
        self.registration_plan.validate_shape(&self.project_id)?;
        let expected = slot_paths(&self.manifest);
        for (slot, (kind, path)) in self.slots.iter().zip(expected) {
            if slot.kind != kind || slot.path != path {
                return Err(invalid());
            }
            for proof in [&slot.before, &slot.after].into_iter().flatten() {
                proof.validate_shape()?;
            }
            if (kind == "database" || kind == "attachments")
                && (slot.before.is_none() || slot.after.is_none())
            {
                return Err(invalid());
            }
            if !matches!(kind, "database" | "attachments") && slot.after.is_some() {
                return Err(invalid());
            }
        }
        if self.manifest.id != self.project_id
            || self.current.project_id != self.project_id
            || self.target.project_id != self.project_id
            || sha256(&serde_json::to_vec_pretty(&self.current)?) != self.live_data_digest
            || sha256(&serde_json::to_vec_pretty(&self.target)?) != self.target_snapshot_digest
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn completion(
        &self,
        sha: &str,
        journal_sha: &str,
        direction: DataRestoreDirection,
        record_identity: PersistedIdentity,
    ) -> Result<Completion, ApiError> {
        let epoch = self.registration_plan.epoch(direction)?;
        Ok(Completion {
            schema_version: 1,
            policy: POLICY.into(),
            project_id: self.project_id.clone(),
            task_id: self.task_id.clone(),
            plan_sha256: sha.into(),
            journal_sha256: journal_sha.into(),
            record_identity,
            direction,
            document_epoch: epoch.clone(),
            terminal_summary: sha256(&serde_json::to_vec(
                &json!({"policy":POLICY,"projectId":self.project_id,"taskId":self.task_id,"planSha256":sha,"journalSha256":journal_sha,"direction":direction,"documentEpoch":epoch,"allSlots":if direction==DataRestoreDirection::Continue{"applied"}else{"before"},"originalRetained":true}),
            )?),
        })
    }
}
pub(super) fn slot_paths(manifest: &ProjectManifest) -> Vec<(&'static str, String)> {
    vec![
        ("database", manifest.database.clone()),
        ("wal", format!("{}-wal", manifest.database)),
        ("shm", format!("{}-shm", manifest.database)),
        ("journal", format!("{}-journal", manifest.database)),
        ("attachments", manifest.attachments_dir.clone()),
    ]
}

pub(super) fn binding(
    project: &ProjectHandle,
    snapshot: &Snapshot,
    check: &mut impl FnMut() -> Result<(), ApiError>,
) -> Result<Binding, ApiError> {
    let root = project.root.join(".git");
    let mut metadata = BTreeMap::new();
    for name in [
        "HEAD",
        "config",
        "index",
        "packed-refs",
        "shallow",
        "info/attributes",
        "info/exclude",
        "refs",
    ] {
        metadata.insert(
            name.into(),
            EntryProof::capture(&root.join(name), name == "refs", check)?,
        );
    }
    Ok(Binding {
        git: DirectoryProof::capture(&root)?,
        metadata,
        roots: snapshot.view.scope.included_roots.clone(),
        source_digest: snapshot.source_digest.clone(),
    })
}
pub(super) fn verify_binding(
    root: &Path,
    plan: &Plan,
    check: &mut impl FnMut() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    check()?;
    let git = root.join(".git");
    if !plan.binding.git.matches(&DirectoryProof::capture(&git)?) {
        return Err(changed());
    }
    for (name, proof) in &plan.binding.metadata {
        if !slot_files::same_optional(
            proof,
            &EntryProof::capture(&git.join(name), name == "refs", check)?,
        ) {
            return Err(changed());
        }
    }
    let mut issues = vec![];
    inspect_repository_files(&git, None, &mut issues)?;
    if !issues.is_empty() {
        return Err(changed().detail("issues", json!(issues)));
    }
    if source_fingerprint(&scan_source(root, &plan.manifest, &plan.binding.roots)?)?
        != plan.binding.source_digest
    {
        return Err(changed());
    }
    Ok(())
}
pub(super) fn load(
    root: &Path,
    id: &str,
    task: &str,
) -> Result<(Journal, Plan, Vec<u8>), ApiError> {
    let bytes = read_file(&root.join("journal.json"), JOURNAL_MAX)?;
    let journal: Journal = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if journal.schema_version != 1
        || journal.operation != "dataRestore"
        || journal.project_id != id
        || journal.task_id != task
        || journal.phase != "prepared"
        || !valid_sha(&journal.plan_digest)
    {
        return Err(invalid());
    }
    let proof = journal.transaction_plan.as_ref().ok_or_else(invalid)?;
    let plan_bytes = read_file(&root.join("plan.json"), MAX_METADATA)?;
    if plan_bytes.len() as u64 != proof.bytes || sha256(&plan_bytes) != proof.sha256 {
        return Err(invalid());
    }
    let plan: Plan = serde_json::from_slice(&plan_bytes).map_err(|_| invalid())?;
    plan.validate(&journal.header())?;
    Ok((journal, plan, bytes))
}
pub(crate) fn inspect_completion(
    root: &Path,
    header: &guard::DataRestoreJournalHeader,
    bytes: &[u8],
    completion: Option<&[u8]>,
    terminal: Option<&[u8]>,
) -> Result<bool, ApiError> {
    let plan: Plan = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    plan.validate(header)?;
    let (journal, _, journal_bytes) = load(root, &header.project_id, &header.task_id)?;
    if serde_json::to_vec(&journal.header())? != serde_json::to_vec(header)? {
        return Err(invalid());
    }
    for entry in fs::read_dir(root)? {
        let name = entry?.file_name().into_string().map_err(|_| invalid())?;
        if (name.starts_with("completion") && name != "completion.json")
            || (name.starts_with("terminal") && name != "terminal.json")
        {
            return Err(invalid());
        }
    }
    let Some(bytes) = completion else {
        return if terminal.is_some() {
            Err(invalid())
        } else {
            Ok(false)
        };
    };
    if bytes.len() > 16 * 1024 {
        return Err(invalid());
    }
    let receipt: Completion = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if receipt.record_identity.is_legacy()
        || fs::symlink_metadata(root.join("completion.json"))?.mode() & 0o777 != 0o600
    {
        return Err(invalid());
    }
    let sha = &header.transaction_plan.as_ref().ok_or_else(invalid)?.sha256;
    if receipt
        != plan.completion(
            sha,
            &sha256(&journal_bytes),
            receipt.direction,
            receipt.record_identity.clone(),
        )?
    {
        return Err(invalid());
    }
    let Some(terminal) = terminal else {
        if receipt.record_identity.is_legacy()
            || !receipt
                .record_identity
                .matches_path(&root.join("completion.json"))
                .map_err(|_| changed())?
        {
            return Err(changed());
        }
        return Ok(false);
    };
    let actual: Terminal = serde_json::from_slice(terminal).map_err(|_| invalid())?;
    if actual.record_identity.is_legacy() {
        return Err(invalid());
    }
    if actual != Terminal::new(&receipt, bytes, actual.record_identity.clone()) {
        return Err(invalid());
    }
    let mode = fs::symlink_metadata(root.join("terminal.json"))?.mode() & 0o777;
    if mode == 0o600 {
        if !actual
            .record_identity
            .matches_path(&root.join("terminal.json"))
            .map_err(|_| changed())?
            || !receipt
                .record_identity
                .matches_path(&root.join("completion.json"))
                .map_err(|_| changed())?
        {
            return Err(changed());
        }
        return Ok(false);
    }
    if mode != 0o400 {
        return Err(changed());
    }
    Ok(true)
}

// Failed candidates are bounded retained evidence, never authorization.
pub(crate) fn metadata_usage(operation: &Path) -> Result<(usize, u64), ApiError> {
    let root = files::root(&operation.join("metadata-attempts"))?;
    let mut count = 0usize;
    let mut total = 0u64;
    for entry in fs::read_dir(operation.join("metadata-attempts"))? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| invalid())?;
        let suffix = name
            .strip_prefix("completion-")
            .or_else(|| name.strip_prefix("terminal-"))
            .and_then(|name| name.strip_suffix(".json"))
            .ok_or_else(invalid)?;
        if suffix.len() != 32 || !suffix.bytes().all(|value| value.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        let file = files::read(&root, &name)?;
        let metadata = file.metadata()?;
        if metadata.len() > 16 * 1024 || metadata.mode() & 0o777 != 0o600 {
            return Err(invalid());
        }
        count += 1;
        total = total.checked_add(metadata.len()).ok_or_else(limit)?;
        if count > 128 || total > 2 * 1024 * 1024 {
            return Err(limit());
        }
    }
    Ok((count, (count as u64 + 2) * 16 * 1024))
}

pub(super) fn classify(
    root: &Path,
    operation: &Path,
    plan: &Plan,
    check: &mut impl FnMut() -> Result<(), ApiError>,
) -> Result<Vec<DataRestoreSlotState>, ApiError> {
    slot_files::verify_parents(root, &plan.parents)?;
    slot_files::verify_parents(operation, &plan.operation_parents)?;
    let mut result = vec![];
    for slot in &plan.slots {
        let directory = slot.kind == "attachments";
        let live = EntryProof::capture(&root.join(&slot.path), directory, check)?;
        let before = EntryProof::capture(
            &operation.join(format!("slots/{}/before", slot.kind)),
            directory,
            check,
        )?;
        let after = EntryProof::capture(
            &operation.join(format!("slots/{}/after", slot.kind)),
            directory,
            check,
        )?;
        let state = if slot_files::same_optional(&live, &slot.before)
            && before.is_none()
            && slot_files::same_optional(&after, &slot.after)
        {
            if slot.before.is_none() && slot.after.is_none() {
                "absent"
            } else {
                "before"
            }
        } else if live.is_none()
            && slot_files::same_optional(&before, &slot.before)
            && slot_files::same_optional(&after, &slot.after)
        {
            "displaced"
        } else if slot_files::same_optional(&live, &slot.after)
            && slot_files::same_optional(&before, &slot.before)
            && after.is_none()
        {
            "applied"
        } else {
            return Err(changed().detail("slot", slot.kind.clone()));
        };
        result.push(DataRestoreSlotState {
            kind: slot.kind.clone(),
            path: slot.path.clone(),
            state: state.into(),
        });
    }
    Ok(result)
}
pub(super) fn original_retained(states: &[DataRestoreSlotState]) -> bool {
    states.iter().all(|slot| {
        matches!(
            slot.state.as_str(),
            "before" | "displaced" | "applied" | "absent"
        )
    })
}
pub(super) fn complete_slots(
    states: &[DataRestoreSlotState],
    plan: &Plan,
    direction: DataRestoreDirection,
) -> bool {
    states.iter().zip(&plan.slots).all(|(state, slot)| {
        if direction == DataRestoreDirection::Undo {
            matches!(state.state.as_str(), "before" | "absent")
        } else if slot.before.is_none() && slot.after.is_none() {
            state.state == "absent"
        } else if slot.after.is_none() {
            state.state == "displaced" || state.state == "applied"
        } else {
            state.state == "applied"
        }
    })
}

pub(super) fn prepare_slots(
    project: &ProjectHandle,
    stage: &Path,
    current: &version_data::DataSnapshotManifest,
    target: &version_data::DataSnapshotManifest,
    check: &mut impl FnMut() -> Result<(), ApiError>,
) -> Result<Vec<Slot>, ApiError> {
    let mut slots = vec![];
    for (kind, path) in slot_paths(&project.manifest) {
        let directory = kind == "attachments";
        let before = EntryProof::capture(&project.root.join(&path), directory, check)?;
        let after =
            EntryProof::capture(&stage.join(format!("slots/{kind}/after")), directory, check)?;
        if matches!(kind, "database" | "attachments") && (before.is_none() || after.is_none()) {
            return Err(invalid());
        }
        slots.push(Slot {
            kind: kind.into(),
            path,
            before,
            after,
        });
    }
    manifest_matches(&slots, current, false)?;
    manifest_matches(&slots, target, true)?;
    Ok(slots)
}
fn manifest_matches(
    slots: &[Slot],
    manifest: &version_data::DataSnapshotManifest,
    after: bool,
) -> Result<(), ApiError> {
    let proof = |kind: &str| -> Result<&EntryProof, ApiError> {
        let slot = slots
            .iter()
            .find(|slot| slot.kind == kind)
            .ok_or_else(invalid)?;
        if after {
            slot.after.as_ref()
        } else {
            slot.before.as_ref()
        }
        .ok_or_else(invalid)
    };
    if after {
        let db = proof("database")?.file.as_ref().ok_or_else(invalid)?;
        if db.bytes != manifest.database.bytes || db.sha256 != manifest.database.sha256 {
            return Err(changed());
        }
    }
    let tree = proof("attachments")?.tree.as_ref().ok_or_else(invalid)?;
    if tree.directories.keys().cloned().collect::<Vec<_>>() != manifest.directories
        || tree.files.len() != manifest.attachments.len()
        || tree.files.iter().any(|(path, file)| {
            manifest
                .attachments
                .get(path)
                .is_none_or(|data| file.sha256 != data.sha256 || file.bytes != data.bytes)
        })
    {
        return Err(changed());
    }
    Ok(())
}
fn record_bytes(
    operation: &Path,
    journal: &Journal,
    plan: &Plan,
    name: &str,
    identity: PersistedIdentity,
    direction: DataRestoreDirection,
) -> Result<Vec<u8>, ApiError> {
    let digest = &journal
        .transaction_plan
        .as_ref()
        .ok_or_else(invalid)?
        .sha256;
    let journal_digest = sha256(&read_file(&operation.join("journal.json"), JOURNAL_MAX)?);
    if name == "completion" {
        return Ok(serde_json::to_vec_pretty(&plan.completion(
            digest,
            &journal_digest,
            direction,
            identity,
        )?)?);
    }
    let bytes = read_file(&operation.join("completion.json"), 16 * 1024)?;
    let completion: Completion = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if completion
        != plan.completion(
            digest,
            &journal_digest,
            direction,
            completion.record_identity.clone(),
        )?
        || !completion
            .record_identity
            .matches_path(&operation.join("completion.json"))
            .map_err(|_| changed())?
    {
        return Err(changed());
    }
    Ok(serde_json::to_vec_pretty(&Terminal::new(
        &completion,
        &bytes,
        identity,
    ))?)
}

fn verify_metadata(
    operation: &Path,
    journal: &Journal,
    plan: &Plan,
    committed: Option<DataRestoreDirection>,
) -> Result<(), ApiError> {
    if !plan
        .operation_identity
        .matches_path(operation)
        .map_err(|_| changed())?
    {
        return Err(changed());
    }
    journal
        .transaction_plan
        .as_ref()
        .ok_or_else(invalid)?
        .verify(&operation.join("plan.json"), MAX_METADATA)?;
    let names = fs::read_dir(operation)?
        .map(|entry| {
            entry.and_then(|entry| {
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| std::io::Error::other("non UTF-8"))
            })
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut expected = BTreeSet::from([
        "plan.json".into(),
        "journal.json".into(),
        "registration-apply.json".into(),
        "registration-undo.json".into(),
        "metadata-attempts".into(),
        "metadata-reserve".into(),
        "slots".into(),
    ]);
    for name in ["completion.json", "terminal.json"] {
        if operation.join(name).try_exists()? {
            expected.insert(name.into());
        }
    }
    if names != expected {
        return Err(changed());
    }
    metadata_usage(operation)?;
    let kinds = fs::read_dir(operation.join("slots"))?
        .map(|entry| {
            entry.and_then(|entry| {
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| std::io::Error::other("non UTF-8"))
            })
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if kinds != plan.slots.iter().map(|slot| slot.kind.clone()).collect() {
        return Err(changed());
    }
    for slot in &plan.slots {
        for entry in fs::read_dir(operation.join("slots").join(&slot.kind))? {
            let name = entry?.file_name();
            if name != "before" && name != "after" {
                return Err(changed());
            }
        }
    }
    let reserve_names = fs::read_dir(operation.join("metadata-reserve"))?
        .map(|entry| {
            entry.and_then(|entry| {
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| std::io::Error::other("non UTF-8"))
            })
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if reserve_names
        .iter()
        .any(|name| !plan.metadata_reserve.contains_key(name))
    {
        return Err(changed());
    }
    for (name, proof) in &plan.metadata_reserve {
        if reserve_names.contains(name) {
            let (observed, bytes) = read_proof(
                &operation.join("metadata-reserve").join(name),
                16 * 1024,
                true,
            )?;
            if !proof.identity.matches_captured(&observed.identity) || proof.mode != observed.mode {
                return Err(changed());
            }
            if !bytes.is_empty() {
                let direction = committed.ok_or_else(changed)?;
                let expected = record_bytes(
                    operation,
                    journal,
                    plan,
                    name,
                    proof.identity.clone(),
                    direction,
                )?;
                if !expected.starts_with(&bytes) {
                    return Err(changed());
                }
            }
        } else {
            let canonical = operation.join(format!("{name}.json"));
            if !proof
                .identity
                .matches_path(&canonical)
                .map_err(|_| changed())?
                || !(fs::symlink_metadata(&canonical)?.mode() == proof.mode
                    || (name == "terminal"
                        && fs::symlink_metadata(&canonical)?.mode() & 0o777 == 0o400))
            {
                return Err(changed());
            }
        }
    }
    Ok(())
}

fn publish_record(
    operation: &Path,
    plan: &Plan,
    name: &str,
    build: impl Fn(PersistedIdentity) -> Result<Vec<u8>, ApiError>,
    faults: &mut Faults,
    check: &mut impl FnMut() -> Result<(), ApiError>,
) -> Result<Vec<u8>, ApiError> {
    let canonical = operation.join(format!("{name}.json"));
    if canonical.try_exists()? {
        let bytes = read_file(&canonical, 16 * 1024)?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let identity: PersistedIdentity =
            serde_json::from_value(value.get("recordIdentity").cloned().ok_or_else(invalid)?)
                .map_err(|_| invalid())?;
        if identity.is_legacy()
            || !identity.matches_path(&canonical).map_err(|_| changed())?
            || build(identity.clone())? != bytes
        {
            return Err(changed());
        }
        let file = files::read(&files::root(operation)?, &format!("{name}.json"))?;
        let opened_identity = PersistedIdentity::from_file(&file).map_err(|_| changed())?;
        let mode = file.metadata()?.mode() & 0o777;
        if !identity.matches_captured(&opened_identity)
            || !opened_identity
                .matches_path(&canonical)
                .map_err(|_| changed())?
            || !(mode == 0o600 || (name == "terminal" && mode == 0o400))
        {
            return Err(changed());
        }
        file.sync_all()?;
        files::root(&operation.join("metadata-attempts"))?.sync_all()?;
        files::root(&operation.join("metadata-reserve"))?.sync_all()?;
        files::root(operation)?.sync_all()?;
        return Ok(bytes);
    }
    let (count, _) = metadata_usage(operation)?;
    check()?;
    faults.at("before-metadata-candidate-create")?;
    let candidate = if count < 128 {
        operation
            .join("metadata-attempts")
            .join(format!("{name}-{}.json", uuid::Uuid::new_v4().simple()))
    } else {
        operation.join("metadata-reserve").join(name)
    };
    let parent = files::root(candidate.parent().ok_or_else(invalid)?)?;
    let file_name = candidate
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?;
    let mut output = if count < 128 {
        files::create(&parent, file_name)?
    } else {
        use std::{
            ffi::CString,
            os::fd::{AsRawFd, FromRawFd},
        };
        let file_name = CString::new(file_name).map_err(|_| invalid())?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                file_name.as_ptr(),
                libc::O_RDWR | libc::O_APPEND | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(changed());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        let expected = plan.metadata_reserve.get(name).ok_or_else(invalid)?;
        if metadata.ino() != expected.identity.inode
            || metadata.len() > 16 * 1024
            || metadata.nlink() != 1
            || metadata.mode() != expected.mode
        {
            return Err(changed());
        }
        file
    };
    faults.at("after-metadata-candidate-create")?;
    let observed_identity = PersistedIdentity::capture(&candidate).map_err(|_| changed())?;
    let metadata = output.metadata()?;
    if observed_identity.is_legacy()
        || observed_identity.device != metadata.dev()
        || observed_identity.inode != metadata.ino()
    {
        return Err(changed());
    }
    let identity = if count < 128 {
        observed_identity
    } else {
        let expected = &plan
            .metadata_reserve
            .get(name)
            .ok_or_else(invalid)?
            .identity;
        if !expected.matches_captured(&observed_identity) {
            return Err(changed());
        }
        expected.clone()
    };
    let bytes = build(identity)?;
    if bytes.len() > 16 * 1024 {
        return Err(limit());
    }
    let mut prefix = vec![];
    if count >= 128 {
        (&mut output).take(16 * 1024 + 1).read_to_end(&mut prefix)?;
        if prefix.len() as u64 != metadata.len() || !bytes.starts_with(&prefix) {
            return Err(changed().detail("candidateCapacity", 128));
        }
    }
    let remaining = &bytes[prefix.len()..];
    let split = remaining.len() / 2;
    output.write_all(&remaining[..split])?;
    faults.at("after-metadata-partial-write")?;
    output.write_all(&remaining[split..])?;
    faults.at("before-metadata-file-sync")?;
    output.sync_all()?;
    faults.at("after-metadata-file-sync")?;
    parent.sync_all()?;
    if read_file(&candidate, 16 * 1024)? != bytes {
        return Err(changed());
    }
    check()?;
    faults.at("before-metadata-canonical-publication")?;
    slot_files::move_exclusive_with(&candidate, &canonical, || {
        faults.at("after-metadata-visible-before-directory-sync")
    })?;
    faults.at("after-metadata-directory-sync")?;
    Ok(bytes)
}

fn direction_for(state: DataRestoreEpochState) -> Option<DataRestoreDirection> {
    match state {
        DataRestoreEpochState::Before => None,
        DataRestoreEpochState::Applied => Some(DataRestoreDirection::Continue),
        DataRestoreEpochState::Undone => Some(DataRestoreDirection::Undo),
    }
}

fn seal_terminal(
    operation: &Path,
    bytes: &[u8],
    faults: &mut Faults,
    check: &mut impl FnMut() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    use std::io::{Seek, SeekFrom};
    let terminal: Terminal = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let directory = files::root(operation)?;
    let mut file = files::read(&directory, "terminal.json")?;
    let identity = PersistedIdentity::from_file(&file).map_err(|_| changed())?;
    let before = file.metadata()?;
    let mut actual = vec![];
    (&mut file).take(16 * 1024 + 1).read_to_end(&mut actual)?;
    if !terminal.record_identity.matches_captured(&identity)
        || !identity
            .matches_path(&operation.join("terminal.json"))
            .map_err(|_| changed())?
        || before.nlink() != 1
        || !matches!(before.mode() & 0o777, 0o600 | 0o400)
        || actual != bytes
    {
        return Err(changed());
    }
    file.sync_all()?;
    files::root(&operation.join("metadata-attempts"))?.sync_all()?;
    files::root(&operation.join("metadata-reserve"))?.sync_all()?;
    directory.sync_all()?;
    faults.at("after-terminal-namespace-sync-before-seal")?;
    check()?;
    file.set_permissions(fs::Permissions::from_mode(0o400))?;
    faults.at("after-terminal-seal-before-file-sync")?;
    file.seek(SeekFrom::Start(0))?;
    actual.clear();
    (&mut file).take(16 * 1024 + 1).read_to_end(&mut actual)?;
    if file.metadata()?.mode() & 0o777 != 0o400
        || file.metadata()?.nlink() != 1
        || !identity.matches_captured(&PersistedIdentity::from_file(&file).map_err(|_| changed())?)
        || !identity
            .matches_path(&operation.join("terminal.json"))
            .map_err(|_| changed())?
        || actual != bytes
    {
        return Err(changed());
    }
    file.sync_all()?;
    faults.at("after-terminal-seal-file-sync")?;
    Ok(())
}
fn state_name(state: DataRestoreEpochState) -> &'static str {
    match state {
        DataRestoreEpochState::Before => "before",
        DataRestoreEpochState::Applied => "applied",
        DataRestoreEpochState::Undone => "undone",
    }
}

#[allow(clippy::too_many_arguments)] // One owner retains lock, immutable plan and cancellation/fault boundaries.
pub(super) fn execute(
    workspace: &Path,
    access: &mut DataRestoreExclusiveAccess,
    journal: &mut Journal,
    plan: &Plan,
    direction: DataRestoreDirection,
    check: &mut impl FnMut() -> Result<(), ApiError>,
    faults: &mut Faults,
    tasks: &BuildManager,
    task: &str,
) -> Result<Value, ApiError> {
    let operation = access.operation().to_path_buf();
    let root = access.root().to_path_buf();
    let state = access.epoch_state()?;
    if direction_for(state).is_some_and(|committed| committed != direction) {
        return Err(changed());
    }
    verify_binding(&root, plan, check)?;
    verify_metadata(
        &operation,
        journal,
        plan,
        direction_for(access.epoch_state()?),
    )?;
    let journal_sha = sha256(&read_file(&operation.join("journal.json"), JOURNAL_MAX)?);
    if operation.join("completion.json").try_exists()? {
        let value: Completion =
            serde_json::from_slice(&read_file(&operation.join("completion.json"), 16 * 1024)?)
                .map_err(|_| invalid())?;
        if value
            != plan.completion(
                &journal
                    .transaction_plan
                    .as_ref()
                    .ok_or_else(invalid)?
                    .sha256,
                &journal_sha,
                direction,
                value.record_identity.clone(),
            )?
            || direction_for(state) != Some(direction)
        {
            return Err(changed());
        }
    }
    if state == DataRestoreEpochState::Before {
        let indices = if direction == DataRestoreDirection::Continue {
            (0..plan.slots.len()).collect::<Vec<_>>()
        } else {
            (0..plan.slots.len()).rev().collect()
        };
        for index in indices {
            loop {
                check()?;
                verify_binding(&root, plan, check)?;
                verify_metadata(
                    &operation,
                    journal,
                    plan,
                    direction_for(access.epoch_state()?),
                )?;
                let classified = classify(&root, &operation, plan, check)?;
                let slot = &plan.slots[index];
                let state = classified[index].state.as_str();
                let live = root.join(&slot.path);
                let before = operation.join(format!("slots/{}/before", slot.kind));
                let after = operation.join(format!("slots/{}/after", slot.kind));
                let movement = if direction == DataRestoreDirection::Continue {
                    if state == "before" && slot.before.is_some() {
                        Some((live, before))
                    } else if state == "displaced" && slot.after.is_some() {
                        Some((after, live))
                    } else {
                        None
                    }
                } else if state == "applied" && slot.after.is_some() {
                    Some((live, after))
                } else if (state == "displaced" || state == "applied") && slot.before.is_some() {
                    Some((before, live))
                } else {
                    None
                };
                let Some((from, to)) = movement else { break };
                tasks.phase(task, &format!("data-restore-{}", slot.kind))?;
                faults.at("before-slot-move")?;
                check()?;
                slot_files::move_exclusive_with(&from, &to, || {
                    faults.at("after-slot-visible-before-directory-sync")
                })?;
                faults.at("after-slot-directory-sync")?;
                classify(&root, &operation, plan, check)?;
            }
        }
        let states = classify(&root, &operation, plan, check)?;
        if !complete_slots(&states, plan, direction) {
            return Err(changed());
        }
        verify_binding(&root, plan, check)?;
        verify_metadata(
            &operation,
            journal,
            plan,
            direction_for(access.epoch_state()?),
        )?;
        check()?;
        if direction == DataRestoreDirection::Continue {
            crate::backups::check_database(&root.join(&plan.manifest.database))?;
            classify(&root, &operation, plan, check)?;
        }
        tasks.phase(task, "data-restore-epoch")?;
        faults.at("before-epoch")?;
        check()?;
        access.publish_epoch(direction)?;
        faults.at("after-epoch")?;
    }
    check()?;
    verify_binding(&root, plan, check)?;
    verify_metadata(
        &operation,
        journal,
        plan,
        direction_for(access.epoch_state()?),
    )?;
    let states = classify(&root, &operation, plan, check)?;
    if !complete_slots(&states, plan, direction)
        || direction_for(access.epoch_state()?) != Some(direction)
    {
        return Err(changed());
    }
    sync_live_parents(&root, plan)?;
    slot_files::sync_ordinary(&operation.join("slots"))?;
    let completion_bytes = publish_record(
        &operation,
        plan,
        "completion",
        |identity| {
            Ok(serde_json::to_vec_pretty(
                &plan.completion(
                    &journal
                        .transaction_plan
                        .as_ref()
                        .ok_or_else(invalid)?
                        .sha256,
                    &journal_sha,
                    direction,
                    identity,
                )?,
            )?)
        },
        faults,
        check,
    )?;
    let completion: Completion =
        serde_json::from_slice(&completion_bytes).map_err(|_| invalid())?;
    check()?;
    verify_binding(&root, plan, check)?;
    verify_metadata(
        &operation,
        journal,
        plan,
        direction_for(access.epoch_state()?),
    )?;
    let states = classify(&root, &operation, plan, check)?;
    if !complete_slots(&states, plan, direction)
        || direction_for(access.epoch_state()?) != Some(direction)
    {
        return Err(changed());
    }
    tasks.phase(task, "data-restore-finalize")?;
    let terminal_bytes = publish_record(
        &operation,
        plan,
        "terminal",
        |identity| {
            Ok(serde_json::to_vec_pretty(&Terminal::new(
                &completion,
                &completion_bytes,
                identity,
            ))?)
        },
        faults,
        check,
    )?;
    check()?;
    verify_binding(&root, plan, check)?;
    verify_metadata(
        &operation,
        journal,
        plan,
        direction_for(access.epoch_state()?),
    )?;
    let states = classify(&root, &operation, plan, check)?;
    if !complete_slots(&states, plan, direction)
        || direction_for(access.epoch_state()?) != Some(direction)
    {
        return Err(changed());
    }
    sync_live_parents(&root, plan)?;
    slot_files::sync_ordinary(&operation.join("slots"))?;
    seal_terminal(&operation, &terminal_bytes, faults, check)?;
    let released = guard::inspect(workspace, &plan.project_id);
    if !released.pending.is_empty() || !released.issues.is_empty() {
        return Err(invalid());
    }
    Ok(
        json!({"operation":if task==plan.task_id{"data-restore-apply"}else if direction==DataRestoreDirection::Continue{"data-restore-continue"}else{"data-restore-undo"},"operationOutcome":"committed","dataState":if direction==DataRestoreDirection::Continue{"applied"}else{"undone"},"restoreTaskId":plan.task_id,"targetCommit":plan.target_commit,"documentEpoch":completion.document_epoch,"liveDataChanged":direction==DataRestoreDirection::Continue,"pending":false,"businessBlocked":false,"originalRetained":true,"retainedPath":operation}),
    )
}

fn sync_live_parents(root: &Path, plan: &Plan) -> Result<(), ApiError> {
    for path in plan.parents.keys() {
        files::root(&root.join(path))?.sync_all()?;
    }
    Ok(())
}

pub(super) fn receipt_error(
    workspace: &Path,
    operation: &Path,
    plan: &Plan,
    error: ApiError,
    stop: &watch::Receiver<bool>,
) -> ApiError {
    let state = plan
        .registration_plan
        .visible_state(workspace, &plan.project_id)
        .ok();
    let mut check = || {
        let _ = stop;
        Ok(())
    };
    let slots = plan
        .registration_plan
        .verify_root(workspace, &plan.project_id)
        .ok()
        .and_then(|root| classify(&root, operation, plan, &mut check).ok());
    let guard = guard::inspect(workspace, &plan.project_id);
    let pending = !guard.pending.is_empty() || !guard.issues.is_empty();
    error
        .outcome(match state {
            Some(DataRestoreEpochState::Before) => "not_committed",
            Some(_) => "committed",
            None => "unknown",
        })
        .detail("dataState", state.map(state_name).unwrap_or("unknown"))
        .detail("restoreTaskId", plan.task_id.clone())
        .detail("pending", pending)
        .detail("businessBlocked", pending)
        .detail(
            "originalRetained",
            slots.as_ref().is_some_and(|value| original_retained(value)),
        )
        .detail(
            "liveDataChanged",
            slots.as_ref().is_none_or(|value| {
                value
                    .iter()
                    .any(|slot| !matches!(slot.state.as_str(), "before" | "absent"))
            }),
        )
        .detail("retainedPath", operation.display().to_string())
}

pub(super) fn status(
    workspace: &Path,
    operation: &Path,
    id: &str,
    task: &str,
    stop: &watch::Receiver<bool>,
) -> DataRestoreRecoveryOperation {
    let mut result = DataRestoreRecoveryOperation {
        restore_task_id: task.into(),
        target_commit: String::new(),
        phase: String::new(),
        plan_digest: String::new(),
        journal_digest: String::new(),
        operation_outcome: "unknown".into(),
        data_state: "unknown".into(),
        document_epoch: None,
        can_continue: false,
        can_undo: false,
        completion_published: false,
        original_retained: false,
        retained_path: operation.into(),
        slots: vec![],
        issues: vec![],
    };
    let checked = (|| {
        let (journal, plan, bytes) = load(operation, id, task)?;
        result.target_commit = plan.target_commit.clone();
        result.phase = journal.phase.clone();
        result.plan_digest = journal.plan_digest.clone();
        result.journal_digest = sha256(&bytes);
        let state = plan.registration_plan.visible_state(workspace, id)?;
        result.data_state = state_name(state).into();
        result.operation_outcome = if state == DataRestoreEpochState::Before {
            "not_committed"
        } else {
            "committed"
        }
        .into();
        result.document_epoch = direction_for(state)
            .map(|direction| plan.registration_plan.epoch(direction))
            .transpose()?;
        let root = plan.registration_plan.verify_root(workspace, id)?;
        let mut check = || {
            if *stop.borrow() {
                Err(interrupted())
            } else {
                Ok(())
            }
        };
        verify_binding(&root, &plan, &mut check)?;
        verify_metadata(operation, &journal, &plan, direction_for(state))?;
        result.slots = classify(&root, operation, &plan, &mut check)?;
        result.original_retained = original_retained(&result.slots);
        plan.registration_plan
            .epoch_state(workspace, id, operation)?;
        let direction = direction_for(state);
        result.phase = match state {
            DataRestoreEpochState::Applied => "committing",
            DataRestoreEpochState::Undone => "undo-committing",
            DataRestoreEpochState::Before
                if result
                    .slots
                    .iter()
                    .any(|slot| !matches!(slot.state.as_str(), "before" | "absent")) =>
            {
                "exchanging"
            }
            _ => "prepared",
        }
        .into();
        result.can_continue =
            direction.is_none() || direction == Some(DataRestoreDirection::Continue);
        result.can_undo = direction.is_none() || direction == Some(DataRestoreDirection::Undo);
        if operation.join("completion.json").try_exists()? {
            let completion: Completion =
                serde_json::from_slice(&read_file(&operation.join("completion.json"), 16 * 1024)?)
                    .map_err(|_| invalid())?;
            if completion
                != plan.completion(
                    &journal
                        .transaction_plan
                        .as_ref()
                        .ok_or_else(invalid)?
                        .sha256,
                    &sha256(&bytes),
                    completion.direction,
                    completion.record_identity.clone(),
                )?
                || direction != Some(completion.direction)
            {
                return Err(invalid());
            }
            result.completion_published = true;
            result.phase = "finalizing".into();
        }
        Ok::<(), ApiError>(())
    })();
    if let Err(error) = checked {
        result.can_continue = false;
        result.can_undo = false;
        result.issues.push(error);
    }
    result
}
