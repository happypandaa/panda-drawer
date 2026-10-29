//! Binds one editable author tree to a Node instance. Runtime generations and
//! immutable download candidates are never silently promoted to author trees.
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
pub(crate) struct NodeDevelopmentBinding {
    pub source_root: PathBuf,
    pub manifest_id: String,
    root_identity: PersistedIdentity,
    registered_manifest_text: String,
    pub registered_manifest_digest: String,
}
impl PartialEq for NodeDevelopmentBinding {
    fn eq(&self, other: &Self) -> bool {
        self.source_root == other.source_root
            && self.manifest_id == other.manifest_id
            && self.registered_manifest_digest == other.registered_manifest_digest
            && self.registered_manifest_text == other.registered_manifest_text
            && self.root_identity.device == other.root_identity.device
            && self.root_identity.inode == other.root_identity.inode
            && self.root_identity.volume_uuid == other.root_identity.volume_uuid
    }
}
impl NodeDevelopmentBinding {
    pub(crate) fn registered_manifest(&self) -> Result<Value> {
        Ok(serde_json::from_str(&self.registered_manifest_text)?)
    }
    pub(crate) fn validate_record(&self) -> Result<()> {
        let manifest = self.registered_manifest()?;
        if format!(
            "{:x}",
            Sha256::digest(self.registered_manifest_text.as_bytes())
        ) != self.registered_manifest_digest
            || manifest["app"]["id"] != self.manifest_id
            || manifest["runtime"] != "node-service"
        {
            return Err(development_changed());
        }
        Ok(())
    }
    pub(crate) fn source(&self, app_id: &str, runtime_is_source: bool) -> NodeDevelopmentSource {
        NodeDevelopmentSource {
            app_id: app_id.into(),
            source_root: self.source_root.clone(),
            manifest_id: self.manifest_id.clone(),
            runtime_is_source,
            root_identity: self.root_identity.clone(),
        }
    }
}

/// Owned identity token. It never contains saved application configuration.
#[derive(Clone, Debug)]
pub struct NodeDevelopmentSource {
    pub app_id: String,
    pub source_root: PathBuf,
    pub manifest_id: String,
    /// Until a runtime generation is published, editing this tree also changes
    /// the registered program. AI and source restore must first separate it.
    pub runtime_is_source: bool,
    root_identity: PersistedIdentity,
}
impl NodeDevelopmentSource {
    pub fn validate(&self) -> Result<()> {
        ordinary_path(&self.source_root, true)?;
        if !self
            .root_identity
            .matches_path(&self.source_root)
            .unwrap_or(false)
        {
            return Err(development_changed());
        }
        let manifest =
            TrialManifest::load(&self.source_root).map_err(|_| fail("源码中的应用说明无效。"))?;
        if manifest.runtime != TrialRuntime::NodeService || manifest.app.id != self.manifest_id {
            return Err(development_changed());
        }
        Ok(())
    }
}

/// The per-instance token blocks start/update/delete while an operation is
/// using the author tree, without holding NodeApps' global async mutex during
/// dependency installation, builds, or Git subprocesses. Stop stays available.
pub struct NodeDevelopmentSourceLease<'a> {
    apps: &'a NodeApps,
    source: NodeDevelopmentSource,
    registration_digest: String,
    active: Arc<DevelopmentActiveToken>,
}
struct DevelopmentActiveToken {
    active: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    id: String,
    _operation: NodeOperationPermit,
}
impl Drop for DevelopmentActiveToken {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&self.id);
        }
    }
}
impl NodeDevelopmentSourceLease<'_> {
    /// A registered process can keep the actual instance token after a caller
    /// or borrowed lease disappears. No lifetime is extended with unsafe code.
    pub(crate) fn keep_alive(&self) -> Arc<dyn Send + Sync> {
        self.active.clone()
    }
    pub fn source_root(&self) -> &Path {
        &self.source.source_root
    }
    pub fn source(&self) -> &NodeDevelopmentSource {
        &self.source
    }
    pub fn registration_digest(&self) -> &str {
        &self.registration_digest
    }
    pub fn control_path(&self) -> PathBuf {
        self.apps.root.join(&self.source.app_id)
    }
    pub fn source_restore_control_path(&self) -> PathBuf {
        self.control_path().join("source-restore.json")
    }
    pub fn validate(&self) -> Result<()> {
        self.apps.require_not_removing(&self.source.app_id)?;
        if node_app_install::registration_digest(&self.apps.root.join(&self.source.app_id))?
            .as_deref()
            != Some(&self.registration_digest)
        {
            return Err(development_changed());
        }
        self.source.validate()
    }
}

