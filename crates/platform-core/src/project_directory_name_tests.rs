use super::*;

#[test]
fn pinned_npm_public_slugify_golden_cases_match_native_names() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("project_directory_name_data/golden.json")).unwrap();
    assert_eq!(fixtures["version"], "2.6.2");
    let id = "prj_1234567890abcdef1234567890abcdef";
    let suffix = format!("{:x}", Sha256::digest(id.as_bytes()));
    for case in fixtures["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let expected = format!("{}-{}", case["slug"].as_str().unwrap(), &suffix[..8]);
        assert_eq!(readable_directory_name(name, id), expected, "{name:?}");
    }
}

#[test]
fn output_is_one_bounded_ascii_component_even_for_unsafe_or_empty_names() {
    for name in [
        "",
        ".",
        "..",
        "/../../\\",
        "😀🧑‍💻",
        "NUL",
        "COM1",
        "lpt9",
        "A\0B\nC",
        &"中文/".repeat(1000),
    ] {
        let directory = readable_directory_name(name, "../../not-a-path-id");
        assert!(directory.len() <= MAX_SLUG_BYTES + 1 + 8);
        assert!(directory
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'));
        assert!(!directory.starts_with('-') && !directory.ends_with('-'));
        assert!(!directory.contains("--"));
    }
    assert!(readable_directory_name("😀🚀", "id").starts_with("project-"));
    assert!(readable_directory_name("CON", "id").starts_with("project-con-"));
    assert!(readable_directory_name("lpt9", "id").starts_with("project-lpt9-"));
}

#[test]
fn suffix_uses_the_complete_identity_and_names_are_deterministic() {
    let first = readable_directory_name("同名", "prj_11111111111111111111111111111111");
    let second = readable_directory_name("同名", "prj_11111111111111111111111111111112");
    assert_ne!(first, second);
    assert_eq!(
        first,
        readable_directory_name("同名", "prj_11111111111111111111111111111111")
    );
    assert_eq!(
        readable_directory_name("Renamed", "prj_11111111111111111111111111111111")
            .rsplit('-')
            .next(),
        first.rsplit('-').next()
    );
}

#[test]
fn generated_mapping_is_sorted_unique_and_contains_only_safe_fragments() {
    let table = mappings();
    assert!(table.len() > 20000);
    assert!(table.windows(2).all(|pair| pair[0].scalar < pair[1].scalar));
    assert!(table.iter().all(|entry| entry.flags <= 3
        && entry
            .fragment
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')));
}
