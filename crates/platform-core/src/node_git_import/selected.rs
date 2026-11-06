//! Selected folders are user-owned. Preparation stays private until publication.
use super::*;
use crate::{projects::ProjectOwnership, source_acquisition::SourceCandidate};

impl NodeGitImportManager {
    pub(super) fn import_ownership(&self, j: &Journal) -> ProjectOwnership {
        if j.input.selected_directory.is_some() {
            ProjectOwnership::User
        } else {
            ProjectOwnership::Platform
        }
    }

    pub(super) fn registration_source(&self, j: &Journal) -> Result<(PathBuf, PersistedIdentity)> {
        if let Some(selected) = &j.input.selected_directory {
            return Ok((
                selected.path.clone(),
                j.selected_root_identity
                    .clone()
                    .ok_or_else(|| invalid("所选文件夹身份缺失。"))?,
            ));
        }
        Ok((
            j.source_path
                .clone()
                .ok_or_else(|| invalid("项目目录缺失。"))?,
            j.source_identity
                .clone()
                .ok_or_else(|| invalid("项目目录身份缺失。"))?,
        ))
    }

    pub(super) fn validate_selected_target(&self, j: &Journal) -> Result<()> {
        let Some(selected) = &j.input.selected_directory else {
            return Ok(());
        };
        let actual = crate::source_acquisition::validate_selected_directory(selected)?;
        if !j
            .selected_root_identity
            .as_ref()
            .is_some_and(|expected| expected.matches_captured(&actual))
        {
            return Err(invalid("所选文件夹已变化，请重新选择并检查。"));
        }
        Ok(())
    }

    pub(super) async fn publish_prepared_selected(
        &self,
        j: &mut Journal,
        source: &SourceCandidate,
        scope: Arc<ImportScope>,
    ) -> Result<()> {
        let Some(selected) = j.input.selected_directory.clone() else {
            return Ok(());
        };
        self.validate_selected_target(j)?;
        let lease = self
            .store
            .reserve_task(&format!("source:{}", j.source_id), &j.task_id)?;
        let sources = self.sources.as_ref().map_err(Clone::clone)?.clone();
        let source = source.clone();
        let app_id = j.app_id.clone();
        let task_id = j.task_id.clone();
        self.phase(j, "publishing")?;
        let root = tokio::task::spawn_blocking(move || {
            let _scope = (scope, lease);
            sources.publish_selected_source(&source, &app_id, &task_id, &selected)
        })
        .await
        .map_err(|_| ApiError::new("NODE_GIT_IMPORT_INTERRUPTED", "项目发布中断。"))??;
        let (expected_root, identity) = self.registration_source(j)?;
        if root != expected_root || !identity.matches_path(&root).unwrap_or(false) {
            return Err(invalid("所选文件夹已变化，请重新选择并检查。"));
        }
        let bytes = read_manifest(&root)?;
        let manifest = PandaManifest::parse(&bytes).map_err(|error| invalid(&error.to_string()))?;
        manifest
            .inspect_local(&root)
            .map_err(|error| invalid(&error.to_string()))?;
        if manifest.agent.is_none() {
            return Err(invalid(
                "所选文件夹只用于推荐的 agent 应用，下载内容缺少有效 agent 声明。",
            ));
        }
        j.source_path = Some(root);
        j.source_identity = Some(identity);
        j.manifest_digest = Some(format!("{:x}", Sha256::digest(bytes)));
        self.save(j)
    }
}
