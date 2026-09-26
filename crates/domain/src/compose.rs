//! Compose generation from an immutable, validated template snapshot.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use yaml_rust2::{Yaml, YamlEmitter};

use crate::{
    identity::InstanceId,
    instance::StoragePresence,
    template::{Node, ResolvedTemplate, Value},
};

/// A confirmed input value; absent optional values are omitted only where allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputValue {
    String(String),
    Integer(i64),
    Boolean(bool),
}

impl InputValue {
    fn text(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Integer(value) => value.to_string(),
            Self::Boolean(value) => value.to_string(),
        }
    }
}

/// A previously verified storage allocation, never created by the generator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Storage {
    /// An existing dedicated absolute host directory.
    Bind(String),
    /// An explicitly created volume with verified ownership.
    Volume {
        name: String,
        presence: StoragePresence,
    },
}

/// Values committed with the selected snapshot and its spec.
pub struct ConfirmedCompose<'a> {
    /// Immutable snapshot containing every complete version.
    pub snapshot: &'a ResolvedTemplate,
    /// Selected version key in that snapshot.
    pub version: &'a str,
    /// Instance identity used for the dedicated project and resource labels.
    pub instance_id: InstanceId,
    /// Fixed management scope for ownership inspection.
    pub scope_id: &'a str,
    /// Confirmed spec revision.
    pub spec_revision: u64,
    /// Confirmed inputs, including existing secret values.
    pub inputs: &'a BTreeMap<String, InputValue>,
    /// Reserved TCP host ports by slot.
    pub ports: &'a BTreeMap<String, u16>,
    /// Verified storage allocations by slot.
    pub storage: &'a BTreeMap<String, Storage>,
    /// Image reference from the selected complete definition.
    pub source_image: &'a str,
    /// Recorded immutable execution reference, including a sha256 digest.
    pub execution_image: &'a str,
    /// Recorded platform used when resolving the execution image.
    pub platform: &'a str,
}

/// A mismatch between the confirmed records and the selected definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeError {
    MissingVersion,
    InvalidSpec,
    InvalidImage,
    InvalidAllocation,
    Serialization,
}

/// A typed standard Compose document for one isolated instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeModel {
    /// Dedicated project name.
    pub name: String,
    /// Single service with fixed network and host exposure rules.
    pub service: ComposeService,
    /// Explicitly created external volume names.
    pub volumes: Vec<String>,
}

/// Runtime configuration for the only service named main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeService {
    /// Pinned image execution reference.
    pub image: String,
    /// Resolved Linux platform.
    pub platform: String,
    /// Environment values as actual strings, before Compose escaping.
    pub environment: BTreeMap<String, String>,
    /// Optional exec argv.
    pub command: Option<Vec<String>>,
    /// Exec healthcheck argv, without shell interpretation.
    pub healthcheck: Vec<String>,
    /// Timing settings in seconds and retry count.
    pub health_timing: [i64; 4],
    /// Fixed localhost TCP bindings.
    pub ports: Vec<(u16, u16)>,
    /// Dedicated writable mounts.
    pub mounts: Vec<(Storage, String)>,
    /// Template-authorized restart policy.
    pub restart: String,
    /// Optional byte limit.
    pub mem_limit: Option<i64>,
    /// Ownership and revision evidence.
    pub labels: BTreeMap<String, String>,
}

