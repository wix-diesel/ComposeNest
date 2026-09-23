use std::collections::{HashMap, HashSet};

use regex::Regex;

use super::structure::{TemplateManifest, VersionDefinition};
use super::validation::{field, optional};
use super::yaml::{Context, Node, TemplateError, Value};

/// An author-facing warning that does not invalidate a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateWarning {
    /// Version containing the warning.
    pub version: String,
    /// Path within that version document.
    pub path: String,
    /// Explanation for the author.
    pub message: String,
}

/// A complete, validated version with effective input policies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedVersion {
    /// Version key in manifest order.
    pub key: String,
    /// Full definition, preserving source order and omitted fields.
    pub definition: VersionDefinition,
    /// Effective clone policy for each input, in input display order.
    pub input_policies: Vec<(String, String)>,
    /// Author-facing warnings.
    pub warnings: Vec<TemplateWarning>,
}

/// A manifest and all its independently validated versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTemplate {
    /// Manifest, including omitted-versus-empty distinctions.
    pub manifest: TemplateManifest,
    /// Complete versions in manifest display order.
    pub versions: Vec<ResolvedVersion>,
    /// Effective policy for switching versions during clone.
    pub version_clone: String,
    /// Effective policy for choosing storage during clone.
    pub storage_clone: String,
}

/// Resolves every listed version; a missing or invalid non-default version fails the package.
///
/// Definitions are supplied by the caller after structural parsing. This function never reads
/// files or merges values between versions.
pub fn resolve_template(
    manifest: TemplateManifest,
    definitions: &[(String, VersionDefinition)],
) -> Result<ResolvedTemplate, TemplateError> {
    let mut resolved = Vec::with_capacity(manifest.versions.len());
    let mut input_types = HashMap::new();
    for (key, file) in &manifest.versions {
        let context = Context {
            package: &manifest.id,
            version: Some(key),
            file,
        };
        let definition = definitions
            .iter()
            .find(|(provided, _)| provided == key)
            .map(|(_, definition)| definition)
            .ok_or_else(|| context.error(None, "$", "listed version definition is missing"))?;
        let version = resolve_version(key, definition, &context, &mut input_types)?;
        resolved.push(version);
    }
    let mut provided = HashSet::new();
    for (key, _) in definitions {
        if !provided.insert(key) {
            let context = Context {
                package: &manifest.id,
                version: Some(key),
                file: "template.yaml",
            };
            return Err(context.error(None, "$.versions", "duplicate version definition"));
        }
        if !manifest.versions.iter().any(|(listed, _)| listed == key) {
            let context = Context {
                package: &manifest.id,
                version: Some(key),
                file: "template.yaml",
            };
            return Err(context.error(None, "$.versions", "unlisted version definition"));
        }
    }
    let version_clone = scalar(optional(&manifest.document, "versionClone"))
        .unwrap_or("copy")
        .to_owned();
    let storage_clone = scalar(optional(&manifest.document, "storageClone"))
        .unwrap_or("copy")
        .to_owned();
    Ok(ResolvedTemplate {
        manifest,
        versions: resolved,
        version_clone,
        storage_clone,
    })
}

