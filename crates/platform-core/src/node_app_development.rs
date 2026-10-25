//! Binds one editable author tree to a Node instance. Runtime generations and
//! immutable download candidates are never silently promoted to author trees.
use super::*;
use crate::file_identity::PersistedIdentity;

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
    pub fn source_restore_control_path(&self) -> PathBuf {
        self.apps
            .root
            .join(&self.source.app_id)
            .join("source-restore.json")
    }
    pub fn validate(&self) -> Result<()> {
        self.apps.require_no_pending_delete(&self.source.app_id)?;
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
        // Upgrade legacy metadata only after matching the exact original
        // declaration. Never guess an earlier configuration schema.
        self.manifest(record)?;
        let bytes = read_small(&record.source_path.join("local-app.json"))?;
        let manifest: Value = serde_json::from_slice(&bytes)?;
        let binding = NodeDevelopmentBinding {
            source_root: record.source_path.clone(),
            manifest_id: manifest["app"]["id"]
                .as_str()
                .ok_or_else(development_changed)?
                .into(),
            root_identity: PersistedIdentity::capture(&record.source_path)
                .map_err(|_| development_changed())?,
            registered_manifest_text: String::from_utf8(bytes)
                .map_err(|_| development_changed())?,
            registered_manifest_digest: record.manifest_digest.clone(),
        };
        binding.validate_record()?;
        write_new_json(
            &self.root.join(&record.id).join("development-source.json"),
            &binding,
        )?;
        Ok(Some(binding))
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

    async fn source_lease(
        &self,
        id: &str,
        require_stopped: bool,
    ) -> Result<NodeDevelopmentSourceLease<'_>> {
        let inner = self.inner.lock().await;
        if inner.shutting_down {
            return Err(fail("平台正在关闭，不能开始开发操作。"));
        }
        self.require_no_pending_delete(id)?;
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
            || self.data_root.starts_with(&source.source_root)
            || source
                .source_root
                .components()
                .any(|part| part.as_os_str() == ".platform")
        {
            return Err(fail("请选择平台运行目录之外的源码仓库。"));
        }
        source.validate()?;
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
            }),
        };
        lease.validate()?;
        Ok(lease)
    }

    /// Binding is explicit and never clones, initializes Git, or changes source
    /// files. An external application already has its authoritative source.
    pub async fn bind_development_source(&self, id: &str, source: PathBuf) -> Result<Value> {
        let inner = self.inner.lock().await;
        self.require_install_stopped(id, &inner)?;
        let record = self.registration(id)?;
        if record.managed.is_none() && source != record.source_path {
            return Err(fail(
                "此应用的源码目录已由原登记确定，请先构建并更新运行副本。",
            ));
        }
        let runtime_manifest = self.manifest(&record)?;
        let source_manifest = self.load_source(&source)?;
        crate::node_development_git::require_source_restore_clear(&source)?;
        if runtime_manifest.app.id != source_manifest.app.id {
            return Err(fail("所选源码不属于此应用。"));
        }
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
        self.detail(&record, &runtime_manifest, &inner)
    }
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
