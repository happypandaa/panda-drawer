#[path = "build_support/profile.rs"]
mod profile;
#[allow(dead_code)]
#[path = "build_support/signed_resources.rs"]
mod signed_resources;
#[cfg(windows)]
#[allow(dead_code)]
#[path = "build_support/windows_resources.rs"]
mod windows_resources;

use std::{env, fs, path::PathBuf};

fn main() {
    let resource_anchors = env::var_os("CARGO_FEATURE_PRODUCTION_RESOURCE_ANCHORS").is_some();
    assert!(
        !resource_anchors || cfg!(any(unix, windows)),
        "production resource anchors require a supported native build host"
    );
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    signed_resources::validate_input_path(&manifest_dir).expect("unsafe source root path");
    let target = env::var("TARGET").unwrap();
    let profile_file = if target == "x86_64-pc-windows-msvc" {
        "production.windows-x64.json"
    } else {
        "production.json"
    };
    let profile_path = manifest_dir.join("../../build-profiles").join(profile_file);
    println!("cargo:rerun-if-changed={}", profile_path.display());
    println!("cargo:rerun-if-changed=build_support/profile.rs");
    let signing_policy = manifest_dir.join("../../build-profiles/macos-signing-policy.json");
    println!("cargo:rerun-if-changed={}", signing_policy.display());
    println!("cargo:rerun-if-changed=build_support/signed_resources.rs");
    println!("cargo:rerun-if-changed=build_support/signed_resources_unix.rs");
    println!("cargo:rerun-if-changed=build_support/windows_resources.rs");
    println!("cargo:rerun-if-changed=build_support/windows_resources_io.rs");
    for source in [
        "node-runtime-release.json",
        "development-node-release.json",
        "development-git-release.json",
        "node-runtime-release.windows-x64.json",
        "development-node-release.windows-x64.json",
        "development-git-release.windows-x64.json",
    ] {
        println!(
            "cargo:rerun-if-changed={}",
            manifest_dir.join("../../scripts").join(source).display()
        );
    }
    for variable in [
        "PANDA_DRAWER_COMPILED_TOOL_MANIFEST",
        "PANDA_DRAWER_COMPILED_TOOL_ROOT",
        "PANDA_DRAWER_UPDATE_TEST_VERSION",
        "PANDA_DRAWER_UPDATE_TEST_RELEASE_TAG",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let mut values = vec![("BUILD_TARGET", target.clone())];
    let production = env::var_os("CARGO_FEATURE_PRODUCTION_PROFILE").is_some();
    let local_update_test = env::var_os("CARGO_FEATURE_LOCAL_UPDATE_TEST").is_some();
    assert!(
        !local_update_test
            || (!production
                && matches!(target.as_str(), "aarch64-apple-darwin" | "x86_64-pc-windows-msvc")),
        "the isolated update trial requires macOS arm64 or Windows x64 MSVC and cannot use the production profile"
    );
    let online_test_tag = if local_update_test {
        match env::var("PANDA_DRAWER_UPDATE_TEST_RELEASE_TAG") {
            Ok(tag) => {
                let parts = tag
                    .strip_prefix("auto-update-test-")
                    .or_else(|| tag.strip_prefix("auto-update-oss-test-"))
                    .or_else(|| tag.strip_prefix("auto-update-dual-test-"))
                    .and_then(|suffix| suffix.split_once('-'));
                assert!(
                    parts.is_some_and(|(date, time)| {
                        date.len() == 8
                            && time.len() == 6
                            && date
                                .bytes()
                                .chain(time.bytes())
                                .all(|byte| byte.is_ascii_digit())
                    }),
                    "the online-update test requires a dedicated update test tag"
                );
                tag
            }
            Err(env::VarError::NotPresent) => String::new(),
            Err(error) => panic!("invalid online-update test tag: {error}"),
        }
    } else {
        String::new()
    };
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
    } else if local_update_test {
        let version = env::var("PANDA_DRAWER_UPDATE_TEST_VERSION")
            .expect("the local-update test profile requires its build version");
        assert!(
            matches!(version.as_str(), "0.1.0" | "0.1.1"),
            "the local-update test profile only supports 0.1.0 and 0.1.1"
        );
        let (identifier, display_name, distribution, release_repository) =
            if online_test_tag.starts_with("auto-update-dual-test-") {
                (
                    "test.happypandaa.pandadrawer.dual-update",
                    "熊猫小抽屉 · 双线路更新测试版",
                    "isolated-dual-update-test",
                    "https://github.com/happypandaa/panda-drawer",
                )
            } else if online_test_tag.starts_with("auto-update-oss-test-") {
                (
                    "test.happypandaa.pandadrawer.oss-update",
                    "熊猫小抽屉 · 国内更新测试版",
                    "isolated-oss-update-test",
                    "",
                )
            } else if !online_test_tag.is_empty() {
                (
                    "test.happypandaa.pandadrawer.online-update",
                    "熊猫小抽屉 · 线上更新测试版",
                    "isolated-online-update-test",
                    "https://github.com/happypandaa/panda-drawer",
                )
            } else {
                (
                    "test.happypandaa.pandadrawer.update",
                    "熊猫小抽屉 · 更新测试版",
                    "isolated-local-update-test",
                    "",
                )
            };
        values.extend([
            ("BUILD_PROFILE", "local-update-test".into()),
            ("APP_VERSION", version),
            ("APP_IDENTIFIER", identifier.into()),
            ("ENVIRONMENT", "local-update-test".into()),
            ("DISPLAY_NAME", display_name.into()),
            ("DISTRIBUTION", distribution.into()),
            ("RELEASE_REPOSITORY", release_repository.into()),
            (
                "MINIMUM_SYSTEM_VERSION",
                if target == "aarch64-apple-darwin" {
                    "15.0"
                } else {
                    "10"
                }
                .into(),
            ),
            ("PROFILE_DIGEST", String::new()),
        ]);
    } else {
        values.extend([
            ("BUILD_PROFILE", "development".into()),
            ("APP_VERSION", env::var("CARGO_PKG_VERSION").unwrap()),
            ("APP_IDENTIFIER", "dev.happypandaa.pandadrawer".into()),
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
    values.push(("UPDATE_TEST_RELEASE_TAG", online_test_tag));
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
        if target == "x86_64-pc-windows-msvc" {
            #[cfg(windows)]
            {
                let runtime_source = fs::read(
                    manifest_dir.join("../../scripts/node-runtime-release.windows-x64.json"),
                )
                .expect("Windows runtime Node source anchor");
                let node_source = fs::read(
                    manifest_dir.join("../../scripts/development-node-release.windows-x64.json"),
                )
                .expect("Windows development Node source anchor");
                let git_source = fs::read(
                    manifest_dir.join("../../scripts/development-git-release.windows-x64.json"),
                )
                .expect("Windows development Git source anchor");
                let trusted =
                    windows_resources::TrustedContext::new(windows_resources::TrustInputs {
                        profile: &profile_bytes,
                        target: &target,
                        node_runtime: &runtime_source,
                        development_node: &node_source,
                        development_git: &git_source,
                    })
                    .expect("refusing invalid Windows resource trust inputs");
                compiled_manifest = windows_resources::read_manifest(&manifest_path)
                    .expect("refusing missing or unsafe Windows resource manifest");
                let manifest = windows_resources::parse(&compiled_manifest, &trusted)
                    .expect("refusing mismatched Windows resource manifest");
                windows_resources::verify_resources(&resource_root, &manifest)
                    .expect("refusing Windows tools that differ from compile anchors");
            }
            #[cfg(not(windows))]
            panic!("Windows production resource anchors require a native Windows build");
        } else {
            let policy_bytes = fs::read(&signing_policy).expect("fixed signing policy");
            let runtime_source =
                fs::read(manifest_dir.join("../../scripts/node-runtime-release.json"))
                    .expect("runtime Node source anchor");
            let node_source =
                fs::read(manifest_dir.join("../../scripts/development-node-release.json"))
                    .expect("development Node source anchor");
            let git_source =
                fs::read(manifest_dir.join("../../scripts/development-git-release.json"))
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
    let mut generated = format!(
        "pub const PRODUCTION_PROFILE: bool = {production};\npub const LOCAL_UPDATE_TEST_PROFILE: bool = {local_update_test};\n"
    );
    for (name, value) in values {
        generated.push_str(&format!("pub const {name}: &str = {value:?};\n"));
    }
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("build_metadata.rs"),
        generated,
    )
    .expect("write compiled metadata");
}
