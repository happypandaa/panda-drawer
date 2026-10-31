//! In-memory cleanup plans retain the original Mac confirmation and lifetime policy.
use super::*;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Candidate {
    pub(super) id: String,
    pub(super) kind: String,
    pub(super) label: String,
    pub(super) path: PathBuf,
    pub(super) project_id: Option<String>,
    pub(super) name: Option<String>,
    pub(super) total_bytes: Option<u64>,
    pub(super) file_count: Option<u64>,
    pub(super) can_clean: bool,
    pub(super) blocked_reasons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) root_identity: Option<Identity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) parents: Vec<(PathBuf, Identity)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) entries: Vec<Entry>,
    #[serde(default)]
    pub(super) guards: BTreeMap<PathBuf, String>,
}
impl Candidate {
    pub(super) fn new(id: String, kind: &str, label: &str, path: PathBuf) -> Self {
        Self {
            id,
            kind: kind.into(),
            label: label.into(),
            path,
            project_id: None,
            name: None,
            total_bytes: None,
            file_count: None,
            can_clean: false,
            blocked_reasons: vec![],
            root_identity: None,
            parents: vec![],
            entries: vec![],
            guards: BTreeMap::new(),
        }
    }
    pub(super) fn public(&self) -> Value {
        json!({"id":self.id,"kind":self.kind,"label":self.label,"path":self.path,
            "projectId":self.project_id,"name":self.name,"totalBytes":wire_u64(self.total_bytes),
            "fileCount":wire_u64(self.file_count),"canClean":self.can_clean,"blockedReasons":self.blocked_reasons})
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Plan {
    pub(super) schema_version: u32,
    pub(super) plan_id: String,
    pub(super) instance_id: String,
    pub(super) workspace_root: PathBuf,
    pub(super) workspace_identity: Identity,
    pub(super) expires_at: u64,
    pub(super) items: Vec<Candidate>,
    pub(super) task_id: Option<String>,
    pub(super) selected_ids: Vec<String>,
    pub(super) preview_task_id: String,
}
#[derive(Deserialize)]
pub(super) struct DerivedCleanupResult {
    pub(super) items: Vec<DerivedCleanupItem>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DerivedCleanupItem {
    pub(super) id: String,
    pub(super) project_id: Option<String>,
    pub(super) path: PathBuf,
}
pub(super) fn deleted_scope_matches(
    owner: Option<&str>,
    candidate: &str,
    path: &Path,
    id: &str,
    source_ids: &[String],
    roots: &[PathBuf],
) -> bool {
    if let Some(owner) = owner {
        return owner == id;
    }
    source_ids.iter().any(|source| source == candidate)
        || roots
            .iter()
            .any(|root| path.starts_with(root) || root.starts_with(path))
}
impl Plan {
    pub(super) fn public(&self) -> Value {
        json!({"planId":self.plan_id,"expiresAt":self.expires_at,"workspaceRoot":self.workspace_root,
            "confirmationText":CONFIRMATION,"items":self.items.iter().map(Candidate::public).collect::<Vec<_>>(),
            "preserved":["当前项目及其源码、数据库、附件和发布版本","Node 项目的项目文件夹、数据和配置","未完成的网页项目取回和恢复资料以及操作日志","固定应用身份、设置、运行记录及迁移记录"],
            "warning":"永久清理不能撤销。取回前的原件清理后无法再找回那次取回之前的数据。请先停止外部 AI、编辑器或数据库工具对所选目录的访问。"})
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SubmitInput {
    pub(super) plan_id: String,
    pub(super) item_ids: Vec<String>,
    pub(super) confirmation: String,
}