/// Builds a model solely from the selected snapshot and confirmed records.
pub fn generate(source: &ConfirmedCompose<'_>) -> Result<ComposeModel, ComposeError> {
    let version = source
        .snapshot
        .versions
        .iter()
        .find(|version| version.key == source.version)
        .ok_or(ComposeError::MissingVersion)?;
    let repository = source.source_image.split('@').next().unwrap_or_default();
    let repository = repository
        .rsplit_once(':')
        .filter(|(_, suffix)| !suffix.contains('/'))
        .map_or(repository, |(name, _)| name);
    let valid_digest =
        source
            .execution_image
            .split_once("@sha256:")
            .is_some_and(|(name, digest)| {
                name == repository
                    && digest.len() == 64
                    && digest.bytes().all(|b| b.is_ascii_hexdigit())
            });
    if source.spec_revision == 0
        || source.scope_id.is_empty()
        || source.source_image != version.definition.image
        || !version
            .definition
            .platforms
            .iter()
            .any(|p| p == source.platform)
        || !valid_digest
    {
        return Err(ComposeError::InvalidImage);
    }
    let doc = &version.definition.document;
    let definitions = entries(optional(doc, "inputs"));
    if check_keys(
        source.inputs.keys().map(String::as_str),
        definitions.iter().map(|(k, _)| k.as_str()),
        false,
    )
    .is_err()
    {
        return Err(ComposeError::InvalidSpec);
    }
    for (key, definition) in definitions {
        let kind = string(field(definition, "type"));
        let valid = match (kind, source.inputs.get(key)) {
            ("string" | "select" | "secret", Some(InputValue::String(_)))
            | ("integer", Some(InputValue::Integer(_)))
            | ("boolean", Some(InputValue::Boolean(_))) => true,
            (_, None) => matches!(
                optional(definition, "required").map(|n| &n.value),
                Some(Value::Boolean(false))
            ),
            _ => false,
        };
        if !valid {
            return Err(ComposeError::InvalidSpec);
        }
    }
    let service = field(doc, "service");
    let mut environment = BTreeMap::new();
    for (key, value) in entries(optional(service, "environment")) {
        if let Some(value) = resolve(value, source.inputs)? {
            environment.insert(key.clone(), value);
        }
    }
    let command = optional(service, "command")
        .map(|node| resolve_argv(node, source.inputs))
        .transpose()?;
    let health = field(service, "healthcheck");
    let healthcheck = resolve_argv(field(health, "command"), source.inputs)?;
    let timing = [
        integer(optional(health, "intervalSeconds"), 5),
        integer(optional(health, "timeoutSeconds"), 3),
        integer(optional(health, "startPeriodSeconds"), 0),
        integer(optional(health, "retries"), 12),
    ];
    let mut ports = Vec::new();
    let port_slots = entries(optional(service, "ports"));
    check_keys(
        source.ports.keys().map(String::as_str),
        port_slots.iter().map(|(k, _)| k.as_str()),
        true,
    )?;
    let mut seen_ports = BTreeSet::new();
    for (slot, definition) in port_slots {
        let host = *source
            .ports
            .get(slot)
            .ok_or(ComposeError::InvalidAllocation)?;
        if host < 1024 || !seen_ports.insert(host) {
            return Err(ComposeError::InvalidAllocation);
        }
        ports.push((
            host,
            integer(Some(field(definition, "container")), 0) as u16,
        ));
    }
    let mut mounts = Vec::new();
    let mut volumes = Vec::new();
    let storage_slots = entries(optional(service, "storage"));
    check_keys(
        source.storage.keys().map(String::as_str),
        storage_slots.iter().map(|(k, _)| k.as_str()),
        true,
    )?;
    let mut seen_storage = BTreeSet::new();
    for (slot, definition) in storage_slots {
        let allocation = source
            .storage
            .get(slot)
            .ok_or(ComposeError::InvalidAllocation)?;
        let identity = match allocation {
            Storage::Bind(path)
                if Path::new(path).is_absolute()
                    && !path.chars().any(char::is_control)
                    && !Path::new(path)
                        .components()
                        .any(|part| part == std::path::Component::ParentDir) =>
            {
                path
            }
            Storage::Volume {
                name,
                presence: StoragePresence::Present,
            } if *name == format!("cn-{:032x}-{slot}", source.instance_id.as_u128()) => {
                volumes.push(name.clone());
                name
            }
            _ => return Err(ComposeError::InvalidAllocation),
        };
        if !seen_storage.insert(identity.clone()) {
            return Err(ComposeError::InvalidAllocation);
        }
        mounts.push((
            allocation.clone(),
            string(field(definition, "container")).to_owned(),
        ));
    }
    let mut labels = BTreeMap::new();
    labels.insert("io.composenest.scope".into(), source.scope_id.into());
    labels.insert(
        "io.composenest.instance".into(),
        format!("{:032x}", source.instance_id.as_u128()),
    );
    labels.insert(
        "io.composenest.spec-revision".into(),
        source.spec_revision.to_string(),
    );
    Ok(ComposeModel {
        name: source.instance_id.compose_project_name(),
        service: ComposeService {
            image: source.execution_image.to_owned(),
            platform: source.platform.to_owned(),
            environment,
            command,
            healthcheck,
            health_timing: timing,
            ports,
            mounts,
            restart: optional(service, "restart")
                .map(string)
                .unwrap_or("no")
                .to_owned(),
            mem_limit: optional(service, "memoryLimitMiB").map(|n| integer(Some(n), 0) * 1_048_576),
            labels,
        },
        volumes,
    })
}

fn check_keys<'a>(
    actual: impl Iterator<Item = &'a str>,
    expected: impl Iterator<Item = &'a str>,
    exact: bool,
) -> Result<(), ComposeError> {
    let actual: BTreeSet<_> = actual.collect();
    let expected: BTreeSet<_> = expected.collect();
    if !actual.is_subset(&expected) || (exact && actual != expected) {
        return Err(ComposeError::InvalidAllocation);
    }
    Ok(())
}

