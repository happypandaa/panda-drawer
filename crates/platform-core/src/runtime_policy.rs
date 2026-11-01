//! Declarative browser policy. This is disclosure and browser enforcement, not isolation
//! between arbitrary same-origin project scripts or a network egress firewall.
use crate::ApiError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::{Host, Url};

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimePolicy {
    #[serde(default)]
    pub network: NetworkRequirements,
    #[serde(default)]
    pub devices: DeviceRequirements,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NetworkRequirements {
    #[serde(default)]
    pub connect: Vec<NetworkRequirement>,
    #[serde(default)]
    pub images: Vec<NetworkRequirement>,
    #[serde(default)]
    pub media: Vec<NetworkRequirement>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkRequirement {
    pub origin: String,
    pub purpose: String,
    pub data_sent: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeviceRequirements {
    #[serde(
        default,
        deserialize_with = "present_device",
        skip_serializing_if = "Option::is_none"
    )]
    pub microphone: Option<DeviceRequirement>,
    #[serde(
        default,
        deserialize_with = "present_device",
        skip_serializing_if = "Option::is_none"
    )]
    pub camera: Option<DeviceRequirement>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeviceRequirement {
    pub purpose: String,
}

fn present_device<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

impl RuntimePolicy {
    /// `value` is the manifest's runtimeRequirements member, not the whole manifest.
    /// A missing member deserializes as null for backwards-compatible local projects.
    pub fn parse(value: &Value) -> Result<Self, ApiError> {
        if value.is_null() {
            return Ok(Self::default());
        }
        let object = value
            .as_object()
            .ok_or_else(|| invalid("runtimeRequirements must be an object"))?;
        for group in ["network", "devices"] {
            if object.get(group).is_some_and(|value| !value.is_object()) {
                return Err(invalid(
                    "Runtime network and devices declarations must be objects",
                ));
            }
        }
        if let Some(network) = value.get("network").and_then(Value::as_object) {
            if network.values().any(|entries| {
                entries
                    .as_array()
                    .is_none_or(|entries| entries.iter().any(|entry| !entry.is_object()))
            }) {
                return Err(invalid(
                    "Runtime network groups must be arrays of declaration objects",
                ));
            }
        }
        if let Some(devices) = value.get("devices").and_then(Value::as_object) {
            if devices.values().any(|device| !device.is_object()) {
                return Err(invalid(
                    "Declared devices require a purpose object; omit unused devices",
                ));
            }
        }
        let mut policy: Self = serde_json::from_value(value.clone()).map_err(|_| {
            invalid(
                "runtimeRequirements must contain only network arrays and purpose-bearing devices",
            )
        })?;
        for (group, entries) in [
            ("connect", &mut policy.network.connect),
            ("images", &mut policy.network.images),
            ("media", &mut policy.network.media),
        ] {
            let mut seen = std::collections::HashSet::new();
            for entry in entries {
                validate_origin(&entry.origin).map_err(|error| error.detail("group", group))?;
                if !seen.insert(entry.origin.clone()) {
                    return Err(
                        invalid("Duplicate origin in runtimeRequirements network group")
                            .detail("group", group),
                    );
                }
                entry.purpose = required_text(&entry.purpose, "purpose")?;
                entry.data_sent = required_text(&entry.data_sent, "dataSent")?;
            }
        }
        for requirement in [&mut policy.devices.microphone, &mut policy.devices.camera]
            .into_iter()
            .flatten()
        {
            requirement.purpose = required_text(&requirement.purpose, "purpose")?;
        }
        Ok(policy)
    }

    pub fn requirements(&self) -> Value {
        json!(self)
    }

    pub fn devices(&self) -> &DeviceRequirements {
        &self.devices
    }

    pub fn content_security_policy(&self) -> String {
        let append = |entries: &[NetworkRequirement]| {
            entries
                .iter()
                .map(|entry| format!(" {}", entry.origin))
                .collect::<String>()
        };
        format!(
            "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:{}; media-src 'self' blob:{}; connect-src 'self'{}; font-src 'self'; worker-src 'self' blob:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'",
            append(&self.network.images), append(&self.network.media), append(&self.network.connect)
        )
    }

    pub fn permissions_policy(&self) -> String {
        let value = |allowed| if allowed { "(self)" } else { "()" };
        format!(
            "camera={}, microphone={}, geolocation=(), display-capture=()",
            value(self.devices.camera.is_some()),
            value(self.devices.microphone.is_some())
        )
    }
}

fn invalid(message: &str) -> ApiError {
    ApiError::new("INVALID_RUNTIME_REQUIREMENTS", message)
}

fn required_text(value: &str, field: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(|character| character.is_control()) {
        return Err(
            invalid("Runtime declarations require non-empty purpose/dataSent text")
                .detail("field", field),
        );
    }
    Ok(value.to_owned())
}

