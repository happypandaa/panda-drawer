//! Native editable-source publication and identity-bound interruption recovery.
#[cfg(any(unix, windows))]
use super::*;
#[cfg(unix)]
#[path = "source_publication/unix.rs"]
mod unix;

#[cfg(windows)]
#[path = "source_publication/windows.rs"]
mod windows;

impl SourceAcquisitionManager {
    #[cfg(target_os = "macos")]
    fn selected_receipt(&self, app_id: &str, create: bool) -> Result<PathBuf, ApiError> {
        if Uuid::parse_str(app_id)
            .map(|id| id.to_string() != app_id)
            .unwrap_or(true)
        {
            return Err(invalid("应用标识无效。"));
        }
        let receipt = self
            .paths
            .workspace_root()
            .join(".platform/source-publications")
            .join(app_id);
        if create {
            let owner = crate::node_project_ownership::NodeProjectOwnership::new(
                self.paths.data_root().into(),
                self.paths.workspace_root().into(),
            );
            owner.record_publication(app_id)?;
            private_dir(&receipt)?;
            owner.record_publication(app_id)?;
        }
        Ok(receipt)
    }

    #[cfg(target_os = "macos")]
    fn selected_binding(
        &self,
        source: &SourceCandidate,
        app_id: &str,
        lease_id: &str,
    ) -> Result<crate::source_acquisition::selected_directory::Binding, ApiError> {
        if self.owner_app_id(&source.source_id)? != app_id {
            return Err(invalid("来源只属于原项目，不能发布给另一个实例。"));
        }
        let digest = source
            .content_digest
            .as_deref()
            .ok_or_else(|| invalid("源码摘要缺失。"))?;
        let current = self.verify_ready_leased(&source.source_id, digest, lease_id)?;
        if current.task_id != source.task_id {
            return Err(invalid("源码任务身份已变化，未发布。"));
        }
        Ok(crate::source_acquisition::selected_directory::Binding {
            source_id: source.source_id.clone(),
            app_id: app_id.into(),
            lease_id: lease_id.into(),
            digest: digest.into(),
        })
    }

    pub(crate) fn prepare_selected_source(
        &self,
        source: &SourceCandidate,
        app_id: &str,
        lease_id: &str,
        selection: &crate::source_acquisition::SelectedDirectory,
    ) -> Result<PathBuf, ApiError> {
        #[cfg(target_os = "macos")]
        {
            let binding = self.selected_binding(source, app_id, lease_id)?;
            crate::source_acquisition::validate_selected_directory(selection)?;
            let receipt = self.selected_receipt(app_id, true)?;
            let original = source
                .source_path
                .as_deref()
                .ok_or_else(|| invalid("源码位置缺失。"))?;
            crate::source_acquisition::selected_directory::prepare(
                &receipt,
                &binding,
                selection,
                || unix::copy_selected_working(original, &receipt),
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (source, app_id, lease_id, selection);
            Err(crate::source_acquisition::selected_directory::unavailable())
        }
    }

    pub(crate) fn publish_selected_source(
        &self,
        source: &SourceCandidate,
        app_id: &str,
        lease_id: &str,
        selection: &crate::source_acquisition::SelectedDirectory,
    ) -> Result<PathBuf, ApiError> {
        #[cfg(target_os = "macos")]
        {
            let binding = self.selected_binding(source, app_id, lease_id)?;
            crate::source_acquisition::selected_directory::publish(
                &self.selected_receipt(app_id, false)?,
                &binding,
                selection,
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (source, app_id, lease_id, selection);
            Err(crate::source_acquisition::selected_directory::unavailable())
        }
    }

    pub(crate) fn reconcile_selected_publication(
        &self,
        source_id: &str,
        app_id: &str,
        lease_id: &str,
        selection: &crate::source_acquisition::SelectedDirectory,
    ) -> Result<Option<PathBuf>, ApiError> {
        #[cfg(target_os = "macos")]
        {
            let receipt = self.selected_receipt(app_id, false)?;
            if !receipt.try_exists()? {
                return Ok(None);
            }
            crate::source_acquisition::selected_directory::reconcile(
                &receipt, source_id, app_id, lease_id, selection,
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (source_id, app_id, lease_id, selection);
            Err(crate::source_acquisition::selected_directory::unavailable())
        }
    }

    pub(crate) fn cleanup_selected_publication(
        &self,
        source_id: &str,
        app_id: &str,
        lease_id: &str,
        selection: &crate::source_acquisition::SelectedDirectory,
        committed: bool,
    ) -> Result<(), ApiError> {
        #[cfg(target_os = "macos")]
        {
            // Successful registration releases only the private receipt via the
            // importer's existing cleanup. User source never becomes owned.
            if committed {
                return Ok(());
            }
            let receipt = self.selected_receipt(app_id, false)?;
            if !receipt.try_exists()? {
                return Ok(());
            }
            crate::source_acquisition::selected_directory::cleanup(
                &receipt, source_id, app_id, lease_id, selection,
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (source_id, app_id, lease_id, selection, committed);
            Err(crate::source_acquisition::selected_directory::unavailable())
        }
    }
}