fn development_changed() -> ApiError {
    ApiError::new(
        "NODE_DEVELOPMENT_SOURCE_CHANGED",
        "源码目录或应用身份已变化，请重新绑定。",
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
pub(crate) fn require_export_location_unclaimed(data_root: &Path, parent: &Path) -> Result<()> {
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
    pub(super) fn require_no_development_operation(&self, id: &str) -> Result<()> {
        let active = self
            .development_active
            .lock()
            .map_err(|_| fail("无法核对开发操作状态。"))?;
        if active.contains(id) {
            return Err(ApiError::new(
                "NODE_DEVELOPMENT_BUSY",
                "开发操作进行中，请稍后重试。",
            ));
        }
        Ok(())
    }

    pub(crate) fn require_no_source_restore(&self, id: &str) -> Result<()> {
        crate::node_development_git::require_source_restore_control_clear(
            &self.root.join(id).join("source-restore.json"),
        )?;
        let registration = self.root.join(id).join("registration.json");
        crate::projects::check_no_symlinks(&registration, true)?;
        if !registration.try_exists()? {
            return Ok(());
        }
        let record = self.registration(id)?;
        let binding = self.read_development_binding(id)?.or_else(|| {
            record
                .managed
                .as_ref()
                .and_then(|managed| managed.development_source.clone())
        });
        let source = binding
            .map(|binding| binding.source_root)
            .or_else(|| record.managed.is_none().then_some(record.source_path));
        if let Some(source) = source {
            crate::node_development_git::require_source_restore_clear(&source)?;
        }
        Ok(())
    }

    /// Recovery owns the same instance token, while bypassing only the durable
    /// pending-restore gate. Runtime and deletion protections remain enforced.
    pub async fn development_restore_source_lease(
        &self,
        id: &str,
    ) -> Result<NodeDevelopmentSourceLease<'_>> {
        let lease = self.source_lease(id, false).await?;
        let inner = self.inner.lock().await;
        let marker = self.root.join(id).join("active.json");
        crate::projects::check_no_symlinks(&marker, true)?;
        if lease.source.runtime_is_source
            || inner.shutting_down
            || inner.running.contains_key(id)
            || marker.try_exists()?
            || has_start_intent_cleanup(&self.root.join(id))?
        {
            return Err(ApiError::new(
                "NODE_SOURCE_RESTORE_RUNTIME_ACTIVE",
                "请先分离运行副本并关闭应用，确认服务完成收尾后再取回源码。",
            ));
        }
        Ok(lease)
    }

    pub(super) fn read_development_binding(
        &self,
        id: &str,
    ) -> Result<Option<NodeDevelopmentBinding>> {
        let path = self.root.join(id).join("development-source.json");
        crate::projects::check_no_symlinks(&path, true)?;
        if !path.try_exists()? {
            return Ok(None);
        }
        Ok(Some(serde_json::from_slice(&read_small(&path)?)?))
    }

    pub(super) fn development_binding(
        &self,
        record: &Registration,
    ) -> Result<Option<NodeDevelopmentBinding>> {
        if let Some(binding) = self.read_development_binding(&record.id)? {
            binding.validate_record()?;
            // This checks the declared association only; a missing/replaced
            // source must not make an independently installed program fail.
            if record.managed.is_some() {
                let program: Value = serde_json::from_slice(&read_small(
                    &record.source_path.join("local-app.json"),
                )?)?;
                if program["app"]["id"] != binding.manifest_id {
                    return Err(development_changed());
                }
            } else if binding.source_root != record.source_path
                || binding.registered_manifest_digest != record.manifest_digest
            {
                return Err(development_changed());
            }
            return Ok(Some(binding));
        }
        if let Some(managed) = &record.managed {
            if let Some(binding) = &managed.development_source {
                binding.validate_record()?;
            }
            return Ok(managed.development_source.clone());
        }
        Ok(None)
    }

    pub(super) fn initialize_development_binding(
        &self,
        record: &ExternalRegistration,
    ) -> Result<()> {
        let source = &record.source_path;
        self.claim_source_directory(&record.id, source)?;
        let bytes = read_small(&source.join("local-app.json"))?;
        let manifest = TrialManifest::parse(&bytes).map_err(|_| development_changed())?;
        let binding = NodeDevelopmentBinding {
            source_root: source.clone(),
            manifest_id: manifest.app.id,
            root_identity: PersistedIdentity::capture(source).map_err(|_| development_changed())?,
            registered_manifest_text: String::from_utf8(bytes)
                .map_err(|_| development_changed())?,
            registered_manifest_digest: record.manifest_digest.clone(),
        };
        binding.validate_record()?;
        if let Some(previous) = self.read_development_binding(&record.id)? {
            if previous != binding {
                return Err(development_changed());
            }
            return Ok(());
        }
        write_new_json(
            &self.root.join(&record.id).join("development-source.json"),
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

    /// Registration checks before allocating an instance or its private files.
    /// The normal same-app registration shortcut runs before this method.
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

    /// A generated starter is a pending instance from its first accepted write.
    /// Only its exact durable source proof can reuse that instance for its first
    /// registration; arbitrary external directories always receive a fresh UUID.
    pub(super) fn pending_generated_source_id(&self, source: &Path) -> Result<Option<String>> {
        let Some(owner) = self.source_owner(source)? else {
            return Ok(None);
        };
        if !self
            .project_ownership()
            .has_generated_source(&owner.app_id, source)?
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

    /// A live ownership claim makes author trees exclusive at admission time.
    /// Deletion consumes this project's recorded claim; it does not scan other
    /// instances or maintain a shared-source reference count.
    pub(crate) fn claim_source_directory(&self, id: &str, source: &Path) -> Result<()> {
        claim_source_path(&self.data_root, self.deletion_workspace(), id, source)
    }

    pub(super) fn discard_pending_generated_source(&self, id: &str, source: &Path) -> Result<()> {
        let _guard = SOURCE_OWNERSHIP_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .map_err(|_| fail("源码归属锁不可用。"))?;
        let ownership = self.project_ownership();
        if !ownership.has_generated_source(id, source)? {
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

    /// Before registration publication no runtime can have started. Undo only
    /// the newly captured private slot; generated source retains its pending
    /// identity, while ordinary external author directories are never targets.
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

    pub async fn development_source_lease(
        &self,
        id: &str,
    ) -> Result<NodeDevelopmentSourceLease<'_>> {
        self.source_lease(id, false).await
    }

    pub async fn development_build_source_lease(
        &self,
        id: &str,
    ) -> Result<NodeDevelopmentSourceLease<'_>> {
        self.source_lease(id, true).await
    }

    pub(crate) async fn development_build_source_lease_admitted(
        &self,
        id: &str,
        permit: &NodeOperationPermit,
    ) -> Result<NodeDevelopmentSourceLease<'_>> {
        permit.validate(self, id)?;
        self.source_lease_admitted(id, true, permit.clone()).await
    }

    async fn source_lease(
        &self,
        id: &str,
        require_stopped: bool,
    ) -> Result<NodeDevelopmentSourceLease<'_>> {
        let permit = self.admit_operation(id, "Git 或源码操作")?;
        self.source_lease_admitted(id, require_stopped, permit)
            .await
    }

    async fn source_lease_admitted(
        &self,
        id: &str,
        require_stopped: bool,
        permit: NodeOperationPermit,
    ) -> Result<NodeDevelopmentSourceLease<'_>> {
        let inner = self.inner.lock().await;
        if inner.shutting_down {
            return Err(fail("平台正在关闭，不能开始开发操作。"));
        }
        self.require_not_removing(id)?;
        self.require_no_development_operation(id)?;
        if require_stopped {
            self.require_install_stopped(id, &inner)?;
        }
        let record = self.registration(id)?;
        let binding = self.development_binding(&record)?.ok_or_else(|| {
            ApiError::new(
                "NODE_DEVELOPMENT_SOURCE_UNBOUND",
                "请先选择此应用的源码目录。",
            )
        })?;
        let source = binding.source(id, record.managed.is_none());
        if source.source_root.starts_with(&self.data_root)
            && !self.is_published_source(&source.source_root)
            || self.data_root.starts_with(&source.source_root)
            || source
                .source_root
                .components()
                .any(|part| part.as_os_str() == ".platform")
        {
            return Err(fail("请选择平台运行目录之外的源码仓库。"));
        }
        source.validate()?;
        self.claim_source_directory(id, &source.source_root)?;
        let registration_digest = node_app_install::registration_digest(&self.root.join(id))?
            .ok_or_else(development_changed)?;
        self.development_active
            .lock()
            .map_err(|_| fail("无法登记开发操作。"))?
            .insert(id.into());
        let lease = NodeDevelopmentSourceLease {
            apps: self,
            source,
            registration_digest,
            active: Arc::new(DevelopmentActiveToken {
                active: self.development_active.clone(),
                id: id.into(),
                _operation: permit,
            }),
        };
        lease.validate()?;
        Ok(lease)
    }

    /// Binding is explicit and never clones, initializes Git, or changes source
    /// files. An external application already has its authoritative source.
    pub async fn bind_development_source(&self, id: &str, source: PathBuf) -> Result<Value> {
        let _operation = self.admit_operation(id, "源码绑定")?;
        let inner = self.inner.lock().await;
        self.bind_development_source_locked(id, source, &inner, false)
    }

    pub(crate) async fn bind_imported_development_source(
        &self,
        id: &str,
        source: PathBuf,
        expected_source_id: &str,
        expected_digest: &str,
        expected_generation: &str,
        permit: &NodeOperationPermit,
    ) -> Result<Value> {
        permit.validate(self, id)?;
        let inner = self.inner.lock().await;
        self.require_not_removing(id)?;
        let record = self.registration(id)?;
        let managed = record.managed.as_ref().ok_or_else(development_changed)?;
        if managed.provenance.source_id != expected_source_id
            || managed.provenance.source_digest != expected_digest
            || managed.generation != expected_generation
        {
            return Err(development_changed());
        }
        if self
            .read_development_binding(id)?
            .is_some_and(|binding| binding.source_root != source)
        {
            return Err(development_changed());
        }
        self.bind_development_source_locked(id, source, &inner, true)
    }

    fn bind_development_source_locked(
        &self,
        id: &str,
        source: PathBuf,
        inner: &Inner,
        imported_publication: bool,
    ) -> Result<Value> {
        self.require_install_stopped(id, inner)?;
        let record = self.registration(id)?;
        if record.managed.is_none() && source != record.source_path {
            return Err(fail(
                "此应用直接运行原登记目录，不能更换源码目录。请在原项目文件夹中修改。",
            ));
        }
        let runtime_manifest = self.manifest(&record)?;
        if !self.is_published_source(&source) {
            self.require_external_source_boundary(&source)?;
        }
        let source_manifest = self.load_source(&source)?;
        crate::node_development_git::require_source_restore_clear(&source)?;
        if runtime_manifest.app.id != source_manifest.app.id {
            return Err(fail("所选源码不属于此应用。"));
        }
        if self.is_published_source(&source) && !imported_publication {
            // Only the validated publication flow may establish the first
            // claim. A public bind cannot take a directory while publication
            // has made it visible but not yet bound its intended instance.
            let (hash, identity) = self.source_claim_key(&source)?;
            let owner_path = self
                .data_root
                .join("node-source-owners")
                .join(hash)
                .join("owner.json");
            let owner: SourceDirectoryOwner = serde_json::from_slice(&read_small(&owner_path)?)?;
            if owner.version != 1
                || owner.app_id != id
                || owner.source_root != source
                || !owner.identity.matches_captured(&identity)
            {
                return Err(ApiError::new(
                    "NODE_SOURCE_ALREADY_OWNED",
                    "平台发布的源码只能由所属项目使用。",
                ));
            }
        }
        self.claim_source_directory(id, &source)?;
        let bytes = read_small(&record.source_path.join("local-app.json"))?;
        let identity = PersistedIdentity::capture(&source).map_err(|_| development_changed())?;
        let binding = NodeDevelopmentBinding {
            source_root: source,
            manifest_id: source_manifest.app.id,
            root_identity: identity,
            registered_manifest_text: String::from_utf8(bytes)
                .map_err(|_| development_changed())?,
            registered_manifest_digest: record.manifest_digest.clone(),
        };
        binding.validate_record()?;
        crate::builds::atomic_json(
            &self.root.join(id).join("development-source.json"),
            &binding,
        )?;
        self.detail(&record, &runtime_manifest, inner)
    }
}

/// Admission and the final frozen-copy boundary both call this check. Only
/// this live ownership index supplies source paths; arbitrary author directories
/// and retired project formats are never searched for recovery files.
pub(crate) fn require_source_restores_clear_for_migration(
    data_root: &Path,
    workspace: &Path,
) -> Result<()> {
    let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
        data_root.to_path_buf(),
        workspace.to_path_buf(),
    );
    for (ledger, paths) in ownership.all_owned_scopes()? {
        let id = ledger
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(development_changed)?;
        for directory in paths
            .iter()
            .filter(|path| path.parent() == Some(data_root.join("node-source-owners").as_path()))
        {
            if !directory.try_exists()? {
                continue;
            }
            let owner: SourceDirectoryOwner =
                serde_json::from_slice(&read_small(&directory.join("owner.json"))?)?;
            require_claimed_source_owner(data_root, id, &owner.source_root)?;
            crate::node_development_git::require_source_restore_clear(&owner.source_root)
                .map_err(|error| migration_restore_error(error, id, &owner.source_root))?;
        }
        crate::node_development_git::require_source_restore_control_clear(
            &data_root
                .join("node-apps")
                .join(id)
                .join("source-restore.json"),
        )
        .map_err(|error| error.detail("appId", id))?;
    }
    Ok(())
}
fn migration_restore_error(mut error: ApiError, id: &str, source: &Path) -> ApiError {
    if error.code == "NODE_SOURCE_RESTORE_PENDING" {
        error.message = "请先继续或撤回源码取回，或删除该项目，再迁移工作区。".into();
    }
    error
        .detail("appId", id)
        .detail("sourcePath", json!(source))
}

