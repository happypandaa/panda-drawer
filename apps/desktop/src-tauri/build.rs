fn main() {
    println!("cargo:rerun-if-env-changed=PANDA_DRAWER_UPDATE_PUBLIC_KEY");
    let test_profile = std::env::var_os("CARGO_FEATURE_LOCAL_UPDATE_TEST").is_some();
    let production = std::env::var_os("CARGO_FEATURE_PRODUCTION_PROFILE").is_some();
    println!("cargo:rerun-if-env-changed=PANDA_DRAWER_COMPILED_CLI_SHA256");
    let cli_sha256 = if production
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var_os("CARGO_FEATURE_PRODUCTION_RESOURCE_ANCHORS").is_some()
    {
        let value = std::env::var("PANDA_DRAWER_COMPILED_CLI_SHA256")
            .expect("Windows production desktop requires the verified bundled CLI digest");
        assert!(
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "invalid compiled Windows CLI SHA256"
        );
        value
    } else {
        String::new()
    };
    println!("cargo:rustc-env=PANDA_DRAWER_COMPILED_CLI_SHA256={cli_sha256}");
    let key = if production {
        const KEY_FILE: &str = "../../../build-profiles/production-updater.pub";
        println!("cargo:rerun-if-changed={KEY_FILE}");
        std::fs::read_to_string(KEY_FILE)
            .expect("the production update public key must be present")
            .trim_end()
            .to_owned()
    } else if test_profile {
        std::env::var("PANDA_DRAWER_UPDATE_PUBLIC_KEY")
            .expect("the local-update test profile requires its public verification key")
    } else {
        String::new()
    };
    if production || test_profile {
        assert!(
            (40..=512).contains(&key.len())
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte)),
            "invalid public update verification key format"
        );
    }
    println!("cargo:rustc-env=PANDA_DRAWER_UPDATE_PUBLIC_KEY={key}");
    tauri_build::build();
}
