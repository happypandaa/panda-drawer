use super::*;

struct MigrationFixture {
    _temp: tempfile::TempDir,
    data: PathBuf,
    old: PathBuf,
    new: PathBuf,
    source: PathBuf,
    id: String,
}
impl MigrationFixture {
    fn new(relative: &str, generated: bool) -> Self {
        let temp = tempfile::Builder::new()
            .prefix("disposable-node-direct-source-migration-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let data = root.join("data");
        let old = root.join("old");
        let new = root.join("new");
        let source = old.join(relative);
        let target = new.join(relative);
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::create_dir_all(&data).unwrap();
        let id = Uuid::new_v4().to_string();
        let ownership =
            crate::node_project_ownership::NodeProjectOwnership::new(data.clone(), old.clone());
        ownership.initialize(&id).unwrap();
        let identity = PersistedIdentity::capture(&source).unwrap();
        if generated {
            ownership.record_generated_source(&id, &source).unwrap();
        }
        if relative.starts_with("project-sources/") {
            ownership
                .reserve_published_source_identity(
                    &id,
                    source.file_name().unwrap().to_str().unwrap(),
                    &identity,
                )
                .unwrap();
        }
        claim_source_after_verified_migration(&data, &old, &id, &source).unwrap();
        let slot = data.join("node-apps").join(&id);
        fs::create_dir_all(&slot).unwrap();
        let manifest =
            json!({"app":{"id":"original-author-id"},"runtime":"node-service"}).to_string();
        let digest = format!("{:x}", Sha256::digest(manifest.as_bytes()));
        crate::builds::atomic_json(
            &slot.join("registration.json"),
            &json!({"version":1,"id":id,"sourcePath":source,"manifestDigest":digest}),
        )
        .unwrap();
        crate::builds::atomic_json(
            &slot.join("development-source.json"),
            &NodeDevelopmentBinding {
                source_root: source.clone(),
                manifest_id: "original-author-id".into(),
                root_identity: identity,
                registered_manifest_text: manifest,
                registered_manifest_digest: digest,
            },
        )
        .unwrap();
        // The latest author declaration no longer matches old metadata. Migration
        // concerns directory ownership and must not reject this app.id change.
        fs::write(
            source.join("local-app.json"),
            "{\"app\":{\"id\":\"changed-author-id\"}}",
        )
        .unwrap();
        fs::copy(source.join("local-app.json"), target.join("local-app.json")).unwrap();
        Self {
            _temp: temp,
            data,
            old,
            new,
            source,
            id,
        }
    }
    fn registration(&self) -> Value {
        serde_json::from_slice(
            &fs::read(
                self.data
                    .join("node-apps")
                    .join(&self.id)
                    .join("registration.json"),
            )
            .unwrap(),
        )
        .unwrap()
    }
}

#[test]
fn published_direct_registration_follows_verified_copy_without_receipt_or_app_identity_gate() {
    let fixture = MigrationFixture::new("project-sources/disposable-source", false);
    rebind_published_after_migration(&fixture.data, &fixture.old, &fixture.new).unwrap();
    let target = fixture.new.join("project-sources/disposable-source");
    assert_eq!(fixture.registration()["sourcePath"], json!(target));
    let binding: NodeDevelopmentBinding = serde_json::from_slice(
        &fs::read(
            fixture
                .data
                .join("node-apps")
                .join(&fixture.id)
                .join("development-source.json"),
        )
        .unwrap(),
    )
    .unwrap();
    binding.validate_directory().unwrap();
    assert_eq!(binding.source_root, target);
    // A crash after the binding write can repeat without re-adopting old bytes.
    rebind_published_after_migration(&fixture.data, &fixture.old, &fixture.new).unwrap();
    assert_eq!(fixture.registration()["sourcePath"], json!(target));
    assert!(fixture.source.exists());
}

#[test]
fn generated_direct_source_follows_copy_but_user_author_source_does_not() {
    for generated in [false, true] {
        let fixture = MigrationFixture::new("custom-disposable-source", generated);
        rebind_published_after_migration(&fixture.data, &fixture.old, &fixture.new).unwrap();
        let expected = if generated {
            fixture.new.join("custom-disposable-source")
        } else {
            fixture.source.clone()
        };
        assert_eq!(fixture.registration()["sourcePath"], json!(expected));
        assert!(fixture.source.exists());
    }
}

#[test]
fn replaced_source_directory_is_not_adopted_for_migration() {
    let fixture = MigrationFixture::new("project-sources/disposable-source", false);
    fs::rename(
        &fixture.source,
        fixture.source.with_file_name("disposable-original-source"),
    )
    .unwrap();
    fs::create_dir(&fixture.source).unwrap();
    assert!(rebind_published_after_migration(&fixture.data, &fixture.old, &fixture.new).is_err());
    assert_eq!(fixture.registration()["sourcePath"], json!(fixture.source));
}

#[test]
fn cleanup_receipt_matching_uses_stored_identity_after_source_disappears() {
    let fixture = MigrationFixture::new("project-sources/disposable-source", false);
    let binding: NodeDevelopmentBinding = serde_json::from_slice(
        &fs::read(
            fixture
                .data
                .join("node-apps")
                .join(&fixture.id)
                .join("development-source.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let expected = PersistedIdentity::capture(&fixture.source).unwrap();
    fs::remove_dir_all(&fixture.source).unwrap();
    assert!(binding.matches_identity(&fixture.source, &expected));
    assert!(binding.validate_directory().is_err());
}
