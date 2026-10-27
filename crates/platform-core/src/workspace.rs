//! Whole-workspace migration. The fixed application directory owns the commit
//! journal and task, so neither progress nor recovery writes modify the source.
use crate::{
    builds,
    file_identity::PersistedIdentity,
    projects::{check_no_symlinks, valid_id},
    ApiError, ProjectStore, RuntimePaths, TaskRecord, TaskStatus, APP_IDENTIFIER, ENVIRONMENT,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, CString},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{symlink, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;
const MARKER: &str = ".platform/workspace-selection.json";
const RETIRED: &str = ".platform/workspace-retired.json";
pub(crate) type DirectoryIdentity = PersistedIdentity;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MigrationJournal {
    schema_version: u32,
    identifier: String,
    environment: String,
    pub task_id: String,
    pub source_path: PathBuf,
    pub destination_path: PathBuf,
    source_identity: DirectoryIdentity,
    target_empty_identity: DirectoryIdentity,
    destination_identity: Option<DirectoryIdentity>,
    pub stage_path: PathBuf,
    pub phase: String,
    pub published: bool,
    pub committed: bool,
    pub resolved: bool,
    pub epochs: BTreeMap<String, String>,
    pub epochs_applied: bool,
    #[serde(default)]
    pub blocked_projects: BTreeMap<String, ApiError>,
    pub file_count: u64,
    pub total_bytes: u64,
    #[serde(default)]
    started_at: u64,
    #[serde(default)]
    retained_source_warning: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Entry {
    path: String,
    kind: String,
    bytes: u64,
    digest: Option<String>,
    target: Option<PathBuf>,
    mode: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MigrationInput {
    pub destination_path: PathBuf,
}
fn ioerr(e: impl std::fmt::Display) -> ApiError {
    ApiError::new("WORKSPACE_MIGRATION_FAILED", e.to_string())
}
fn identity(path: &Path) -> Result<DirectoryIdentity> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() || m.file_type().is_symlink() {
        bail!("Expected ordinary directory: {}", path.display())
    }
    PersistedIdentity::capture(path)
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    check_no_symlinks(path, false)?;
    let m = fs::metadata(path)?;
    if !m.is_file() || m.len() > 32_000_000 {
        bail!("Invalid migration metadata: {}", path.display())
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn journal_path(data: &Path, id: &str) -> PathBuf {
    data.join("migrations").join(id).join("journal.json")
}
fn task_dir(data: &Path, id: &str) -> PathBuf {
    data.join("migrations").join(id).join("task")
}
fn validate_journal(j: &MigrationJournal, id: &str) -> Result<()> {
    if j.schema_version != 1
        || j.identifier != APP_IDENTIFIER
        || j.environment != ENVIRONMENT
        || j.task_id != id
        || !valid_id(id)
    {
        bail!("Migration journal identity is invalid")
    }
    Ok(())
}
/// A settings redirect is accepted only with a published target and its fixed
/// journal. Missing committed media is an error; the default is never recreated.
pub(crate) fn validate_selection(data: &Path, root: &Path, migration: Option<&str>) -> Result<()> {
    clean_absolute(root)?;
    if let Some(id) = migration {
        if !valid_id(id) {
            bail!("Invalid workspace migration selection")
        }
        let j: MigrationJournal = read_json(&journal_path(data, id))?;
        validate_journal(&j, id)?;
        if j.destination_path != root || !j.published {
            bail!("Settings do not select a published migration target")
        }
        let marker:Value=read_json(&root.join(MARKER)).with_context(||format!("Committed workspace is unavailable at {}; reconnect its disk. The old copy will not be opened automatically",root.display()))?;
        if marker["taskId"] != id
            || marker["identifier"] != APP_IDENTIFIER
            || marker["environment"] != ENVIRONMENT
            || !j
                .destination_identity
                .as_ref()
                .is_some_and(|expected| expected.matches_path(root).unwrap_or(false))
        {
            bail!("Committed workspace identity cannot be verified at {}; existing data is unchanged. If this followed a reboot, inspect the offline workspace identity repair preview",root.display())
        }
    } else {
        if root != data.join("workspace") {
            bail!("Workspace redirect has no committed migration journal")
        }
        check_no_symlinks(root, true)?;
        if root.join(RETIRED).exists() {
            bail!(
                "The default workspace is a retired migration copy; refusing an implicit fallback"
            )
        }
        let migrations = data.join("migrations");
        if migrations.is_dir() {
            for entry in fs::read_dir(migrations)? {
                let entry = entry?;
                let file = entry.path().join("journal.json");
                if file.is_file() {
                    let j: MigrationJournal = read_json(&file)?;
                    if j.committed && j.source_path == root {
                        bail!("The default workspace was migrated; refusing to reopen its historical copy without a committed selection")
                    }
                }
            }
        }
    }
    Ok(())
}

/// Only an explicit offline repair may bridge a pre-UUID migration record to
/// this mounted volume. Completed journal and marker evidence is preserved.
pub(crate) fn legacy_repair_identity(
    data: &Path,
    root: &Path,
    id: &str,
) -> Result<DirectoryIdentity> {
    if !valid_id(id) {
        bail!("Invalid migration selection")
    }
    let j: MigrationJournal = read_json(&journal_path(data, id))?;
    validate_journal(&j, id)?;
    if j.destination_path != root
        || !j.published
        || !j.committed
        || !j.resolved
        || !j.epochs_applied
        || j.phase != "completed"
    {
        bail!("Identity repair requires a completed, resolved migration selecting this workspace")
    }
    let marker: Value = read_json(&root.join(MARKER))?;
    if marker["schemaVersion"] != 1
        || marker["taskId"] != id
        || marker["identifier"] != APP_IDENTIFIER
        || marker["environment"] != ENVIRONMENT
    {
        bail!("Workspace selection marker differs from its committed journal")
    }
    let expected = j
        .destination_identity
        .context("Committed destination identity is missing")?;
    if !expected.is_legacy() {
        bail!("Stable workspace identity cannot be rebound by legacy repair")
    }
    let actual = identity(root)?;
    if expected.inode != actual.inode || expected.inode == 0 {
        bail!("Workspace directory was replaced; legacy device repair cannot authorize it")
    }
    Ok(expected)
}
fn clean_absolute(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        bail!("Select an absolute normalized directory")
    };
    Ok(())
}
fn synced_path(path: &Path) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let mut roots = vec![
        home.join("Library/Mobile Documents"),
        home.join("Library/CloudStorage"),
        home.join("Dropbox"),
        home.join("Google Drive"),
        home.join("OneDrive"),
    ];
    if let Ok(bytes) = fs::read(home.join(".dropbox/info.json")) {
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
            if let Some(accounts) = value.as_object() {
                for account in accounts.values() {
                    if let Some(p) = account["path"].as_str() {
                        roots.push(PathBuf::from(p));
                    }
                }
            }
        }
    }
    roots.iter().any(|r| path.starts_with(r))
}
pub(crate) fn filesystem(path: &Path) -> Result<Value> {
    let c = CString::new(path.as_os_str().as_bytes())?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(c.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    #[cfg(target_os = "macos")]
    let (name, local) = (
        unsafe { CStr::from_ptr(stat.f_fstypename.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
        stat.f_flags & (libc::MNT_LOCAL as u32) != 0,
    );
    #[cfg(not(target_os = "macos"))]
    let (name, local) = (format!("{}", stat.f_type), true);
    let available = stat.f_bavail.saturating_mul(stat.f_bsize as u64);
    Ok(json!({"type":name,"local":local,"availableBytes":available}))
}
pub(crate) fn preflight(paths: &RuntimePaths, destination: &Path) -> Result<Value, ApiError> {
    let check = || -> Result<Value> {
        clean_absolute(destination)?;
        check_no_symlinks(destination, false)?;
        let target = destination.canonicalize()?;
        let source = paths.workspace_root().canonicalize()?;
        if target == source || target.starts_with(&source) || source.starts_with(&target) {
            bail!("Target must be separate from the source, its parents and its children")
        }
        if target.components().any(|c| {
            c.as_os_str()
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".app")
        }) {
            bail!("Workspace cannot be inside an application bundle")
        }
        let data = paths.data_root();
        for protected in [
            paths.runtime_dir(),
            paths.migrations_dir(),
            data.join("cache"),
        ] {
            if target.starts_with(&protected) || protected.starts_with(&target) {
                bail!("Target overlaps fixed platform configuration or migration journals")
            }
        }
        if synced_path(&target) {
            bail!("Cloud synchronization directories are not supported as active workspaces")
        }
        let meta = fs::metadata(&target)?;
        if !meta.is_dir() || fs::read_dir(&target)?.next().is_some() {
            bail!("Select a writable empty directory")
        }
        let fsinfo = filesystem(&target)?;
        #[cfg(not(test))]
        if fsinfo["type"] != "apfs" || fsinfo["local"] != true {
            bail!("Migration targets must be local APFS disks")
        }
        #[cfg(test)]
        if cfg!(target_os = "macos") && (fsinfo["type"] != "apfs" || fsinfo["local"] != true) {
            bail!("Migration targets must be local APFS disks")
        }
        let probe = target.join(format!(".write-probe-{}", Uuid::new_v4().simple()));
        let f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&probe)?;
        f.sync_all()?;
        drop(f);
        fs::remove_file(probe)?;
        Ok(
            json!({"sourcePath":source,"destinationPath":target,"filesystem":fsinfo,"sourceRetained":true}),
        )
    };
    check().map_err(|e| {
        ApiError::new("INVALID_MIGRATION_TARGET", e.to_string()).outcome("not_started")
    })
}
fn exclusions(root: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut excluded = BTreeSet::from([
        PathBuf::from(".platform/cache/storage"),
        PathBuf::from(MARKER),
        PathBuf::from(RETIRED),
        PathBuf::from(".platform/identity-binding.json"),
    ]);
    for (id, path) in crate::projects::enumerate_project_roots(root)? {
        if !path.is_dir() {
            continue;
        }
        if let Ok(m) = read_json::<crate::ProjectManifest>(&path.join("project.json")) {
            if crate::projects::validate_manifest(&m, &id, &path).is_ok() {
                excluded.insert(
                    path.strip_prefix(root)?
                        .to_path_buf()
                        .join(m.source_dir)
                        .join("node_modules"),
                );
            }
        }
    }
    Ok(excluded)
}
fn collect(root: &Path, excluded: &BTreeSet<PathBuf>) -> Result<Vec<Entry>> {
    fn walk(
        root: &Path,
        relative: &Path,
        excluded: &BTreeSet<PathBuf>,
        out: &mut Vec<Entry>,
    ) -> Result<()> {
        let mut entries =
            fs::read_dir(root.join(relative))?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let rel = relative.join(e.file_name());
            if excluded.iter().any(|p| rel == *p || rel.starts_with(p)) {
                continue;
            }
            let path = rel
                .to_str()
                .context("Workspace contains a filename that is not valid UTF-8")?
                .to_owned();
            let meta = fs::symlink_metadata(e.path())?;
            let mode = meta.permissions().mode() & 0o777;
            if meta.file_type().is_symlink() {
                out.push(Entry {
                    path,
                    kind: "symlink".into(),
                    bytes: 0,
                    digest: None,
                    target: Some(fs::read_link(e.path())?),
                    mode,
                });
            } else if meta.is_dir() {
                out.push(Entry {
                    path,
                    kind: "directory".into(),
                    bytes: 0,
                    digest: None,
                    target: None,
                    mode,
                });
                walk(root, &rel, excluded, out)?;
            } else if meta.is_file() {
                let mut f = File::open(e.path())?;
                let mut hash = Sha256::new();
                let mut buf = [0; 128 * 1024];
                loop {
                    let n = f.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    hash.update(&buf[..n]);
                }
                out.push(Entry {
                    path,
                    kind: "file".into(),
                    bytes: meta.len(),
                    digest: Some(format!("{:x}", hash.finalize())),
                    target: None,
                    mode,
                });
            } else {
                bail!(
                    "Workspace contains a live socket or unsupported file: {}",
                    e.path().display()
                )
            }
        }
        Ok(())
    }
    let mut out = vec![];
    walk(root, Path::new(""), excluded, &mut out)?;
    Ok(out)
}
fn copy_entries(source: &Path, target: &Path, entries: &[Entry]) -> Result<()> {
    fs::create_dir(target)?;
    fs::set_permissions(target, fs::Permissions::from_mode(0o700))?;
    for e in entries {
        let from = source.join(&e.path);
        let to = target.join(&e.path);
        match e.kind.as_str() {
            "directory" => {
                fs::create_dir(&to)?;
            }
            "symlink" => symlink(e.target.as_ref().context("Missing symlink target")?, &to)?,
            "file" => {
                let m = fs::symlink_metadata(&from)?;
                if !m.is_file() || m.file_type().is_symlink() {
                    bail!("Source changed while copying: {}", from.display())
                }
                let mut input = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&from)?;
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&to)?;
                std::io::copy(&mut input, &mut output)?;
                output.sync_all()?;
                fs::set_permissions(&to, fs::Permissions::from_mode(e.mode))?;
            }
            _ => bail!("Unknown migration entry kind"),
        }
    }
    for e in entries.iter().rev().filter(|e| e.kind == "directory") {
        let p = target.join(&e.path);
        File::open(&p)?.sync_all()?;
        fs::set_permissions(p, fs::Permissions::from_mode(e.mode))?;
    }
    File::open(target)?.sync_all()?;
    Ok(())
}
pub(crate) fn create(paths: &RuntimePaths, destination: &Path) -> Result<TaskRecord, ApiError> {
    let check = preflight(paths, destination)?;
    let id = format!("task_{}", Uuid::new_v4().simple());
    let task = TaskRecord {
        task_id: id.clone(),
        project_id: None,
        task_type: "workspaceMigration".into(),
        status: TaskStatus::Queued,
        phase: "queued".into(),
        result: None,
        error: None,
    };
    let dest = PathBuf::from(
        check["destinationPath"]
            .as_str()
            .ok_or_else(|| ioerr("Missing migration destination"))?,
    );
    let j = MigrationJournal {
        schema_version: 1,
        identifier: APP_IDENTIFIER.into(),
        environment: ENVIRONMENT.into(),
        task_id: id.clone(),
        source_path: paths.workspace_root().into(),
        source_identity: identity(paths.workspace_root()).map_err(ioerr)?,
        target_empty_identity: identity(&dest).map_err(ioerr)?,
        stage_path: dest
            .parent()
            .ok_or_else(|| ioerr("Target has no parent"))?
            .join(format!(".local-projects-{id}.partial")),
        destination_path: dest,
        destination_identity: None,
        phase: "queued".into(),
        published: false,
        committed: false,
        resolved: false,
        epochs: BTreeMap::new(),
        epochs_applied: false,
        blocked_projects: BTreeMap::new(),
        file_count: 0,
        total_bytes: 0,
        started_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        retained_source_warning: None,
    };
    builds::atomic_json(&journal_path(paths.data_root(), &id), &j)?;
    builds::atomic_json(&task_dir(paths.data_root(), &id).join("task.json"), &task)?;
    Ok(task)
}
pub(crate) fn phase(paths: &RuntimePaths, id: &str, phase: &str) -> Result<(), ApiError> {
    let mut t = task(paths, id)?.ok_or_else(|| ioerr("Migration task missing"))?;
    t.status = TaskStatus::Running;
    t.phase = phase.into();
    builds::atomic_json(&task_dir(paths.data_root(), id).join("task.json"), &t)?;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(task_dir(paths.data_root(), id).join("log.txt"))?;
    writeln!(log, "{phase}")?;
    log.sync_all()?;
    Ok(())
}
pub(crate) fn read_journal(paths: &RuntimePaths, id: &str) -> Result<MigrationJournal, ApiError> {
    read_json(&journal_path(paths.data_root(), id)).map_err(ioerr)
}
pub(crate) fn save_journal(paths: &RuntimePaths, j: &MigrationJournal) -> Result<(), ApiError> {
    builds::atomic_json(&journal_path(paths.data_root(), &j.task_id), j)
}
pub(crate) fn validate_idle(paths: &RuntimePaths) -> Result<(), ApiError> {
    crate::projects::check_pending_deletions(paths.workspace_root())?;
    crate::source_restore_guard::check_workspace_available(paths.workspace_root())?;
    crate::data_restore_guard::check_workspace_available(paths.workspace_root())?;
    crate::project_git::check_data_workspace_available(paths.workspace_root())?;
    let directory = paths.workspace_root().join(".platform/operations");
    check_no_symlinks(&directory, true)?;
    if directory.is_dir() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            for name in ["journal.json", "lifecycle.json"] {
                let path = entry.path().join(name);
                check_no_symlinks(&path, true)?;
                if path.is_file() {
                    let value: Value = read_json(&path).map_err(ioerr)?;
                    if value["resolved"] != true {
                        return Err(ApiError::new("RECOVERY_REQUIRED","Resolve the unfinished project operation before migrating the workspace").detail("taskId",value["taskId"].clone()).detail("journal",json!(path)));
                    }
                }
            }
        }
    }
    let reservations = paths
        .workspace_root()
        .join(".platform/project-state/reservations");
    check_no_symlinks(&reservations, true)?;
    if reservations.is_dir() && fs::read_dir(&reservations)?.next().is_some() {
        return Err(ApiError::new(
            "RECOVERY_REQUIRED",
            "An unfinished project identity reservation must be reconciled before migration",
        ));
    }
    Ok(())
}
/// Called only after all accepted tasks, database work and file readers drain.
pub(crate) fn copy_and_commit(
    paths: &RuntimePaths,
    store: &ProjectStore,
    id: &str,
) -> Result<(RuntimePaths, MigrationJournal), ApiError> {
    let run = || -> Result<(RuntimePaths, MigrationJournal)> {
        let mut j: MigrationJournal = read_json(&journal_path(paths.data_root(), id))?;
        validate_journal(&j, id)?;
        validate_idle(paths)?;
        preflight(paths, &j.destination_path)?;
        if !j.source_identity.matches_path(&j.source_path)?
            || !j.target_empty_identity.matches_path(&j.destination_path)?
        {
            bail!("Source or empty target directory identity changed")
        }
        let excluded = exclusions(&j.source_path)?;
        phase(paths, id, "inventory")?;
        let entries = collect(&j.source_path, &excluded)?;
        j.file_count = entries.iter().filter(|e| e.kind == "file").count() as u64;
        j.total_bytes = entries.iter().map(|e| e.bytes).sum();
        let available = filesystem(&j.destination_path)?["availableBytes"]
            .as_u64()
            .unwrap_or(0);
        if available < j.total_bytes.saturating_add(16 * 1024 * 1024) {
            return Err(ApiError::new(
                "SPACE_INSUFFICIENT",
                "Insufficient space for a complete workspace copy",
            )
            .detail("requiredBytes", j.total_bytes)
            .detail("availableBytes", available)
            .into());
        }
        // Epochs are journaled before settings commit, then applied idempotently.
        for (id, path) in crate::projects::enumerate_project_roots(&j.source_path)? {
            if path.is_dir() && store.is_retired(&id)?.is_none() {
                j.epochs.insert(id, Uuid::new_v4().to_string());
            }
        }
        for entry in fs::read_dir(j.source_path.join(".platform/project-state"))? {
            let p = entry?.path();
            if p.extension().is_some_and(|e| e == "json") {
                if let Some(id) = p.file_stem().and_then(|s| s.to_str()) {
                    if valid_id(id) && store.is_retired(id)?.is_none() {
                        j.epochs
                            .entry(id.into())
                            .or_insert_with(|| Uuid::new_v4().to_string());
                    }
                }
            }
        }
        for project_id in j.epochs.keys() {
            if let Err(error) = store.project(project_id) {
                if error.code != "WORKSPACE_MIGRATING" {
                    j.blocked_projects.insert(project_id.clone(), error);
                }
            }
        }
        j.phase = "copying".into();
        save_journal(paths, &j)?;
        builds::atomic_json(
            &paths.migrations_dir().join(id).join("manifest.json"),
            &entries,
        )?;
        phase(paths, id, "copying")?;
        copy_entries(&j.source_path, &j.stage_path, &entries)?;
        phase(paths, id, "verifying")?;
        if collect(&j.source_path, &excluded)? != entries {
            bail!("Workspace source changed during migration; no new location was committed")
        }
        if collect(&j.stage_path, &excluded)? != entries {
            bail!("Workspace copy differs from its source; no new location was committed")
        }
        // Preserve all control data, including damaged protected originals. Only
        // active registrations are rebound later; no project scripts run here.
        // The verified copy is unchanged. Keep its target inventory beside the
        // journal for later history-cleanup ownership checks.
        builds::atomic_json(
            &paths.migrations_dir().join(id).join("target-manifest.json"),
            &collect(&j.stage_path, &excluded)?,
        )?;
        j.destination_identity = Some(identity(&j.stage_path)?);
        builds::atomic_json(
            &j.stage_path.join(MARKER),
            &json!({"schemaVersion":1,"identifier":APP_IDENTIFIER,"environment":ENVIRONMENT,"taskId":id}),
        )?;
        j.phase = "publishing".into();
        save_journal(paths, &j)?;
        phase(paths, id, "publishing")?;
        if !j.target_empty_identity.matches_path(&j.destination_path)?
            || fs::read_dir(&j.destination_path)?.next().is_some()
        {
            bail!("Migration target was modified before publication")
        }
        fs::remove_dir(&j.destination_path)?;
        fs::rename(&j.stage_path, &j.destination_path)?;
        File::open(j.destination_path.parent().unwrap())?.sync_all()?;
        j.published = true;
        j.phase = "committing".into();
        save_journal(paths, &j)?;
        phase(paths, id, "committing")?;
        let selected = paths.commit_workspace(&j.destination_path, id)?;
        j.committed = true;
        save_journal(&selected, &j)?;
        Ok((selected, j))
    };
    run().map_err(|e| {
        if let Some(error) = e.downcast_ref::<ApiError>() {
            error.clone()
        } else {
            ioerr(e)
        }
    })
}
pub(crate) fn initialize_selected(
    paths: &RuntimePaths,
    store: &ProjectStore,
    j: &mut MigrationJournal,
) -> Result<(), ApiError> {
    phase(paths, &j.task_id, "reopening")?;
    for (id, epoch) in &j.epochs {
        store.rebind_migrated(id, epoch)?;
        if let Some(error) = j.blocked_projects.get(id) {
            store.retain_migrated_diagnostic(id, error.clone())?;
        }
    }
    // Retired registrations remain restoration/ownership evidence. Rebase only
    // their copied absolute root after this explicit migration, without giving
    // them a new epoch, database identity or active status.
    for entry in fs::read_dir(paths.workspace_root().join(".platform/project-state"))? {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|id| valid_id(id))
        else {
            continue;
        };
        if store.is_retired(id)?.is_some() {
            store.rebase_retired_registration_after_migration(id, &j.source_path)?;
        }
    }
    j.epochs_applied = true;
    crate::node_apps::development::rebind_published_after_migration(
        paths.data_root(),
        &j.source_path,
        &j.destination_path,
    )?;
    save_journal(paths, j)?;
    let cache = paths.workspace_root().join(".platform/cache/storage");
    check_no_symlinks(&cache, true)?;
    if cache.is_dir() {
        fs::remove_dir_all(cache)?;
    }
    Ok(())
}
pub(crate) fn finish(
    paths: &RuntimePaths,
    id: &str,
    result: Result<Value, ApiError>,
) -> Result<(), ApiError> {
    let mut t = task(paths, id)?.ok_or_else(|| ioerr("Migration task missing"))?;
    match result {
        Ok(value) => {
            t.status = TaskStatus::Succeeded;
            t.phase = "completed".into();
            t.result = Some(value);
            t.error = None;
        }
        Err(error) => {
            t.status = TaskStatus::Failed;
            t.phase = "failed".into();
            t.result = Some(error.details.clone());
            t.error = Some(error);
        }
    }
    builds::atomic_json(&task_dir(paths.data_root(), id).join("task.json"), &t)
}
pub(crate) fn task(paths: &RuntimePaths, id: &str) -> Result<Option<TaskRecord>, ApiError> {
    if !valid_id(id) {
        return Err(ApiError::new("INVALID_TASK_ID", "Invalid task ID"));
    }
    let path = task_dir(paths.data_root(), id).join("task.json");
    check_no_symlinks(&path, true)?;
    if !path.is_file() {
        return Ok(None);
    };
    read_json(&path).map(Some).map_err(ioerr)
}
pub(crate) fn logs(paths: &RuntimePaths, id: &str, cursor: u64) -> Result<Option<Value>, ApiError> {
    let Some(t) = task(paths, id)? else {
        return Ok(None);
    };
    let p = task_dir(paths.data_root(), id).join("log.txt");
    check_no_symlinks(&p, true)?;
    let bytes = if p.is_file() { fs::read(p)? } else { vec![] };
    let start = usize::try_from(cursor)
        .unwrap_or(usize::MAX)
        .min(bytes.len());
    let end = (start + 64000).min(bytes.len());
    Ok(Some(
        json!({"taskId":id,"cursor":start,"nextCursor":end,"text":String::from_utf8_lossy(&bytes[start..end]),"truncated":false,"gaps":[],"eof":t.status.is_final()&&end==bytes.len()}),
    ))
}
pub(crate) fn completed_result(j: &MigrationJournal) -> Value {
    json!({"sourcePath":j.source_path,"destinationPath":j.destination_path,"workspaceRoot":j.destination_path,"sourceRetained":true,"operationOutcome":"committed","fileCount":j.file_count,"totalBytes":j.total_bytes,"requiresReconnect":true,"blockedProjects":j.blocked_projects})
}
pub(crate) fn mark_old_retired(paths: &RuntimePaths, j: &MigrationJournal) -> Result<(), ApiError> {
    let mut done = j.clone();
    match identity(&j.source_path) {
        Ok(_)
            if j.source_identity
                .matches_path(&j.source_path)
                .unwrap_or(false) =>
        {
            if let Err(error) = builds::atomic_json(
                &j.source_path.join(RETIRED),
                &json!({"schemaVersion":1,"taskId":j.task_id,"destinationPath":j.destination_path,"retired":true}),
            ) {
                done.retained_source_warning =
                    Some(format!("Could not mark retained source: {error}"));
            }
        }
        _ => {
            done.retained_source_warning = Some(
                "Retained source is unavailable or its identity changed; it was not modified"
                    .into(),
            )
        }
    }
    done.resolved = true;
    done.phase = "completed".into();
    save_journal(paths, &done)
}
pub(crate) fn is_committed(paths: &RuntimePaths, id: &str) -> Result<bool, ApiError> {
    let settings: Value = read_json(&paths.data_root().join("settings.json")).map_err(ioerr)?;
    Ok(settings["migrationId"].as_str() == Some(id))
}
/// Run before opening databases. Settings decide the committed side. An
/// unavailable committed target has already failed validation, never falls back.
pub(crate) fn recover_startup(paths: &RuntimePaths) -> Result<(), ApiError> {
    let selected: Value = read_json(&paths.data_root().join("settings.json")).map_err(ioerr)?;
    for entry in fs::read_dir(paths.migrations_dir())? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if !valid_id(&id) || !entry.path().join("journal.json").is_file() {
            continue;
        }
        let mut j = read_journal(paths, &id)?;
        validate_journal(&j, &id).map_err(ioerr)?;
        let chosen = selected["migrationId"].as_str() == Some(id.as_str());
        if chosen && !j.committed {
            j.committed = true;
            save_journal(paths, &j)?;
        }
        if chosen && !j.epochs_applied {
            let store = ProjectStore::new(paths.workspace_root().into())?;
            initialize_selected(paths, &store, &mut j)?;
        }
        let Some(mut t) = task(paths, &id)? else {
            continue;
        };
        if !t.status.is_final() {
            t.status = TaskStatus::Interrupted;
            t.phase = "interrupted".into();
            let outcome = if chosen { "committed" } else { "not_committed" };
            t.result = Some(if chosen {
                completed_result(&j)
            } else {
                json!({"sourcePath":j.source_path,"destinationPath":j.destination_path,"operationOutcome":outcome,"sourceRetained":true,"targetRetained":j.destination_path.exists()||j.stage_path.exists()})
            });
            t.error=Some(ApiError::new("TASK_INTERRUPTED","Platform stopped before the migration task returned; no operation was automatically repeated").outcome(outcome));
            builds::atomic_json(&task_dir(paths.data_root(), &id).join("task.json"), &t)?;
        }
        if chosen && !j.resolved {
            mark_old_retired(paths, &j)?;
        }
    }
    Ok(())
}
pub(crate) fn info(paths: &RuntimePaths, store: &ProjectStore) -> Result<Value, ApiError> {
    let mut retained = vec![];
    let mut active = None;
    let mut last = None;
    let mut last_at = 0;
    for entry in fs::read_dir(paths.migrations_dir())? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if !valid_id(&id) || !entry.path().join("journal.json").is_file() {
            continue;
        }
        let j = read_journal(paths, &id)?;
        if let Some(t) = task(paths, &id)? {
            if !t.status.is_final() {
                active = Some(t.clone())
            }
            if j.started_at >= last_at {
                last_at = j.started_at;
                last = Some(t);
            }
        }
        if j.committed && j.source_path != paths.workspace_root() {
            let actual = identity(&j.source_path).ok();
            let cleanup = crate::history_cleanup::retired_cleanup_status(paths, &id)?;
            let cleaned = cleanup.is_some()
                && matches!(fs::symlink_metadata(&j.source_path), Err(error) if error.kind() == std::io::ErrorKind::NotFound);
            retained.push(json!({"path":j.source_path,"taskId":id,"available":actual.is_some(),"identityMatches":j.source_identity.matches_path(&j.source_path).unwrap_or(false),"canDelete":false,"cleaned":cleaned,"cleanup":cleanup,"reason":if cleaned {"Retained snapshot was explicitly cleaned; the cleanup receipt remains"}else{"Inspect this registered snapshot in the history cleanup preview"}}));
        }
    }
    Ok(
        json!({"workspaceRoot":paths.workspace_root(),"dataRoot":paths.data_root(),"formatVersion":1,"filesystem":filesystem(paths.workspace_root()).map_err(ioerr)?["type"],"filesystemInfo":filesystem(paths.workspace_root()).map_err(ioerr)?,"migrationState":if store.workspace_paused(){"paused"}else if active.is_some(){"migrating"}else{"ready"},"activeTask":active,"lastTaskId":last.map(|t|t.task_id),"retainedRoots":retained,"sizeBytes":null}),
    )
}

