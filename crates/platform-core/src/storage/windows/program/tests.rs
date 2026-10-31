use super::*;
use std::time::Duration;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    manager: PathBuf,
    cli: PathBuf,
    resources: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .prefix("panda-disposable-program-accounting-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let manager = root.join("panda-drawer.exe");
        let cli = root.join("local-projects-dev.exe");
        let resources = root.join("resources");
        fs::write(&manager, b"manager").unwrap();
        fs::write(&cli, b"cli").unwrap();
        fs::create_dir(&resources).unwrap();
        fs::create_dir(root.join("unselected-cargo-deps")).unwrap();
        fs::write(root.join("unselected-cargo-deps/large-pdb"), vec![0; 10000]).unwrap();
        fs::write(resources.join("resource"), b"resource").unwrap();
        Self {
            _temp: temp,
            root,
            manager,
            cli,
            resources,
        }
    }
    fn scope(&self) -> Arc<InstallationScope> {
        InstallationScope::for_test(&self.manager, &self.cli, &self.resources)
    }
}

#[test]
fn exact_three_members_exclude_the_executable_parent_and_bind_cache_stamps() {
    let fixture = Fixture::new();
    let result = scan(&fixture.scope(), &[]);
    assert_eq!(result["totalBytes"], 18);
    assert_eq!(result["fileCount"], 3);
    assert_eq!(result["complete"], true, "{result}");
    assert!(result["root"].is_null());
    let binding = &result["installationBinding"];
    assert_eq!(binding["members"].as_array().unwrap().len(), 3);
    assert_eq!(binding["observedBefore"], binding["observedAfter"]);
    assert_eq!(
        binding["observedBefore"][0]["fileId"]
            .as_array()
            .unwrap()
            .len(),
        16
    );
    assert_eq!(binding["version"], crate::APP_VERSION);
    assert_eq!(binding["target"], crate::build_profile::BUILD_TARGET);
    assert_eq!(
        fs::metadata(fixture.root.join("unselected-cargo-deps/large-pdb"))
            .unwrap()
            .len(),
        10000
    );
}

#[test]
fn selected_cli_inside_resources_is_counted_once_and_long_unicode_is_counted() {
    let fixture = Fixture::new();
    let nested = fixture.resources.join("中".repeat(86));
    fs::create_dir(&nested).unwrap();
    let cli = nested.join(format!("{}.exe", "文".repeat(86)));
    fs::write(&cli, b"cli").unwrap();
    let scope = InstallationScope::for_test(&fixture.manager, &cli, &fixture.resources);
    let result = scan(&scope, &[]);
    assert_eq!(result["complete"], true, "{result}");
    assert_eq!(result["totalBytes"], 18);
    assert_eq!(result["fileCount"], 3);
}

