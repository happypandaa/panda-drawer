#[path = "build_support/profile.rs"]
mod profile;
#[allow(dead_code)]
#[path = "build_support/signed_resources.rs"]
mod signed_resources;

use std::{env, fs, path::PathBuf};

fn main() {
    let resource_anchors = env::var_os("CARGO_FEATURE_PRODUCTION_RESOURCE_ANCHORS").is_some();
    assert!(
        !resource_anchors || cfg!(unix),
        "production resource anchors are unavailable on non-Unix build hosts"
    );
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    signed_resources::validate_input_path(&manifest_dir).expect("unsafe source root path");
    let profile_path = manifest_dir.join("../../build-profiles/production.json");
    println!("cargo:rerun-if-changed={}", profile_path.display());
    println!("cargo:rerun-if-changed=build_support/profile.rs");
    let signing_policy = manifest_dir.join("../../build-profiles/macos-signing-policy.json");
    println!("cargo:rerun-if-changed={}", signing_policy.display());
    println!("cargo:rerun-if-changed=build_support/signed_resources.rs");
    println!("cargo:rerun-if-changed=build_support/signed_resources_unix.rs");
    for source in [
        "node-runtime-release.json",
        "development-node-release.json",
        "development-git-release.json",
    ] {
        println!(
            "cargo:rerun-if-changed={}",
            manifest_dir.join("../../scripts").join(source).display()
        );
    }
    for variable in [
        "PANDA_DRAWER_COMPILED_TOOL_MANIFEST",
        "PANDA_DRAWER_COMPILED_TOOL_ROOT",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let target = env::var("TARGET").unwrap();
    let mut values = vec![("BUILD_TARGET", target.clone())];
    let production = env::var_os("CARGO_FEATURE_PRODUCTION_PROFILE").is_some();
    if production {
        let bytes = fs::read(&profile_path).expect("fixed production profile is required");
        let p = profile::ProductionProfile::parse(&bytes, &target)
            .expect("refusing invalid production profile");
        values.extend([
            ("BUILD_PROFILE", p.profile),
            ("APP_VERSION", p.version),
            ("APP_IDENTIFIER", p.identifier),
            ("ENVIRONMENT", p.environment),
            ("DISPLAY_NAME", p.display_name),
            ("DISTRIBUTION", p.distribution),
            ("RELEASE_REPOSITORY", p.release_repository),
            ("MINIMUM_SYSTEM_VERSION", p.minimum_system_version),
            ("PROFILE_DIGEST", profile::digest(&bytes)),
        ]);
    } else {
        values.extend([
            ("BUILD_PROFILE", "development".into()),
            ("APP_VERSION", env::var("CARGO_PKG_VERSION").unwrap()),
            ("APP_IDENTIFIER", "dev.localprojects.platform.m0".into()),
            ("ENVIRONMENT", "development".into()),
            ("DISPLAY_NAME", "熊猫小抽屉 · 开发版".into()),
            ("DISTRIBUTION", "internal-development".into()),
            ("RELEASE_REPOSITORY", String::new()),
            (
                "MINIMUM_SYSTEM_VERSION",
                if target.ends_with("apple-darwin") {
                    "15.0"
                } else {
                    ""
                }
                .into(),
            ),
            ("PROFILE_DIGEST", String::new()),
        ]);
    }
    let mut compiled_manifest = Vec::new();
    if resource_anchors {
        assert!(
            production,
            "resource anchors require the production profile"
        );
        let manifest_path = PathBuf::from(
            env::var_os("PANDA_DRAWER_COMPILED_TOOL_MANIFEST")
                .expect("production resource anchors require a compiled tool manifest"),
        );
        let resource_root = PathBuf::from(
            env::var_os("PANDA_DRAWER_COMPILED_TOOL_ROOT")
                .expect("production resource anchors require a private staged resource root"),
        );
        signed_resources::validate_input_path(&manifest_path).expect("unsafe manifest input path");
        signed_resources::validate_input_path(&resource_root).expect("unsafe staged resource path");
        println!("cargo:rerun-if-changed={}", manifest_path.display());
        println!("cargo:rerun-if-changed={}", resource_root.display());
        let profile_bytes = fs::read(&profile_path).expect("fixed production profile");
        let policy_bytes = fs::read(&signing_policy).expect("fixed signing policy");
        let runtime_source = fs::read(manifest_dir.join("../../scripts/node-runtime-release.json"))
            .expect("runtime Node source anchor");
        let node_source =
            fs::read(manifest_dir.join("../../scripts/development-node-release.json"))
                .expect("development Node source anchor");
        let git_source = fs::read(manifest_dir.join("../../scripts/development-git-release.json"))
            .expect("development Git source anchor");
        let trusted = signed_resources::TrustedContext::new(signed_resources::TrustInputs {
            profile: &profile_bytes,
            target: &target,
            signing_policy: &policy_bytes,
            node_runtime: &runtime_source,
            development_node: &node_source,
            development_git: &git_source,
        })
        .expect("refusing unconfigured or invalid resource trust inputs");
        compiled_manifest = signed_resources::read_manifest(&manifest_path)
            .expect("refusing missing or unsafe signed manifest");
        let manifest = signed_resources::parse(&compiled_manifest, &trusted)
            .expect("refusing mismatched signed manifest");
        signed_resources::verify_resources(&resource_root, &manifest)
            .expect("refusing staged tools that differ from compile anchors");
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(
        out.join("compiled_signed_tool_manifest.json"),
        &compiled_manifest,
    )
    .expect("write immutable manifest input");
    let signed_config = format!("pub const COMPILED_RESOURCE_ANCHORS: bool = {resource_anchors};\npub const COMPILED_RESOURCE_MANIFEST_SHA256: &str = {:?};\n", if resource_anchors { signed_resources::digest(&compiled_manifest) } else { String::new() });
    fs::write(out.join("compiled_signed_tool_config.rs"), signed_config)
        .expect("write compiled anchor metadata");
    let mut generated = format!("pub const PRODUCTION_PROFILE: bool = {production};\n");
    for (name, value) in values {
        generated.push_str(&format!("pub const {name}: &str = {value:?};\n"));
    }
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("build_metadata.rs"),
        generated,
    )
    .expect("write compiled metadata");
}
