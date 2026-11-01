//! Source-directory identity and exclusive ownership for Node instances.
//! Author declarations never grant source ownership or deletion authority.
use super::*;
use crate::file_identity::PersistedIdentity;

static SOURCE_OWNERSHIP_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> =
    std::sync::OnceLock::new();

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceDirectoryOwner {
    version: u32,
    app_id: String,
    source_root: PathBuf,
    identity: PersistedIdentity,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NodeSourceBinding {
    pub source_root: PathBuf,
    root_identity: PersistedIdentity,
}
impl PartialEq for NodeSourceBinding {
    fn eq(&self, other: &Self) -> bool {
        self.source_root == other.source_root
            && self.root_identity.device == other.root_identity.device
            && self.root_identity.inode == other.root_identity.inode
            && self.root_identity.volume_uuid == other.root_identity.volume_uuid
    }
}
impl NodeSourceBinding {
    pub(crate) fn validate_record(&self) -> Result<()> {
        if !self.source_root.is_absolute()
            || self.source_root.components().any(|part| {
                !matches!(
                    part,
                    Component::RootDir | Component::Prefix(_) | Component::Normal(_)
                )
            })
        {
            return Err(development_changed());
        }
        Ok(())
    }
    pub(crate) fn matches_identity(&self, source: &Path, expected: &PersistedIdentity) -> bool {
        self.source_root == source && self.root_identity.matches_captured(expected)
    }
    /// Directory ownership survives author declaration edits.
    pub(crate) fn validate_directory(&self) -> Result<()> {
        ordinary_path(&self.source_root, true)?;
        if !self
            .root_identity
            .matches_path(&self.source_root)
            .unwrap_or(false)
        {
            return Err(development_changed());
        }
        Ok(())
    }
}

fn development_changed() -> ApiError {
    ApiError::new(
        "NODE_DEVELOPMENT_SOURCE_CHANGED",
        "项目文件夹已缺失、被替换或归属无法核对，请检查文件夹后重试。",
    )
}

fn source_already_owned() -> ApiError {
    ApiError::new(
        "NODE_SOURCE_ALREADY_OWNED",
        "这个源码文件夹已属于另一个项目，或位于其他项目内，请为新项目使用独立副本。",
    )
}

fn source_claim_key(source: &Path) -> Result<(String, PersistedIdentity)> {
    ordinary_path(source, true)?;
    let identity = PersistedIdentity::capture(source).map_err(|_| development_changed())?;
    let key = match &identity.volume_uuid {
        Some(volume) => json!({"volume":volume,"inode":identity.inode}),
        None => json!({"device":identity.device,"inode":identity.inode}),
    };
    Ok((
        format!("{:x}", Sha256::digest(serde_json::to_vec(&key)?)),
        identity,
    ))
}

fn source_owner(data_root: &Path, source: &Path) -> Result<Option<SourceDirectoryOwner>> {
    let (hash, identity) = source_claim_key(source)?;
    let directory = data_root.join("node-source-owners").join(hash);
    crate::projects::check_no_symlinks(&directory, true)?;
    if !directory.try_exists()? {
        return Ok(None);
    }
    ordinary_path(&directory, true)?;
    let owner: SourceDirectoryOwner =
        serde_json::from_slice(&read_small(&directory.join("owner.json"))?)?;
    if owner.version != 1
        || !owner.identity.matches_captured(&identity)
        || owner.source_root != source
        || Uuid::parse_str(&owner.app_id).is_err()
    {
        return Err(development_changed());
    }
    Ok(Some(owner))
}

pub(crate) fn require_claimed_source_owner(
    data_root: &Path,
    id: &str,
    source: &Path,
) -> Result<PersistedIdentity> {
    let owner = source_owner(data_root, source)?.ok_or_else(development_changed)?;
    if owner.app_id != id {
        return Err(source_already_owned());
    }
    Ok(owner.identity)
}

/// The caller must hold the verified public migration and prove this exact copy.
pub(crate) fn claim_source_after_verified_migration(
    data_root: &Path,
    workspace: &Path,
    id: &str,
    source: &Path,
) -> Result<()> {
    claim_source_path(data_root, workspace.to_path_buf(), id, source)
}

pub(crate) fn require_new_web_project_location(data_root: &Path, location: &Path) -> Result<()> {
    require_source_ancestors(data_root, location, None).map_err(|error| {
        if error.code == "NODE_SOURCE_ALREADY_OWNED" {
            ApiError::new(
                "PROJECT_DIRECTORY_OWNED_BY_NODE",
                "项目目录位于另一个 Node 项目的源码内，请选择独立的工作区。",
            )
            .detail("cause", json!(error))
        } else {
            error
        }
    })
}

fn require_source_ancestors(data_root: &Path, source: &Path, id: Option<&str>) -> Result<()> {
    crate::web_project_ownership::WebProjectOwnership::require_source_unoccupied(
        data_root, source,
    )?;
    for ancestor in source.ancestors().skip(1) {
        if let Some(owner) = source_owner(data_root, ancestor)? {
            if id != Some(owner.app_id.as_str()) {
                return Err(source_already_owned());
            }
        }
    }
    // Admission checks the exclusive owner index in the reverse direction
    // too: registering a parent must not absorb an existing child project.
    // Deletion never uses this index scan or retains shared project content.
    let owners = data_root.join("node-source-owners");
    crate::projects::check_no_symlinks(&owners, true)?;
    if !owners.try_exists()? {
        return Ok(());
    }
    ordinary_path(&owners, true)?;
    for entry in fs::read_dir(&owners)? {
        let entry = entry?;
        ordinary_path(&entry.path(), true)?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(development_changed)?;
        if name == ".pending-deletions" {
            continue;
        }
        if name.len() != 64
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(development_changed());
        }
        let owner: SourceDirectoryOwner =
            serde_json::from_slice(&read_small(&entry.path().join("owner.json"))?)?;
        if owner.version != 1 || Uuid::parse_str(&owner.app_id).is_err() {
            return Err(development_changed());
        }
        if owner.source_root != source
            && owner.source_root.starts_with(source)
            && id != Some(owner.app_id.as_str())
            && owner
                .identity
                .matches_path(&owner.source_root)
                .unwrap_or(false)
        {
            return Err(source_already_owned());
        }
    }
    Ok(())
}
pub(crate) fn with_source_ownership_admission<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    let _guard = SOURCE_OWNERSHIP_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .map_err(|_| fail("源码归属锁不可用。"))?;
    work()
}
pub(crate) fn require_delivery_location_unclaimed(data_root: &Path, parent: &Path) -> Result<()> {
    for ancestor in parent.ancestors() {
        if source_owner(data_root, ancestor)?.is_some() {
            return Err(source_already_owned());
        }
    }
    Ok(())
}