/// Roots owned by completed public migrations. This is deliberately separate
/// from historical cleanup's narrower protection rules: explicit pre-uninstall
/// cleanup may remove retained projects and recovery evidence, but must never
/// infer ownership from a path alone or silently skip an unavailable disk.
pub(crate) fn local_cleanup_roots(paths: &RuntimePaths) -> Result<Vec<PathBuf>, ApiError> {
    paths.validate_cleanup_selection().map_err(ioerr)?;
    let mut journals = Vec::new();
    let mut cleaned = BTreeSet::new();
    for entry in fs::read_dir(paths.migrations_dir())? {
        let entry = entry?;
        let id = entry
            .file_name()
            .into_string()
            .map_err(|_| ioerr("无法核对迁移记录名称，未开始清理。"))?;
        let j = read_journal(paths, &id)?;
        validate_journal(&j, &id).map_err(ioerr)?;
        if !j.committed
            || !j.published
            || !j.resolved
            || !j.epochs_applied
            || j.phase != "completed"
            || task(paths, &id)?.is_none_or(|task| !task.status.is_final())
        {
            return Err(ioerr(
                "存在未完成或无法核对的迁移，请先处理后再清理全部本机数据。",
            ));
        }
        check_no_symlinks(&j.stage_path, true)?;
        if j.stage_path.try_exists()? {
            return Err(ioerr("存在迁移暂存目录，不能猜测其身份后清理。"));
        }
        if crate::history_cleanup::retired_cleanup_status(paths, &id)?
            .is_some_and(|receipt| receipt["completed"] == true)
        {
            cleaned.insert(j.source_path.clone());
        }
        journals.push(j);
    }
    let mut roots = BTreeSet::from([paths.workspace_root().to_path_buf()]);
    for journal in journals {
        let destination = journal
            .destination_identity
            .as_ref()
            .ok_or_else(|| ioerr("迁移目标缺少身份记录，未开始清理。"))?;
        for (path, expected) in [
            (&journal.source_path, &journal.source_identity),
            (&journal.destination_path, destination),
        ] {
            clean_absolute(path).map_err(ioerr)?;
            check_no_symlinks(path, true)?;
            match fs::symlink_metadata(path) {
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound && cleaned.contains(path) => {}
                Ok(metadata)
                    if metadata.is_dir() && expected.matches_path(path).unwrap_or(false) =>
                {
                    roots.insert(path.clone());
                }
                _ => {
                    return Err(ioerr(format!(
                        "工作区不可访问或身份已变化，请连接原磁盘并核对：{}",
                        path.display()
                    )))
                }
            }
        }
    }
    Ok(roots.into_iter().collect())
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
