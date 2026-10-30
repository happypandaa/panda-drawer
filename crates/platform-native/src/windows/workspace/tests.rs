use super::*;
#[test]
fn native_component_comparison_preserves_windows_utf16_case_and_boundaries() {
    assert!(within(
        Path::new(r"C:\Users\测试\Root\project"),
        Path::new(r"c:\USERS\测试\root")
    )
    .unwrap());
    assert!(within(Path::new(r"\\?\C:\Root\project"), Path::new(r"C:\root")).unwrap());
    assert!(!within(Path::new(r"C:\Root-extra\project"), Path::new(r"C:\root")).unwrap());
    assert!(!within(Path::new(r"D:\Root\project"), Path::new(r"C:\root")).unwrap());
}

#[test]
fn registered_root_alias_is_resolved_to_its_actual_volume_and_unknown_is_not_clear() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-registered-root-alias-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let target = root.join("actual-root");
    let selected = target.join("selected");
    fs::create_dir_all(&selected).unwrap();
    let alias = root.join("registered-root-junction");
    let status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&alias)
        .arg(&target)
        .status()
        .unwrap();
    assert!(status.success(), "Could not create disposable root alias");
    let selected_scope = files::pin_directory(&selected).unwrap();
    assert!(
        require_outside_registered_roots(&selected_scope.file, std::slice::from_ref(&alias))
            .unwrap_err()
            .to_string()
            .contains("inside a system registered")
    );
    assert!(require_outside_registered_roots(
        &selected_scope.file,
        &[root.join("unknown-registered-root")]
    )
    .unwrap_err()
    .to_string()
    .contains("unconfirmed"));
    require_outside_registered_roots(&selected_scope.file, &[root.join("actual-root-extra")])
        .unwrap_err();
    fs::create_dir(root.join("actual-root-extra")).unwrap();
    require_outside_registered_roots(&selected_scope.file, &[root.join("actual-root-extra")])
        .unwrap();
    drop(selected_scope);
    fs::remove_dir(alias).unwrap();
}
#[test]
fn actual_disposable_directory_sync_and_volume_queries_do_not_read_or_change_content() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-workspace-admission-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let before = files::open_no_reparse(&root).unwrap().identity;
    fs::write(root.join("retained"), b"retain disposable content").unwrap();
    let cloud = cloud_membership(&files::open_no_reparse(&root).unwrap().file).unwrap();
    eprintln!("Actual disposable Cloud Filter membership: {cloud:?}");
    let roots = registered_roots_on_worker().unwrap();
    eprintln!("System registered sync-root count: {}", roots.len());
    require_unsynced(&root).unwrap();
    let info = volume::inspect(&root).unwrap();
    eprintln!("Actual workspace volume: {info:?}");
    if info.filesystem.eq_ignore_ascii_case("NTFS")
        && info.local() == Some(true)
        && info.internal == Some(true)
    {
        require_workspace_volume(&root.join("missing").join("workspace")).unwrap();
    }
    assert!(!root.join("missing").exists());
    assert!(before.same_object(&files::open_no_reparse(&root).unwrap().identity));
    assert_eq!(
        fs::read(root.join("retained")).unwrap(),
        b"retain disposable content"
    );
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
}

#[test]
fn volume_policy_never_promotes_fixed_unknown_or_external_media_to_internal() {
    let mut info = volume::VolumeInfo {
        filesystem: "NTFS".into(),
        volume_serial: 7,
        available_bytes: 0,
        total_bytes: 0,
        drive_type: 3,
        internal: Some(true),
        internal_disk_numbers: vec![0],
        internal_diagnostic: None,
    };
    require_local_internal_ntfs(&info).unwrap();
    info.internal = None;
    info.internal_diagnostic = Some("disposable missing PnP observation".into());
    assert!(require_local_internal_ntfs(&info)
        .unwrap_err()
        .to_string()
        .contains("missing PnP observation"));
    info.internal = Some(false);
    assert!(require_local_internal_ntfs(&info)
        .unwrap_err()
        .to_string()
        .contains("external"));
    info.internal = Some(true);
    info.drive_type = 4;
    assert!(require_local_internal_ntfs(&info).is_err());
    info.drive_type = 3;
    info.filesystem = "exFAT".into();
    assert!(require_local_internal_ntfs(&info).is_err());
}

struct DisposableSyncRoot {
    // Drop the original root pin before TempDir attempts directory removal.
    root_scope: files::PinnedDirectory,
    root: PathBuf,
    expected: files::FileIdentity,
    registered: bool,
    temporary: Option<tempfile::TempDir>,
}
impl DisposableSyncRoot {
    fn close(&mut self) -> io::Result<()> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::{
            CloudFilters::CfUnregisterSyncRoot,
            FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
                FILE_SHARE_WRITE,
            },
        };
        if !self.registered {
            return Ok(());
        }
        // The test registered this exact new directory only. Hold the
        // current raw reparse-aware handle against rename before undoing
        // that registration; never unregister a same-name replacement.
        let pinned = File::options()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&self.root)?;
        if !self.expected.same_object(&files::identity(&pinned)?) {
            return Err(unknown(
                "Disposable sync-root directory identity changed; registration retained",
            ));
        }
        let path = native_path(&self.root)?;
        let result = unsafe { CfUnregisterSyncRoot(path.as_ptr()) };
        if result < 0 {
            return Err(unknown(format!(
                "Disposable sync-root unregister failed: HRESULT 0x{:08x}",
                result as u32
            )));
        }
        if cloud_membership(&pinned)? != CloudMembership::NotRegistered {
            return Err(unknown(
                "Disposable sync-root unregister result is unconfirmed",
            ));
        }
        self.registered = false;
        Ok(())
    }
}
impl Drop for DisposableSyncRoot {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            if let Some(temporary) = self.temporary.take() {
                let retained = temporary.keep();
                eprintln!(
                    "DISPOSABLE SYNC REGISTRATION RETAINED at {} (root identity {:?}): {error}",
                    retained.display(),
                    self.expected
                );
            }
        }
    }
}

