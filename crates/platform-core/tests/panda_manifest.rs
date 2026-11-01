use platform_core::panda_manifest::*;
use serde_json::{json, Value};
use std::fs;
fn node() -> Value {
    json!({"manifestVersion":1,"runtime":"node","app":{"name":"测试"},"web":{"root":"web","entry":"index.html"},"backend":{"root":"backend","entry":"server.mjs","nodeMajor":24,"protocolVersion":1}})
}
fn web() -> Value {
    json!({"manifestVersion":1,"runtime":"web","app":{"name":"测试"},"web":{"root":".","entry":"index.html"}})
}
fn parse(value: &Value) -> ManifestResult<PandaManifest> {
    PandaManifest::parse(&serde_json::to_vec(value).unwrap())
}
fn fixture() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("panda-disposable-unified-manifest-")
        .tempdir()
        .unwrap()
}
#[test]
fn minimal_formats_do_not_require_author_identity_or_version() {
    assert_eq!(parse(&node()).unwrap().runtime, PandaRuntime::Node);
    assert_eq!(parse(&web()).unwrap().runtime, PandaRuntime::Web);
    let mut value = node();
    value["app"]["id"] = json!("old-author");
    assert!(parse(&value).unwrap_err().to_string().contains("app.id"));
    let mut value = web();
    value["$schema"] = json!("https://example.invalid/not-fetched");
    assert!(parse(&value).is_ok());
}
#[test]
fn strict_json_rejects_duplicate_unknown_null_and_old_fields() {
    for bytes in [
        br#"{"manifestVersion":1,"manifestVersion":1,"runtime":"web","app":{"name":"test"},"web":{"root":".","entry":"index.html"}}"#.as_slice(),
        br#"{"manifestVersion":1,"runtime":"web","app":{"name":"test","name":"other"},"web":{"root":".","entry":"index.html"}}"#.as_slice(),
    ] { assert!(PandaManifest::parse(bytes).unwrap_err().to_string().contains("重复字段")); }
    for (field, value) in [
        ("window", Value::Null),
        ("documentation", Value::Null),
        ("packageFormat", json!("local-web-app")),
        ("formatVersion", json!(2)),
        ("sourceDigest", json!("forged")),
    ] {
        let mut input = node();
        input[field] = value;
        assert!(parse(&input).is_err(), "accepted {field}");
    }
    let mut input = web();
    input["runtimeRequirements"] = json!({"devices":{"camera":null}});
    assert!(parse(&input).is_err());
    let mut input = node();
    input["app"]["version"] = Value::Null;
    assert!(parse(&input).is_err());
    let mut input = node();
    input["documentation"] = json!({"readme":null});
    assert!(parse(&input).is_err());
    let mut bytes = serde_json::to_vec(&node()).unwrap();
    bytes.extend_from_slice(b" {}");
    assert!(PandaManifest::parse(&bytes).is_err());
}
#[test]
fn runtime_rules_and_diagnostics_are_specific() {
    for (field, value) in [
        ("platform", json!({"sdkApiVersion":1})),
        (
            "development",
            json!({"build":{"kind":"npm","script":"build"}}),
        ),
        ("runtimeRequirements", json!({})),
    ] {
        let mut input = node();
        input[field] = value;
        let error = parse(&input).unwrap_err().to_string();
        assert!(error.contains(field), "{error}");
    }
    let mut input = web();
    input["backend"] = node()["backend"].clone();
    assert!(parse(&input).unwrap_err().to_string().contains("backend"));
    let mut input = node();
    input.as_object_mut().unwrap().remove("backend");
    assert!(parse(&input).unwrap_err().to_string().contains("backend"));
    let mut input = node();
    input["backend"]["nodeMajor"] = json!(22);
    assert!(parse(&input)
        .unwrap_err()
        .to_string()
        .contains("backend.nodeMajor"));
    let mut input = web();
    input["development"] = json!({"build":{"kind":"npm","script":"build:web"}});
    assert_eq!(parse(&input).unwrap().development.unwrap().source_dir, ".");
    let mut input = node();
    input["runtime"] = json!("test-private-value-never-print");
    assert!(!parse(&input)
        .unwrap_err()
        .to_string()
        .contains("test-private-value"));
}
#[test]
fn documentation_fields_are_independent_and_checked_only_when_declared() {
    let source = fixture();
    fs::write(source.path().join("index.html"), "html").unwrap();
    fs::write(source.path().join("guide.md"), "guide").unwrap();
    let mut input = web();
    input["documentation"] = json!({"readme":"guide.md"});
    assert!(parse(&input).unwrap().inspect(source.path()).is_ok());
    input["documentation"] = json!({"development":"missing.md"});
    assert!(parse(&input)
        .unwrap()
        .inspect(source.path())
        .unwrap_err()
        .to_string()
        .contains("documentation.development"));
    let mut input = node();
    input["documentation"] = json!({"database":"guide.md"});
    assert!(parse(&input).is_err());
}
#[test]
fn configuration_declarations_preserve_types_and_never_allow_web_secrets() {
    let field = json!({"name":"API_KEY","label":"测试","type":"string","required":true,"secret":true,"description":"测试用途","howToGet":["测试步骤"]});
    let mut input = node();
    input["configuration"] = json!({"version":1,"fields":[field.clone()]});
    assert!(parse(&input).is_ok());
    input["configuration"]["fields"][0]["default"] = json!("not-a-real-key");
    assert!(parse(&input).is_err());
    let mut input = web();
    input["configuration"] = json!({"version":1,"fields":[field]});
    assert!(parse(&input).unwrap_err().to_string().contains("secret"));
    input["configuration"]["fields"][0]["secret"] = json!(false);
    input["configuration"]["fields"][0]["howToGet"] = json!("wrong-shape");
    assert!(parse(&input).is_err());
}
#[test]
fn missing_web_output_is_buildable_but_node_outputs_are_required() {
    let source = fixture();
    let mut input = web();
    input["web"]["root"] = json!("dist");
    input["development"] = json!({"build":{"kind":"npm","script":"bundle"}});
    assert_eq!(
        parse(&input).unwrap().inspect(source.path()).unwrap()["needsBuild"],
        true
    );
    input.as_object_mut().unwrap().remove("development");
    assert!(parse(&input).unwrap().inspect(source.path()).is_err());
    assert!(parse(&node()).unwrap().inspect(source.path()).is_err());
}
#[test]
fn new_reader_only_observes_panda_json_and_keeps_read_budget() {
    let source = fixture();
    fs::write(
        source.path().join("alternate-manifest.json"),
        serde_json::to_vec(&node()).unwrap(),
    )
    .unwrap();
    assert!(PandaManifest::load(source.path()).is_err());
    fs::write(
        source.path().join(MANIFEST_FILE),
        serde_json::to_vec(&node()).unwrap(),
    )
    .unwrap();
    assert!(PandaManifest::load(source.path()).is_ok());
    assert!(PandaManifest::parse(&vec![b' '; MAX_MANIFEST_BYTES + 1]).is_err());
}
#[cfg(unix)]
#[test]
fn web_links_may_resolve_inside_but_never_outside_or_into_protected_content() {
    use std::os::unix::fs::symlink;
    let source = fixture();
    let outside = fixture();
    fs::create_dir(source.path().join("public")).unwrap();
    fs::write(source.path().join("public/index.html"), "html").unwrap();
    symlink(source.path().join("public"), source.path().join("alias")).unwrap();
    let mut input = web();
    input["web"]["root"] = json!("alias");
    assert!(parse(&input).unwrap().inspect(source.path()).is_ok());
    fs::write(outside.path().join("index.html"), "outside").unwrap();
    symlink(outside.path(), source.path().join("outside")).unwrap();
    input["web"]["root"] = json!("outside");
    assert!(parse(&input).unwrap().inspect(source.path()).is_err());
    fs::create_dir(source.path().join("data")).unwrap();
    fs::write(source.path().join("data/index.html"), "private").unwrap();
    symlink(
        source.path().join("data"),
        source.path().join("public-data"),
    )
    .unwrap();
    input["web"]["root"] = json!("public-data");
    assert!(parse(&input).unwrap().inspect(source.path()).is_err());
    input["web"]["root"] = json!("public");
    symlink(
        source.path().join("data/index.html"),
        source.path().join("public/entry.html"),
    )
    .unwrap();
    input["web"]["entry"] = json!("entry.html");
    assert!(parse(&input).unwrap().inspect(source.path()).is_err());
}
#[test]
fn static_data_subdirectories_are_distinct_from_project_business_data() {
    let source = fixture();
    fs::create_dir_all(source.path().join("public/data")).unwrap();
    fs::write(source.path().join("public/data/index.html"), "public").unwrap();
    let mut input = web();
    input["web"]["root"] = json!("public");
    input["web"]["entry"] = json!("data/index.html");
    assert!(parse(&input).unwrap().inspect(source.path()).is_ok());
    input["web"]["root"] = json!("public/data");
    input["web"]["entry"] = json!("index.html");
    assert!(parse(&input).unwrap().inspect(source.path()).is_ok());
    input["web"]["root"] = json!("data/missing");
    input["development"] = json!({"build":{"kind":"npm","script":"build"}});
    assert!(parse(&input).unwrap().inspect(source.path()).is_err());
}