/// A generated source may live at an explicitly selected custom workspace path.
/// Its current registration follows the verified copy, while both copies remain
/// exclusively owned until this instance is deleted.
fn rebind_generated_source_after_migration(
    data_root: &Path,
    old: &Path,
    new: &Path,
    id: &str,
    binding_path: &Path,
    binding: &NodeDevelopmentBinding,
) -> Result<bool> {
    let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
        data_root.to_path_buf(),
        new.to_path_buf(),
    );
    let Some(generated) = ownership.generated_source(id)? else {
        return Ok(false);
    };
    let relative = if let Ok(relative) = generated.strip_prefix(old) {
        relative
    } else if let Ok(relative) = generated.strip_prefix(new) {
        relative
    } else {
        return Ok(false);
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(development_changed());
    }
    let previous = old.join(relative);
    let target = new.join(relative);
    if binding.source_root != previous && binding.source_root != target {
        // A managed instance may have explicitly rebound its active author
        // directory. The earlier generated tree remains owned material, while
        // its independent external binding keeps its own migration policy.
        return Ok(false);
    }
    binding.validate_record()?;
    let declared = require_claimed_source_owner(data_root, id, &binding.source_root)?;
    if !binding.root_identity.matches_captured(&declared) {
        return Err(development_changed());
    }
    ordinary_path(&target, true)?;
    if !ownership.has_generated_source(id, &generated)? {
        return Err(development_changed());
    }
    let manifest = TrialManifest::load(&target).map_err(|_| development_changed())?;
    if manifest.app.id != binding.manifest_id {
        return Err(development_changed());
    }
    let registration_path = data_root
        .join("node-apps")
        .join(id)
        .join("registration.json");
    let mut registration: Value = serde_json::from_slice(&read_small(&registration_path)?)?;
    if registration["id"] != id {
        return Err(development_changed());
    }
    let mut rebound = binding.clone();
    rebound.source_root = target.clone();
    rebound.root_identity =
        PersistedIdentity::capture(&target).map_err(|_| development_changed())?;
    match registration["version"].as_u64() {
        Some(1) => {
            let mut record: ExternalRegistration = serde_json::from_value(registration)?;
            if (record.source_path != previous && record.source_path != target)
                || record.manifest_digest != binding.registered_manifest_digest
            {
                return Err(development_changed());
            }
            if digest_manifest(&target)? != record.manifest_digest {
                return Err(development_changed());
            }
            record.source_path = target.clone();
            registration = serde_json::to_value(record)?;
        }
        Some(2) => {
            let mut record: ManagedNodeRegistration = serde_json::from_value(registration)?;
            if let Some(recorded) = &record.development_source {
                if recorded.manifest_id != binding.manifest_id
                    || (recorded.source_root != previous && recorded.source_root != target)
                {
                    return Err(development_changed());
                }
            }
            record.development_source = Some(rebound.clone());
            registration = serde_json::to_value(record)?;
        }
        _ => return Err(development_changed()),
    }
    claim_source_after_verified_migration(data_root, new, id, &target)?;
    crate::builds::atomic_json(binding_path, &rebound)?;
    crate::builds::atomic_json(&registration_path, &registration)?;
    Ok(true)
}

