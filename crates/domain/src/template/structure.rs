use super::validation::*;
use super::yaml::{Context, Node, TemplateError, Value, parse};

/// A validated Schema 1 package manifest in author-defined version order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateManifest {
    /// Stable template identifier.
    pub id: String,
    /// Revision of the template definition.
    pub template_version: String,
    /// Version selected for new instances.
    pub default_version: String,
    /// Version keys and package-relative definition paths in display order.
    pub versions: Vec<(String, String)>,
    /// Validated source tree, retaining metadata and item locations.
    pub document: Node,
}

/// A validated full definition for one service version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionDefinition {
    /// Fixed image reference.
    pub image: String,
    /// Supported Linux platforms in author-defined order.
    pub platforms: Vec<String>,
    /// Validated source tree, retaining all nested settings and locations.
    pub document: Node,
}

/// Parses and structurally validates a Schema 1 manifest without reading referenced files.
pub fn parse_manifest(package: &str, bytes: &[u8]) -> Result<TemplateManifest, TemplateError> {
    let context = Context {
        package,
        version: None,
        file: "template.yaml",
    };
    let document = parse(bytes, &context)?;
    object(
        &document,
        "$",
        &context,
        &[
            "schemaVersion",
            "id",
            "templateVersion",
            "name",
            "description",
            "defaultVersion",
            "versions",
        ],
        &[
            "author",
            "homepage",
            "license",
            "versionClone",
            "storageClone",
        ],
    )?;
    let schema = field(&document, "schemaVersion");
    if !matches!(schema.value, Value::Integer(1)) {
        return Err(context.error(
            Some(schema.position),
            "$.schemaVersion",
            "only integer schemaVersion: 1 is supported",
        ));
    }
    for key in [
        "id",
        "templateVersion",
        "name",
        "description",
        "defaultVersion",
    ] {
        string(field(&document, key), &format!("$.{key}"), &context)?;
    }
    string_length(field(&document, "name"), "$.name", &context, 1, 100)?;
    string_length(
        field(&document, "description"),
        "$.description",
        &context,
        1,
        2_000,
    )?;
    let id = string(field(&document, "id"), "$.id", &context)?;
    if id.len() < 3
        || id.len() > 128
        || id.split('.').count() < 2
        || id.split('.').any(|part| {
            !part.starts_with(|c: char| c.is_ascii_lowercase())
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
    {
        return Err(context.error(
            Some(field(&document, "id").position),
            "$.id",
            "use 2 or more lowercase identifier segments separated by dots",
        ));
    }
    let revision = string(
        field(&document, "templateVersion"),
        "$.templateVersion",
        &context,
    )?;
    if revision.split('.').count() != 3
        || revision.split('.').any(|part| {
            part.is_empty()
                || (part.starts_with('0') && part.len() > 1)
                || !part.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(context.error(
            Some(field(&document, "templateVersion").position),
            "$.templateVersion",
            "use a three-part numeric version such as \"1.0.0\"",
        ));
    }
    for key in ["author", "homepage", "license"] {
        if let Some(node) = optional(&document, key) {
            string(node, &format!("$.{key}"), &context)?;
        }
    }
    if let Some(homepage) = optional(&document, "homepage")
        && !string(homepage, "$.homepage", &context)?.starts_with("https://")
    {
        return Err(context.error(
            Some(homepage.position),
            "$.homepage",
            "homepage must use HTTPS",
        ));
    }
    for key in ["versionClone", "storageClone"] {
        if let Some(node) = optional(&document, key) {
            choice(node, &format!("$.{key}"), &context, &["copy", "ask"])?;
        }
    }
    let versions_node = field(&document, "versions");
    let versions_map = map(versions_node, "$.versions", &context)?;
    length(
        versions_map.len(),
        1,
        32,
        versions_node,
        "$.versions",
        &context,
    )?;
    let mut versions = Vec::new();
    for (version, path_node) in versions_map {
        let path = string(path_node, &format!("$.versions.{version}"), &context)?;
        if version.is_empty()
            || version.len() > 64
            || !version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(context.error(
                Some(path_node.position),
                &format!("$.versions.{version}"),
                "version key must contain 1–64 ASCII letters, digits, dots, hyphens or underscores",
            ));
        }
        if !valid_version_path(path) {
            return Err(context.error(
                Some(path_node.position),
                &format!("$.versions.{version}"),
                "use a safe versions/<name>.yaml relative path",
            ));
        }
        if versions.iter().any(|(_, existing)| existing == path) {
            return Err(context.error(
                Some(path_node.position),
                &format!("$.versions.{version}"),
                "two versions cannot reference the same file",
            ));
        }
        versions.push((version.clone(), path.to_owned()));
    }
    let default_version = string(
        field(&document, "defaultVersion"),
        "$.defaultVersion",
        &context,
    )?;
    if !versions
        .iter()
        .any(|(version, _)| version == default_version)
    {
        return Err(context.error(
            Some(field(&document, "defaultVersion").position),
            "$.defaultVersion",
            "select a key listed in versions",
        ));
    }
    Ok(TemplateManifest {
        id: string(field(&document, "id"), "$.id", &context)?.to_owned(),
        template_version: string(
            field(&document, "templateVersion"),
            "$.templateVersion",
            &context,
        )?
        .to_owned(),
        default_version: default_version.to_owned(),
        versions,
        document,
    })
}

/// Parses and structurally validates a full version document without semantic cross-references.
pub fn parse_version(
    package: &str,
    version: &str,
    file: &str,
    bytes: &[u8],
) -> Result<VersionDefinition, TemplateError> {
    let context = Context {
        package,
        version: Some(version),
        file,
    };
    let document = parse(bytes, &context)?;
    object(
        &document,
        "$",
        &context,
        &["image", "platforms", "service"],
        &["inputs", "connections"],
    )?;
    let image = string(field(&document, "image"), "$.image", &context)?.to_owned();
    if image.is_empty() {
        return Err(context.error(
            Some(field(&document, "image").position),
            "$.image",
            "image cannot be empty",
        ));
    }
    let platforms_node = field(&document, "platforms");
    let platform_nodes = sequence(platforms_node, "$.platforms", &context)?;
    length(
        platform_nodes.len(),
        1,
        2,
        platforms_node,
        "$.platforms",
        &context,
    )?;
    let mut platforms = Vec::new();
    for (index, node) in platform_nodes.iter().enumerate() {
        let value = choice(
            node,
            &format!("$.platforms[{index}]"),
            &context,
            &["linux/amd64", "linux/arm64"],
        )?;
        if platforms.iter().any(|existing| existing == value) {
            return Err(context.error(
                Some(node.position),
                &format!("$.platforms[{index}]"),
                "duplicate platform",
            ));
        }
        platforms.push(value.to_owned());
    }
    if let Some(node) = optional(&document, "inputs") {
        named_map(node, "$.inputs", &context, 64, validate_input)?;
    }
    validate_service(field(&document, "service"), "$.service", &context)?;
    if let Some(node) = optional(&document, "connections") {
        named_map(node, "$.connections", &context, 16, validate_connection)?;
    }
    Ok(VersionDefinition {
        image,
        platforms,
        document,
    })
}

fn valid_version_path(path: &str) -> bool {
    let Some(name) = path
        .strip_prefix("versions/")
        .and_then(|part| part.strip_suffix(".yaml"))
    else {
        return false;
    };
    let base = name.split('.').next().unwrap_or("");
    let reserved = matches!(
        base.to_ascii_lowercase().as_str(),
        "con"
            | "prn"
            | "aux"
            | "nul"
            | "com1"
            | "com2"
            | "com3"
            | "com4"
            | "com5"
            | "com6"
            | "com7"
            | "com8"
            | "com9"
            | "lpt1"
            | "lpt2"
            | "lpt3"
            | "lpt4"
            | "lpt5"
            | "lpt6"
            | "lpt7"
            | "lpt8"
            | "lpt9"
    );
    !reserved
        && !name.is_empty()
        && name.len() <= 64
        && !name.ends_with('.')
        && !name.contains("..")
        && (name.as_bytes()[0].is_ascii_lowercase() || name.as_bytes()[0].is_ascii_digit())
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}
