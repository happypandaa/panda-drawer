//! Exported structure comes from the runtime's Rust declarations and branch rules.
use super::*;

fn non_null(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(Value::Array(types)) = object.get_mut("type") {
                types.retain(|value| value != "null");
                let only = (types.len() == 1).then(|| types[0].clone());
                if let Some(only) = only {
                    object.insert("type".into(), only);
                }
            }
            if let Some(Value::Array(alternatives)) = object.get_mut("anyOf") {
                alternatives.retain(|value| value.get("type") != Some(&json!("null")));
                if alternatives.len() == 1 {
                    let only = alternatives.remove(0);
                    object.remove("anyOf");
                    if let Some(only) = only.as_object() {
                        object.extend(only.clone());
                    }
                }
            }
            if object.get("default") == Some(&Value::Null) {
                object.remove("default");
            }
            for child in object.values_mut() {
                non_null(child);
            }
        }
        Value::Array(array) => {
            for child in array {
                non_null(child);
            }
        }
        _ => {}
    }
}

pub fn export_schema() -> Value {
    let mut base = serde_json::to_value(schemars::schema_for!(PandaManifest))
        .expect("derived schema serializes");
    non_null(&mut base);
    base["properties"]["manifestVersion"]["const"] = json!(MANIFEST_VERSION);
    base["$defs"]["BackendDecl"]["properties"]["nodeMajor"]["const"] = json!(NODE_MAJOR);
    base["$defs"]["BackendDecl"]["properties"]["protocolVersion"]["const"] =
        json!(BACKEND_PROTOCOL_VERSION);
    base["$defs"]["BackendDecl"]["properties"]["apiPrefixes"] = json!({
        "type": "array",
        "uniqueItems": true,
        "items": {"type": "string", "pattern": BACKEND_API_PREFIX_PATTERN}
    });
    base["$defs"]["PlatformDecl"]["properties"]["sdkApiVersion"]["const"] =
        json!(crate::API_VERSION);
    base["$defs"]["ConfigurationDecl"]["properties"]["version"]["const"] = json!(1);
    let definitions = base
        .as_object_mut()
        .expect("schema object")
        .remove("$defs")
        .unwrap_or(json!({}));
    let mut branches = Vec::new();
    for rule in &RUNTIME_RULES {
        let mut branch = base.clone();
        branch
            .as_object_mut()
            .expect("schema object")
            .remove("$schema");
        branch
            .as_object_mut()
            .expect("schema object")
            .remove("title");
        branch["properties"]["runtime"] = json!({"const":rule.runtime});
        let properties = branch["properties"]
            .as_object_mut()
            .expect("schema properties");
        for field in rule.forbidden {
            properties.remove(*field);
        }
        for field in rule.required {
            let required = branch["required"].as_array_mut().expect("required fields");
            if !required.contains(&json!(field)) {
                required.push(json!(field));
            }
        }
        if !rule.allow_secrets {
            let mut config = definitions["ConfigurationDecl"].clone();
            let mut field = definitions["ConfigurationField"].clone();
            field["properties"]["secret"] = json!({"const":false});
            config["properties"]["fields"]["items"] = field;
            branch["properties"]["configuration"] = config;
        }
        if rule.runtime == PandaRuntime::Node {
            let mut documentation = definitions["DocumentationDecl"].clone();
            documentation["properties"]
                .as_object_mut()
                .expect("documentation properties")
                .remove("database");
            branch["properties"]["documentation"] = documentation;
        }
        branches.push(branch);
    }
    json!({"$schema":"https://json-schema.org/draft/2020-12/schema","title":"PandaManifest","description":"Generated from platform-core's strict panda.json declarations; filesystem and configuration references are checked by Panda.","oneOf":branches,"$defs":definitions})
}