#[test]
fn same_length_resource_write_between_passes_never_reports_complete() {
    let fixture = Fixture::new();
    let scope = fixture.scope();
    let file = fixture.resources.join("resource");
    let before = fs::metadata(&file).unwrap().modified().unwrap();
    let result = scan_with(&scope, &[], || {
        fs::write(&file, b"RESOURCE").unwrap();
        File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(before + Duration::from_nanos(100))
            .unwrap();
    });
    assert_eq!(result["complete"], false, "{result}");
    assert!(result["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["reason"] == "changed_during_scan"));
    // A subsequent fresh scan measures the new stable tree, never an old cache.
    let updated = scan(&scope, &[]);
    assert_eq!(updated["complete"], true, "{updated}");
    assert_ne!(
        result["installationBinding"]["resourcesSignature"],
        updated["installationBinding"]["resourcesSignature"]
    );
}

#[test]
fn executable_replacement_and_resource_root_replacement_remain_unknown() {
    let fixture = Fixture::new();
    let scope = fixture.scope();
    fs::rename(&fixture.cli, fixture.root.join("old-cli")).unwrap();
    fs::write(&fixture.cli, b"cli").unwrap();
    let result = scan(&scope, &[]);
    assert_eq!(result["complete"], false);
    assert_eq!(result["totalBytes"], 15);
    assert!(result["installationBinding"]["observedBefore"][1].is_null());
    fs::rename(&fixture.resources, fixture.root.join("old-resources")).unwrap();
    fs::create_dir(&fixture.resources).unwrap();
    fs::write(fixture.resources.join("foreign"), vec![0; 10000]).unwrap();
    let result = scan(&scope, &[]);
    assert_eq!(result["complete"], false);
    assert_eq!(result["totalBytes"], 7);
    assert!(result["installationBinding"]["observedBefore"][2].is_null());
    assert_eq!(
        fs::metadata(fixture.resources.join("foreign"))
            .unwrap()
            .len(),
        10000
    );
}

#[test]
fn ordinary_read_only_hard_linked_resources_add_no_private_acl_or_link_gate() {
    let fixture = Fixture::new();
    let outside = fixture.root.join("external-original");
    fs::write(&outside, b"outside").unwrap();
    fs::hard_link(&outside, fixture.resources.join("linked-resource")).unwrap();
    let mut permissions = fs::metadata(&outside).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&outside, permissions).unwrap();
    let result = scan(&fixture.scope(), &[]);
    assert_eq!(result["complete"], true, "{result}");
    assert_eq!(result["totalBytes"], 25);
    assert!(fs::metadata(&outside).unwrap().permissions().readonly());
    assert_eq!(fs::read(&outside).unwrap(), b"outside");
    platform_native::windows::files::set_read_only(&outside, false).unwrap();
}

#[test]
fn headless_without_declaration_keeps_program_unknown_instead_of_zero() {
    let (_temp, paths) = RuntimePaths::for_test();
    let _store = ProjectStore::new(paths.workspace_root().into()).unwrap();
    let result = workspace_scan(&paths, None, "headless-test").unwrap();
    assert!(result["platformProgram"]["totalBytes"].is_null());
    assert_eq!(result["platformProgram"]["complete"], false);
    assert_eq!(result["knownLocalTotal"]["complete"], false);
    assert!(!paths
        .data_root()
        .join("cache/storage/platform.json")
        .exists());
}

#[test]
fn exact_members_within_application_data_are_excluded_from_that_aggregate() {
    let (_temp, paths) = RuntimePaths::for_test();
    let _store = ProjectStore::new(paths.workspace_root().into()).unwrap();
    let root = paths.data_root().join("disposable-installed-program");
    fs::create_dir(&root).unwrap();
    let manager = root.join("panda-drawer.exe");
    let cli = root.join("local-projects-dev.exe");
    let resources = root.join("resources");
    fs::write(&manager, b"manager").unwrap();
    fs::write(&cli, b"cli").unwrap();
    fs::create_dir(&resources).unwrap();
    fs::write(resources.join("resource"), b"resource").unwrap();
    fs::write(root.join("unclaimed-neighbor"), b"neighbor").unwrap();
    let scope = InstallationScope::for_test(&manager, &cli, &resources);
    let result = workspace_scan(&paths, Some(&scope), "selected-test").unwrap();
    assert_eq!(result["platformProgram"]["totalBytes"], 18);
    let data = result["otherLocalLocations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|scope| scope["scope"]["kind"] == "applicationData")
        .unwrap();
    assert!(data["excludedRoots"]
        .as_array()
        .unwrap()
        .contains(&json!(
            PathBuf::from("disposable-installed-program").join("resources")
        )));
    assert!(numeric(&data["categories"]["applicationData"]["bytes"]).unwrap() >= 8);
    assert_eq!(data["complete"], true, "{data}");
    let cached: Value = read_json(&paths.data_root().join("cache/storage/platform.json")).unwrap();
    assert_eq!(
        cached["installationBinding"],
        result["platformProgram"]["installationBinding"]
    );
    assert_eq!(cached["target"], crate::build_profile::BUILD_TARGET);
}

#[test]
fn selected_hard_link_paths_keep_logical_file_byte_semantics() {
    let fixture = Fixture::new();
    fs::remove_file(&fixture.cli).unwrap();
    fs::hard_link(&fixture.manager, &fixture.cli).unwrap();
    let result = scan(&fixture.scope(), &[]);
    assert_eq!(result["complete"], true, "{result}");
    assert_eq!(result["totalBytes"], 22);
    assert_eq!(result["fileCount"], 3);
}