fn claim_source_path(data_root: &Path, workspace: PathBuf, id: &str, source: &Path) -> Result<()> {
    if Uuid::parse_str(id).is_err() {
        return Err(fail("源码归属标识无效。"));
    }
    let _guard = SOURCE_OWNERSHIP_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .map_err(|_| fail("源码归属锁不可用。"))?;
    crate::project_tool_boundary::require_source_excludes_global_tools(data_root, source)?;
    require_source_ancestors(data_root, source, Some(id))?;
    let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
        data_root.to_path_buf(),
        workspace,
    );
    if let Some(owner) = source_owner(data_root, source)? {
        if owner.app_id != id {
            return Err(source_already_owned());
        }
        let (hash, _) = source_claim_key(source)?;
        return ownership.record_source_claim(id, &hash);
    }
    let (hash, identity) = source_claim_key(source)?;
    let root = data_root.join("node-source-owners");
    private_dir(&root)?;
    let directory = root.join(&hash);
    // Record the missing target before mkdir, then capture its identity
    // before writing any content. Crash recovery never adopts another tree.
    ownership.record_source_claim(id, &hash)?;
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    if let Err(error) = builder.create(&directory) {
        ownership.forget_source_claim(id, &hash)?;
        return Err(error.into());
    }
    ownership.record_source_claim(id, &hash)?;
    write_new_json(
        &directory.join("owner.json"),
        &SourceDirectoryOwner {
            version: 1,
            app_id: id.into(),
            source_root: source.into(),
            identity,
        },
    )
}

impl NodeApps {
    pub(super) fn read_source_binding(&self, id: &str) -> Result<Option<NodeSourceBinding>> {
        let path = self.root.join(id).join("source-binding.json");
        crate::projects::check_no_symlinks(&path, true)?;
        if !path.try_exists()? {
            return Ok(None);
        }
        Ok(Some(serde_json::from_slice(&read_small(&path)?)?))
    }

