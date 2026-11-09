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
    base["$defs"]["AgentDecl"]["properties"]["instructions"] = json!({
        "type": "string",
        "minLength": 1,
        "maxLength": 1024,
        "pattern": AGENT_INSTRUCTIONS_PATTERN
    });
    base["$defs"]["ConfigurationDecl"]["properties"]["version"]["const"] = json!(1);
    // A backend is optional. Without one every configuration value is public.
    let mut public_configuration = base["$defs"]["ConfigurationDecl"].clone();
    let mut public_field = base["$defs"]["ConfigurationField"].clone();
    public_field["properties"]["secret"] = json!({"const": false});
    public_configuration["properties"]["fields"]["items"] = public_field;
    base["allOf"] = json!([{
        "if": {"not": {"required": ["backend"]}},
        "then": {"properties": {"configuration": public_configuration}}
    }]);
    base["description"] = json!("Generated from platform-core's strict panda.json v2 declarations; filesystem and configuration references are checked by Panda.");
    base
}
