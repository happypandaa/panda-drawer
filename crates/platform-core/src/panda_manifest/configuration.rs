//! Shared author-declared configuration. Saved values and credentials live elsewhere.
use super::{https, invalid, present, text, ManifestResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(untagged)]
enum ConfigurationDefault {
    String(String),
    Integer(i64),
    Boolean(bool),
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationDecl {
    pub version: u32,
    #[schemars(length(max = 32))]
    pub fields: Vec<ConfigurationField>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConfigurationType {
    String,
    Url,
    Integer,
    Boolean,
    Select,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(transform = configuration_field_schema)]
pub struct ConfigurationField {
    pub name: String,
    pub label: String,
    pub r#type: ConfigurationType,
    pub required: bool,
    pub secret: bool,
    pub description: String,
    #[schemars(length(min = 1, max = 12))]
    pub how_to_get: Vec<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub placeholder: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<ConfigurationDefault>")]
    pub default: Option<Value>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub endpoint_field: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub help_url: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub options: Option<Vec<SelectOption>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

pub fn valid_business_variable(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 96
        || !bytes[0].is_ascii_uppercase()
        || !bytes
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
    {
        return false;
    }
    if [
        "APP_",
        "PLATFORM_",
        "PANDA_DRAWER_",
        "NODE_",
        "LD_",
        "DYLD_",
        "VITE_",
        "LC_",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
    {
        return false;
    }
    ![
        "PATH",
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "TMP",
        "TEMP",
        "TMPDIR",
        "HOST",
        "PORT",
        "LANG",
        "USER",
        "LOGNAME",
        "SHELL",
        "COMSPEC",
        "SYSTEMROOT",
        "WINDIR",
        "BASH_ENV",
        "ENV",
        "PYTHONPATH",
        "PYTHONHOME",
    ]
    .contains(&name)
}

impl ConfigurationDecl {
    pub(crate) fn validate(&self) -> ManifestResult<()> {
        if self.version != 1 {
            return Err(invalid("configuration.version", "仅支持配置声明版本 1"));
        }
        if self.fields.len() > 32 {
            return Err(invalid("configuration.fields", "最多声明 32 项配置"));
        }
        let mut names = HashSet::new();
        for (index, field) in self.fields.iter().enumerate() {
            let path = format!("configuration.fields[{index}]");
            let error = |name: &str, reason: &str| invalid(&format!("{path}.{name}"), reason);
            if !valid_business_variable(&field.name) {
                return Err(error("name", "变量名无效或占用了平台保留名称"));
            }
            if !names.insert(&field.name) {
                return Err(error("name", "变量名重复"));
            }
            if !text(&field.label, 160) {
                return Err(error("label", "名称须为非空文本且不超过 160 UTF-8 字节"));
            }
            if !text(&field.description, 4000) {
                return Err(error(
                    "description",
                    "用途须为非空文本且不超过 4000 UTF-8 字节",
                ));
            }
            if field.how_to_get.is_empty() || field.how_to_get.len() > 12 {
                return Err(error("howToGet", "须提供 1 至 12 个获取步骤"));
            }
            for (step, value) in field.how_to_get.iter().enumerate() {
                if !text(value, 2000) {
                    return Err(error(
                        &format!("howToGet[{step}]"),
                        "获取步骤须为非空文本且不超过 2000 UTF-8 字节",
                    ));
                }
            }
            if field
                .placeholder
                .as_ref()
                .is_some_and(|value| !text(value, 1000))
            {
                return Err(error("placeholder", "输入提示无效或超过 1000 UTF-8 字节"));
            }
            if field.help_url.as_ref().is_some_and(|value| !https(value)) {
                return Err(error("helpUrl", "须为不含账号、查询或片段的 HTTPS 地址"));
            }
            if field.secret && field.r#type != ConfigurationType::String {
                return Err(error("type", "秘密配置只支持 string 类型"));
            }
            if field.secret && field.default.is_some() {
                return Err(error("default", "秘密配置不能提供默认值"));
            }
            match (field.r#type, &field.options) {
                (ConfigurationType::Select, Some(options))
                    if !options.is_empty() && options.len() <= 32 =>
                {
                    let mut values = HashSet::new();
                    for (option_index, option) in options.iter().enumerate() {
                        if !text(&option.label, 160) {
                            return Err(error(
                                &format!("options[{option_index}].label"),
                                "选项名称无效",
                            ));
                        }
                        if !text(&option.value, 1000) || !values.insert(&option.value) {
                            return Err(error(
                                &format!("options[{option_index}].value"),
                                "选项值无效或重复",
                            ));
                        }
                    }
                }
                (ConfigurationType::Select, _) => {
                    return Err(error("options", "select 须提供 1 至 32 个选项"))
                }
                (_, Some(_)) => return Err(error("options", "只有 select 可以声明选项")),
                _ => {}
            }
            if let Some(default) = &field.default {
                let valid = match field.r#type {
                    ConfigurationType::String => {
                        default.as_str().is_some_and(|value| text(value, 4000))
                    }
                    ConfigurationType::Url => default.as_str().is_some_and(https),
                    ConfigurationType::Integer => default.as_i64().is_some(),
                    ConfigurationType::Boolean => default.is_boolean(),
                    ConfigurationType::Select => default.as_str().is_some_and(|value| {
                        field.options.as_ref().is_some_and(|options| {
                            options.iter().any(|option| option.value == value)
                        })
                    }),
                };
                if !valid {
                    return Err(error("default", "默认值与配置类型或选项不一致"));
                }
            }
        }
        for (index, field) in self.fields.iter().enumerate() {
            if let Some(endpoint) = &field.endpoint_field {
                if !field.secret
                    || !self.fields.iter().any(|candidate| {
                        candidate.name == *endpoint
                            && candidate.r#type == ConfigurationType::Url
                            && !candidate.secret
                    })
                {
                    return Err(invalid(
                        &format!("configuration.fields[{index}].endpointField"),
                        "秘密凭据须引用本声明中有效的非秘密 url 字段",
                    ));
                }
            }
        }
        Ok(())
    }
}

// These conditional structural rules are exported with the declaration itself;
// cross-field references, URL semantics and byte budgets remain runtime checks.
fn configuration_field_schema(schema: &mut schemars::Schema) {
    let mut rules = vec![
        json!({"if":{"properties":{"secret":{"const":true}}},"then":{"properties":{"type":{"const":"string"}},"not":{"required":["default"]}}}),
        json!({"if":{"properties":{"type":{"const":"select"}}},"then":{"required":["options"],"properties":{"options":{"minItems":1,"maxItems":32}}},"else":{"not":{"required":["options"]}}}),
    ];
    for (kind, default_type) in [
        (ConfigurationType::String, "string"),
        (ConfigurationType::Url, "string"),
        (ConfigurationType::Integer, "integer"),
        (ConfigurationType::Boolean, "boolean"),
        (ConfigurationType::Select, "string"),
    ] {
        rules.push(json!({"if":{"properties":{"type":{"const":kind}}},"then":{"properties":{"default":{"type":default_type}}}}));
    }
    schema.insert("allOf".into(), json!(rules));
}