#[test]
fn actual_new_disposable_sync_registration_is_detected_and_confirmed_unregistered() {
    use windows_sys::Win32::Storage::CloudFilters::{
        CfRegisterSyncRoot, CF_HYDRATION_POLICY, CF_HYDRATION_POLICY_ALWAYS_FULL,
        CF_POPULATION_POLICY, CF_POPULATION_POLICY_ALWAYS_FULL, CF_REGISTER_FLAG_NONE,
        CF_SYNC_POLICIES, CF_SYNC_REGISTRATION,
    };
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-cloud-sync-admission-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let selected = files::pin_directory(&root).unwrap();
    let expected = files::identity(&selected.file).unwrap();
    // Never alter a provider that was already present around this new
    // disposable directory, including a registered ancestor of TEMP.
    assert_eq!(
        cloud_membership(&selected.file).unwrap(),
        CloudMembership::NotRegistered
    );
    require_unsynced(&root).unwrap();
    eprintln!(
        "Registering only disposable Cloud Files root {} with exact original identity {expected:?}",
        root.display()
    );
    let identity_bytes = expected.file_id;
    let name = super::super::wide(std::ffi::OsStr::new(
        "Panda Drawer disposable admission test",
    ))
    .unwrap();
    let version = super::super::wide(std::ffi::OsStr::new("1.0")).unwrap();
    let registration = CF_SYNC_REGISTRATION {
        StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
        ProviderName: name.as_ptr(),
        ProviderVersion: version.as_ptr(),
        SyncRootIdentity: identity_bytes.as_ptr().cast(),
        SyncRootIdentityLength: identity_bytes.len() as u32,
        ProviderId: windows_sys::core::GUID::from_u128(0x6ebd8c3b_6a4e_469d_814d_543b1e563a2f),
        ..Default::default()
    };
    let policies = CF_SYNC_POLICIES {
        StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
        Hydration: CF_HYDRATION_POLICY {
            Primary: CF_HYDRATION_POLICY_ALWAYS_FULL,
            ..Default::default()
        },
        Population: CF_POPULATION_POLICY {
            Primary: CF_POPULATION_POLICY_ALWAYS_FULL,
            ..Default::default()
        },
        ..Default::default()
    };
    let path = native_path(&root).unwrap();
    let mut scope = DisposableSyncRoot {
        root_scope: selected,
        temporary: Some(temporary),
        root: root.clone(),
        expected,
        registered: false,
    };
    let result = unsafe {
        CfRegisterSyncRoot(
            path.as_ptr(),
            &registration,
            &policies,
            CF_REGISTER_FLAG_NONE,
        )
    };
    if result < 0 {
        // Even a failed registration is checked before the disposable tree
        // can be removed, so an unexpected partial result stays explicit.
        let state = cloud_membership(&scope.root_scope.file);
        if !matches!(state, Ok(CloudMembership::NotRegistered)) {
            // A failed native call does not grant ownership of a newly
            // observed provider. Retain this exact path instead of
            // unregistering an outcome that could belong to another actor.
            let retained = scope.temporary.take().unwrap().keep();
            eprintln!("DISPOSABLE SYNC REGISTRATION OUTCOME UNCONFIRMED at {} (original identity {:?}): {state:?}",retained.display(),scope.expected);
        }
        panic!(
            "Disposable CfRegisterSyncRoot failed: HRESULT 0x{:08x}",
            result as u32
        );
    }
    scope.registered = true;
    assert!(matches!(
        cloud_membership(&scope.root_scope.file).unwrap(),
        CloudMembership::Registered { .. }
    ));
    assert!(require_unsynced(&root).is_err());
    // Initial/reselected workspace admission checks volume only, matching the
    // Mac entry-point policy. Sync refusal belongs to migration targets.
    require_workspace_volume(&root).unwrap();
    // Full ordinary children are rejected too; no placeholder hydration or
    // provider callback/network activity is needed to recognize the root.
    let child = root.join("ordinary-full-directory");
    fs::create_dir(&child).unwrap();
    assert!(matches!(
        cloud_membership(&files::open_no_reparse(&child).unwrap().file).unwrap(),
        CloudMembership::Registered { .. }
    ));
    assert!(require_unsynced(&child).is_err());
    scope.close().unwrap();
    assert_eq!(
        cloud_membership(&scope.root_scope.file).unwrap(),
        CloudMembership::NotRegistered
    );
    assert!(scope
        .expected
        .same_object(&files::identity(&scope.root_scope.file).unwrap()));
    eprintln!(
        "Confirmed disposable Cloud Files root unregistered: {}",
        root.display()
    );
}

#[test]
fn sync_inventory_uses_its_own_apartment_when_the_caller_is_sta() {
    use windows::Win32::System::WinRT::RO_INIT_SINGLETHREADED;
    unsafe { RoInitialize(RO_INIT_SINGLETHREADED) }.unwrap();
    struct Sta;
    impl Drop for Sta {
        fn drop(&mut self) {
            unsafe { RoUninitialize() };
        }
    }
    let _sta = Sta;
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-sta-sync-query-")
        .tempdir()
        .unwrap();
    require_unsynced(temporary.path()).unwrap();
}
