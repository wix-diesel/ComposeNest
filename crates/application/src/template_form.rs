//! Ordered, secret-free form definitions shared by create and clone previews.

use serde::Serialize;
use serde_json::Value;

use crate::create_plan::entries;

/// One validated input definition; candidate values live in the plan preview.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormInput {
    /// Stable input key used in field paths and edits.
    pub key: String,
    /// Plain-text display label.
    pub label: String,
    /// Schema 1 input type.
    pub input_type: String,
    /// Whether absence is prohibited; empty strings are validated separately.
    pub required: bool,
    /// Optional plain-text explanation.
    pub description: Option<String>,
    /// Selected Version constraints, interpreted authoritatively by Core.
    pub validation: Value,
    /// Select choices in declaration order.
    pub options: Vec<FormOption>,
    /// Whether secret-v1 generation is allowed by the selected definition.
    pub can_generate: bool,
}

/// One select choice from the validated Template.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormOption {
    /// Submitted choice value.
    pub value: String,
    /// Plain-text display label.
    pub label: String,
}

/// A port or writable storage slot from the selected Version.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormSlot {
    /// Stable slot key.
    pub key: String,
    /// Plain-text display label.
    pub label: String,
    /// Container destination, never an allocated host resource.
    pub container: Value,
}

/// A connection description referencing active input and port slots.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormConnection {
    /// Stable connection key.
    pub key: String,
    /// Plain-text display label.
    pub label: String,
    /// Port slot to display with the Core port candidate.
    pub port: String,
    /// Relevant input keys in declaration order, including masked secrets.
    pub inputs: Vec<String>,
}

/// Form metadata projected only from a registered revision or private Snapshot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateForm {
    /// Plain-text Template name.
    pub name: String,
    /// Package revision, distinct from the selected service Version.
    pub template_version: String,
    /// Plain-text Template explanation.
    pub description: String,
    /// Active inputs in Template display order.
    pub inputs: Vec<FormInput>,
    /// Active port slots in Template display order.
    pub ports: Vec<FormSlot>,
    /// Active storage slots in Template display order.
    pub storage: Vec<FormSlot>,
    /// Active connection descriptions in Template display order.
    pub connections: Vec<FormConnection>,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

pub(crate) fn project_form(template: &Value, definition: &Value) -> TemplateForm {
    let slots = |value: &Value| {
        entries(value)
            .into_iter()
            .map(|(key, item)| FormSlot {
                label: text(&item["label"]),
                container: item["container"].clone(),
                key,
            })
            .collect::<Vec<_>>()
    };
    let ports = slots(&definition["service"]["ports"]);
    let connections = if let Some(connections) = definition.get("connections") {
        entries(connections)
            .into_iter()
            .map(|(key, item)| FormConnection {
                key,
                label: text(&item["label"]),
                port: text(&item["port"]),
                inputs: item["inputs"]
                    .as_array()
                    .map_or_else(Vec::new, |items| items.iter().map(text).collect()),
            })
            .collect()
    } else {
        ports
            .iter()
            .map(|slot| FormConnection {
                key: slot.key.clone(),
                label: slot.label.clone(),
                port: slot.key.clone(),
                inputs: Vec::new(),
            })
            .collect()
    };
    TemplateForm {
        name: text(&template["manifest"]["name"]),
        template_version: text(&template["manifest"]["templateVersion"]),
        description: text(&template["manifest"]["description"]),
        inputs: entries(&definition["inputs"])
            .into_iter()
            .map(|(key, input)| FormInput {
                key,
                label: text(&input["label"]),
                input_type: text(&input["type"]),
                required: input["required"] != false,
                description: input["description"].as_str().map(str::to_owned),
                validation: input
                    .get("validation")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({})),
                options: input["options"].as_array().map_or_else(Vec::new, |items| {
                    items
                        .iter()
                        .map(|item| FormOption {
                            value: text(&item["value"]),
                            label: text(&item["label"]),
                        })
                        .collect()
                }),
                can_generate: input["type"] == "secret"
                    && input["validation"].get("pattern").is_none()
                    && input["validation"]["minLength"].as_u64().unwrap_or(1) <= 32
                    && input["validation"]["maxLength"].as_u64().unwrap_or(4096) >= 32,
            })
            .collect(),
        storage: slots(&definition["service"]["storage"]),
        ports,
        connections,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn input_metadata_never_contains_defaults_or_secret_candidates() {
        let definition = json!({"inputs": {"order": ["secret", "count", "flag"], "values": {
            "secret": {"label": "Password", "type": "secret", "description": "Explicit input", "validation": {"minLength": 33}},
            "count": {"label": "Count", "type": "integer", "required": false, "default": 0, "validation": {"min": -10, "max": 10}},
            "flag": {"label": "Flag", "type": "boolean", "default": false}
        }}});
        let form = project_form(&json!({}), &definition);
        assert_eq!(
            form.inputs[0].description.as_deref(),
            Some("Explicit input")
        );
        assert!(!form.inputs[0].can_generate);
        assert!(!form.inputs[1].required);
        assert!(form.inputs[2].required);
        assert_eq!(form.inputs[1].validation["min"], -10);
        assert!(!serde_json::to_string(&form).unwrap().contains("default"));
        assert_eq!(
            serde_json::to_value(&form).unwrap()["inputs"][1]["inputType"],
            "integer"
        );
    }

    #[test]
    fn connection_order_and_omission_differ_from_an_empty_map() {
        let template = json!({"manifest": {"name": "<b>Plain text</b>", "templateVersion": "1.0.0", "description": "Description"}});
        let mut definition = json!({"service": {"ports": {
            "order": ["z", "a"], "values": {
                "a": {"label": "A", "container": 1234},
                "z": {"label": "Z", "container": 5678}
            }
        }}});
        let form = project_form(&template, &definition);
        assert_eq!(form.name, "<b>Plain text</b>");
        assert_eq!(
            form.ports
                .iter()
                .map(|slot| slot.key.as_str())
                .collect::<Vec<_>>(),
            ["z", "a"]
        );
        assert_eq!(form.connections[0].port, "z");
        definition["connections"] = json!({"order": [], "values": {}});
        assert!(project_form(&template, &definition).connections.is_empty());
        definition["connections"] = json!({"order": ["second", "first"], "values": {
            "first": {"label": "First", "port": "a", "inputs": []},
            "second": {"label": "Second", "port": "z", "inputs": ["password", "user"]}
        }});
        let form = project_form(&template, &definition);
        assert_eq!(form.connections[0].key, "second");
        assert_eq!(form.connections[0].inputs, ["password", "user"]);
    }
}
