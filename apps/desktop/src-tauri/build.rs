fn main() {
    println!("cargo:rerun-if-env-changed=PANDA_DRAWER_UPDATE_PUBLIC_KEY");
    let test_profile = std::env::var_os("CARGO_FEATURE_LOCAL_UPDATE_TEST").is_some();
    let production = std::env::var_os("CARGO_FEATURE_PRODUCTION_PROFILE").is_some();
    let key = if test_profile || production {
        std::env::var("PANDA_DRAWER_UPDATE_PUBLIC_KEY").unwrap_or_default()
    } else {
        String::new()
    };
    assert!(
        !test_profile || !key.is_empty(),
        "the local-update test profile requires its public verification key"
    );
    if !key.is_empty() {
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