fn resolve_version(
    key: &str,
    definition: &VersionDefinition,
    context: &Context<'_>,
    input_types: &mut HashMap<String, String>,
) -> Result<ResolvedVersion, TemplateError> {
    let document = &definition.document;
    let inputs = entries(optional(document, "inputs"));
    let mut policies = Vec::new();
    for (name, input) in inputs {
        let kind = scalar(Some(field(input, "type"))).unwrap_or_default();
        let path = format!("$.inputs.{name}");
        if let Some(previous) = input_types.get(name) {
            if previous != kind {
                return Err(context.error(
                    Some(field(input, "type").position),
                    &format!("{path}.type"),
                    "input type differs between versions; use a new key",
                ));
            }
        } else {
            input_types.insert(name.clone(), kind.to_owned());
        }
        validate_input_semantics(input, kind, &path, context)?;
        let policy = scalar(optional(input, "clone")).unwrap_or(if kind == "secret" {
            "regenerate"
        } else {
            "copy"
        });
        policies.push((name.clone(), policy.to_owned()));
    }

    let service = field(document, "service");
    let mut used = HashSet::new();
    for (name, value) in entries(optional(service, "environment")) {
        validate_reference(
            value,
            &format!("$.service.environment.{name}"),
            inputs,
            true,
            context,
            &mut used,
        )?;
    }
    for (base, command) in [
        ("$.service.command", optional(service, "command")),
        (
            "$.service.healthcheck.command",
            Some(field(field(service, "healthcheck"), "command")),
        ),
    ] {
        if let Some(command) = command
            && let Value::Sequence(items) = &command.value
        {
            for (index, value) in items.iter().enumerate() {
                validate_reference(
                    value,
                    &format!("{base}[{index}]"),
                    inputs,
                    false,
                    context,
                    &mut used,
                )?;
            }
        }
    }
    let ports = entries(optional(service, "ports"));
    validate_ports(ports, context)?;
    validate_storage(entries(optional(service, "storage")), context)?;
    validate_connections(
        entries(optional(document, "connections")),
        ports,
        inputs,
        context,
    )?;

    let warnings = inputs
        .iter()
        .filter(|(name, _)| !used.contains(name.as_str()))
        .map(|(name, _)| TemplateWarning {
            version: key.to_owned(),
            path: format!("$.inputs.{name}"),
            message: "input is not used by service configuration".to_owned(),
        })
        .collect();
    Ok(ResolvedVersion {
        key: key.to_owned(),
        definition: definition.clone(),
        input_policies: policies,
        warnings,
    })
}

fn validate_input_semantics(
    input: &Node,
    kind: &str,
    path: &str,
    context: &Context<'_>,
) -> Result<(), TemplateError> {
    let validation = optional(input, "validation");
    let minimum = number(validation.and_then(|v| optional(v, "minLength")))
        .unwrap_or(if kind == "secret" { 1 } else { 0 });
    let maximum = number(validation.and_then(|v| optional(v, "maxLength"))).unwrap_or(4096);
    let low = number(validation.and_then(|v| optional(v, "min"))).unwrap_or(-9_007_199_254_740_991);
    let high = number(validation.and_then(|v| optional(v, "max"))).unwrap_or(9_007_199_254_740_991);
    if minimum > maximum || low > high {
        return Err(context.error(
            Some(input.position),
            &format!("{path}.validation"),
            "minimum exceeds maximum",
        ));
    }
    let pattern = validation.and_then(|v| optional(v, "pattern"));
    let expression = if let Some(pattern) = pattern {
        let source = scalar(Some(pattern)).unwrap_or_default();
        Some(Regex::new(&format!(r"\A(?:{source})\z")).map_err(|error| {
            context.error(
                Some(pattern.position),
                &format!("{path}.validation.pattern"),
                format!("invalid regular expression: {error}"),
            )
        })?)
    } else {
        None
    };
    let options = optional(input, "options").and_then(|node| match &node.value {
        Value::Sequence(items) => Some(items.as_slice()),
        _ => None,
    });
    if let Some(options) = options {
        let mut seen = HashSet::new();
        for (index, option) in options.iter().enumerate() {
            let value = scalar(Some(field(option, "value"))).unwrap_or_default();
            if !seen.insert(value) {
                return Err(context.error(
                    Some(option.position),
                    &format!("{path}.options[{index}].value"),
                    "duplicate select option",
                ));
            }
        }
    }
    if let Some(default) = optional(input, "default") {
        let valid = match &default.value {
            Value::String(value) if kind == "string" => {
                let length = value.chars().count() as i64;
                length >= minimum
                    && length <= maximum
                    && !value.chars().any(char::is_control)
                    && expression
                        .as_ref()
                        .is_none_or(|regex| regex.is_match(value))
            }
            Value::String(value) if kind == "select" => options.is_some_and(|options| {
                options
                    .iter()
                    .any(|option| scalar(Some(field(option, "value"))) == Some(value))
            }),
            Value::Integer(value) if kind == "integer" => *value >= low && *value <= high,
            Value::Boolean(_) if kind == "boolean" => true,
            _ => false,
        };
        if !valid {
            return Err(context.error(
                Some(default.position),
                &format!("{path}.default"),
                "default violates input validation",
            ));
        }
    }
    if kind == "secret" {
        let initial = scalar(optional(input, "initial")).unwrap_or("generate");
        let clone = scalar(optional(input, "clone")).unwrap_or("regenerate");
        if (initial == "generate" || clone == "regenerate")
            && (minimum > 32 || maximum < 32 || pattern.is_some())
        {
            return Err(context.error(
                Some(input.position),
                path,
                "secret-v1 requires a 32-character range without a pattern; use ask policies",
            ));
        }
    }
    Ok(())
}