fn validate_origin(origin: &str) -> Result<(), ApiError> {
    let uri =
        Url::parse(origin).map_err(|_| invalid("Network origin must be an exact HTTPS origin"))?;
    let Some(Host::Domain(host)) = uri.host() else {
        return Err(invalid(
            "Network origin must use a public DNS hostname, not an IP address",
        ));
    };
    let reserved = [
        "localhost",
        "local",
        "internal",
        "lan",
        "home",
        "corp",
        "arpa",
        "invalid",
        "test",
    ];
    let labels: Vec<_> = host.split('.').collect();
    if uri.scheme() != "https"
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.query().is_some()
        || uri.fragment().is_some()
        || origin != uri.origin().ascii_serialization()
        || labels.len() < 2
        || labels.iter().any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
        || reserved
            .iter()
            .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
        || !labels.last().is_some_and(|label| {
            label
                .chars()
                .any(|character| character.is_ascii_alphabetic())
        })
    {
        return Err(invalid("Network origin must be canonical HTTPS with a public hostname and no path, credentials, wildcard, query or fragment"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_requirements_are_local_only_and_devices_denied() {
        for value in [Value::Null, json!({}), json!({"network":{},"devices":{}})] {
            let policy = RuntimePolicy::parse(&value).unwrap();
            assert_eq!(policy, RuntimePolicy::default());
            assert!(policy
                .permissions_policy()
                .contains("camera=(), microphone=()"));
            assert!(!policy.content_security_policy().contains("https:"));
            assert_eq!(
                policy.requirements(),
                json!({"network":{"connect":[],"images":[],"media":[]},"devices":{}})
            );
        }
    }

    #[test]
    fn network_groups_do_not_grant_other_resource_types() {
        let entry =
            |origin| json!({"origin":origin,"purpose":" public lookup ","dataSent":"search text"});
        let policy = RuntimePolicy::parse(&json!({"network":{
            "connect":[entry("https://api.example.com")],
            "images":[entry("https://images.example.com")],
            "media":[entry("https://media.example.com:8443")]
        },"devices":{"microphone":{"purpose":"Record a spoken note"}}}))
        .unwrap();
        let directives: Vec<_> = policy
            .content_security_policy()
            .split(';')
            .map(str::to_owned)
            .collect();
        assert!(directives
            .iter()
            .any(|line| line.trim() == "connect-src 'self' https://api.example.com"));
        assert!(directives
            .iter()
            .any(|line| line.trim() == "img-src 'self' data: blob: https://images.example.com"));
        assert!(directives
            .iter()
            .any(|line| line.trim() == "media-src 'self' blob: https://media.example.com:8443"));
        assert_eq!(policy.network.connect[0].purpose, "public lookup");
        assert!(policy
            .permissions_policy()
            .contains("camera=(), microphone=(self)"));
    }

    #[test]
    fn unsafe_or_non_origin_urls_are_rejected() {
        for origin in [
            "http://api.example.com",
            "https://api.example.com/",
            "https://api.example.com/path",
            "https://api.example.com?key=secret",
            "https://api.example.com#fragment",
            "https://user:pass@api.example.com",
            "https://*.example.com",
            "https://127.0.0.1",
            "https://[::1]",
            "https://192.168.1.1",
            "https://2130706433",
            "https://localhost",
            "https://a.localhost",
            "https://a.local",
            "https://a.internal",
            "https://home.arpa",
            "https://printer.lan",
            "https://example.com.",
            "https://-bad.example.com",
            "https://api.example.com:443",
            "https://API.example.com",
            "https://api.example.com;script-src *",
        ] {
            assert!(
                RuntimePolicy::parse(
                    &json!({"network":{"connect":[{"origin":origin,"purpose":"p","dataSent":"d"}]}})
                )
                .is_err(),
                "{origin}"
            );
        }
    }

    #[test]
    fn malformed_declarations_do_not_silently_grant_permissions() {
        for value in [
            json!([]),
            json!(true),
            json!({"network":null}),
            json!({"network":[]}),
            json!({"devices":[]}),
            json!({"devices":{"camera":["purpose"]}}),
            json!({"network":{"connect":[["https://example.com","purpose","data"]]}}),
            json!({"devices":false}),
            json!({"network":{"connect":{}}}),
            json!({"network":{"connect":[{"origin":"https://example.com","purpose":"p"}]}}),
            json!({"network":{"images":[{"origin":"https://example.com","purpose":" ","dataSent":"d"}]}}),
            json!({"devices":{"camera":{}}}),
            json!({"devices":{"camera":null}}),
            json!({"devices":{"microphone":{"purpose":"\n"}}}),
            json!({"devices":{"screen":{"purpose":"share"}}}),
            json!({"network":{"scripts":[]}}),
            json!({"typo":{}}),
        ] {
            assert!(RuntimePolicy::parse(&value).is_err(), "{value}");
        }
    }

    #[test]
    fn duplicate_declarations_are_rejected_but_groups_are_independent() {
        let entry = json!({"origin":"https://example.com","purpose":"p","dataSent":"d"});
        assert!(RuntimePolicy::parse(
            &json!({"network":{"connect":[entry.clone(),entry.clone()]}})
        )
        .is_err());
        assert!(RuntimePolicy::parse(
            &json!({"network":{"connect":[entry.clone()],"images":[entry]}})
        )
        .is_ok());
    }
}
