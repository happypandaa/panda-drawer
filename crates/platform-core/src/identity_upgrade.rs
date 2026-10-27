//! Explicit, offline compatibility for migration records predating volume UUIDs.
//! Original settings/journals are never rewritten. A selected root is bound only
//! after a preview is confirmed under the application's exclusive instance lock.
use crate::{
    file_identity::PersistedIdentity, projects::check_no_symlinks, RuntimePaths, APP_IDENTIFIER,
    ENVIRONMENT,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
use uuid::Uuid;

const BINDING: &str = ".platform/identity-binding.json";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    schema_version: u32,
    identifier: String,
    environment: String,
    workspace_root: PathBuf,
    workspace_identity: PersistedIdentity,
    data_root: PathBuf,
    data_identity: PersistedIdentity,
    legacy_device: u64,
    repair_id: String,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn private_bytes(path: &Path) -> Result<Vec<u8>> {
    let before = PersistedIdentity::capture(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.len() > 8_000_000
        || meta.permissions().mode() & 0o077 != 0
        || meta.uid() != unsafe { libc::geteuid() }
    {
        bail!(
            "Identity repair metadata must be a private, independent ordinary file: {}",
            path.display()
        )
    }
    if !before.same_runtime_object(&PersistedIdentity::from_file(&file)?) {
        bail!("Identity repair metadata changed while opening")
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(8_000_001)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 8_000_000 || !before.same_runtime_object(&PersistedIdentity::capture(path)?) {
        bail!("Identity repair metadata changed while reading")
    }
    Ok(bytes)
}
use std::os::unix::fs::PermissionsExt;

fn read_binding(scope: &Path) -> Result<Binding> {
    let binding: Binding = serde_json::from_slice(&private_bytes(&scope.join(BINDING))?)?;
    if binding.schema_version != 1
        || binding.identifier != APP_IDENTIFIER
        || binding.environment != ENVIRONMENT
        || binding.workspace_root != scope
        || binding.repair_id.len() != 64
        || !binding.repair_id.bytes().all(|b| b.is_ascii_hexdigit())
        || binding.workspace_identity.is_legacy()
        || binding.data_identity.is_legacy()
        || !binding
            .workspace_identity
            .matches_captured(&PersistedIdentity::capture(scope)?)
        || !binding
            .data_identity
            .matches_captured(&PersistedIdentity::capture(&binding.data_root)?)
    {
        bail!("Workspace identity binding does not match this directory or volume")
    }
    let receipt_dir = binding
        .data_root
        .join("identity-repairs")
        .join(&binding.repair_id);
    let receipt: Value =
        serde_json::from_slice(&private_bytes(&receipt_dir.join("receipt.json"))?)?;
    if receipt["schemaVersion"] != 1 || receipt["binding"] != serde_json::to_value(&binding)? {
        bail!("Workspace identity repair receipt is missing or inconsistent")
    }
    let plan = &receipt["plan"];
    let evidence = &plan["evidence"];
    if plan["planId"] != binding.repair_id
        || digest(&serde_json::to_vec(evidence)?) != binding.repair_id
        || evidence["schemaVersion"] != 1
        || evidence["identifier"] != APP_IDENTIFIER
        || evidence["environment"] != ENVIRONMENT
        || evidence["workspaceRoot"] != json!(binding.workspace_root)
        || evidence["workspaceIdentity"] != serde_json::to_value(&binding.workspace_identity)?
        || evidence["dataRoot"] != json!(binding.data_root)
        || evidence["dataIdentity"] != serde_json::to_value(&binding.data_identity)?
        || evidence["legacyIdentity"]["device"] != binding.legacy_device
        || evidence["legacyIdentity"]["inode"] != binding.workspace_identity.inode
        || evidence["legacyIdentity"].get("volumeUuid").is_some()
        || receipt["originals"] != evidence["originals"]
    {
        bail!("Workspace identity repair plan does not match its evidence and binding")
    }
    for name in [
        "settings.json",
        "migration-journal.json",
        "workspace-selection.json",
    ] {
        if receipt["originals"][name].as_str()
            != Some(digest(&private_bytes(&receipt_dir.join(name))?).as_str())
        {
            bail!("Original identity repair evidence is missing or changed")
        }
    }
    Ok(binding)
}

/// A legacy device change is accepted only inside an explicitly bound scope,
/// on its verified volume, and with the original object's inode unchanged.
pub(crate) fn legacy_identity_matches(path: &Path, device: u64, inode: u64) -> Result<bool> {
    if inode == 0 {
        return Ok(false);
    }
    let current = PersistedIdentity::capture(path)?;
    if current.inode != inode {
        return Ok(false);
    }
    for scope in path.ancestors() {
        let file = scope.join(BINDING);
        match fs::symlink_metadata(&file) {
            Ok(_) => {
                let binding = read_binding(scope)?;
                return Ok(binding.legacy_device == device
                    && current.volume_uuid == binding.workspace_identity.volume_uuid);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) if e.raw_os_error() == Some(libc::ENOTDIR) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(false)
}

pub(crate) fn preview(paths: &RuntimePaths, migration: Option<&str>) -> Result<Value> {
    let id = migration
        .context("Legacy identity repair requires an explicitly selected completed migration")?;
    let root = paths.workspace_root();
    check_no_symlinks(root, false)?;
    for directory in [root.join("projects"), root.join(".platform")] {
        check_no_symlinks(&directory, false)?;
        if !directory.is_dir() {
            bail!("Required workspace directory is missing")
        }
    }
    let expected = crate::workspace::legacy_repair_identity(paths.data_root(), root, id)?;
    let observed = PersistedIdentity::capture(root)?;
    if observed.is_legacy() {
        bail!("Stable volume UUID is unavailable; identity repair cannot run")
    }
    let settings = private_bytes(&paths.data_root().join("settings.json"))?;
    let journal = private_bytes(
        &paths
            .data_root()
            .join("migrations")
            .join(id)
            .join("journal.json"),
    )?;
    let marker = private_bytes(&root.join(".platform/workspace-selection.json"))?;
    let evidence = json!({"schemaVersion":1,"identifier":APP_IDENTIFIER,"environment":ENVIRONMENT,
        "workspaceRoot":root,"workspaceIdentity":observed,"dataRoot":paths.data_root(),
        "dataIdentity":PersistedIdentity::capture(paths.data_root())?,"migrationId":id,
        "legacyIdentity":expected,"originals":{"settings.json":digest(&settings),
        "migration-journal.json":digest(&journal),"workspace-selection.json":digest(&marker)}});
    let plan_id = digest(&serde_json::to_vec(&evidence)?);
    let already_bound = if root.join(BINDING).try_exists()? {
        read_binding(root)?;
        true
    } else {
        false
    };
    Ok(
        json!({"planId":plan_id,"workspaceRoot":root,"volumeUuid":observed.volume_uuid,
        "state":if already_bound {"already_bound"}else{"confirmation_required"},
        "evidence":evidence,"writes":[root.join(BINDING),paths.data_root().join("identity-repairs").join(&plan_id)],
        "preservesOriginalRecords":true,"movesWorkspace":false,"touchesProjectData":false}),
    )
}

/// Called only with the fixed application's exclusive instance lock held.
pub(crate) fn apply(paths: &RuntimePaths, migration: Option<&str>, input: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Input {
        plan_id: String,
        workspace_root: PathBuf,
        volume_uuid: String,
        confirm: bool,
    }
    let input: Input = serde_json::from_value(input)?;
    let plan = preview(paths, migration)?;
    if !input.confirm
        || plan["planId"] != input.plan_id
        || input.workspace_root != paths.workspace_root()
        || plan["volumeUuid"] != input.volume_uuid
    {
        bail!("Identity repair confirmation is stale or does not select this workspace and volume")
    }
    if plan["state"] == "already_bound" {
        return Ok(
            json!({"state":"already_bound","workspaceRoot":paths.workspace_root(),"planId":input.plan_id}),
        );
    }
    let evidence = &plan["evidence"];
    let binding = Binding {
        schema_version: 1,
        identifier: APP_IDENTIFIER.into(),
        environment: ENVIRONMENT.into(),
        workspace_root: paths.workspace_root().into(),
        workspace_identity: serde_json::from_value(evidence["workspaceIdentity"].clone())?,
        data_root: paths.data_root().into(),
        data_identity: serde_json::from_value(evidence["dataIdentity"].clone())?,
        legacy_device: evidence["legacyIdentity"]["device"]
            .as_u64()
            .context("Missing legacy device")?,
        repair_id: input.plan_id.clone(),
    };
    let repairs = paths.data_root().join("identity-repairs");
    private_dir(&repairs)?;
    let receipt_dir = repairs.join(&input.plan_id);
    private_dir(&receipt_dir)?;
    let migration_id = migration.context("Missing migration ID")?;
    let originals = [
        ("settings.json", paths.data_root().join("settings.json")),
        (
            "migration-journal.json",
            paths
                .data_root()
                .join("migrations")
                .join(migration_id)
                .join("journal.json"),
        ),
        (
            "workspace-selection.json",
            paths
                .workspace_root()
                .join(".platform/workspace-selection.json"),
        ),
    ];
    for (name, source) in originals {
        let bytes = private_bytes(&source)?;
        if evidence["originals"][name] != digest(&bytes) {
            bail!("Identity evidence changed before repair")
        }
        publish_new(&receipt_dir, name, &bytes)?;
    }
    let receipt =
        json!({"schemaVersion":1,"binding":binding,"originals":evidence["originals"],"plan":plan});
    publish_new(
        &receipt_dir,
        "receipt.json",
        &serde_json::to_vec_pretty(&receipt)?,
    )?;
    // Recheck every input after writing evidence, before the one commit point.
    if preview(paths, migration)?["planId"] != input.plan_id {
        bail!("Identity repair preview changed before publication")
    }
    publish_new(
        &paths.workspace_root().join(".platform"),
        "identity-binding.json",
        &serde_json::to_vec_pretty(&binding)?,
    )?;
    read_binding(paths.workspace_root())?;
    Ok(
        json!({"state":"bound","workspaceRoot":paths.workspace_root(),"planId":input.plan_id,
        "receiptPath":receipt_dir.join("receipt.json"),"preservesOriginalRecords":true,"touchesProjectData":false}),
    )
}

fn private_dir(path: &Path) -> Result<()> {
    let parent = path.parent().context("Missing repair parent")?;
    let expected = PersistedIdentity::capture(parent)?;
    let dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(parent)?;
    if !expected.same_runtime_object(&PersistedIdentity::from_file(&dir)?) {
        bail!("Repair parent changed while opening")
    }
    let name = CString::new(
        path.file_name()
            .context("Missing repair directory name")?
            .as_encoded_bytes(),
    )?;
    if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(error.into());
        }
    }
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let child = unsafe { File::from_raw_fd(fd) };
    let m = child.metadata()?;
    if !m.is_dir() || m.permissions().mode() & 0o077 != 0 || m.uid() != unsafe { libc::geteuid() } {
        bail!("Identity repair directory must be private")
    }
    if !expected.same_runtime_object(&PersistedIdentity::capture(parent)?)
        || !PersistedIdentity::from_file(&child)?
            .same_runtime_object(&PersistedIdentity::capture(path)?)
    {
        bail!("Repair directory changed during creation")
    }
    dir.sync_all()?;
    Ok(())
}

/// Publish a complete file via a directory fd, never overwrite existing evidence.
/// Repeating an interrupted preparation accepts only byte-identical originals.
fn publish_new(parent: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let parent_identity = PersistedIdentity::capture(parent)?;
    let dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(parent)?;
    if !parent_identity.same_runtime_object(&PersistedIdentity::from_file(&dir)?) {
        bail!("Repair parent changed while opening")
    }
    let temp = CString::new(format!(".identity-{}.tmp", Uuid::new_v4().simple()))?;
    let name_c = CString::new(name)?;
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            temp.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let result = (|| -> Result<()> {
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(bytes)?;
        file.sync_all()?;
        if !parent_identity.same_runtime_object(&PersistedIdentity::capture(parent)?) {
            bail!("Repair parent changed before publication")
        }
        if let Err(error) = publish_exclusive(dir.as_raw_fd(), &temp, &name_c) {
            if error.kind() != std::io::ErrorKind::AlreadyExists
                || private_bytes(&parent.join(name))? != bytes
            {
                return Err(error.into());
            }
        }
        dir.sync_all()?;
        Ok(())
    })();
    unsafe { libc::unlinkat(dir.as_raw_fd(), temp.as_ptr(), 0) };
    dir.sync_all()?;
    result
}

#[cfg(target_os = "macos")]
fn publish_exclusive(
    parent: libc::c_int,
    from: &std::ffi::CStr,
    to: &std::ffi::CStr,
) -> std::io::Result<()> {
    if unsafe {
        libc::renameatx_np(
            parent,
            from.as_ptr(),
            parent,
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
#[cfg(not(target_os = "macos"))]
fn publish_exclusive(
    _: libc::c_int,
    _: &std::ffi::CStr,
    _: &std::ffi::CStr,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Exclusive identity repair publication requires a supported platform implementation",
    ))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::{builds, workspace, CreateProjectInput, ProjectStore};
    use std::{collections::BTreeMap, os::unix::fs::symlink};

    struct Fixture {
        _temporary: tempfile::TempDir,
        paths: RuntimePaths,
        migration: String,
        project_directory: String,
        legacy_device: u64,
    }

    impl Fixture {
        async fn new(legacy: bool) -> Self {
            let (temporary, paths) = RuntimePaths::for_test();
            let store = ProjectStore::new(paths.workspace_root().into()).unwrap();
            let project = store
                .create(CreateProjectInput {
                    name: "Disposable workspace identity repair verification".into(),
                    description: Some("Only this temporary fixture may be changed".into()),
                })
                .await
                .unwrap();
            fs::write(
                project.project_root.join("data/attachments/keep.txt"),
                b"identity repair must preserve this attachment",
            )
            .unwrap();
            // This fixture models retained legacy material. New workspaces no
            // longer create the retired backup directory automatically.
            fs::create_dir_all(paths.workspace_root().join("backups")).unwrap();
            fs::write(
                paths.workspace_root().join("backups/keep.txt"),
                b"retained backup",
            )
            .unwrap();
            let destination = temporary
                .path()
                .canonicalize()
                .unwrap()
                .join("disposable-selected-workspace");
            fs::create_dir(&destination).unwrap();
            store.begin_workspace_migration().unwrap();
            let task = workspace::create(&paths, &destination).unwrap();
            store.pause_workspace().await.unwrap();
            let (selected, mut journal) =
                workspace::copy_and_commit(&paths, &store, &task.task_id).unwrap();
            let selected_store = ProjectStore::new(selected.workspace_root().into()).unwrap();
            workspace::initialize_selected(&selected, &selected_store, &mut journal).unwrap();
            workspace::mark_old_retired(&selected, &journal).unwrap();
            workspace::finish(
                &selected,
                &task.task_id,
                Ok(workspace::completed_result(&journal)),
            )
            .unwrap();
            selected_store.shutdown().await;
            store.shutdown().await;
            let actual = PersistedIdentity::capture(selected.workspace_root()).unwrap();
            let legacy_device = actual.device.wrapping_add(1);
            if legacy {
                let file = selected
                    .migrations_dir()
                    .join(&task.task_id)
                    .join("journal.json");
                let mut value: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
                value["destinationIdentity"] = json!({"device":legacy_device,"inode":actual.inode});
                builds::atomic_json(&file, &value).unwrap();
            }
            Self {
                _temporary: temporary,
                paths: selected,
                migration: task.task_id,
                project_directory: project
                    .project_root
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .into(),
                legacy_device,
            }
        }
        fn preview(&self) -> Value {
            preview(&self.paths, Some(&self.migration)).unwrap()
        }
        fn apply(&self, input: Value) -> Result<Value> {
            apply(&self.paths, Some(&self.migration), input)
        }
        fn project_path(&self) -> PathBuf {
            self.paths
                .workspace_root()
                .join("projects")
                .join(&self.project_directory)
        }
        fn receipt_dir(&self, plan: &Value) -> PathBuf {
            self.paths
                .data_root()
                .join("identity-repairs")
                .join(plan["planId"].as_str().unwrap())
        }
        fn original_paths(&self) -> [PathBuf; 3] {
            [
                self.paths.data_root().join("settings.json"),
                self.paths
                    .migrations_dir()
                    .join(&self.migration)
                    .join("journal.json"),
                self.paths
                    .workspace_root()
                    .join(".platform/workspace-selection.json"),
            ]
        }
        fn legacy_matches(&self) -> Result<bool> {
            let inode = PersistedIdentity::capture(self.paths.workspace_root())?.inode;
            legacy_identity_matches(self.paths.workspace_root(), self.legacy_device, inode)
        }
    }

    fn input(plan: &Value) -> Value {
        json!({"planId":plan["planId"],"workspaceRoot":plan["workspaceRoot"],
            "volumeUuid":plan["volumeUuid"],"confirm":true})
    }
    fn tree_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn collect(root: &Path, current: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(current).unwrap() {
                let path = entry.unwrap().path();
                let metadata = fs::symlink_metadata(&path).unwrap();
                assert!(!metadata.file_type().is_symlink());
                if metadata.is_dir() {
                    collect(root, &path, files);
                } else {
                    files.insert(
                        path.strip_prefix(root).unwrap().into(),
                        fs::read(path).unwrap(),
                    );
                }
            }
        }
        let mut files = BTreeMap::new();
        collect(root, root, &mut files);
        files
    }

    #[tokio::test]
    async fn explicit_repair_preserves_originals_and_project_data_and_is_idempotent() {
        let fixture = Fixture::new(true).await;
        let originals = fixture.original_paths().map(|path| fs::read(path).unwrap());
        let project = tree_files(&fixture.project_path());
        assert!(fixture.paths.reinitialize_for_test().is_err());
        assert!(!fixture.legacy_matches().unwrap());
        let plan = fixture.preview();
        assert_eq!(plan["state"], "confirmation_required");
        assert!(!fixture.paths.workspace_root().join(BINDING).exists());
        assert!(!fixture.paths.data_root().join("identity-repairs").exists());
        let result = fixture.apply(input(&plan)).unwrap();
        assert_eq!(result["state"], "bound");
        assert!(fixture.legacy_matches().unwrap());
        assert!(fixture.paths.reinitialize_for_test().is_ok());
        for (path, bytes) in fixture.original_paths().into_iter().zip(originals) {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        assert_eq!(tree_files(&fixture.project_path()), project);
        assert_eq!(
            fs::read(fixture.paths.workspace_root().join("backups/keep.txt")).unwrap(),
            b"retained backup"
        );
        let binding = fixture.paths.workspace_root().join(BINDING);
        let receipt = fixture.receipt_dir(&plan).join("receipt.json");
        let binding_bytes = fs::read(&binding).unwrap();
        let receipt_bytes = fs::read(&receipt).unwrap();
        assert_eq!(fs::metadata(&binding).unwrap().nlink(), 1);
        assert_eq!(fs::metadata(&receipt).unwrap().nlink(), 1);
        assert_eq!(
            fixture.apply(input(&plan)).unwrap()["state"],
            "already_bound"
        );
        assert_eq!(fs::read(binding).unwrap(), binding_bytes);
        assert_eq!(fs::read(receipt).unwrap(), receipt_bytes);
        let nested = fixture.project_path().join("data/attachments/keep.txt");
        let observed = PersistedIdentity::capture(&nested).unwrap();
        let legacy: PersistedIdentity =
            serde_json::from_value(json!([fixture.legacy_device, observed.inode])).unwrap();
        assert!(legacy.matches_path(&nested).unwrap());
        assert!(!legacy_identity_matches(
            &nested,
            fixture.legacy_device.wrapping_add(1),
            observed.inode
        )
        .unwrap());
        let outside = fixture
            ._temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("outside-scope.txt");
        fs::write(&outside, b"not authorized by workspace binding").unwrap();
        let outside_inode = PersistedIdentity::capture(&outside).unwrap().inode;
        assert!(!legacy_identity_matches(&outside, fixture.legacy_device, outside_inode).unwrap());
        let mut wrong_volume = observed;
        wrong_volume.volume_uuid = Some(Uuid::new_v4().to_string());
        assert!(!wrong_volume.matches_path(&nested).unwrap());
    }

    #[tokio::test]
    async fn stale_or_inexact_confirmation_never_creates_repair_evidence() {
        let fixture = Fixture::new(true).await;
        let plan = fixture.preview();
        let valid = input(&plan);
        for (field, value) in [
            ("confirm", json!(false)),
            ("planId", json!("0".repeat(64))),
            ("volumeUuid", json!(Uuid::new_v4().to_string())),
            ("workspaceRoot", json!(fixture.paths.data_root())),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(fixture.apply(invalid).is_err());
        }
        let settings = fixture.paths.data_root().join("settings.json");
        let mut changed: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
        changed["lastPort"] = json!(49876);
        builds::atomic_json(&settings, &changed).unwrap();
        assert!(fixture.apply(valid).is_err());
        assert!(!fixture.paths.workspace_root().join(BINDING).exists());
        assert!(!fixture.paths.data_root().join("identity-repairs").exists());
        assert!(preview(&fixture.paths, None).is_err());
    }

    #[tokio::test]
    async fn stable_records_replaced_roots_and_symlinked_receipt_directories_fail_closed() {
        let stable = Fixture::new(false).await;
        let error = preview(&stable.paths, Some(&stable.migration))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Stable workspace identity cannot be rebound"),
            "{error}"
        );
        let fixture = Fixture::new(true).await;
        let plan = fixture.preview();
        let root = fixture.paths.workspace_root();
        let retained = root.with_file_name("retained-original-identity-fixture");
        fs::rename(root, &retained).unwrap();
        fs::create_dir(root).unwrap();
        for name in ["projects", "backups", ".platform"] {
            fs::create_dir(root.join(name)).unwrap();
        }
        fs::copy(
            retained.join(".platform/workspace-selection.json"),
            root.join(".platform/workspace-selection.json"),
        )
        .unwrap();
        assert!(fixture.apply(input(&plan)).is_err());
        assert!(!retained.join(BINDING).exists());
        assert!(!root.join(BINDING).exists());
        assert_eq!(
            fs::read(
                retained
                    .join("projects")
                    .join(&fixture.project_directory)
                    .join("data/attachments/keep.txt")
            )
            .unwrap(),
            b"identity repair must preserve this attachment"
        );
        let fixture = Fixture::new(true).await;
        let plan = fixture.preview();
        let outside = fixture
            ._temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("outside-repair-target");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, fixture.paths.data_root().join("identity-repairs")).unwrap();
        assert!(fixture.apply(input(&plan)).is_err());
        assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
        assert!(!fixture.paths.workspace_root().join(BINDING).exists());
    }

    #[tokio::test]
    async fn receipt_and_original_evidence_tampering_revokes_legacy_compatibility() {
        let fixture = Fixture::new(true).await;
        let plan = fixture.preview();
        fixture.apply(input(&plan)).unwrap();
        let file = fixture.receipt_dir(&plan).join("receipt.json");
        let original: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        for pointer in [
            "/binding/legacyDevice",
            "/originals/settings.json",
            "/plan/planId",
            "/plan/evidence/legacyIdentity/device",
        ] {
            let mut changed = original.clone();
            *changed.pointer_mut(pointer).unwrap() = json!("tampered");
            builds::atomic_json(&file, &changed).unwrap();
            assert!(
                fixture.legacy_matches().is_err(),
                "tampering accepted at {pointer}"
            );
            builds::atomic_json(&file, &original).unwrap();
            assert!(fixture.legacy_matches().unwrap());
        }
        let original_copy = fixture.receipt_dir(&plan).join("settings.json");
        let bytes = fs::read(&original_copy).unwrap();
        fs::write(&original_copy, b"changed evidence").unwrap();
        assert!(fixture.legacy_matches().is_err());
        fs::write(&original_copy, bytes).unwrap();
        assert!(fixture.legacy_matches().unwrap());
        let binding = fixture.paths.workspace_root().join(BINDING);
        let mut changed: Value = serde_json::from_slice(&fs::read(&binding).unwrap()).unwrap();
        changed["workspaceIdentity"]["volumeUuid"] = json!(Uuid::new_v4().to_string());
        builds::atomic_json(&binding, &changed).unwrap();
        assert!(fixture.legacy_matches().is_err());
    }

    #[tokio::test]
    async fn interrupted_preparation_reuses_only_identical_original_evidence() {
        let fixture = Fixture::new(true).await;
        let plan = fixture.preview();
        private_dir(&fixture.paths.data_root().join("identity-repairs")).unwrap();
        let receipt_dir = fixture.receipt_dir(&plan);
        private_dir(&receipt_dir).unwrap();
        let settings = private_bytes(&fixture.paths.data_root().join("settings.json")).unwrap();
        publish_new(&receipt_dir, "settings.json", &settings).unwrap();
        publish_new(
            &receipt_dir,
            ".identity-interrupted.tmp",
            b"incomplete unrelated temporary file",
        )
        .unwrap();
        assert_eq!(fixture.apply(input(&plan)).unwrap()["state"], "bound");
        assert!(fixture.legacy_matches().unwrap());
        let fixture = Fixture::new(true).await;
        let plan = fixture.preview();
        private_dir(&fixture.paths.data_root().join("identity-repairs")).unwrap();
        let receipt_dir = fixture.receipt_dir(&plan);
        private_dir(&receipt_dir).unwrap();
        publish_new(
            &receipt_dir,
            "settings.json",
            b"conflicting original evidence",
        )
        .unwrap();
        assert!(fixture.apply(input(&plan)).is_err());
        assert_eq!(
            fs::read(receipt_dir.join("settings.json")).unwrap(),
            b"conflicting original evidence"
        );
        assert!(!fixture.paths.workspace_root().join(BINDING).exists());
    }
}
