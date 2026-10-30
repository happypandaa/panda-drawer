//! Window-only entry snapshots for readable projects without build provenance.
//! Receipts are in the fixed project control root, never copied during migration.
use super::*;
use std::io::Read;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ViewReceipt {
    version: u32,
    project_id: String,
    pub(super) epoch: String,
    view_id: String,
    pub(super) host: String,
    pub(super) scope: String,
    owner_identity: PersistedIdentity,
    container_identity: PersistedIdentity,
    source_path: PathBuf,
    source_identity: PersistedIdentity,
    pub(super) phase: String,
    entry: String,
}
pub(super) fn valid_view_id(id: &str) -> bool {
    id.len() == 37
        && id.starts_with("view_")
        && id[5..].bytes().all(|byte| byte.is_ascii_hexdigit())
}
pub(super) fn view_error() -> ApiError {
    ApiError::new(
        "BUILD_VIEW_INVALID",
        "The window snapshot no longer has its original ownership receipt",
    )
}
fn identity(path: &std::path::Path) -> Result<PersistedIdentity, ApiError> {
    PersistedIdentity::capture(path)
        .map_err(|error| ApiError::new("FILESYSTEM_ERROR", error.to_string()))
}
impl ArtifactAccess {
    pub(super) fn views_root(&self, id: &str) -> Result<PathBuf, ApiError> {
        crate::web_project_ownership::WebProjectOwnership::new(
            self.0.paths.data_root().into(),
            self.0.paths.workspace_root().into(),
        )
        .directory(id)
        .map(|root| root.join("artifact-views"))
    }
    pub(super) fn capture_view(
        &self,
        project: &ProjectHandle,
    ) -> Result<ArtifactGeneration, ApiError> {
        let output = checked_relative(&project.root, &project.manifest.output_dir, false)?;
        let entry = std::path::Path::new(&project.manifest.entry)
            .strip_prefix(&project.manifest.output_dir)
            .ok()
            .and_then(|path| path.to_str())
            .ok_or_else(view_error)?
            .to_owned();
        builds::validate_artifact(&output, &entry)?;
        let digest = builds::tree_digest(&output, false)?;
        let ownership = crate::web_project_ownership::WebProjectOwnership::new(
            self.0.paths.data_root().into(),
            self.0.paths.workspace_root().into(),
        );
        // New window material only. This does not adopt or migrate author source.
        ownership.initialize(&project.manifest.id)?;
        let owner_root = ownership.directory(&project.manifest.id)?;
        let views = self.views_root(&project.manifest.id)?;
        builds::ensure_dir(&views)?;
        let id = format!("view_{}", Uuid::new_v4().simple());
        let container = views.join(&id);
        fs::create_dir(&container)?;
        let owner_identity = identity(&owner_root)?;
        let container_identity = identity(&container)?;
        let mut receipt = ViewReceipt {
            version: 1,
            project_id: project.manifest.id.clone(),
            epoch: project.document_epoch.clone(),
            view_id: id.clone(),
            host: host().into(),
            scope: self.0.scope.clone(),
            owner_identity: owner_identity.clone(),
            container_identity: container_identity.clone(),
            source_path: project.root.clone(),
            source_identity: identity(&project.root)?,
            phase: "preparing".into(),
            entry: entry.clone(),
        };
        let result = (|| {
            // This receipt precedes all payload writes and also covers a crash
            // during the copy; no task or browsing history is retained.
            builds::atomic_json(&container.join("view.json"), &receipt)?;
            #[cfg(test)]
            if let Some((entered, release)) = self.0.capture_pause.lock().unwrap().take() {
                let _ = entered.send(());
                let _ = release.recv();
            }
            builds::copy_tree(&output, &container.join("dist"))?;
            project.check()?;
            builds::validate_artifact(&container.join("dist"), &entry)?;
            if builds::tree_digest(&output, false)? != digest
                || builds::tree_digest(&container.join("dist"), false)? != digest
            {
                return Err(ApiError::new(
                    "BUILD_INPUT_CHANGED",
                    "The webpage changed while its window entry was being captured",
                ));
            }
            receipt.phase = "ready".into();
            builds::atomic_json(&container.join("view.json"), &receipt)?;
            Ok(ArtifactGeneration {
                build_id: id,
                root: container.join("dist"),
                entry_relative: entry,
            })
        })();
        if result.is_err() && owner_identity.matches_path(&owner_root).unwrap_or(false) {
            let _ = crate::history_cleanup::remove_owned_directory(&container, &container_identity);
        }
        result
    }
    pub(super) fn view_receipt(
        &self,
        id: &str,
        view: &str,
    ) -> Result<(PathBuf, ViewReceipt), ApiError> {
        if !valid_view_id(view) {
            return Err(view_error());
        }
        let views = self.views_root(id)?;
        let owner = views.parent().ok_or_else(view_error)?;
        let container = views.join(view);
        check_no_symlinks(&container, false)?;
        let mut file = platform_native::filesystem::open_read(
            container.join("view.json"),
            platform_native::filesystem::ReadOptions::NoLinksCloseOnExec,
        )?;
        let metadata = platform_native::tool_files::Facts::from_file(&file)?;
        if !metadata.is_file() || metadata.links() != 1 || metadata.len() > 16384 {
            return Err(view_error());
        }
        let mut bytes = vec![];
        (&mut file).take(16385).read_to_end(&mut bytes)?;
        let receipt: ViewReceipt = serde_json::from_slice(&bytes)?;
        if receipt.version != 1
            || receipt.project_id != id
            || receipt.view_id != view
            || Uuid::parse_str(&receipt.epoch).is_err()
            || Uuid::parse_str(&receipt.host).is_err()
            || Uuid::parse_str(&receipt.scope).is_err()
            || !matches!(receipt.phase.as_str(), "preparing" | "ready")
            || !receipt.owner_identity.matches_path(owner).unwrap_or(false)
            || !receipt
                .container_identity
                .matches_path(&container)
                .unwrap_or(false)
        {
            return Err(view_error());
        }
        Ok((container, receipt))
    }
    fn remove_view(&self, id: &str, view: &str, own_scope: bool) -> Result<PathBuf, ApiError> {
        let (path, receipt) = self.view_receipt(id, view)?;
        if own_scope && (receipt.scope != self.0.scope || receipt.host != host()) {
            return Err(view_error());
        }
        crate::history_cleanup::remove_owned_directory(&path, &receipt.container_identity)?;
        Ok(path)
    }
    pub(super) fn schedule_view_cleanup(&self, key: &Key) {
        let mut state = self.0.state.lock().unwrap();
        let label = format!("view:{}", key.build);
        if state.stopping
            || state.sealed_instances.contains(&key.instance())
            || !state.cleaning.insert(label.clone())
        {
            return;
        }
        let owner = self.clone();
        let key = key.clone();
        let instance = key.instance();
        self.scheduling_checkpoint();
        let job = self.0.runtime.spawn_blocking(move || {
            // A retired workspace may be gone; only these fixed control objects
            // are touched, after exact owner/container/scope identity checks.
            if let Err(issue) = owner.remove_view(&key.project, &key.build, true) {
                if owner
                    .views_root(&key.project)
                    .is_ok_and(|root| root.join(&key.build).exists())
                {
                    eprintln!("Window build snapshot cleanup pending: {}", issue.code);
                }
            }
            owner.0.state.lock().unwrap().cleaning.remove(&label);
        });
        self.register_job(instance, job);
        drop(state);
    }
    pub(super) fn cleanup_views(
        &self,
        project: &ProjectHandle,
        report: &mut CleanupReport,
    ) -> Result<(), ApiError> {
        let views = self.views_root(&project.manifest.id)?;
        check_no_symlinks(&views, true)?;
        if !views.is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(&views)? {
            let entry = entry?;
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !valid_view_id(&id) {
                continue;
            }
            let (path, receipt) = match self.view_receipt(&project.manifest.id, &id) {
                Ok(value) => value,
                Err(issue) => {
                    report.issue(entry.path(), issue);
                    continue;
                }
            };
            let mut state = self.0.state.lock().unwrap();
            if state
                .pins
                .keys()
                .any(|key| key.project == project.manifest.id && key.build == id)
            {
                continue;
            }
            // A different still-running/unknown scope in this same host may
            // belong to a pre-migration state. Fresh registries cannot adopt it.
            let eligible = receipt.host != host()
                || receipt.scope == self.0.scope
                || scopes().lock().unwrap().get(&receipt.scope) == Some(&false);
            if !eligible {
                continue;
            }
            let key = Key::new(project, &id);
            if !state.deleting.insert(key.clone()) {
                continue;
            }
            drop(state);
            let result = self.remove_view(&project.manifest.id, &id, false);
            self.0.state.lock().unwrap().deleting.remove(&key);
            match result {
                Ok(path) => report.removed_paths.push(path),
                Err(issue) => report.issue(path, issue),
            }
        }
        Ok(())
    }
}
