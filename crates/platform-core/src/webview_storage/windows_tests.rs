use super::*;
use std::fs;

const A: &str = "prj_disposable_profile_a";
const B: &str = "prj_disposable_profile_b";
fn fixture() -> (tempfile::TempDir, StoreLedger) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-bound-webview-中文 space-")
        .tempdir()
        .unwrap();
    let ledger = StoreLedger::new(temporary.path().canonicalize().unwrap());
    (temporary, ledger)
}
#[test]
fn only_a_durable_registered_root_can_bind_without_creating_material() {
    let (_temporary, ledger) = fixture();
    assert!(ledger.bind_profile(ProjectKind::Web, A).is_err());
    assert!(!ledger.directory().exists());
    fs::create_dir(ledger.directory()).unwrap();
    fs::create_dir(ledger.project_directory(ProjectKind::Web, A).unwrap()).unwrap();
    let sentinel = ledger
        .project_directory(ProjectKind::Web, A)
        .unwrap()
        .join("unregistered");
    fs::write(&sentinel, b"retain unknown profile").unwrap();
    assert!(ledger.bind_profile(ProjectKind::Web, A).is_err());
    assert!(!ledger.path(ProjectKind::Web, A).unwrap().exists());
    assert_eq!(fs::read(sentinel).unwrap(), b"retain unknown profile");
}
#[test]
fn two_real_registered_profiles_remain_distinct_private_and_pinned() {
    let (_temporary, ledger) = fixture();
    let aid = ledger.record(ProjectKind::Web, A).unwrap();
    let bid = ledger.record(ProjectKind::Web, B).unwrap();
    let a = ledger.bind_profile(ProjectKind::Web, A).unwrap();
    let b = ledger.bind_profile(ProjectKind::Web, B).unwrap();
    assert_eq!(
        (a.kind(), a.id(), a.identifier()),
        (ProjectKind::Web, A, aid)
    );
    assert_eq!(b.identifier(), bid);
    assert_ne!(a.identifier(), b.identifier());
    assert_ne!(a.directory(), b.directory());
    a.verify_actual_directory(a.directory()).unwrap();
    assert_eq!(
        a.verify_actual_directory(b.directory()).unwrap_err().code,
        "WEBVIEW_PROFILE_ACTUAL_MISMATCH"
    );
    assert_eq!(
        a.verify_actual_directory(&ledger.directory().join("missing"))
            .unwrap_err()
            .code,
        "WEBVIEW_PROFILE_ACTUAL_UNKNOWN"
    );
    native::verify_private(&native::open_no_reparse(a.directory()).unwrap().file).unwrap();
    let bytes = fs::read(ledger.path(ProjectKind::Web, A).unwrap()).unwrap();
    fs::write(
        a.directory().join("retained-engine-data"),
        b"retain profile contents",
    )
    .unwrap();
    assert!(fs::rename(a.directory(), ledger.directory().join("held-root")).is_err());
    assert!(fs::rename(ledger.directory(), ledger.data.join("held-container")).is_err());
    a.verify().unwrap();
    drop((a, b));
    assert_eq!(
        fs::read(ledger.path(ProjectKind::Web, A).unwrap()).unwrap(),
        bytes
    );
    assert_eq!(
        fs::read(
            ledger
                .project_directory(ProjectKind::Web, A)
                .unwrap()
                .join("retained-engine-data")
        )
        .unwrap(),
        b"retain profile contents"
    );
}
#[test]
fn full_128_bit_id_and_replaced_directory_cannot_be_recaptured_as_authority() {
    let (_temporary, ledger) = fixture();
    ledger.record(ProjectKind::Web, A).unwrap();
    let path = ledger.path(ProjectKind::Web, A).unwrap();
    let original = fs::read(&path).unwrap();
    let mut record: serde_json::Value = serde_json::from_slice(&original).unwrap();
    let byte = record["directoryIdentity"]["windowsFileId"][15]
        .as_u64()
        .unwrap();
    record["directoryIdentity"]["windowsFileId"][15] = serde_json::json!(byte ^ 1);
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(ledger.bind_profile(ProjectKind::Web, A).is_err());
    fs::write(&path, &original).unwrap();
    let root = ledger.project_directory(ProjectKind::Web, A).unwrap();
    fs::rename(&root, ledger.directory().join("held-original")).unwrap();
    native::create_private_directory(&root).unwrap();
    fs::write(&path, &original).unwrap();
    fs::write(root.join("foreign-sentinel"), b"retain replacement").unwrap();
    assert!(ledger.bind_profile(ProjectKind::Web, A).is_err());
    assert_eq!(
        fs::read(root.join("foreign-sentinel")).unwrap(),
        b"retain replacement"
    );
    assert_eq!(fs::read(path).unwrap(), original);
}
#[test]
fn registered_record_hard_links_keep_the_original_admission_policy() {
    let (temporary, ledger) = fixture();
    ledger.record(ProjectKind::Web, A).unwrap();
    let record = ledger.path(ProjectKind::Web, A).unwrap();
    let other_name = temporary.path().join("disposable-record-hardlink");
    fs::hard_link(&record, &other_name).unwrap();
    let before = fs::read(&other_name).unwrap();
    let profile = ledger.bind_profile(ProjectKind::Web, A).unwrap();
    profile.verify().unwrap();
    drop(profile);
    assert_eq!(fs::read(other_name).unwrap(), before);
}
#[test]
fn removal_pending_and_unknown_receipts_prevent_binding_without_mutation() {
    let (_temporary, ledger) = fixture();
    ledger.record(ProjectKind::Web, A).unwrap();
    let path = ledger.path(ProjectKind::Web, A).unwrap();
    let original = fs::read(&path).unwrap();
    let receipt = ledger.confirmation_path(ProjectKind::Web, A).unwrap();
    fs::write(&receipt, &original).unwrap();
    assert_eq!(
        ledger.bind_profile(ProjectKind::Web, A).err().unwrap().code,
        "WEBVIEW_STORE_REMOVAL_PENDING"
    );
    fs::write(&receipt, b"retained unknown receipt").unwrap();
    assert!(ledger.bind_profile(ProjectKind::Web, A).is_err());
    assert_eq!(fs::read(&receipt).unwrap(), b"retained unknown receipt");
    assert_eq!(fs::read(path).unwrap(), original);
}
#[test]
fn a_junction_cannot_turn_the_registered_name_into_an_external_profile() {
    let (_temporary, ledger) = fixture();
    ledger.record(ProjectKind::Web, A).unwrap();
    let root = ledger.project_directory(ProjectKind::Web, A).unwrap();
    let external = ledger.data.join("disposable-external-original");
    fs::rename(&root, &external).unwrap();
    fs::write(external.join("sentinel"), b"retain external original").unwrap();
    let status = std::process::Command::new(std::env::var_os("ComSpec").unwrap())
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&root)
        .arg(&external)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(ledger.bind_profile(ProjectKind::Web, A).is_err());
    assert_eq!(
        fs::read(external.join("sentinel")).unwrap(),
        b"retain external original"
    );
    fs::remove_dir(root).unwrap();
}
