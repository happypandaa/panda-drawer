//! Optional presentation-only translations. Identifiers and capabilities always
//! come from the original declaration, never from a localized display projection.
use super::{invalid, present, text, AppDecl, ManifestResult, PandaManifest};
use schemars::JsonSchema;
use serde::{
    de::{MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use std::{collections::BTreeMap, fmt, marker::PhantomData};

fn present_map<'de, D, T>(deserializer: D) -> Result<Option<BTreeMap<String, T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct UniqueMap<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for UniqueMap<T> {
        type Value = BTreeMap<String, T>;
        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an object without duplicate keys")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut values = BTreeMap::new();
            while let Some(key) = map.next_key::<String>()? {
                if values.contains_key(&key) {
                    return Err(serde::de::Error::custom(format!("duplicate field `{key}`")));
                }
                values.insert(key, map.next_value()?);
            }
            Ok(values)
        }
    }
    deserializer
        .deserialize_map(UniqueMap(PhantomData))
        .map(Some)
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Localization {
    #[serde(rename = "zh-CN")]
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub zh_cn: Option<LocalizedContent>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub en: Option<LocalizedContent>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalizedContent {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub app: Option<LocalizedApp>,
    #[serde(
        default,
        deserialize_with = "present_map",
        skip_serializing_if = "Option::is_none"
    )]
    pub configuration: Option<BTreeMap<String, LocalizedConfigurationField>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub runtime_requirements: Option<LocalizedRuntimeRequirements>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalizedApp {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalizedConfigurationField {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub label: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
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
    pub how_to_get: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "present_map",
        skip_serializing_if = "Option::is_none"
    )]
    pub options: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalizedRuntimeRequirements {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub network: Option<LocalizedNetworkRequirements>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub devices: Option<LocalizedDeviceRequirements>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalizedNetworkRequirements {
    #[serde(
        default,
        deserialize_with = "present_map",
        skip_serializing_if = "Option::is_none"
    )]
    pub connect: Option<BTreeMap<String, LocalizedNetworkRequirement>>,
    #[serde(
        default,
        deserialize_with = "present_map",
        skip_serializing_if = "Option::is_none"
    )]
    pub images: Option<BTreeMap<String, LocalizedNetworkRequirement>>,
    #[serde(
        default,
        deserialize_with = "present_map",
        skip_serializing_if = "Option::is_none"
    )]
    pub media: Option<BTreeMap<String, LocalizedNetworkRequirement>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalizedNetworkRequirement {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub purpose: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub data_sent: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalizedDeviceRequirements {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub camera: Option<LocalizedDeviceRequirement>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub microphone: Option<LocalizedDeviceRequirement>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalizedDeviceRequirement {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub purpose: Option<String>,
}

impl Localization {
    pub fn get(&self, locale: &str) -> Option<&LocalizedContent> {
        match locale {
            "zh-CN" => self.zh_cn.as_ref(),
            "en" => self.en.as_ref(),
            _ => None,
        }
    }
    pub(super) fn validate(&self, manifest: &PandaManifest) -> ManifestResult<()> {
        for (locale, content) in [("zh-CN", &self.zh_cn), ("en", &self.en)] {
            if let Some(content) = content {
                content.validate(manifest, &format!("localization.{locale}"))?;
            }
        }
        Ok(())
    }
}

fn optional_text(path: &str, value: &Option<String>, limit: usize) -> ManifestResult<()> {
    if value.as_ref().is_some_and(|value| !text(value, limit)) {
        return Err(invalid(
            path,
            &format!("译文须为非空文本、不含 NUL 且不超过 {limit} UTF-8 字节"),
        ));
    }
    Ok(())
}
fn purpose(path: &str, value: &Option<String>) -> ManifestResult<()> {
    if value
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.chars().any(char::is_control))
    {
        return Err(invalid(
            path,
            "用途及发送数据译文须为非空文本且不含控制字符",
        ));
    }
    Ok(())
}
impl LocalizedContent {
    fn validate(&self, manifest: &PandaManifest, path: &str) -> ManifestResult<()> {
        if let Some(app) = &self.app {
            optional_text(&format!("{path}.app.name"), &app.name, 240)?;
            optional_text(&format!("{path}.app.description"), &app.description, 4000)?;
        }
        if let Some(fields) = &self.configuration {
            for (name, translated) in fields {
                let path = format!("{path}.configuration.{name}");
                let original = manifest
                    .configuration
                    .as_ref()
                    .and_then(|config| config.fields.iter().find(|field| field.name == *name))
                    .ok_or_else(|| invalid(&path, "引用的配置字段 name 不存在"))?;
                optional_text(&format!("{path}.label"), &translated.label, 160)?;
                optional_text(
                    &format!("{path}.description"),
                    &translated.description,
                    4000,
                )?;
                optional_text(
                    &format!("{path}.placeholder"),
                    &translated.placeholder,
                    1000,
                )?;
                if let Some(steps) = &translated.how_to_get {
                    if steps.is_empty() || steps.len() > 12 {
                        return Err(invalid(
                            &format!("{path}.howToGet"),
                            "须提供 1 至 12 个获取步骤",
                        ));
                    }
                    for (index, step) in steps.iter().enumerate() {
                        if !text(step, 2000) {
                            return Err(invalid(
                                &format!("{path}.howToGet[{index}]"),
                                "获取步骤译文须为非空文本、不含 NUL 且不超过 2000 UTF-8 字节",
                            ));
                        }
                    }
                }
                if let Some(options) = &translated.options {
                    for (value, label) in options {
                        let path = format!("{path}.options.{value}");
                        if !original.options.as_ref().is_some_and(|options| {
                            options.iter().any(|option| option.value == *value)
                        }) {
                            return Err(invalid(&path, "引用的配置选项 value 不存在"));
                        }
                        if !text(label, 160) {
                            return Err(invalid(
                                &path,
                                "选项译文须为非空文本、不含 NUL 且不超过 160 UTF-8 字节",
                            ));
                        }
                    }
                }
            }
        }
        if let Some(requirements) = &self.runtime_requirements {
            if let Some(network) = &requirements.network {
                for (group, translations) in [
                    ("connect", &network.connect),
                    ("images", &network.images),
                    ("media", &network.media),
                ] {
                    if let Some(translations) = translations {
                        for (origin, translation) in translations {
                            let path =
                                format!("{path}.runtimeRequirements.network.{group}.{origin}");
                            let exists =
                                manifest
                                    .runtime_requirements
                                    .as_ref()
                                    .is_some_and(|policy| {
                                        let entries = match group {
                                            "connect" => &policy.network.connect,
                                            "images" => &policy.network.images,
                                            _ => &policy.network.media,
                                        };
                                        entries.iter().any(|entry| entry.origin == *origin)
                                    });
                            if !exists {
                                return Err(invalid(&path, "引用的网络 origin 不存在于该分组"));
                            }
                            purpose(&format!("{path}.purpose"), &translation.purpose)?;
                            purpose(&format!("{path}.dataSent"), &translation.data_sent)?;
                        }
                    }
                }
            }
            if let Some(devices) = &requirements.devices {
                for (device, translation) in [
                    ("camera", &devices.camera),
                    ("microphone", &devices.microphone),
                ] {
                    if let Some(translation) = translation {
                        let path = format!("{path}.runtimeRequirements.devices.{device}");
                        let exists = manifest
                            .runtime_requirements
                            .as_ref()
                            .is_some_and(|policy| match device {
                                "camera" => policy.devices.camera.is_some(),
                                _ => policy.devices.microphone.is_some(),
                            });
                        if !exists {
                            return Err(invalid(&path, "引用的设备声明不存在"));
                        }
                        purpose(&format!("{path}.purpose"), &translation.purpose)?;
                    }
                }
            }
        }
        Ok(())
    }
}

impl PandaManifest {
    /// Platform display metadata only; callers retain the original declaration.
    /// This does not select a language for an application's own UI or SDK.
    pub fn localized_app(&self, locale: &str) -> AppDecl {
        let mut app = self.app.clone();
        if let Some(translation) = self
            .localization
            .as_ref()
            .and_then(|translations| translations.get(locale))
            .and_then(|content| content.app.as_ref())
        {
            if let Some(name) = &translation.name {
                app.name.clone_from(name);
            }
            if let Some(description) = &translation.description {
                app.description = Some(description.clone());
            }
        }
        app
    }
}