/// Called only by the verified public workspace migration, after the target
/// copy has been selected. External author folders are deliberately untouched.
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
    for entry in fs::read_dir(apps)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if Uuid::parse_str(&id).is_err() {
            continue;
        }
        let path = entry.path().join("development-source.json");
        if !path.try_exists()? {
            continue;
        }
        ordinary_path(&path, false)?;
        let mut binding: NodeDevelopmentBinding = serde_json::from_slice(&read_small(&path)?)?;
        if rebind_generated_source_after_migration(data_root, old, new, &id, &path, &binding)? {
            continue;
        }
        let (relative, already_rebound) = if let Ok(relative) = binding
            .source_root
            .strip_prefix(old.join("project-sources"))
        {
            (relative, false)
        } else if let Ok(relative) = binding
            .source_root
            .strip_prefix(new.join("project-sources"))
        {
            (relative, true)
        } else {
            continue;
        };
        if relative.components().count() != 1
            || !matches!(relative.components().next(), Some(Component::Normal(_)))
        {
            return Err(development_changed());
        }
        binding.validate_record()?;
        let target = new.join("project-sources").join(relative);
        ordinary_path(&target, true)?;
        let receipt = new
            .join(".platform/source-publications")
            .join(&id)
            .join("publication.json");
        ordinary_path(&receipt, false)?;
        let mut publication: Value = serde_json::from_slice(&read_small(&receipt)?)?;
        let original: PersistedIdentity =
            serde_json::from_value(publication["payloadIdentity"].clone())?;
        if publication["appId"] != id
            || publication["directory"].as_str() != relative.to_str()
            || (!already_rebound && !binding.root_identity.matches_captured(&original))
            || (already_rebound
                && (!binding
                    .root_identity
                    .matches_path(&target)
                    .map_err(|_| development_changed())?
                    || (!binding.root_identity.matches_captured(&original)
                        && !original
                            .matches_path(&old.join("project-sources").join(relative))
                            .map_err(|_| development_changed())?)))
        {
            return Err(development_changed());
        }
        let manifest = TrialManifest::load(&target).map_err(|_| development_changed())?;
        if manifest.app.id != binding.manifest_id {
            return Err(development_changed());
        }
        binding.source_root = target;
        binding.root_identity =
            PersistedIdentity::capture(&binding.source_root).map_err(|_| development_changed())?;
        publication["payloadIdentity"] = serde_json::to_value(&binding.root_identity)?;
        claim_source_path(data_root, new.to_path_buf(), &id, &binding.source_root)?;
        // Rebind the per-instance association before updating its copied receipt.
        // A retry sees the new association and never rebinds an external folder.
        crate::builds::atomic_json(&path, &binding)?;
        crate::builds::atomic_json(&receipt, &publication)?;
    }
    // A crash can occur after exclusive directory publication but before its
    // Node binding is written. The verified public migration also carries this
    // receipt, so retain its exact old ownership proof while rebasing identity.
    let publications = new.join(".platform/source-publications");
    if publications.try_exists()? {
        ordinary_path(&publications, true)?;
        for entry in fs::read_dir(&publications)? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if Uuid::parse_str(&id).is_err() {
                continue;
            }
            let path = entry.path().join("publication.json");
            if !path.try_exists()? {
                continue;
            }
            ordinary_path(&path, false)?;
            let mut publication: Value = serde_json::from_slice(&read_small(&path)?)?;
            let Some(name) = publication["directory"].as_str().map(str::to_owned) else {
                continue;
            };
            if publication["appId"] != id
                || Path::new(&name).components().count() != 1
                || !matches!(
                    Path::new(&name).components().next(),
                    Some(Component::Normal(_))
                )
            {
                return Err(development_changed());
            }
            let target = new.join("project-sources").join(&name);
            if !target.try_exists()? {
                continue;
            }
            ordinary_path(&target, true)?;
            let original: PersistedIdentity =
                serde_json::from_value(publication["payloadIdentity"].clone())?;
            if original
                .matches_path(&target)
                .map_err(|_| development_changed())?
            {
                claim_source_path(data_root, new.to_path_buf(), &id, &target)?;
                continue;
            }
            let previous = old.join("project-sources").join(&name);
            if !original
                .matches_path(&previous)
                .map_err(|_| development_changed())?
            {
                return Err(development_changed());
            }
            publication["payloadIdentity"] = serde_json::to_value(
                PersistedIdentity::capture(&target).map_err(|_| development_changed())?,
            )?;
            claim_source_path(data_root, new.to_path_buf(), &id, &target)?;
            crate::builds::atomic_json(&path, &publication)?;
        }
    }
    Ok(())
}

pub(super) fn build_capability(binding: &NodeDevelopmentBinding, available: bool) -> Value {
    if !available {
        return json!({"state":"unavailable","reason":"请先绑定可用的源码目录。"});
    }
    let manifest = match TrialManifest::load(&binding.source_root) {
        Ok(manifest) => manifest,
        Err(_) => return json!({"state":"unavailable","reason":"源码中的应用说明无效。"}),
    };
    let package_root = match node_app_install::npm_root(&binding.source_root, &manifest) {
        Ok(root) => root,
        Err(_) => return json!({"state":"unavailable","reason":"缺少完整的依赖声明与锁文件。"}),
    };
    if node_app_install::validate_npm_input(&package_root).is_err() {
        return json!({"state":"unavailable","reason":"依赖声明或锁文件需要修正。"});
    }
    let reason = match manifest.development.as_ref().map(|value| &value.build) {
        Some(crate::node_app_manifest::NodeDevelopmentBuild::Npm { .. }) => {
            "已声明构建流程；结果将在构建时校验。"
        }
        _ => "可准备运行副本；结果将在构建时校验。",
    };
    json!({"state":"platform-build","reason":reason})
}

#[cfg(test)]
#[path = "node_app_development_tests.rs"]
mod tests;