    pub(super) fn initialize_source_binding(&self, record: &ExternalRegistration) -> Result<()> {
        let source = &record.source_path;
        self.claim_source_directory(&record.id, source)?;
        let binding = NodeSourceBinding {
            source_root: source.clone(),
            root_identity: PersistedIdentity::capture(source).map_err(|_| development_changed())?,
        };
        binding.validate_record()?;
        if let Some(previous) = self.read_source_binding(&record.id)? {
            if previous.source_root != binding.source_root
                || !previous
                    .root_identity
                    .matches_captured(&binding.root_identity)
            {
                return Err(development_changed());
            }
            // Idempotent registration may refresh metadata after author edits.
            return crate::builds::atomic_json(
                &self.root.join(&record.id).join("source-binding.json"),
                &binding,
            );
        }
        write_new_json(
            &self.root.join(&record.id).join("source-binding.json"),
            &binding,
        )
    }

    fn source_claim_key(&self, source: &Path) -> Result<(String, PersistedIdentity)> {
        source_claim_key(source)
    }

    fn source_owner(&self, source: &Path) -> Result<Option<SourceDirectoryOwner>> {
        source_owner(&self.data_root, source)
    }

    pub(super) fn require_external_source_boundary(&self, source: &Path) -> Result<()> {
        crate::workspace::require_source_outside_pending_migrations(&self.data_root, source)?;
        let workspace = self.deletion_workspace();
        for name in ["projects", ".platform", "project-sources", "backups"] {
            let managed = workspace.join(name);
            if source.starts_with(&managed) || managed.starts_with(source) {
                return Err(ApiError::new(
                    "NODE_SOURCE_ALREADY_OWNED",
                    "这个文件夹属于平台管理的项目或材料，请选择独立的外部文件夹。",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn require_unclaimed_source(&self, source: &Path) -> Result<()> {
        self.require_external_source_boundary(source)?;
        if self.is_published_source(source) {
            return Err(source_already_owned());
        }
        self.require_source_ancestors(source, None)?;
        if self.source_owner(source)?.is_some() {
            return Err(source_already_owned());
        }
        Ok(())
    }

    pub(super) fn pending_generated_source_id(&self, source: &Path) -> Result<Option<String>> {
        let Some(owner) = self.source_owner(source)? else {
            return Ok(None);
        };
        if !self
            .project_ownership()
            .has_initialization_source(&owner.app_id, source)?
            || self
                .root
                .join(&owner.app_id)
                .join("registration.json")
                .try_exists()?
        {
            return Err(source_already_owned());
        }
        self.require_source_ancestors(source, Some(&owner.app_id))?;
        Ok(Some(owner.app_id))
    }

    fn require_source_ancestors(&self, source: &Path, id: Option<&str>) -> Result<()> {
        require_source_ancestors(&self.data_root, source, id)
    }

    pub(crate) fn claim_source_directory(&self, id: &str, source: &Path) -> Result<()> {
        claim_source_path(&self.data_root, self.deletion_workspace(), id, source)
    }

    pub(super) fn discard_pending_generated_source(&self, id: &str, source: &Path) -> Result<()> {
        let _guard = SOURCE_OWNERSHIP_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .map_err(|_| fail("源码归属锁不可用。"))?;
        let ownership = self.project_ownership();
        if !ownership.has_initialization_source(id, source)? {
            return Err(development_changed());
        }
        if self.root.join(id).try_exists()? {
            return Err(development_changed());
        }
        let (hash, _) = self.source_claim_key(source)?;
        let directory = self.data_root.join("node-source-owners").join(&hash);
        // The ledger captured the claim directory before owner.json was written;
        // it also covers a failed write of that file without accepting a foreign
        // directory whose identity differs from the reservation.
        let owned = ownership.owned_paths(id)?;
        if directory.try_exists()? && owned.contains(&directory) {
            fs::remove_dir_all(&directory)?;
        }
        ownership.forget_source_claim(id, &hash)?;
        let ledger = ownership.directory(id)?;
        ordinary_path(&ledger, true)?;
        fs::remove_dir_all(ledger)?;
        Ok(())
    }

    pub(super) fn discard_unpublished_registration(
        &self,
        id: &str,
        slot_identity: Option<&PersistedIdentity>,
        preserve_pending: bool,
    ) -> Result<()> {
        if let Some(identity) = slot_identity {
            let slot = self.root.join(id);
            if !identity
                .matches_path(&slot)
                .map_err(|_| development_changed())?
            {
                return Err(development_changed());
            }
            fs::remove_dir_all(&slot)?;
        }
        if preserve_pending {
            return Ok(());
        }
        let _guard = SOURCE_OWNERSHIP_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .map_err(|_| fail("源码归属锁不可用。"))?;
        let ownership = self.project_ownership();
        let ledger = ownership.directory(id)?;
        if !ledger.try_exists()? {
            return Ok(());
        }
        let owned = ownership.owned_paths(id)?;
        if ownership.generated_source(id)?.is_some() {
            return Err(development_changed());
        }
        for path in owned {
            if path == ledger
                || path.parent() == Some(self.data_root.join("node-source-owners").as_path())
            {
                if path.try_exists()? {
                    fs::remove_dir_all(path)?;
                }
            } else {
                return Err(development_changed());
            }
        }
        Ok(())
    }
    pub async fn project_source(&self, id: &str) -> Result<PathBuf> {
        let _operation = self.admit_operation(id, "项目文件夹")?;
        let inner = self.inner.lock().await;
        if inner.shutting_down {
            return Err(fail("平台正在退出。"));
        }
        let record = self.registration(id)?;
        let binding = self
            .read_source_binding(id)?
            .ok_or_else(development_changed)?;
        if binding.source_root != record.source_path {
            return Err(development_changed());
        }
        binding.validate_directory()?;
        require_claimed_source_owner(&self.data_root, id, &binding.source_root)?;
        Ok(binding.source_root)
    }
}

/// Rebind only platform-owned published project folders after a verified copy.
/// User-selected originals, including initialized empty folders, stay in place.
pub(crate) fn rebind_published_after_migration(
    data_root: &Path,
    old: &Path,
    new: &Path,
) -> Result<()> {
    let apps = data_root.join("node-apps");
    if !apps.try_exists()? {
        return Ok(());
    }
    ordinary_path(&apps, true)?;
    let ownership =
        crate::node_project_ownership::NodeProjectOwnership::new(data_root.into(), old.into());
    for entry in fs::read_dir(&apps)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if Uuid::parse_str(&id).is_err() {
            continue;
        }
        let binding_path = entry.path().join("source-binding.json");
        if !binding_path.try_exists()? {
            continue;
        }
        let mut binding: NodeSourceBinding = serde_json::from_slice(&read_small(&binding_path)?)?;
        let relative = if let Ok(relative) = binding.source_root.strip_prefix(old) {
            relative.to_path_buf()
        } else if let Ok(relative) = binding.source_root.strip_prefix(new) {
            relative.to_path_buf()
        } else {
            continue;
        };
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(development_changed());
        }
        let previous = old.join(&relative);
        let target = new.join(&relative);
        if !ownership.owns_source_directory(&id, &binding.source_root)? {
            continue;
        }
        // The verified source directory and private claim, not author metadata,
        // authorize this rebind.
        binding.validate_directory()?;
        let declared = require_claimed_source_owner(data_root, &id, &binding.source_root)?;
        if !binding.root_identity.matches_captured(&declared) {
            return Err(development_changed());
        }
        ordinary_path(&target, true)?;
        let registration_path = entry.path().join("registration.json");
        let mut registration: Value = serde_json::from_slice(&read_small(&registration_path)?)?;
        if registration["id"] != id {
            return Err(development_changed());
        }
        if registration["version"] != 3 {
            return Err(development_changed());
        }
        let current = registration["sourcePath"]
            .as_str()
            .map(PathBuf::from)
            .ok_or_else(development_changed)?;
        if current != previous && current != target {
            return Err(development_changed());
        }
        registration["sourcePath"] = serde_json::to_value(&target)?;
        binding.source_root = target.clone();
        binding.root_identity =
            PersistedIdentity::capture(&target).map_err(|_| development_changed())?;
        claim_source_after_verified_migration(data_root, new, &id, &target)?;
        crate::builds::atomic_json(&registration_path, &registration)?;
        crate::builds::atomic_json(&binding_path, &binding)?;
        let receipt_path = new
            .join(".platform/source-publications")
            .join(&id)
            .join("publication.json");
        crate::projects::check_no_symlinks(&receipt_path, true)?;
        if receipt_path.try_exists()? {
            let mut receipt: Value = serde_json::from_slice(&read_small(&receipt_path)?)?;
            if receipt["appId"] != id {
                return Err(development_changed());
            }
            receipt["payloadIdentity"] = serde_json::to_value(&binding.root_identity)?;
            crate::builds::atomic_json(&receipt_path, &receipt)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "node_source_ownership_tests.rs"]
mod tests;
