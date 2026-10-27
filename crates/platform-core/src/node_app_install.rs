//! Preparation and journaled publication of self-contained, trusted Node apps.
//!
//! The service resolves/leases source IDs;
//! NodeApps owns the per-slot lock and configuration check at publication. This
//! module never starts application code, reads saved configuration, or removes
//! old programs, data, credentials, or interrupted operation evidence.
use crate::{
    builds::{atomic_json, BuildManager},
    environment::{ExecutableInfo, Toolchain},
    node_app_manifest::{TrialManifest, TrialRuntime},
    node_development_build::NodeProcessScope,
    owned_process::{IoPlan, ProcessOptions, ProcessPhase, ProcessPipes},
    process_diagnostics::ProcessDiagnostics,
    projects::check_no_symlinks,
    ApiError,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{process::Command, sync::watch, time::Instant};
use uuid::Uuid;

type Result<T> = std::result::Result<T, ApiError>;
const MAX_METADATA: u64 = 256 * 1024;
// A journal can retain both original declaration bytes and a prepared manifest
// in target and publication records. Application manifests remain capped at 64 KiB.
const MAX_INSTALL_JOURNAL: u64 = 1024 * 1024;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_TREE: u64 = 512 * 1024 * 1024;
const MAX_FILES: usize = 50_000;
const MAX_OUTPUT: usize = 64 * 1024;
const NPM_SPACE_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy)]
struct NpmSpaceBudget {
    max_file: u64,
    max_total: u64,
    max_entries: usize,
    minimum_free: u64,
}
impl Default for NpmSpaceBudget {
    fn default() -> Self {
        Self {
            max_file: MAX_FILE,
            // Extraction and npm's private download cache coexist. The final
            // program still has its separate, smaller MAX_TREE limit.
            max_total: 2 * MAX_TREE,
            max_entries: 2 * MAX_FILES,
            minimum_free: 64 * 1024 * 1024,
        }
    }
}

fn failure(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}
fn invalid(message: &str) -> ApiError {
    failure("NODE_INSTALL_INVALID", message)
}
fn changed() -> ApiError {
    failure(
        "NODE_INSTALL_CHANGED",
        "安装材料或应用状态已变化，请重新准备。",
    )
}

/// Only the service may construct this after resolving a ready source ID,
/// checking its recorded content digest, and retaining the workspace/source
/// lease. It must never be deserialized from a native or HTTP request.
#[derive(Clone)]
pub(crate) struct VerifiedNodeSource {
    source_id: String,
    root: PathBuf,
    content_digest: String,
    commit: Option<String>,
    build_source_digest: Option<String>,
    revalidate: Option<Arc<dyn Fn() -> Result<()> + Send + Sync>>,
}
impl VerifiedNodeSource {
    pub(crate) fn new(
        source_id: String,
        root: PathBuf,
        content_digest: String,
        commit: Option<String>,
    ) -> Result<Self> {
        if !identifier(&source_id, "src_")
            || !hex(&content_digest, 64)
            || commit
                .as_deref()
                .is_some_and(|v| !hex(v, 40) && !hex(v, 64))
        {
            return Err(invalid("受管源码身份无效。"));
        }
        ordinary_directory(&root)?;
        Ok(Self {
            source_id,
            root,
            content_digest,
            commit,
            build_source_digest: None,
            revalidate: None,
        })
    }
    pub(crate) fn with_build_source_digest(mut self, digest: Option<String>) -> Result<Self> {
        if digest.as_deref().is_some_and(|value| !hex(value, 64)) {
            return Err(invalid("构建源码摘要无效。"));
        }
        self.build_source_digest = digest;
        Ok(self)
    }
    pub(crate) fn with_revalidation(
        mut self,
        check: Arc<dyn Fn() -> Result<()> + Send + Sync>,
    ) -> Self {
        self.revalidate = Some(check);
        self
    }
}

