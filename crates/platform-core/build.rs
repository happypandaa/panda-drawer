#[path = "build_support/profile.rs"]
mod profile;

use std::{env, fs, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let profile_path = manifest_dir.join("../../build-profiles/production.json");
    println!("cargo:rerun-if-changed={}", profile_path.display());
    println!("cargo:rerun-if-changed=build_support/profile.rs");
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
