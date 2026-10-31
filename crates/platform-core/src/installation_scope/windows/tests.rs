use super::*;

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
            .prefix("panda-disposable-program-scope-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let manager = root.join("panda-drawer.exe");
        let cli = root.join("local-projects-dev.exe");
        let resources = root.join("resources");
        fs::write(&manager, b"selected manager; never execute").unwrap();
        fs::write(&cli, b"selected CLI; never execute").unwrap();
        fs::create_dir(&resources).unwrap();
        Self {
            _temp: temp,
            root,
            manager,
            cli,
            resources,
        }
    }
    fn scope(&self) -> Scope {
        Scope::capture(&self.manager, &self.cli, &self.resources)
    }
}

#[test]
fn selected_scope_protects_only_resources_and_exact_executables_in_both_path_forms() {
    let fixture = Fixture::new();
    let scope = fixture.scope();
    fs::create_dir(fixture.resources.join("nested")).unwrap();
    for target in [
        fixture.resources.join("nested/export.zip"),
        fixture.manager.clone(),
        fixture.cli.clone(),
    ] {
        assert_eq!(
            scope.require_destination(&target).unwrap_err().code,
            "INSTALLATION_DESTINATION_PROTECTED"
        );
        let dos = target
            .to_string_lossy()
            .strip_prefix(r"\\?\")
            .unwrap()
            .to_owned();
        assert_eq!(
            scope.require_destination(Path::new(&dos)).unwrap_err().code,
            "INSTALLATION_DESTINATION_PROTECTED"
        );
    }
    scope
        .require_destination(&fixture.root.join("completed-external-delivery.zip"))
        .unwrap();
    fs::create_dir(fixture.root.join("unrelated-cargo-output")).unwrap();
    scope
        .require_destination(&fixture.root.join("unrelated-cargo-output/result.zip"))
        .unwrap();
    let mut info = json!({});
    scope.add_platform_info(&mut info);
    assert_eq!(info["installationPath"], json!(fixture.root));
    assert_eq!(info["installationScope"]["wholeDirectory"], false);
    assert_eq!(
        info["installationScope"]["members"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn original_identity_protects_hard_link_alias_without_claiming_the_alias_parent() {
    let fixture = Fixture::new();
    let alias_root = fixture.root.join("external-author-directory");
    fs::create_dir(&alias_root).unwrap();
    let alias = alias_root.join("tool-alias.exe");
    fs::hard_link(&fixture.cli, &alias).unwrap();
    let scope = fixture.scope();
    assert!(scope.members[1].observe().is_ok());
    assert_eq!(
        scope.require_destination(&alias).unwrap_err().code,
        "INSTALLATION_DESTINATION_PROTECTED"
    );
    scope
        .require_destination(&alias_root.join("saved.zip"))
        .unwrap();
    assert_eq!(fs::read(&alias).unwrap(), b"selected CLI; never execute");
}

#[test]
fn binding_failure_is_accounting_unknown_and_full_128_bits_are_required() {
    let fixture = Fixture::new();
    fs::remove_file(&fixture.cli).unwrap();
    let scope = fixture.scope();
    assert!(scope.members[1].original.is_none());
    assert_eq!(
        scope.members[1].observe().unwrap_err().code,
        "INSTALLATION_MEMBER_UNBOUND"
    );
    // A missing selected member does not broaden protection to its parent.
    scope
        .require_destination(&fixture.root.join("unrelated.zip"))
        .unwrap();
    let actual = Stamp::capture(&fixture.manager).unwrap();
    let mut wrong = actual.clone();
    wrong.file_id[15] ^= 0x80;
    assert!(!actual.same_object(&wrong));
    wrong = actual.clone();
    wrong.volume_serial ^= 1;
    assert!(!actual.same_object(&wrong));
}

#[test]
fn legitimate_long_unicode_members_use_native_component_lengths() {
    let fixture = Fixture::new();
    let name = "中".repeat(86);
    let nested = fixture.resources.join(&name);
    fs::create_dir(&nested).unwrap();
    let cli = nested.join(format!("{name}.exe"));
    fs::write(&cli, b"unicode CLI marker").unwrap();
    let scope = Scope::capture(&fixture.manager, &cli, &fixture.resources);
    assert!(scope.members.iter().all(|member| member.observe().is_ok()));
    assert_eq!(
        scope
            .require_destination(&nested.join("export.zip"))
            .unwrap_err()
            .code,
        "INSTALLATION_DESTINATION_PROTECTED"
    );
}