/// A public-metadata snapshot, never configuration values or secret bindings.
/// New slots have no registration/manifest and configuration revision zero.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InstallTargetSnapshot {
    pub app_id: String,
    pub registration_digest: Option<String>,
    pub generation: Option<String>,
    pub manifest: Option<Value>,
    pub configuration_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_registration: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub development_source: Option<crate::node_apps::development::NodeDevelopmentBinding>,
}
impl InstallTargetSnapshot {
    pub(crate) fn fresh() -> Self {
        Self {
            app_id: Uuid::new_v4().to_string(),
            registration_digest: None,
            generation: None,
            manifest: None,
            configuration_revision: 0,
            external_registration: None,
            development_source: None,
        }
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if Uuid::parse_str(&self.app_id).is_err()
            || self
                .registration_digest
                .as_deref()
                .is_some_and(|s| !hex(s, 64))
            || self
                .generation
                .as_deref()
                .is_some_and(|s| !identifier(s, "gen_"))
            || (self.registration_digest.is_none()
                && (self.generation.is_some()
                    || self.external_registration.is_some()
                    || self.development_source.is_some()))
            || (self.registration_digest.is_some()
                && (self.generation.is_some() == self.external_registration.is_some()))
            || self.registration_digest.is_some() != self.manifest.is_some()
            || (self.registration_digest.is_none() && self.configuration_revision != 0)
        {
            return Err(invalid("安装目标身份无效。"));
        }
        if let Some(binding) = &self.development_source {
            binding.validate_record()?;
            if self
                .manifest
                .as_ref()
                .is_none_or(|manifest| manifest["app"]["id"] != binding.manifest_id)
            {
                return Err(invalid("开发来源与安装目标不一致。"));
            }
        }
        if let Some(previous) = &self.external_registration {
            if previous["version"] != 1
                || previous["id"] != self.app_id
                || !previous["sourcePath"].is_string()
                || !previous["manifestDigest"].is_string()
                || self.development_source.as_ref().is_none_or(|binding| {
                    previous["sourcePath"]
                        != serde_json::to_value(&binding.source_root).unwrap_or(Value::Null)
                        || previous["manifestDigest"] != binding.registered_manifest_digest
                        || binding.registered_manifest().ok().as_ref() != self.manifest.as_ref()
                })
            {
                return Err(invalid("外部应用来源记录无效。"));
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct InstallControl {
    stop: watch::Receiver<bool>,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
    npm_space: NpmSpaceBudget,
}
impl InstallControl {
    pub(crate) fn new(stop: watch::Receiver<bool>, cancel: watch::Receiver<bool>) -> Self {
        Self {
            stop,
            cancel,
            deadline: Instant::now() + Duration::from_secs(600),
            npm_space: NpmSpaceBudget::default(),
        }
    }
    pub(crate) fn check(&self) -> Result<()> {
        if *self.stop.borrow() || self.stop.has_changed().is_err() {
            return Err(failure(
                "NODE_INSTALL_INTERRUPTED",
                "平台正在退出，安装准备已中断，现场保留。",
            ));
        }
        if *self.cancel.borrow() {
            return Err(failure(
                "NODE_INSTALL_CANCELLED",
                "安装准备已取消，当前程序及数据未改变。",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(failure(
                "NODE_INSTALL_TIMEOUT",
                "安装准备超过 10 分钟预算，现场保留。",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NodeInstallProvenance {
    pub source_id: String,
    pub source_digest: String,
    pub source_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_source_digest: Option<String>,
    pub node: ExecutableInfo,
    pub npm: ExecutableInfo,
    pub prepared_at: u64,
}

/// NodeApps must add a separate managed-v2 reader before this is published.
/// No absolute source/program path is persisted: generation resolves in this
/// stable UUID slot, independently of the source workspace location.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedNodeRegistration {
    pub version: u32,
    pub id: String,
    pub kind: String,
    pub generation: String,
    pub manifest_digest: String,
    pub program_digest: String,
    pub provenance: NodeInstallProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub development_source: Option<crate::node_apps::development::NodeDevelopmentBinding>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PreparedNodeInstall {
    pub candidate_id: String,
    pub app_id: String,
    pub generation: String,
    pub manifest: Value,
    pub program_digest: String,
    pub provenance: NodeInstallProvenance,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InstalledNodeGeneration {
    pub candidate_id: String,
    pub app_id: String,
    pub generation: String,
    pub operation_outcome: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    version: u32,
    candidate_id: String,
    prepare_task_id: String,
    // Bind failed preparation material to its exact source too; prepared
    // provenance is not available when dependency preparation fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_id: Option<String>,
    apply_task_id: Option<String>,
    state: String,
    target: InstallTargetSnapshot,
    prepared: Option<PreparedNodeInstall>,
    registration: Option<ManagedNodeRegistration>,
    error: Option<ApiError>,
}

/// One retained `.installations/install_*` directory as seen by history cleanup.
pub(crate) struct InstallationState {
    pub candidate_id: String,
    pub path: PathBuf,
    pub state: Option<String>,
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub source_id: Option<String>,
}
impl InstallationState {
    /// Only these states can never be continued or applied again.
    pub(crate) fn is_terminal(&self) -> bool {
        matches!(
            self.state.as_deref(),
            Some("committed" | "failed" | "interrupted")
        )
    }
}

#[derive(Clone)]
pub(crate) struct NodeAppInstaller {
    root: PathBuf,
}
impl NodeAppInstaller {
    pub(crate) fn new(data_root: PathBuf) -> Result<Self> {
        ordinary_directory(&data_root)?;
        Ok(Self {
            root: data_root.join("node-apps"),
        })
    }
    fn operations(&self) -> PathBuf {
        self.root.join(".installations")
    }
    /// Read-only states of retained installation directories for history
    /// cleanup. An unreadable journal reports `state: None` and stays protected.
    pub(crate) fn installation_states(&self) -> Result<Vec<InstallationState>> {
        let root = self.operations();
        check_no_symlinks(&root, true)?;
        if !root.try_exists()? {
            return Ok(vec![]);
        }
        let mut states = vec![];
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !identifier(&id, "install_") || !entry.file_type()?.is_dir() {
                continue;
            }
            let journal = self.journal(&id).ok();
            states.push(InstallationState {
                path: entry.path(),
                state: journal.as_ref().map(|value| value.state.clone()),
                app_id: journal.as_ref().map(|value| value.target.app_id.clone()),
                app_name: journal.as_ref().and_then(|value| {
                    value
                        .prepared
                        .as_ref()
                        .map(|prepared| &prepared.manifest)
                        .or(value.target.manifest.as_ref())
                        .and_then(|manifest| manifest["name"].as_str())
                        .map(str::to_owned)
                }),
                source_id: journal.as_ref().and_then(|value| {
                    value.source_id.clone().or_else(|| {
                        value
                            .prepared
                            .as_ref()
                            .map(|prepared| prepared.provenance.source_id.clone())
                    })
                }),
                candidate_id: id,
            });
        }
        states.sort_by(|a, b| a.candidate_id.cmp(&b.candidate_id));
        Ok(states)
    }
    /// The same exclusive lock a commit holds; history cleanup takes it while
    /// removing installation material so no commit can race the removal.
    pub(crate) fn commit_lock(&self) -> Result<File> {
        self.lock()
    }
    fn operation(&self, id: &str) -> Result<PathBuf> {
        if !identifier(id, "install_") {
            return Err(invalid("安装候选标识无效。"));
        }
        let root = self.operations().join(id);
        ordinary_directory(&root)?;
        Ok(root)
    }
    fn slot(&self, id: &str) -> Result<PathBuf> {
        if Uuid::parse_str(id).is_err() {
            return Err(invalid("应用实例标识无效。"));
        }
        let path = self.root.join(id);
        check_no_symlinks(&path, true)?;
        Ok(path)
    }
    fn journal(&self, id: &str) -> Result<Journal> {
        let journal: Journal = serde_json::from_slice(&read_file(
            &self.operation(id)?.join("install.json"),
            MAX_INSTALL_JOURNAL,
        )?)?;
        if journal.version != 1
            || journal.candidate_id != id
            || !identifier(&journal.prepare_task_id, "task_")
            || journal
                .source_id
                .as_deref()
                .is_some_and(|id| !identifier(id, "src_"))
            || journal
                .apply_task_id
                .as_deref()
                .is_some_and(|s| !identifier(s, "task_"))
            || !matches!(
                journal.state.as_str(),
                "preparing"
                    | "prepared"
                    | "committing"
                    | "committed"
                    | "failed"
                    | "interrupted"
                    | "needs-attention"
            )
        {
            return Err(invalid("安装记录身份无效，现场保留。"));
        }
        journal.target.validate()?;
        if let Some(prepared) = &journal.prepared {
            if prepared.candidate_id != id
                || journal
                    .source_id
                    .as_deref()
                    .is_some_and(|id| id != prepared.provenance.source_id)
                || prepared.app_id != journal.target.app_id
                || !identifier(&prepared.generation, "gen_")
                || !hex(&prepared.program_digest, 64)
                || prepared
                    .provenance
                    .build_source_digest
                    .as_deref()
                    .is_some_and(|value| !hex(value, 64))
            {
                return Err(invalid("安装产物记录身份无效。"));
            }
        }
        if matches!(
            journal.state.as_str(),
            "prepared" | "committing" | "committed"
        ) && journal.prepared.is_none()
        {
            return Err(invalid("安装状态缺少所需的产物记录。"));
        }
        if matches!(journal.state.as_str(), "committing" | "committed")
            && journal.registration.is_none()
        {
            return Err(invalid("安装提交缺少登记记录。"));
        }
        if let Some(registration) = &journal.registration {
            let prepared = journal
                .prepared
                .as_ref()
                .ok_or_else(|| invalid("安装登记没有对应产物。"))?;
            if registration.version != 2
                || registration.kind != "managed"
                || registration.id != journal.target.app_id
                || registration.generation != prepared.generation
                || registration.program_digest != prepared.program_digest
                || !hex(&registration.manifest_digest, 64)
                || registration.provenance.source_id != prepared.provenance.source_id
                || registration.provenance.source_digest != prepared.provenance.source_digest
                || registration.provenance.source_commit != prepared.provenance.source_commit
                || registration.provenance.build_source_digest
                    != prepared.provenance.build_source_digest
                || registration.development_source != journal.target.development_source
            {
                return Err(invalid("安装登记与候选产物身份不一致。"));
            }
        }
        Ok(journal)
    }
    fn save(&self, journal: &Journal) -> Result<()> {
        if serde_json::to_vec_pretty(journal)?.len() as u64 > MAX_INSTALL_JOURNAL {
            return Err(invalid("安装记录超出大小限制，现场保留。"));
        }
        atomic_json(
            &self.operation(&journal.candidate_id)?.join("install.json"),
            journal,
        )
    }

    /// Caller creates a persistent nodeInstallPrepare task and owns its source
    /// lease/cancellation runner. Shutdown must drain this future before tasks.
    #[cfg(test)]
    pub(crate) async fn prepare(
        &self,
        task_id: &str,
        source: VerifiedNodeSource,
        target: InstallTargetSnapshot,
        tools: Toolchain,
        control: InstallControl,
        tasks: &Arc<BuildManager>,
    ) -> Result<PreparedNodeInstall> {
        let processes = NodeProcessScope::new(
            crate::owned_process::OwnedProcessRegistry::default(),
            tasks.clone(),
        );
        self.prepare_with_scope(task_id, source, target, tools, control, tasks, processes)
            .await
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn prepare_with_scope(
        &self,
        task_id: &str,
        source: VerifiedNodeSource,
        target: InstallTargetSnapshot,
        tools: Toolchain,
        control: InstallControl,
        tasks: &Arc<BuildManager>,
        processes: NodeProcessScope,
    ) -> Result<PreparedNodeInstall> {
        require_task(tasks, task_id, "nodeInstallPrepare")?;
        let result = self
            .prepare_task(task_id, source, target, tools, control, tasks, processes)
            .await;
        if let Err(error) = &result {
            if tasks
                .task(task_id)
                .is_ok_and(|task| !task.status.is_final())
            {
                let _ = tasks.finish_partial(
                    task_id,
                    json!({"installed":false,"partialFilesRetained":true}),
                    error.clone(),
                );
            }
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn prepare_task(
        &self,
        task_id: &str,
        source: VerifiedNodeSource,
        target: InstallTargetSnapshot,
        tools: Toolchain,
        control: InstallControl,
        tasks: &Arc<BuildManager>,
        processes: NodeProcessScope,
    ) -> Result<PreparedNodeInstall> {
        target.validate()?;
        control.check()?;
        private_dir(&self.root)?;
        private_dir(&self.operations())?;
        let candidate_id = format!("install_{}", Uuid::new_v4().simple());
        let operation = self.operations().join(&candidate_id);
        fs::create_dir(&operation)?;
        fs::set_permissions(&operation, fs::Permissions::from_mode(0o700))?;
        let processes = processes.with_operation(&operation)?;
        let mut journal = Journal {
            version: 1,
            candidate_id: candidate_id.clone(),
            prepare_task_id: task_id.into(),
            source_id: Some(source.source_id.clone()),
            apply_task_id: None,
            state: "preparing".into(),
            target,
            prepared: None,
            registration: None,
            error: None,
        };
        self.save(&journal)?;
        let result = self
            .prepare_inner(&journal, &source, &tools, &control, tasks, processes)
            .await
            .and_then(|prepared| {
                control.check()?;
                Ok(prepared)
            });
        match result {
            Ok(prepared) => {
                journal.prepared = Some(prepared.clone());
                journal.state = "prepared".into();
                self.save(&journal)?;
                tasks.finish(
                    task_id,
                    Ok(json!({"state":"prepared", "candidate":prepared, "installed":false})),
                )?;
                Ok(prepared)
            }
            Err(error) => {
                journal.state = if error.code == "NODE_INSTALL_INTERRUPTED" {
                    "interrupted"
                } else {
                    "failed"
                }
                .into();
                journal.error = Some(error.clone());
                let _ = self.save(&journal);
                let _ = tasks.finish_partial(task_id, json!({"candidateId":candidate_id,"installed":false,"partialFilesRetained":true}), error.clone());
                Err(error)
            }
        }
    }

    async fn prepare_inner(
        &self,
        journal: &Journal,
        source: &VerifiedNodeSource,
        tools: &Toolchain,
        control: &InstallControl,
        tasks: &Arc<BuildManager>,
        processes: NodeProcessScope,
    ) -> Result<PreparedNodeInstall> {
        tasks.phase(&journal.prepare_task_id, "copying")?;
        let operation = self.operation(&journal.candidate_id)?;
        let program = operation.join("program");
        let from = source.root.clone();
        let to = program.clone();
        let checks = control.clone();
        let revalidate = source.revalidate.clone();
        let source_baseline = tokio::task::spawn_blocking(move || {
            if let Some(check) = &revalidate {
                check()?;
            }
            let before = source_digest(&from, &checks)?;
            copy_source(&from, &to, Path::new(""), &checks)?;
            if source_digest(&from, &checks)? != before || source_digest(&to, &checks)? != before {
                return Err(changed());
            }
            if let Some(check) = &revalidate {
                check()?;
            }
            Ok(before)
        })
        .await
        .map_err(|_| invalid("安装文件复制中断，现场保留。"))??;
        let manifest = load_manifest(&program)?;
        let manifest_value: Value =
            serde_json::from_slice(&read_small(&program.join("local-app.json"))?)?;
        compatible(&journal.target, &manifest_value)?;
        let npm_root = npm_root(&program, &manifest)?;
        validate_npm_input(&npm_root)?;
        tasks.phase(&journal.prepare_task_id, "dependencies")?;
        npm_ci(&operation, &npm_root, tools, control, processes).await?;
        control.check()?;
        let package: Value = serde_json::from_slice(&read_small(&npm_root.join("package.json"))?)?;
        if package["dependencies"]
            .as_object()
            .is_some_and(|deps| !deps.is_empty())
        {
            let _: Value = serde_json::from_slice(&read_small(
                &npm_root.join("node_modules/.package-lock.json"),
            )?)?;
        }
        tasks.phase(&journal.prepare_task_id, "validating")?;
        let validation_root = program.clone();
        let original_root = source.root.clone();
        let checks = control.clone();
        let revalidate = source.revalidate.clone();
        let program_digest = tokio::task::spawn_blocking(move || {
            if let Some(check) = &revalidate {
                check()?;
            }
            // Neither original sources nor copied program inputs may silently
            // change during npm; installed node_modules is excluded here.
            if source_digest(&original_root, &checks)? != source_baseline
                || source_digest(&validation_root, &checks)? != source_baseline
            {
                return Err(changed());
            }
            load_manifest(&validation_root)?;
            let digest = program_digest(&validation_root, &checks)?;
            sync_program(&validation_root, &checks)?;
            checks.check()?;
            Ok(digest)
        })
        .await
        .map_err(|_| invalid("安装产物检查中断，现场保留。"))??;
        Ok(PreparedNodeInstall {
            candidate_id: journal.candidate_id.clone(),
            app_id: journal.target.app_id.clone(),
            generation: format!("gen_{}", Uuid::new_v4().simple()),
            manifest: manifest_value,
            program_digest,
            provenance: NodeInstallProvenance {
                source_id: source.source_id.clone(),
                source_digest: source.content_digest.clone(),
                source_commit: source.commit.clone(),
                build_source_digest: source.build_source_digest.clone(),
                node: tools.node.clone(),
                npm: tools.npm.clone(),
                prepared_at: now(),
            },
        })
    }

    pub(crate) fn prepared(&self, candidate_id: &str) -> Result<PreparedNodeInstall> {
        let journal = self.journal(candidate_id)?;
        if journal.state != "prepared" {
            return Err(invalid("安装候选尚未准备完成。"));
        }
        let prepared = journal
            .prepared
            .ok_or_else(|| invalid("安装候选缺少产物记录。"))?;
        // Retained preparation records must satisfy today's update policy.
        // Historical committed records remain readable through committed().
        compatible(&journal.target, &prepared.manifest)?;
        Ok(prepared)
    }

    pub(crate) fn target(&self, candidate_id: &str) -> Result<InstallTargetSnapshot> {
        let journal = self.journal(candidate_id)?;
        if journal.state != "prepared" {
            return Err(invalid("安装候选尚未准备完成或已提交，请刷新任务结果。"));
        }
        Ok(journal.target)
    }

    pub(crate) fn committed(&self, candidate_id: &str) -> Result<Option<InstalledNodeGeneration>> {
        let journal = self.journal(candidate_id)?;
        if journal.state == "committed" {
            installed(&journal).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Read only operation metadata. Do not return the old target snapshot or
    /// read an installed slot's configuration/credentials while restoring UI.
    pub(crate) fn list(&self) -> Result<Vec<Value>> {
        let root = self.operations();
        check_no_symlinks(&root, true)?;
        if !root.try_exists()? {
            return Ok(Vec::new());
        }
        ordinary_directory(&root)?;
        let mut result = Vec::new();
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !identifier(&id, "install_") {
                continue;
            }
            result.push(match self.journal(&id) {
                Ok(journal) => json!({"candidateId":id,"appId":journal.target.app_id,
                    "targetAppId":journal.target.registration_digest.is_some().then_some(&journal.target.app_id),
                    "state":journal.state,"prepareTaskId":journal.prepare_task_id,"applyTaskId":journal.apply_task_id,
                    "prepared":journal.prepared,"error":journal.error,"issues":[]}),
                Err(_) => json!({"candidateId":id,"appId":null,"targetAppId":null,"state":"unavailable", "prepareTaskId":null,"applyTaskId":null,"prepared":null,"error":null,
                    "issues":[failure("NODE_INSTALL_RECORD_UNAVAILABLE", "安装候选记录无法读取或核验，现场保留；其他候选仍可查看。")]}),
            });
        }
        result.sort_by(|a, b| a["candidateId"].as_str().cmp(&b["candidateId"].as_str()));
        Ok(result)
    }

    /// MUST be called synchronously while NodeApps holds its slot operation lock
    /// and after native code verifies there is no window/session. NodeApps must
    /// reject shutting_down, running/cleanup handles, active.json, and incompatible
    /// configuration with view(new_manifest), without launch_snapshot or save.
    /// The fs lock below also prevents two installer commits/recovery crossing.
    pub(crate) fn commit_stopped(
        &self,
        candidate_id: &str,
        fresh: &InstallTargetSnapshot,
        task_id: &str,
        control: &InstallControl,
        tasks: &BuildManager,
    ) -> Result<InstalledNodeGeneration> {
        require_task(tasks, task_id, "nodeInstallApply")?;
        let result = self.commit_task(candidate_id, fresh, task_id, control, tasks);
        if let Err(error) = &result {
            if tasks
                .task(task_id)
                .is_ok_and(|task| !task.status.is_final())
            {
                let _ = tasks.finish_partial(
                    task_id,
                    json!({"candidateId":candidate_id,"partialFilesRetained":true}),
                    error.clone(),
                );
            }
        }
        result
    }

    fn commit_task(
        &self,
        candidate_id: &str,
        fresh: &InstallTargetSnapshot,
        task_id: &str,
        control: &InstallControl,
        tasks: &BuildManager,
    ) -> Result<InstalledNodeGeneration> {
        let _lock = self.lock()?;
        let mut journal = self.journal(candidate_id)?;
        if journal.state == "committed" {
            let result = installed(&journal)?;
            tasks.finish(task_id, Ok(json!(result)))?;
            return Ok(result);
        }
        if journal.state != "prepared" || journal.target != *fresh {
            return Err(changed());
        }
        control.check()?;
        self.check_target(fresh)?;
        if fresh.registration_digest.is_none() && self.slot(&fresh.app_id)?.try_exists()? {
            return Err(invalid(
                "新安装不能复用已有未登记目录，请保留现场并重新准备。",
            ));
        }
        let prepared = journal
            .prepared
            .as_ref()
            .ok_or_else(|| invalid("安装候选缺少产物。"))?;
        let program = self.operation(candidate_id)?.join("program");
        if program_digest(&program, control)? != prepared.program_digest {
            return Err(changed());
        }
        sync_program(&program, control)?;
        load_manifest(&program)?;
        let manifest_value: Value =
            serde_json::from_slice(&read_small(&program.join("local-app.json"))?)?;
        if manifest_value != prepared.manifest {
            return Err(changed());
        }
        compatible(fresh, &manifest_value)?;
        journal.registration = Some(ManagedNodeRegistration {
            version: 2,
            id: prepared.app_id.clone(),
            kind: "managed".into(),
            generation: prepared.generation.clone(),
            manifest_digest: digest_bytes(&read_small(&program.join("local-app.json"))?),
            program_digest: prepared.program_digest.clone(),
            provenance: prepared.provenance.clone(),
            development_source: fresh.development_source.clone(),
        });
        journal.apply_task_id = Some(task_id.into());
        tasks.phase(task_id, "committing")?;
        // Last cancellation boundary. Once the commit intent is durable, finish
        // or reconcile the transaction; cancellation cannot undo a publication.
        control.check()?;
        let outcome = self.commit_files(&mut journal, |_| Ok(()));
        let committed = match self.pointer_matches(&journal) {
            Ok(committed) => committed,
            Err(error) => {
                let error = error.outcome("unknown");
                journal.state = "needs-attention".into();
                journal.error = Some(error.clone());
                let _ = self.save(&journal);
                let _ = tasks.finish_partial(task_id, json!({"candidateId":candidate_id,"operationOutcome":"unknown","partialFilesRetained":true}), error.clone());
                return Err(error);
            }
        };
        if committed {
            journal.state = "committed".into();
            let result = installed(&journal)?;
            let finalization = outcome
                .and_then(|_| self.save(&journal))
                .and_then(|_| tasks.finish(task_id, Ok(json!(result))));
            if let Err(error) = finalization {
                let error = error.outcome("committed");
                let _ = tasks.finish_partial(task_id, json!(result), error.clone());
                return Err(error);
            }
            return Ok(result);
        }
        let known_uncommitted = registration_digest(&self.slot(&journal.target.app_id)?)
            .is_ok_and(|actual| actual == journal.target.registration_digest);
        let operation_outcome = if known_uncommitted {
            "not-committed"
        } else {
            "unknown"
        };
        let error = outcome
            .err()
            .unwrap_or_else(|| invalid("安装提交结果需要核对，现场保留。"))
            .outcome(operation_outcome);
        journal.state = if known_uncommitted {
            "committing"
        } else {
            "needs-attention"
        }
        .into();
        journal.error = Some(error.clone());
        // Keep committing evidence: recovery decides from the actual pointer.
        let _ = self.save(&journal);
        let _ = tasks.finish_partial(task_id, json!({"candidateId":candidate_id,"operationOutcome":operation_outcome,"partialFilesRetained":true}), error.clone());
        Err(error)
    }

    fn check_target(&self, target: &InstallTargetSnapshot) -> Result<()> {
        target.validate()?;
        let slot = self.slot(&target.app_id)?;
        if slot.join("active.json").try_exists()?
            || crate::node_apps::has_start_intent_cleanup(&slot)?
        {
            return Err(failure(
                "NODE_INSTALL_BUSY",
                "请先正常关闭应用；运行记录未完成收尾时不能更新。",
            ));
        }
        if registration_digest(&slot)? != target.registration_digest {
            return Err(changed());
        }
        if let Some(previous) = &target.external_registration {
            let actual: Value =
                serde_json::from_slice(&read_small(&slot.join("registration.json"))?)?;
            if actual != *previous {
                return Err(changed());
            }
            let binding = target.development_source.as_ref().ok_or_else(changed)?;
            binding.source(&target.app_id, true).validate()?;
            if previous["sourcePath"] != serde_json::to_value(&binding.source_root)?
                || previous["manifestDigest"] != binding.registered_manifest_digest
            {
                return Err(changed());
            }
        }
        if let Some(expected) = &target.generation {
            let record: ManagedNodeRegistration =
                serde_json::from_slice(&read_small(&slot.join("registration.json"))?)?;
            if record.version != 2
                || record.kind != "managed"
                || record.id != target.app_id
                || record.generation != *expected
            {
                return Err(changed());
            }
        }
        Ok(())
    }

    fn commit_files(
        &self,
        journal: &mut Journal,
        mut checkpoint: impl FnMut(CommitPoint) -> Result<()>,
    ) -> Result<()> {
        let prepared = journal
            .prepared
            .as_ref()
            .ok_or_else(|| invalid("没有准备好的安装产物。"))?;
        let slot = self.slot(&prepared.app_id)?;
        if journal.target.registration_digest.is_some() {
            ordinary_directory(&slot)?;
        } else {
            private_dir(&slot)?;
        }
        for name in ["configuration", "data", "cache", "runs", "generations"] {
            let path = slot.join(name);
            if journal.target.registration_digest.is_some() && name != "generations" {
                ordinary_directory(&path)?;
            } else {
                private_dir(&path)?;
            }
        }
        let generation = slot.join("generations").join(&prepared.generation);
        if generation.try_exists()? {
            return Err(invalid("安装代次已存在，不能覆盖。"));
        }
        journal.state = "committing".into();
        self.save(journal)?;
        checkpoint(CommitPoint::BeforeGeneration)?;
        fs::rename(
            self.operation(&journal.candidate_id)?.join("program"),
            &generation,
        )?;
        File::open(generation.parent().unwrap())?.sync_all()?;
        File::open(self.operation(&journal.candidate_id)?)?.sync_all()?;
        checkpoint(CommitPoint::BeforePointer)?;
        self.check_target(&journal.target)?;
        if let Some(expected_generation) = &journal.target.generation {
            let previous_bytes = read_small(&slot.join("registration.json"))?;
            if Some(digest_bytes(&previous_bytes)) != journal.target.registration_digest {
                return Err(changed());
            }
            let previous: ManagedNodeRegistration = serde_json::from_slice(&previous_bytes)?;
            if previous.id != prepared.app_id || previous.generation != *expected_generation {
                return Err(changed());
            }
        }
        atomic_json(
            &slot.join("registration.json"),
            journal
                .registration
                .as_ref()
                .ok_or_else(|| invalid("安装登记缺失。"))?,
        )?;
        checkpoint(CommitPoint::AfterPointer)?;
        Ok(())
    }

    fn pointer_matches(&self, journal: &Journal) -> Result<bool> {
        let Some(expected) = &journal.registration else {
            return Ok(false);
        };
        let slot = self.slot(&journal.target.app_id)?;
        let Some(actual) = registration_digest(&slot)? else {
            return Ok(false);
        };
        Ok(actual == digest_bytes(&serde_json::to_vec_pretty(expected)?))
    }
    fn lock(&self) -> Result<File> {
        private_dir(&self.root)?;
        private_dir(&self.operations())?;
        let path = self.operations().join("commit.lock");
        check_no_symlinks(&path, true)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        if file.metadata()?.nlink() != 1 {
            return Err(invalid("安装锁文件无效。"));
        }
        file.try_lock_exclusive()
            .map_err(|_| failure("NODE_INSTALL_BUSY", "另一个安装正在提交，请稍后刷新。"))?;
        Ok(file)
    }

    /// Run before exposing Node applications, while no installation can submit.
    /// Never deletes candidates or guesses rollback from task status alone.
    pub(crate) fn recover(&self, tasks: &BuildManager) -> Result<Vec<Value>> {
        if !self.operations().try_exists()? {
            return Ok(vec![]);
        }
        let _lock = self.lock()?;
        let mut results = vec![];
        for entry in fs::read_dir(self.operations())? {
            let entry = entry?;
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !identifier(&id, "install_") {
                continue;
            }
            let mut journal = match self.journal(&id) {
                Ok(value) => value,
                Err(_) => {
                    results.push(json!({"candidateId":id,"state":"needs-attention"}));
                    continue;
                }
            };
            if journal.state == "preparing" {
                journal.state = "interrupted".into();
                self.save(&journal)?;
            } else if journal.state == "committing" {
                journal.state = match self.pointer_matches(&journal) {
                    Ok(true) => "committed",
                    Ok(false)
                        if registration_digest(&self.slot(&journal.target.app_id)?)
                            .is_ok_and(|actual| actual == journal.target.registration_digest) =>
                    {
                        "interrupted"
                    }
                    _ => "needs-attention",
                }
                .into();
                self.save(&journal)?;
            }
            let outcome = match journal.state.as_str() {
                "committed" => "committed",
                "prepared" => "prepared",
                "failed" => "not-committed",
                "needs-attention" => "unknown",
                _ => "interrupted",
            };
            let result = json!({"candidateId":id,"appId":journal.target.app_id,"state":journal.state,"operationOutcome":outcome,"partialFilesRetained":true,"recovered":true});
            let task_id = journal
                .apply_task_id
                .as_deref()
                .unwrap_or(&journal.prepare_task_id);
            if tasks.task(task_id).is_ok() {
                tasks.reconcile_operation_result(task_id, result.clone())?;
            }
            results.push(result);
        }
        Ok(results)
    }
}

#[derive(Clone, Copy)]
enum CommitPoint {
    BeforeGeneration,
    BeforePointer,
    AfterPointer,
}

fn installed(journal: &Journal) -> Result<InstalledNodeGeneration> {
    let prepared = journal
        .prepared
        .as_ref()
        .ok_or_else(|| invalid("安装记录缺少产物身份。"))?;
    Ok(InstalledNodeGeneration {
        candidate_id: journal.candidate_id.clone(),
        app_id: prepared.app_id.clone(),
        generation: prepared.generation.clone(),
        operation_outcome: "committed".into(),
    })
}
fn require_task(tasks: &BuildManager, id: &str, kind: &str) -> Result<()> {
    if !identifier(id, "task_") {
        return Err(invalid("安装任务标识无效。"));
    }
    let task = tasks.task(id)?;
    if task.task_type != kind || task.project_id.is_some() || task.status.is_final() {
        return Err(invalid("安装任务类型或状态不符。"));
    }
    Ok(())
}
pub(crate) fn compatible(target: &InstallTargetSnapshot, manifest: &Value) -> Result<()> {
    use crate::node_app_manifest::DataContractDecl;
    let candidate_data = DataContractDecl::from_manifest(manifest)
        .map_err(|_| invalid("候选的数据契约声明无效。"))?;
    if let Some(previous) = &target.manifest {
        if previous["app"]["id"] != manifest["app"]["id"] {
            return Err(failure(
                "NODE_INSTALL_APP_MISMATCH",
                "候选声明不是当前应用，不能作为普通更新。",
            ));
        }
        if previous.get("configuration") != manifest.get("configuration") {
            return Err(failure(
                "NODE_INSTALL_CONFIGURATION_INCOMPATIBLE",
                "本次仅支持配置声明不变的程序更新；原配置和当前程序保留。",
            ));
        }
        let previous_data = DataContractDecl::from_manifest(previous)
            .map_err(|_| invalid("当前应用的数据契约声明无效，原程序和数据保留。"))?;
        let error = if previous_data.is_none() || candidate_data.is_none() {
            Some(failure(
                "NODE_INSTALL_DATA_COMPATIBILITY_UNKNOWN",
                "当前应用或候选未声明数据契约，无法核对普通更新；原程序、配置和数据保留。",
            ))
        } else if previous_data != candidate_data {
            Some(failure(
                "NODE_INSTALL_DATA_INCOMPATIBLE",
                "候选与当前应用的数据契约标识或版本不同，不能直接更新；原程序、配置和数据保留。",
            ))
        } else {
            None
        };
        if let Some(error) = error {
            return Err(error
                .detail("previousDataContract", json!(previous_data))
                .detail("candidateDataContract", json!(candidate_data)));
        }
    }
    Ok(())
}
fn load_manifest(root: &Path) -> Result<TrialManifest> {
    let manifest = TrialManifest::load(root).map_err(|_| invalid("安装候选的应用声明无效。"))?;
    if manifest.runtime != TrialRuntime::NodeService {
        return Err(invalid("此安装入口只支持单个 Node 服务及静态网页。"));
    }
    manifest
        .inspect(root)
        .map_err(|_| invalid("安装候选缺少声明的网页、后端或说明文件。"))?;
    Ok(manifest)
}
pub(crate) fn npm_root(root: &Path, manifest: &TrialManifest) -> Result<PathBuf> {
    let backend = root.join(
        &manifest
            .backend
            .as_ref()
            .ok_or_else(|| invalid("缺少后端声明。"))?
            .root,
    );
    let candidates: Vec<_> = [root.to_path_buf(), backend]
        .into_iter()
        .filter(|p| p.join("package.json").exists() || p.join("package-lock.json").exists())
        .collect();
    if candidates.len() != 1 {
        return Err(invalid(
            "首批安装要求包根或后端根中恰有一套 package.json 与锁文件。",
        ));
    }
    Ok(candidates.into_iter().next().unwrap())
}
pub(crate) fn validate_npm_input(root: &Path) -> Result<()> {
    let package: Value = serde_json::from_slice(&read_small(&root.join("package.json"))?)?;
    let lock: Value = serde_json::from_slice(&read_small(&root.join("package-lock.json"))?)?;
    if root.join("npm-shrinkwrap.json").try_exists()?
        || !matches!(lock["lockfileVersion"].as_u64(), Some(2 | 3))
        || !lock["packages"].is_object()
        || !lock["packages"][""].is_object()
        || !package["workspaces"].is_null()
    {
        return Err(invalid(
            "首批安装要求 npm v2/v3 锁文件，不支持 workspace 或覆盖锁文件。",
        ));
    }
    for field in ["dependencies", "devDependencies", "optionalDependencies"] {
        if package.get(field).unwrap_or(&json!({}))
            != lock["packages"][""].get(field).unwrap_or(&json!({}))
        {
            return Err(invalid("package.json 与锁文件依赖不一致。"));
        }
    }
    // Use an allow-list, not scheme blacklists: npm interprets owner/repo and
    // even uppercase FILE: as Git/filesystem inputs. Aliases/overrides are a
    // separate compatibility slice and must not become a hidden escape hatch.
    fn spec(value: &Value) -> Result<()> {
        let Some(value) = value.as_str() else {
            return Err(invalid("依赖版本必须为字符串。"));
        };
        if value.is_empty()
            || value.starts_with('.')
            || value.len() > 200
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".*~^<>=| +-".contains(&b))
        {
            return Err(invalid(
                "首批安装只支持 registry 版本、范围或标签，不支持路径、Git 或别名依赖。",
            ));
        }
        Ok(())
    }
    fn dependency_map(value: &Value) -> Result<()> {
        if value.is_null() {
            return Ok(());
        }
        let dependencies = value
            .as_object()
            .ok_or_else(|| invalid("依赖声明不是对象。"))?;
        for (name, value) in dependencies {
            let parts: Vec<_> = name.split('/').collect();
            let ordinary = |part: &str| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            };
            let valid = match parts.as_slice() {
                [name] => ordinary(name),
                [scope, name] => scope.strip_prefix('@').is_some_and(ordinary) && ordinary(name),
                _ => false,
            };
            if !valid || name.len() > 214 {
                return Err(invalid("依赖名称无效。"));
            }
            spec(value)?;
        }
        Ok(())
    }
    fn package_fields(value: &Value) -> Result<()> {
        if value["link"].as_bool() == Some(true)
            || !value["workspaces"].is_null()
            || value.get("overrides").is_some_and(|v| !v.is_null())
        {
            return Err(invalid("首批安装不支持链接依赖、workspace 或 overrides。"));
        }
        for field in [
            "dependencies",
            "devDependencies",
            "optionalDependencies",
            "peerDependencies",
        ] {
            dependency_map(&value[field])?;
        }
        Ok(())
    }
    fn resolved(value: &Value) -> Result<()> {
        let Some(value) = value.as_str() else {
            return Err(invalid("依赖下载地址无效。"));
        };
        let url =
            url::Url::parse(value).map_err(|_| invalid("依赖须有明确的 HTTPS 压缩包地址。"))?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !(url.path().ends_with(".tgz") || url.path().ends_with(".tar.gz"))
        {
            return Err(invalid("首批安装仅接受无账号信息的 HTTPS 依赖压缩包。"));
        }
        Ok(())
    }
    package_fields(&package)?;
    for (location, package) in lock["packages"].as_object().unwrap() {
        if !location.is_empty()
            && (!location.starts_with("node_modules/")
                || location
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
                || location.contains(['\\', '\0', ':']))
        {
            return Err(invalid("锁文件包含非受管安装位置。"));
        }
        package_fields(package)?;
        if !location.is_empty() {
            spec(&package["version"])?;
            if let Some(value) = package.get("resolved") {
                resolved(value)?;
            } else if package["inBundle"].as_bool() != Some(true) {
                return Err(invalid("锁文件依赖缺少固定下载地址。"));
            }
        }
    }
    fn legacy(value: &Value) -> Result<()> {
        if let Some(packages) = value.as_object() {
            for package in packages.values() {
                spec(&package["version"])?;
                if let Some(value) = package.get("resolved") {
                    resolved(value)?;
                }
                dependency_map(&package["requires"])?;
                legacy(&package["dependencies"])?;
            }
        }
        Ok(())
    }
    legacy(&lock["dependencies"])
}

fn scan_npm_space(
    operation: &Path,
    control: &InstallControl,
    available: impl Fn(&Path) -> std::io::Result<u64>,
) -> Result<()> {
    fn over_budget() -> ApiError {
        failure(
            "NODE_INSTALL_SPACE_LIMIT",
            "安装准备的文件大小、数量或总占用超过预算，已停止并保留候选现场；当前程序和数据未改变。",
        )
    }
    fn free_space(
        operation: &Path,
        control: &InstallControl,
        available: &impl Fn(&Path) -> std::io::Result<u64>,
    ) -> Result<()> {
        control.check()?;
        let bytes = available(operation).map_err(|_| {
            failure(
                "NODE_INSTALL_SPACE_CHECK_FAILED",
                "无法核对安装目录所在磁盘的余量，已停止准备。",
            )
        })?;
        if bytes < control.npm_space.minimum_free {
            return Err(failure(
                "NODE_INSTALL_SPACE_INSUFFICIENT",
                "磁盘可用空间不足，已停止安装准备并保留现场；当前程序和数据未改变。",
            ));
        }
        Ok(())
    }
    fn visit(
        operation: &Path,
        path: &Path,
        depth: usize,
        control: &InstallControl,
        available: &impl Fn(&Path) -> std::io::Result<u64>,
        usage: &mut (usize, u64),
    ) -> Result<()> {
        control.check()?;
        if depth > 128 {
            return Err(over_budget());
        }
        check_no_symlinks(path, true)?;
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            // npm may rename/delete cache and extraction entries while scanned.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            control.check()?;
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let metadata = match fs::symlink_metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            usage.0 = usage.0.checked_add(1).ok_or_else(over_budget)?;
            if usage.0 > control.npm_space.max_entries {
                return Err(over_budget());
            }
            if usage.0.is_multiple_of(256) {
                free_space(operation, control, available)?;
            }
            if metadata.is_dir() {
                visit(
                    operation,
                    &entry.path(),
                    depth + 1,
                    control,
                    available,
                    usage,
                )?;
            } else if metadata.is_file() || metadata.file_type().is_symlink() {
                // Metadata only: never open file contents or follow npm's links.
                // Final program validation separately checks relative bin links
                // and rejects hard links; transient cache links are counted here.
                if metadata.is_file() && metadata.len() > control.npm_space.max_file {
                    return Err(over_budget());
                }
                usage.1 = usage
                    .1
                    .checked_add(metadata.len())
                    .ok_or_else(over_budget)?;
                if usage.1 > control.npm_space.max_total {
                    return Err(over_budget());
                }
            } else {
                return Err(invalid("安装现场出现特殊文件，已停止准备并保留现场。"));
            }
        }
        Ok(())
    }
    ordinary_directory(operation)?;
    free_space(operation, control, &available)?;
    visit(operation, operation, 0, control, &available, &mut (0, 0))?;
    free_space(operation, control, &available)
}

async fn check_npm_space(operation: &Path, control: &InstallControl) -> Result<()> {
    let operation = operation.to_path_buf();
    let control = control.clone();
    // Await the scanner rather than detaching it on cancellation. It checks the
    // same stop/deadline and cannot outlive the installation's source lease.
    tokio::task::spawn_blocking(move || {
        scan_npm_space(&operation, &control, |path| fs2::available_space(path))
    })
    .await
    .map_err(|_| {
        failure(
            "NODE_INSTALL_SPACE_CHECK_FAILED",
            "安装空间检查中断，现场保留。",
        )
    })?
}

async fn npm_ci(
    operation: &Path,
    cwd: &Path,
    tools: &Toolchain,
    control: &InstallControl,
    processes: NodeProcessScope,
) -> Result<()> {
    control.check()?;
    let home = operation.join("npm-home");
    let cache = operation.join("npm-cache");
    private_dir(&home)?;
    private_dir(&cache)?;
    let user_config = home.join("user.npmrc");
    let global_config = home.join("global.npmrc");
    write_new(&user_config, b"", false)?;
    write_new(&global_config, b"", false)?;
    check_npm_space(operation, control).await?;
    let mut command = Command::new(&tools.node.path);
    command
        .arg(&tools.npm.path)
        .arg("--prefix")
        .arg(cwd)
        .arg("--global=false")
        .arg("--logs-dir")
        .arg(operation.join("npm-disabled-output"))
        .args([
            "ci",
            "--omit=dev",
            "--ignore-scripts",
            "--logs-max=0",
            "--no-audit",
            "--no-fund",
            "--fetch-retries=0",
            "--fetch-timeout=10000",
        ])
        .current_dir(cwd)
        .env_clear()
        .env("PATH", &tools.path)
        .env("HOME", &home)
        .env("NPM_CONFIG_USERCONFIG", &user_config)
        .env("NPM_CONFIG_GLOBALCONFIG", &global_config)
        .env("NPM_CONFIG_CACHE", &cache)
        .env("NPM_CONFIG_LOGS_MAX", "0")
        .env("NPM_CONFIG_UPDATE_NOTIFIER", "false")
        .env("GIT_CEILING_DIRECTORIES", operation)
        .env("NO_COLOR", "1");
    let diagnostics = ProcessDiagnostics::default();
    let stdout_diagnostics = diagnostics.clone();
    let stderr_diagnostics = diagnostics.clone();
    let reader_scope = processes.clone();
    let mut run = processes
        .processes
        .start(
            command,
            ProcessOptions::new(
                "node-install/npm",
                control.deadline.saturating_duration_since(Instant::now()),
            ),
            processes.owner,
            move |pipes| {
                let ProcessPipes {
                    stdin,
                    stdout,
                    stderr,
                    stop: _,
                } = pipes;
                drop(stdin);
                IoPlan::new(async move {
                    reader_scope
                        .finish_reader(
                            "node-install/npm",
                            "ci",
                            stdout_diagnostics.read(stdout, "stdout", Some(MAX_OUTPUT)),
                        )
                        .await
                })
                .drain("install-stderr", async move {
                    stderr_diagnostics
                        .read(stderr, "stderr", Some(MAX_OUTPUT))
                        .await
                })
            },
        )
        .map_err(|error| {
            diagnostics.attach(error, "npm ci --omit=dev --ignore-scripts", tools, None)
        })?;
    let mut next_space_check = Instant::now() + NPM_SPACE_INTERVAL;
    let mut interruption = None;
    let result = loop {
        if let Err(error) = control.check() {
            interruption.get_or_insert(error);
            run.cancel();
        }
        if Instant::now() >= next_space_check {
            // Own and await the actual blocking scanner; cancellation cannot
            // detach its filesystem I/O from the current source lease.
            if let Err(error) = check_npm_space(operation, control).await {
                interruption.get_or_insert(error);
                run.cancel();
            }
            next_space_check = Instant::now() + NPM_SPACE_INTERVAL;
        }
        tokio::select! {
            result = &mut run => break result,
            _ = tokio::time::sleep(Duration::from_millis(25)) => {},
        }
    };
    while matches!(
        run.snapshot().phase,
        ProcessPhase::Running | ProcessPhase::Stopping | ProcessPhase::CleanupPending
    ) {
        if let Err(error) = control.check() {
            interruption.get_or_insert(error);
            run.cancel();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let receipt = run.snapshot();
    let closed = receipt.phase == ProcessPhase::Closed
        && receipt.leader_reaped
        && receipt.group_exited
        && receipt.io_joined;
    if !closed {
        let error = result.err().unwrap_or_else(|| {
            failure(
                "NODE_INSTALL_CLEANUP_PENDING",
                "安装进程归属未确认，不能发布；现场及租约保留。",
            )
        });
        return Err(diagnostics.attach(
            error
                .detail("processResourceClosed", false)
                .outcome("unknown"),
            "npm ci --omit=dev --ignore-scripts",
            tools,
            None,
        ));
    }
    if let Some(error) = interruption {
        return Err(diagnostics.attach(
            error.detail("processResourceClosed", true),
            "npm ci --omit=dev --ignore-scripts",
            tools,
            result.as_ref().ok().map(|output| &output.status),
        ));
    }
    let output = result.map_err(|error| {
        let code = match error.code.as_str() {
            "PROCESS_CANCELLED" => "NODE_INSTALL_INTERRUPTED",
            "PROCESS_WORK_TIMEOUT" => "NODE_INSTALL_TIMEOUT",
            "PROCESS_CLEANUP_PENDING" => "NODE_INSTALL_NPM_FAILED",
            _ => "NODE_INSTALL_OUTPUT_LIMIT",
        };
        diagnostics.attach(
            failure(code, "依赖准备未完成；进程和输出已收尾，当前程序未改变。")
                .detail("processCauseCode", error.code)
                .detail("processResourceClosed", true),
            "npm ci --omit=dev --ignore-scripts",
            tools,
            None,
        )
    })?;
    if !output.status.success() {
        return Err(diagnostics.attach(
            failure(
                "NODE_INSTALL_NPM_FAILED",
                "依赖下载或准备失败；当前程序、配置及数据未改变。",
            )
            .detail("processResourceClosed", true),
            "npm ci --omit=dev --ignore-scripts",
            tools,
            Some(&output.status),
        ));
    }
    check_npm_space(operation, control).await
}
fn excluded(path: &Path) -> bool {
    path.components().any(|part| {
        part.as_os_str().to_str().is_some_and(|name| {
            matches!(
                name,
                "node_modules"
                    | ".git"
                    | ".hg"
                    | ".svn"
                    | ".platform"
                    | ".data"
                    | "data"
                    | "cache"
                    | "configuration"
                    | "credentials"
                    | "runs"
                    | "backups"
                    | "trash"
                    | ".npmrc"
            ) || name == ".env"
                || name.starts_with(".env.")
        })
    })
}
fn copy_source(
    source: &Path,
    target: &Path,
    relative: &Path,
    control: &InstallControl,
) -> Result<()> {
    control.check()?;
    ordinary_directory(&source.join(relative))?;
    private_dir(&target.join(relative))?;
    for entry in fs::read_dir(source.join(relative))? {
        let entry = entry?;
        let path = relative.join(entry.file_name());
        if excluded(&path) {
            continue;
        }
        let from = source.join(&path);
        let metadata = fs::symlink_metadata(&from)?;
        if metadata.is_dir() {
            copy_source(source, target, &path, control)?;
        } else if metadata.is_file() {
            let bytes = read_file(&from, MAX_FILE)?;
            control.check()?;
            write_new(&target.join(path), &bytes, metadata.mode() & 0o111 != 0)?;
        } else {
            return Err(invalid("源码候选不能包含符号链接或特殊文件。"));
        }
    }
    File::open(target.join(relative))?.sync_all()?;
    Ok(())
}
fn source_digest(root: &Path, control: &InstallControl) -> Result<String> {
    tree_digest(root, true, control)
}
fn program_digest(root: &Path, control: &InstallControl) -> Result<String> {
    tree_digest(root, false, control)
}
fn sync_program(root: &Path, control: &InstallControl) -> Result<()> {
    for entry in fs::read_dir(root)? {
        control.check()?;
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            sync_program(&entry.path(), control)?;
        } else if kind.is_file() {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(entry.path())?;
            file.sync_all()?;
        } // Internal relative links were validated; never follow them for sync.
    }
    File::open(root)?.sync_all()?;
    Ok(())
}
fn tree_digest(root: &Path, source: bool, control: &InstallControl) -> Result<String> {
    ordinary_directory(root)?;
    let canonical = fs::canonicalize(root)?;
    let mut hash = Sha256::new();
    let mut budget = (0, 0);
    fn visit(
        root: &Path,
        canonical: &Path,
        relative: &Path,
        source: bool,
        control: &InstallControl,
        hash: &mut Sha256,
        budget: &mut (usize, u64),
    ) -> Result<()> {
        let mut entries =
            fs::read_dir(root.join(relative))?.collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            control.check()?;
            let path = relative.join(entry.file_name());
            if source && excluded(&path) {
                continue;
            }
            let label = path
                .to_str()
                .ok_or_else(|| invalid("安装路径必须是有效 UTF-8。"))?;
            budget.0 += 1;
            if budget.0 > MAX_FILES {
                return Err(invalid("安装文件数量超出当前预算。"));
            }
            hash.update((label.len() as u64).to_le_bytes());
            hash.update(label.as_bytes());
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_dir() {
                hash.update(b"d");
                visit(root, canonical, &path, source, control, hash, budget)?;
            } else if metadata.is_file() {
                if !source
                    && path
                        .extension()
                        .and_then(|v| v.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("node"))
                {
                    return Err(invalid("当前安装不支持第三方 Node 原生扩展（.node）。"));
                }
                // Keep dependency payloads intact, including other platforms'
                // executables and libraries. Their presence is not execution or
                // evidence of runtime compatibility. npm lifecycle scripts stay
                // disabled; the installer never launches these bundled tools.
                let content = read_file(&entry.path(), MAX_FILE)?;
                budget.1 += content.len() as u64;
                if budget.1 > MAX_TREE {
                    return Err(invalid("安装内容超出当前空间预算。"));
                }
                hash.update(b"f");
                hash.update([u8::from(metadata.mode() & 0o111 != 0)]);
                hash.update((content.len() as u64).to_le_bytes());
                hash.update(content);
            } else if !source && metadata.file_type().is_symlink() {
                let link = fs::read_link(entry.path())?;
                if link.is_absolute()
                    || !fs::canonicalize(entry.path())
                        .is_ok_and(|target| target.starts_with(canonical))
                {
                    return Err(invalid("安装依赖链接必须是产物内部的相对链接。"));
                }
                hash.update(b"l");
                hash.update(
                    link.to_str()
                        .ok_or_else(|| invalid("依赖链接编码无效。"))?
                        .as_bytes(),
                );
            } else {
                return Err(invalid("安装内容含不支持的链接或特殊文件。"));
            }
        }
        Ok(())
    }
    visit(
        root,
        &canonical,
        Path::new(""),
        source,
        control,
        &mut hash,
        &mut budget,
    )?;
    Ok(format!("{:x}", hash.finalize()))
}
fn ordinary_directory(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return Err(invalid("安装路径必须为规范的绝对路径。"));
    }
    check_no_symlinks(path, false)?;
    if !fs::metadata(path)?.is_dir() {
        return Err(invalid("安装路径不是普通目录。"));
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<()> {
    check_no_symlinks(path, true)?;
    fs::create_dir_all(path)?;
    ordinary_directory(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    check_no_symlinks(path, false)?;
    let before = fs::symlink_metadata(path)?;
    if !before.is_file() || before.nlink() != 1 || before.len() > limit {
        return Err(invalid("安装文件必须是预算内的独立普通文件。"));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let opened = file.metadata()?;
    if before.dev() != opened.dev() || before.ino() != opened.ino() {
        return Err(changed());
    }
    let mut bytes = vec![];
    file.take(limit + 1).read_to_end(&mut bytes)?;
    let after = fs::symlink_metadata(path)?;
    if bytes.len() as u64 > limit
        || before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.mode() != after.mode()
    {
        return Err(changed());
    }
    Ok(bytes)
}
fn read_small(path: &Path) -> Result<Vec<u8>> {
    read_file(path, MAX_METADATA)
}
fn write_new(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    check_no_symlinks(path, true)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(if executable { 0o700 } else { 0o600 })
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
pub(crate) fn registration_digest(slot: &Path) -> Result<Option<String>> {
    let path = slot.join("registration.json");
    check_no_symlinks(&path, true)?;
    if !path.try_exists()? {
        return Ok(None);
    }
    Ok(Some(digest_bytes(&read_small(&path)?)))
}
/// Resolve only the platform-owned slot/generation. Persisted records cannot
/// supply an absolute runtime root or change the fixed runtime interpreter.
pub(crate) fn managed_program_path(
    node_apps_root: &Path,
    record: &ManagedNodeRegistration,
) -> Result<PathBuf> {
    if record.version != 2
        || record.kind != "managed"
        || Uuid::parse_str(&record.id).is_err()
        || !identifier(&record.generation, "gen_")
        || !hex(&record.manifest_digest, 64)
        || !hex(&record.program_digest, 64)
        || !identifier(&record.provenance.source_id, "src_")
        || !hex(&record.provenance.source_digest, 64)
        || record
            .provenance
            .build_source_digest
            .as_deref()
            .is_some_and(|value| !hex(value, 64))
        || record
            .provenance
            .source_commit
            .as_deref()
            .is_some_and(|value| !hex(value, 40) && !hex(value, 64))
    {
        return Err(invalid("受管安装登记身份无效，现场保留。"));
    }
    ordinary_directory(node_apps_root)?;
    let path = node_apps_root
        .join(&record.id)
        .join("generations")
        .join(&record.generation);
    check_no_symlinks(&path, true)?;
    Ok(path)
}

pub(crate) fn verify_managed_program(
    node_apps_root: &Path,
    record: &ManagedNodeRegistration,
) -> Result<PathBuf> {
    let program = managed_program_path(node_apps_root, record)?;
    let (_stop, stop) = watch::channel(false);
    let (_cancel, cancel) = watch::channel(false);
    let control = InstallControl::new(stop, cancel);
    if program_digest(&program, &control)? != record.program_digest
        || digest_bytes(&read_small(&program.join("local-app.json"))?) != record.manifest_digest
    {
        return Err(changed());
    }
    Ok(program)
}
fn digest_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn hex(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn identifier(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|rest| hex(rest, 32))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
#[path = "node_app_install_tests.rs"]
mod tests;