fn validate_reference(
    value: &Node,
    path: &str,
    inputs: &[(String, Node)],
    environment: bool,
    context: &Context<'_>,
    used: &mut HashSet<String>,
) -> Result<(), TemplateError> {
    let Value::Map(_) = &value.value else {
        return Ok(());
    };
    let reference = field(value, "input");
    let name = scalar(Some(reference)).unwrap_or_default();
    let Some((_, input)) = inputs.iter().find(|(key, _)| key == name) else {
        return Err(context.error(
            Some(reference.position),
            &format!("{path}.input"),
            "input is not defined in this version",
        ));
    };
    let required = !matches!(
        optional(input, "required").map(|node| &node.value),
        Some(Value::Boolean(false))
    );
    let omit = optional(value, "onMissing").is_some();
    if required && omit {
        return Err(context.error(
            Some(value.position),
            &format!("{path}.onMissing"),
            "onMissing is only allowed for optional inputs",
        ));
    }
    if !required && (!environment || !omit) {
        return Err(context.error(
            Some(value.position),
            path,
            "optional input requires environment onMissing: omit and cannot be used in command",
        ));
    }
    used.insert(name.to_owned());
    Ok(())
}

fn validate_ports(ports: &[(String, Node)], context: &Context<'_>) -> Result<(), TemplateError> {
    let mut seen = HashSet::new();
    for (name, port) in ports {
        let container = number(Some(field(port, "container"))).unwrap_or_default();
        if !seen.insert(container) {
            return Err(context.error(
                Some(port.position),
                &format!("$.service.ports.{name}.container"),
                "duplicate container port",
            ));
        }
    }
    Ok(())
}

fn validate_storage(slots: &[(String, Node)], context: &Context<'_>) -> Result<(), TemplateError> {
    let mut paths: Vec<Vec<&str>> = Vec::new();
    for (name, slot) in slots {
        let target = field(slot, "container");
        let path = scalar(Some(target)).unwrap_or_default();
        let segments: Vec<_> = path
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .collect();
        if !path.starts_with('/')
            || segments.is_empty()
            || path.split('/').any(|part| part == "..")
            || path.chars().any(|character| character.is_control())
        {
            return Err(context.error(
                Some(target.position),
                &format!("$.service.storage.{name}.container"),
                "use an absolute Linux path without root, parent segments or control characters",
            ));
        }
        if paths
            .iter()
            .any(|other| segments.starts_with(other) || other.starts_with(&segments))
        {
            return Err(context.error(
                Some(target.position),
                &format!("$.service.storage.{name}.container"),
                "storage targets overlap",
            ));
        }
        paths.push(segments);
    }
    Ok(())
}

fn validate_connections(
    connections: &[(String, Node)],
    ports: &[(String, Node)],
    inputs: &[(String, Node)],
    context: &Context<'_>,
) -> Result<(), TemplateError> {
    for (name, connection) in connections {
        let port = field(connection, "port");
        if !ports
            .iter()
            .any(|(key, _)| Some(key.as_str()) == scalar(Some(port)))
        {
            return Err(context.error(
                Some(port.position),
                &format!("$.connections.{name}.port"),
                "port slot is not defined in this version",
            ));
        }
        if let Some(Node {
            value: Value::Sequence(items),
            ..
        }) = optional(connection, "inputs")
        {
            for (index, item) in items.iter().enumerate() {
                if !inputs
                    .iter()
                    .any(|(key, _)| Some(key.as_str()) == scalar(Some(item)))
                {
                    return Err(context.error(
                        Some(item.position),
                        &format!("$.connections.{name}.inputs[{index}]"),
                        "input is not defined in this version",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn entries(node: Option<&Node>) -> &[(String, Node)] {
    match node.map(|node| &node.value) {
        Some(Value::Map(items)) => items,
        _ => &[],
    }
}

fn scalar(node: Option<&Node>) -> Option<&str> {
    match node.map(|node| &node.value) {
        Some(Value::String(value)) => Some(value),
        _ => None,
    }
}

fn number(node: Option<&Node>) -> Option<i64> {
    match node.map(|node| &node.value) {
        Some(Value::Integer(value)) => Some(*value),
        _ => None,
    }
}