fn entries(node: Option<&Node>) -> &[(String, Node)] {
    match node.map(|n| &n.value) {
        Some(Value::Map(items)) => items,
        _ => &[],
    }
}
fn optional<'a>(node: &'a Node, name: &str) -> Option<&'a Node> {
    entries(Some(node))
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}
fn field<'a>(node: &'a Node, name: &str) -> &'a Node {
    optional(node, name).expect("validated snapshot has required fields")
}
fn string(node: &Node) -> &str {
    match &node.value {
        Value::String(value) => value,
        _ => unreachable!("validated string"),
    }
}
fn integer(node: Option<&Node>, default: i64) -> i64 {
    match node.map(|n| &n.value) {
        Some(Value::Integer(value)) => *value,
        _ => default,
    }
}
fn resolve(
    node: &Node,
    inputs: &BTreeMap<String, InputValue>,
) -> Result<Option<String>, ComposeError> {
    match &node.value {
        Value::String(value) => Ok(Some(value.clone())),
        Value::Integer(value) => Ok(Some(value.to_string())),
        Value::Boolean(value) => Ok(Some(value.to_string())),
        Value::Map(_) => {
            let name = string(field(node, "input"));
            match inputs.get(name) {
                Some(value) => Ok(Some(value.text())),
                None if optional(node, "onMissing").is_some() => Ok(None),
                None => Err(ComposeError::InvalidSpec),
            }
        }
        _ => Err(ComposeError::InvalidSpec),
    }
}
fn resolve_argv(
    node: &Node,
    inputs: &BTreeMap<String, InputValue>,
) -> Result<Vec<String>, ComposeError> {
    let Value::Sequence(items) = &node.value else {
        return Err(ComposeError::InvalidSpec);
    };
    items
        .iter()
        .map(|item| resolve(item, inputs)?.ok_or(ComposeError::InvalidSpec))
        .collect()
}

/// Serializes the typed model, escaping Compose interpolation exactly once.
pub fn to_yaml(model: &ComposeModel) -> Result<String, ComposeError> {
    let service = &model.service;
    let mut values = vec![
        ("image", scalar(&service.image)),
        ("platform", scalar(&service.platform)),
        (
            "labels",
            mapping(service.labels.iter().map(|(k, v)| (k.clone(), scalar(v)))),
        ),
        (
            "environment",
            mapping(
                service
                    .environment
                    .iter()
                    .map(|(k, v)| (k.clone(), scalar(v))),
            ),
        ),
        (
            "healthcheck",
            object(vec![
                (
                    "test",
                    array(
                        std::iter::once("CMD".to_owned())
                            .chain(service.healthcheck.iter().cloned()),
                    ),
                ),
                (
                    "interval",
                    scalar(&format!("{}s", service.health_timing[0])),
                ),
                ("timeout", scalar(&format!("{}s", service.health_timing[1]))),
                (
                    "start_period",
                    scalar(&format!("{}s", service.health_timing[2])),
                ),
                ("retries", Yaml::Integer(service.health_timing[3])),
            ]),
        ),
        ("restart", scalar(&service.restart)),
    ];
    if let Some(argv) = &service.command {
        values.push(("command", array(argv.iter().cloned())));
    }
    if let Some(bytes) = service.mem_limit {
        values.push(("mem_limit", Yaml::Integer(bytes)));
    }
    if !service.ports.is_empty() {
        values.push((
            "ports",
            Yaml::Array(
                service
                    .ports
                    .iter()
                    .map(|(host, target)| {
                        object(vec![
                            ("target", Yaml::Integer(i64::from(*target))),
                            ("published", scalar(&host.to_string())),
                            ("host_ip", scalar("127.0.0.1")),
                            ("protocol", scalar("tcp")),
                        ])
                    })
                    .collect(),
            ),
        ));
    }
    if !service.mounts.is_empty() {
        values.push((
            "volumes",
            Yaml::Array(
                service
                    .mounts
                    .iter()
                    .map(|(storage, target)| {
                        let (kind, identity) = match storage {
                            Storage::Bind(path) => ("bind", path),
                            Storage::Volume { name, .. } => ("volume", name),
                        };
                        let mut fields = vec![
                            ("type", scalar(kind)),
                            ("source", scalar(identity)),
                            ("target", scalar(target)),
                        ];
                        if kind == "bind" {
                            fields.push((
                                "bind",
                                object(vec![("create_host_path", Yaml::Boolean(false))]),
                            ));
                        }
                        object(fields)
                    })
                    .collect(),
            ),
        ));
    }
    let document = object(vec![
        ("name", scalar(&model.name)),
        ("services", object(vec![("main", object(values))])),
        ("networks", object(vec![("default", object(vec![]))])),
        (
            "volumes",
            mapping(model.volumes.iter().map(|name| {
                (
                    name.clone(),
                    object(vec![("external", Yaml::Boolean(true))]),
                )
            })),
        ),
    ]);
    let mut output = String::new();
    YamlEmitter::new(&mut output)
        .dump(&document)
        .map_err(|_| ComposeError::Serialization)?;
    Ok(output)
}

fn scalar(value: &str) -> Yaml {
    Yaml::String(value.replace('$', "$$"))
}
fn array(items: impl Iterator<Item = String>) -> Yaml {
    Yaml::Array(items.map(|value| scalar(&value)).collect())
}
fn object(items: Vec<(&str, Yaml)>) -> Yaml {
    mapping(
        items
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value)),
    )
}
fn mapping(items: impl Iterator<Item = (String, Yaml)>) -> Yaml {
    let mut map = yaml_rust2::yaml::Hash::new();
    for (key, value) in items {
        map.insert(Yaml::String(key), value);
    }
    Yaml::Hash(map)
}