#[test]
fn exported_schema_uses_runtime_rules_and_has_no_nullable_optional_fields() {
    let schema = export_schema();
    assert_eq!(schema["oneOf"].as_array().unwrap().len(), 2);
    let web = &schema["oneOf"][0];
    let node = &schema["oneOf"][1];
    assert!(web["properties"].get("backend").is_none());
    assert!(node["properties"].get("development").is_none());
    assert!(node["required"]
        .as_array()
        .unwrap()
        .contains(&json!("backend")));
    assert_eq!(
        web["properties"]["configuration"]["properties"]["fields"]["items"]["properties"]["secret"]
            ["const"],
        false
    );
    assert!(!schema.to_string().contains("\"null\""));
    assert!(node["properties"]["documentation"]["properties"]
        .get("database")
        .is_none());
}

#[test]
fn optional_icon_is_a_decoded_bounded_png_or_falls_back() {
    let source = fixture();
    let mut input = web();
    input["app"]["icon"] = json!("icon.png");
    let manifest = parse(&input).unwrap();
    assert!(manifest.icon_data_url(source.path()).is_none());
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0, 0, 0, 255]).unwrap();
    }
    fs::write(source.path().join("icon.png"), &bytes).unwrap();
    assert!(manifest
        .icon_data_url(source.path())
        .unwrap()
        .starts_with("data:image/png;base64,"));
    fs::write(source.path().join("icon.png"), b"not-png").unwrap();
    assert!(manifest.icon_data_url(source.path()).is_none());
    input["app"]["icon"] = json!("../outside.png");
    assert!(parse(&input)
        .unwrap()
        .icon_data_url(source.path())
        .is_none());
}

#[test]
fn schema_and_runtime_share_public_format_cases() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../schemas/fixtures/panda-manifest-cases.json"
    ))
    .unwrap();
    for case in cases {
        assert_eq!(
            parse(&case["manifest"]).is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
    }
}
