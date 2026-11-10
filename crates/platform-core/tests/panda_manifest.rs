use platform_core::panda_manifest::*;
use serde_json::{json, Value};
use std::fs;
fn node() -> Value {
    json!({"manifestVersion":2,"app":{"name":"测试"},"web":{"root":"web","entry":"index.html"},"backend":{"root":"backend","entry":"server.mjs","nodeMajor":24,"protocolVersion":1}})
}
fn web() -> Value {
    json!({"manifestVersion":2,"app":{"name":"测试"},"web":{"root":".","entry":"index.html"}})
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
    assert_eq!(parse(&node()).unwrap().runtime(), PandaRuntime::Node);
    assert_eq!(parse(&web()).unwrap().runtime(), PandaRuntime::Web);
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
        br#"{"manifestVersion":2,"manifestVersion":2,"app":{"name":"test"},"web":{"root":".","entry":"index.html"}}"#.as_slice(),
        br#"{"manifestVersion":2,"app":{"name":"test","name":"other"},"web":{"root":".","entry":"index.html"}}"#.as_slice(),
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
fn configured_node() -> Value {
    let mut input = node();
    input["configuration"] = json!({"version":1,"fields":[{
        "name":"LABEL","label":"测试文字","type":"string","required":false,
        "secret":false,"description":"测试用途","howToGet":["自行填写"]
    }]});
    input
}
#[test]
fn field_diagnostic_paths_do_not_duplicate_unknown_keys() {
    let mut input = node();
    input["unknown"] = json!(true);
    assert_eq!(parse(&input).unwrap_err().to_string(), "unknown：未知字段");
    let mut input = node();
    input["backend"]["apiPrefixess"] = json!(["/v1"]);
    assert_eq!(
        parse(&input).unwrap_err().to_string(),
        "backend.apiPrefixess：未知字段"
    );
    let mut input = configured_node();
    input["configuration"]["fields"][0]["unknown"] = json!(true);
    assert_eq!(
        parse(&input).unwrap_err().to_string(),
        "configuration.fields[0].unknown：未知字段"
    );
}
#[test]
fn field_diagnostic_paths_include_duplicate_keys_once() {
    for (input, key, value, expected) in [
        (node(), "manifestVersion", "2", "manifestVersion：重复字段"),
        (
            node(),
            "protocolVersion",
            "1",
            "backend.protocolVersion：重复字段",
        ),
        (
            configured_node(),
            "required",
            "false",
            "configuration.fields[0].required：重复字段",
        ),
    ] {
        let bytes = serde_json::to_string(&input).unwrap().replacen(
            &format!("\"{key}\":"),
            &format!("\"{key}\":{value},\"{key}\":"),
            1,
        );
        assert_eq!(
            PandaManifest::parse(bytes.as_bytes())
                .unwrap_err()
                .to_string(),
            expected
        );
    }
}
#[test]
fn field_diagnostic_paths_keep_missing_keys_under_their_parent() {
    let mut input = node();
    input.as_object_mut().unwrap().remove("manifestVersion");
    assert_eq!(
        parse(&input).unwrap_err().to_string(),
        "manifestVersion：缺少必填字段"
    );
    let mut input = node();
    input["backend"].as_object_mut().unwrap().remove("entry");
    assert_eq!(
        parse(&input).unwrap_err().to_string(),
        "backend.entry：缺少必填字段"
    );
    let mut input = configured_node();
    input["configuration"]["fields"][0]
        .as_object_mut()
        .unwrap()
        .remove("label");
    assert_eq!(
        parse(&input).unwrap_err().to_string(),
        "configuration.fields[0].label：缺少必填字段"
    );
}
#[test]
fn optional_backend_shares_sdk_build_policy_and_documentation() {
    let mut input = node();
    input["platform"] = json!({"sdkApiVersion":1});
    input["development"] = json!({"build":{"kind":"npm","script":"build"}});
    input["runtimeRequirements"] = json!({});
    input["documentation"] = json!({"database":"docs/database.md"});
    assert!(parse(&input).is_ok());
    let mut input = web();
    input["backend"] = node()["backend"].clone();
    assert!(parse(&input).unwrap().has_backend());
    input.as_object_mut().unwrap().remove("backend");
    assert!(!parse(&input).unwrap().has_backend());
    let mut input = node();
    input["backend"]["nodeMajor"] = json!(22);
    assert!(parse(&input)
        .unwrap_err()
        .to_string()
        .contains("backend.nodeMajor"));
    let mut input = web();
    input["development"] = json!({"build":{"kind":"npm","script":"build:web"}});
    assert_eq!(parse(&input).unwrap().development.unwrap().source_dir, ".");
    input["runtime"] = json!("test-private-value-never-print");
    assert!(!parse(&input)
        .unwrap_err()
        .to_string()
        .contains("test-private-value"));
}

#[test]
fn backend_api_prefixes_preserve_defaults_and_accept_explicit_routes_or_no_api() {
    let default = parse(&node()).unwrap().backend.unwrap();
    assert_eq!(default.effective_api_prefixes(), vec!["/api", "/tts"]);
    assert!(default.api_prefixes.is_none());
    for prefixes in [json!(["/v1", "/auth/tokens"]), json!([])] {
        let mut input = node();
        input["backend"]["apiPrefixes"] = prefixes.clone();
        let backend = parse(&input).unwrap().backend.unwrap();
        assert_eq!(
            serde_json::to_value(backend.effective_api_prefixes()).unwrap(),
            prefixes
        );
    }
    let mut input = web();
    input["backend"] = node()["backend"].clone();
    input["backend"]["apiPrefixes"] = json!(["/v1"]);
    assert!(parse(&input).is_ok());
}
#[test]
fn backend_api_prefixes_reject_ambiguous_reserved_or_duplicate_routes() {
    for prefixes in [
        json!([""]),
        json!(["/"]),
        json!(["v1"]),
        json!(["//v1"]),
        json!(["/v1/"]),
        json!(["/v1//chat"]),
        json!(["/."]),
        json!(["/.."]),
        json!(["/v1/../chat"]),
        json!(["/v1/./chat"]),
        json!(["/_platform"]),
        json!(["/_platform/api"]),
        json!(["/_platform-other"]),
        json!(["/%5fplatform"]),
        json!(["/v1%2fchat"]),
        json!(["/v1?query"]),
        json!(["/v1#fragment"]),
        json!(["/v1\\chat"]),
        json!(["/http:host"]),
        json!(["/white space"]),
        json!(["/v1\n"]),
        json!(["/v1", "/v1"]),
        json!([1]),
        json!(null),
        json!("/v1"),
    ] {
        let mut input = node();
        input["backend"]["apiPrefixes"] = prefixes.clone();
        let error = parse(&input).unwrap_err().to_string();
        assert!(error.contains("backend.apiPrefixes"), "{prefixes}: {error}");
    }
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
    assert!(parse(&input).is_ok());
}
#[test]
fn agent_declaration_is_optional_and_strict() {
    assert!(parse(&web()).unwrap().agent.is_none());
    for mut input in [web(), node()] {
        input["agent"] = json!({"instructions":"AGENTS.md"});
        assert_eq!(
            parse(&input).unwrap().agent.unwrap().instructions,
            "AGENTS.md"
        );
    }
    for declaration in [
        Value::Null,
        json!({}),
        json!({"instructions":null}),
        json!({"instructions":true}),
        json!({"instructions":"AGENTS.md","schedule":"daily"}),
    ] {
        let mut input = web();
        input["agent"] = declaration;
        assert!(
            parse(&input).unwrap_err().to_string().contains("agent"),
            "{input}"
        );
    }
}
#[test]
fn agent_instructions_reject_hidden_or_escaping_paths() {
    for path in [
        "",
        " ",
        ".",
        ".AGENTS.md",
        ".rules/AGENTS.md",
        "docs/.AGENTS.md",
        "./AGENTS.md",
        "../AGENTS.md",
        "docs/../AGENTS.md",
        "/AGENTS.md",
        "docs//AGENTS.md",
        "docs/",
        "docs\\AGENTS.md",
        "C:/AGENTS.md",
        "AGENTS\0.md",
    ] {
        let mut input = web();
        input["agent"] = json!({"instructions":path});
        assert!(
            parse(&input)
                .unwrap_err()
                .to_string()
                .contains("agent.instructions"),
            "{path:?}"
        );
    }
}
#[test]
fn agent_instructions_require_an_existing_plain_file_on_every_inspection() {
    let source = fixture();
    fs::write(source.path().join("index.html"), "html").unwrap();
    let mut input = web();
    input["agent"] = json!({"instructions":"AGENTS.md"});
    let manifest = parse(&input).unwrap();
    for result in [
        manifest.inspect(source.path()),
        manifest.inspect_local(source.path()),
    ] {
        let error = result.unwrap_err().to_string();
        assert!(error.contains("agent.instructions"), "{error}");
        assert!(error.contains("AGENTS.md"), "{error}");
        assert!(error.contains("请补上"), "{error}");
    }
    fs::create_dir(source.path().join("AGENTS.md")).unwrap();
    assert!(manifest.inspect(source.path()).is_err());
    fs::remove_dir(source.path().join("AGENTS.md")).unwrap();
    fs::write(source.path().join("AGENTS.md"), "rules").unwrap();
    let inspected = manifest.inspect(source.path()).unwrap();
    assert!(inspected["checkedFiles"]
        .as_array()
        .unwrap()
        .contains(&json!({"role":"agent-instructions","path":"AGENTS.md"})));
    fs::create_dir(source.path().join("docs")).unwrap();
    fs::write(source.path().join("docs/rules.md"), "nested rules").unwrap();
    input["agent"]["instructions"] = json!("docs/rules.md");
    assert!(parse(&input).unwrap().inspect(source.path()).is_ok());
    fs::remove_file(source.path().join("index.html")).unwrap();
    fs::remove_file(source.path().join("docs/rules.md")).unwrap();
    assert!(parse(&input)
        .unwrap()
        .inspect_local(source.path())
        .unwrap_err()
        .to_string()
        .contains("agent.instructions"));
}
#[cfg(unix)]
#[test]
fn agent_instruction_links_stay_inside_the_visible_project_tree() {
    use std::os::unix::fs::symlink;
    let source = fixture();
    let outside = fixture();
    fs::write(source.path().join("index.html"), "html").unwrap();
    fs::write(source.path().join("rules.md"), "rules").unwrap();
    let mut input = web();
    input["agent"] = json!({"instructions":"AGENTS.md"});
    let manifest = parse(&input).unwrap();
    symlink("rules.md", source.path().join("AGENTS.md")).unwrap();
    assert!(manifest.inspect(source.path()).is_ok());
    fs::remove_file(source.path().join("AGENTS.md")).unwrap();
    fs::write(outside.path().join("rules.md"), "external original").unwrap();
    symlink(
        outside.path().join("rules.md"),
        source.path().join("AGENTS.md"),
    )
    .unwrap();
    assert!(manifest.inspect(source.path()).is_err());
    assert_eq!(
        fs::read_to_string(outside.path().join("rules.md")).unwrap(),
        "external original"
    );
    fs::remove_file(source.path().join("AGENTS.md")).unwrap();
    fs::create_dir(source.path().join(".rules")).unwrap();
    fs::write(source.path().join(".rules/guide.md"), "hidden rules").unwrap();
    symlink(".rules/guide.md", source.path().join("AGENTS.md")).unwrap();
    assert!(manifest
        .inspect(source.path())
        .unwrap_err()
        .to_string()
        .contains("隐藏"));
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
fn exported_schema_has_one_model_and_no_nullable_optional_fields() {
    let schema = export_schema();
    assert_eq!(schema["properties"]["manifestVersion"]["const"], 2);
    assert!(schema["properties"].get("runtime").is_none());
    assert!(schema["properties"].get("backend").is_some());
    assert!(schema["properties"].get("development").is_some());
    assert!(!schema["required"]
        .as_array()
        .unwrap()
        .contains(&json!("backend")));
    assert_eq!(
        schema["allOf"][0]["then"]["properties"]["configuration"]["properties"]["fields"]["items"]
            ["properties"]["secret"]["const"],
        false
    );
    assert!(!schema.to_string().contains("\"null\""));
    assert!(schema["$defs"]["DocumentationDecl"]["properties"]
        .get("database")
        .is_some());
}

#[test]
fn old_manifest_has_an_actionable_upgrade_diagnostic() {
    let mut input = node();
    input["manifestVersion"] = json!(1);
    input["runtime"] = json!("node");
    assert!(parse(&input)
        .unwrap_err()
        .to_string()
        .contains("这是旧格式的 panda.json，请让 AI 按平台说明更新"));
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

fn localized_web() -> Value {
    let mut input = web();
    input["app"]["description"] = json!("原始简介");
    input["configuration"] = json!({"version":1,"fields":[{
        "name":"MODE", "label":"模式", "description":"选择模式", "type":"select",
        "required":false,"secret":false,"howToGet":["第一步","第二步"],
        "default":"standard", "options":[{"value":"standard","label":"标准"},{"value":"high","label":"高质量"}]
    }]});
    input["runtimeRequirements"] = json!({
        "network":{"connect":[{"origin":"https://example.com","purpose":"查询","dataSent":"查询文字"}]},
        "devices":{"camera":{"purpose":"拍照"}}
    });
    input["localization"] = json!({"en":{
        "app":{"name":"Example"},
        "configuration":{"MODE":{"label":"Mode","howToGet":["Choose a mode."],"options":{"standard":"Standard"}}},
        "runtimeRequirements":{
            "network":{"connect":{"https://example.com":{"purpose":"Search"}}},
            "devices":{"camera":{"purpose":"Take photos"}}
        }
    }});
    input
}

#[test]
fn localization_is_a_display_projection_without_sdk_or_language_binding() {
    let input = localized_web();
    let manifest = parse(&input).unwrap();
    assert!(manifest.platform.is_none());
    let app = manifest.localized_app("en");
    assert_eq!(app.name, "Example");
    assert_eq!(app.description.as_deref(), Some("原始简介"));
    assert_eq!(manifest.localized_app("zh-CN").name, "测试");
    assert_eq!(manifest.localized_app("fr").name, "测试");
    let serialized = serde_json::to_value(&manifest).unwrap();
    assert_eq!(serialized["app"], input["app"]);
    assert_eq!(serialized["configuration"], input["configuration"]);
    assert_eq!(
        serialized["runtimeRequirements"]["network"]["connect"][0]["purpose"],
        "查询"
    );
    assert_eq!(serialized["localization"], input["localization"]);
    let source = fixture();
    fs::write(
        source.path().join("index.html"),
        "<!doctype html><title>App language</title>",
    )
    .unwrap();
    let inspection = manifest.inspect(source.path()).unwrap();
    assert_eq!(inspection["app"], input["app"]);
    assert_eq!(inspection["localization"], input["localization"]);
    assert_eq!(
        inspection["runtimeRequirements"]["devices"]["camera"]["purpose"],
        "拍照"
    );
    // Missing field translations retain the original options and steps in the
    // stored declaration, even when the display chooses a complete step array.
    assert_eq!(
        inspection["configuration"]["fields"][0]["howToGet"],
        json!(["第一步", "第二步"])
    );
}

#[test]
fn localization_references_only_declared_fields_options_origins_and_devices() {
    for (translation, path) in [
        (
            json!({"configuration":{"UNKNOWN":{"label":"Unknown"}}}),
            "localization.en.configuration.UNKNOWN",
        ),
        (
            json!({"configuration":{"MODE":{"options":{"unknown":"Unknown"}}}}),
            "localization.en.configuration.MODE.options.unknown",
        ),
        (
            json!({"runtimeRequirements":{"network":{"images":{"https://example.com":{"purpose":"Image"}}}}}),
            "localization.en.runtimeRequirements.network.images.https://example.com",
        ),
        (
            json!({"runtimeRequirements":{"network":{"connect":{"https://other.example":{"purpose":"Other"}}}}}),
            "localization.en.runtimeRequirements.network.connect.https://other.example",
        ),
        (
            json!({"runtimeRequirements":{"devices":{"microphone":{"purpose":"Record"}}}}),
            "localization.en.runtimeRequirements.devices.microphone",
        ),
    ] {
        let mut input = localized_web();
        input["localization"]["en"] = translation;
        let error = parse(&input).unwrap_err().to_string();
        assert!(error.starts_with(path), "{error}");
    }
}

#[test]
fn localization_rejects_oversized_text_empty_steps_and_forbidden_overrides() {
    for (translation, path) in [
        (
            json!({"app":{"name":"名".repeat(81)}}),
            "localization.en.app.name",
        ),
        (
            json!({"configuration":{"MODE":{"label":"x".repeat(161)}}}),
            "localization.en.configuration.MODE.label",
        ),
        (
            json!({"configuration":{"MODE":{"howToGet":[]}}}),
            "localization.en.configuration.MODE.howToGet",
        ),
        (
            json!({"configuration":{"MODE":{"howToGet":[" "]}}}),
            "localization.en.configuration.MODE.howToGet",
        ),
        (
            json!({"configuration":{"MODE":{"default":"high"}}}),
            "localization.en.configuration.MODE.default",
        ),
        (
            json!({"runtimeRequirements":{"network":{"connect":{"https://example.com":{"dataSent":"query\ntext"}}}}}),
            "localization.en.runtimeRequirements.network.connect.https://example.com.dataSent",
        ),
    ] {
        let mut input = localized_web();
        input["localization"]["en"] = translation;
        let error = parse(&input).unwrap_err().to_string();
        assert!(error.contains(path), "{error}");
    }
}

#[test]
fn localization_maps_reject_duplicate_reference_keys() {
    let input = serde_json::to_string(&localized_web()).unwrap();
    for (original, duplicate) in [
        ("\"MODE\":{", "\"MODE\":{},\"MODE\":{"),
        (
            "\"standard\":\"Standard\"",
            "\"standard\":\"First\",\"standard\":\"Standard\"",
        ),
        (
            "\"https://example.com\":{",
            "\"https://example.com\":{},\"https://example.com\":{",
        ),
    ] {
        let duplicate = input.replacen(original, duplicate, 1);
        let error = PandaManifest::parse(duplicate.as_bytes())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("localization.en") && error.contains("重复字段"),
            "{error}"
        );
    }
}
