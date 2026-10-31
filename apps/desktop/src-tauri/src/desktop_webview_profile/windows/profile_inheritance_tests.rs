//! Actual ACL inheritance on new disposable profiles; no engine is launched.
use super::*;
use std::{fs, process::Command};

fn powershell(paths: &[&Path], script: &str) -> Vec<String> {
    let paths: Vec<_> = paths
        .iter()
        .map(|path| {
            path.to_str()
                .unwrap()
                .strip_prefix(r"\\?\")
                .unwrap_or(path.to_str().unwrap())
        })
        .collect();
    let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
    let result = Command::new(system.join("System32/WindowsPowerShell/v1.0/powershell.exe"))
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env(
            "PANDA_DISPOSABLE_MANAGER_ACL_PATHS",
            serde_json::to_string(&paths).unwrap(),
        )
        .env_remove("PSModulePath")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn security(paths: &[&Path]) -> Vec<String> {
    powershell(paths, "$ErrorActionPreference='Stop'; foreach ($p in (ConvertFrom-Json $env:PANDA_DISPOSABLE_MANAGER_ACL_PATHS)) { [Convert]::ToBase64String((Get-Acl -LiteralPath $p).GetSecurityDescriptorBinaryForm()) }")
}

fn appcontainer_only_descendant(cache: &Path, child: &Path) {
    // This is a real unprotected capability-only DACL, not a simulated I/O
    // error or an ACL copied from any application data. The fixture's full-
    // trust test process does not possess this WebView capability SID.
    powershell(
        &[cache, child],
        r#"
$ErrorActionPreference='Stop'
$paths=ConvertFrom-Json $env:PANDA_DISPOSABLE_MANAGER_ACL_PATHS
$user=[System.Security.Principal.WindowsIdentity]::GetCurrent().User
$cap='S-1-15-3-1024-101-102-103-104-105-106-107-108'
$parent=New-Object System.Security.AccessControl.DirectorySecurity
$parent.SetSecurityDescriptorSddlForm(('O:'+ $user.Value +'D:AI(A;;FA;;;'+ $user.Value +')(A;OICIIO;0x1301bf;;;'+ $cap +')'))
Set-Acl -LiteralPath $paths[0] -AclObject $parent
$child=New-Object System.Security.AccessControl.DirectorySecurity
$child.SetSecurityDescriptorSddlForm(('O:'+ $user.Value +'D:AI(A;OICIID;0x1301bf;;;'+ $cap +')'))
Set-Acl -LiteralPath $paths[1] -AclObject $child
"#,
    );
}

fn identity(path: &Path) -> native::FileIdentity {
    native::open_no_reparse(path).unwrap().identity
}

#[test]
fn legacy_private_manager_root_prepares_real_capability_only_children_without_changing_owned_identity_or_external_security(
) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-manager-legacy-acl-")
        .tempdir()
        .unwrap();
    let parent = temporary.path().canonicalize().unwrap();
    let root = parent.join("webview-manager");
    fs::create_dir(&root).unwrap();
    native::make_directory_private(&root).unwrap(); // Original protected flags=0 policy.
    let cache = root.join("EBWebView/Default/Cache");
    let child = cache.join("Cache_Data");
    fs::create_dir_all(&child).unwrap();
    let payload = child.join("disposable-cache-content");
    fs::write(&payload, b"unchanged owned browser cache fixture").unwrap();
    let protected = root.join("explicit-protected-child");
    native::create_private_directory(&protected).unwrap();
    let credential = protected.join("explicit-private-file");
    drop(native::create_private_file(&credential).unwrap());
    fs::write(&credential, b"protected fixture bytes stay unchanged").unwrap();
    let sibling = parent.join("external-sibling");
    fs::create_dir(&sibling).unwrap();
    let (root_id, child_id, payload_id) = (identity(&root), identity(&child), identity(&payload));
    let (protected_id, credential_id) = (identity(&protected), identity(&credential));
    let original_security = security(&[&parent, &sibling, &protected, &credential]);
    appcontainer_only_descendant(&cache, &child);
    assert_eq!(
        native::open_no_reparse(&child)
            .err()
            .unwrap()
            .raw_os_error(),
        Some(5)
    );
    let profile = ManagerProfile::initialize(&parent).unwrap();
    profile.verify().unwrap();
    assert!(root_id.same_object(&identity(&root)));
    assert!(child_id.same_object(&identity(&child)));
    assert!(payload_id.same_object(&identity(&payload)));
    assert_eq!(
        fs::read(&payload).unwrap(),
        b"unchanged owned browser cache fixture"
    );
    assert!(protected_id.same_object(&identity(&protected)));
    assert!(credential_id.same_object(&identity(&credential)));
    assert_eq!(
        fs::read(&credential).unwrap(),
        b"protected fixture bytes stay unchanged"
    );
    assert_eq!(
        security(&[&parent, &sibling, &protected, &credential]),
        original_security
    );
    let prepared_security = security(&[&root, &child, &payload]);
    let reopened = ManagerProfile::initialize(&parent).unwrap();
    assert_eq!(security(&[&root, &child, &payload]), prepared_security);
    reopened.release_after_native_close().unwrap();
    profile.release_after_native_close().unwrap();
    // No browser has existed in this fixture. Release both actual profile pin
    // owners, then exercise the real owned-content removal implementation.
    super::super::purge::clear_contents(&root, &root_id).unwrap();
    assert!(root_id.same_object(&identity(&root)));
    assert!(fs::read_dir(&root).unwrap().next().is_none());
    assert_eq!(security(&[&parent, &sibling]), original_security[..2]);
}

#[test]
fn fresh_manager_root_keeps_user_inheritance_and_repeat_initialization_is_read_only() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-manager-fresh-acl-")
        .tempdir()
        .unwrap();
    let parent = temporary.path().canonicalize().unwrap();
    let before = security(&[&parent]);
    let profile = ManagerProfile::initialize(&parent).unwrap();
    let root = profile.directory();
    let child = root.join("ordinary-engine-directory");
    fs::create_dir(&child).unwrap();
    let payload = child.join("ordinary-engine-file");
    fs::write(&payload, b"fresh browser fixture content").unwrap();
    native::verify_current_user_owner(&native::open_no_reparse(&payload).unwrap().file).unwrap();
    let observed = security(&[root, &child, &payload]);
    let reopened = ManagerProfile::initialize(&parent).unwrap();
    assert!(profile
        .original_identity()
        .same_object(reopened.original_identity()));
    assert_eq!(security(&[root, &child, &payload]), observed);
    assert_eq!(security(&[&parent]), before);
    assert_eq!(fs::read(payload).unwrap(), b"fresh browser fixture content");
}
