//! Validation and registration of complete, fixed-byte Template packages.

use std::collections::HashMap;

use composenest_domain::template::{
    Node, ResolvedTemplate, TemplateError, Value, parse_manifest, parse_version, resolve_template,
};
use serde_json::{Map, Value as JsonValue, json};
use sha2::{Digest, Sha256};

use crate::state_store::{StateStore, StoreConflict, TemplateFile, TemplateRevision};

/// Application-assigned source of a package; author metadata cannot change it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateOrigin {
    /// A package shipped in a read-only application resource.
    Bundled,
    /// A package found below the managed local template directory.
    Local,
}

impl TemplateOrigin {
    fn as_str(self) -> &'static str {
        match self {
            Self::Bundled => "bundled",
            Self::Local => "local",
        }
    }
}

/// One package captured as an immutable set of original document bytes.
#[derive(Debug, Clone)]
pub struct TemplatePackage {
    /// Package directory name, used only to identify errors.
    pub name: String,
    /// Source assigned by the caller, never read from the manifest.
    pub origin: TemplateOrigin,
    /// The manifest and explicitly referenced version documents.
    pub files: Vec<TemplateFile>,
}

/// A package-specific registration failure.
#[derive(Debug)]
pub enum CatalogError {
    /// YAML, structure, or semantic validation failed.
    Template(TemplateError),
    /// The fixed byte set is incomplete or contains an unlisted document.
    InvalidPackage(String),
    /// Another package in the same reload claims this template ID and revision.
    AmbiguousRevision,
    /// Persistence rejected the validated revision.
    Store(StoreConflict),
}

/// Outcome for one package, including failures that leave existing revisions intact.
#[derive(Debug)]
pub struct CatalogEntry {
    /// Source package name.
    pub package: String,
    /// Registered immutable revision or a package-specific error.
    pub result: Result<TemplateRevision, CatalogError>,
}

/// Validates each fixed package and registers complete revisions independently.
///
/// A duplicate ID and template version within one reload invalidates every claimant,
/// regardless of enumeration order. No Docker or storage allocation occurs here.
pub fn register_packages(
    store: &impl StateStore,
    packages: Vec<TemplatePackage>,
) -> Vec<CatalogEntry> {
    let mut prepared: Vec<_> = packages
        .into_iter()
        .map(|package| {
            let name = package.name.clone();
            let identity = package_identity(&package);
            (name, identity, prepare_revision(package))
        })
        .collect();
    let mut counts = HashMap::new();
    for (_, identity, _) in &prepared {
        if let Some(identity) = identity {
            *counts.entry(identity.clone()).or_insert(0usize) += 1;
        }
    }
    prepared
        .drain(..)
        .map(|(package, _, result)| {
            let result = result.and_then(|revision| {
                if counts[&(revision.template_id.clone(), revision.version.clone())] > 1 {
                    return Err(CatalogError::AmbiguousRevision);
                }
                store
                    .register_template(&revision)
                    .map_err(CatalogError::Store)?;
                Ok(revision)
            });
            CatalogEntry { package, result }
        })
        .collect()
}

fn package_identity(package: &TemplatePackage) -> Option<(String, String)> {
    let manifest = package
        .files
        .iter()
        .find(|file| file.relative_path == "template.yaml")?;
    let parsed = parse_manifest(&package.name, &manifest.contents).ok()?;
    Some((parsed.id, parsed.template_version))
}

/// Converts one already captured package to a validated, immutable revision.
pub fn prepare_revision(package: TemplatePackage) -> Result<TemplateRevision, CatalogError> {
    if package.files.is_empty() || package.files.len() > 33 {
        return Err(CatalogError::InvalidPackage(
            "package must contain 1–33 documents".into(),
        ));
    }
    let mut files = HashMap::new();
    let mut total = 0usize;
    for file in &package.files {
        total = total.saturating_add(file.contents.len());
        if file.contents.len() > 256 * 1024 || total > 8 * 1024 * 1024 {
            return Err(CatalogError::InvalidPackage(
                "package exceeds the 256 KiB per-file or 8 MiB total limit".into(),
            ));
        }
        if files.insert(file.relative_path.as_str(), file).is_some() {
            return Err(CatalogError::InvalidPackage(
                "duplicate document path".into(),
            ));
        }
    }
    let manifest_file = files
        .get("template.yaml")
        .ok_or_else(|| CatalogError::InvalidPackage("template.yaml is missing".into()))?;
    let manifest =
        parse_manifest(&package.name, &manifest_file.contents).map_err(CatalogError::Template)?;
    if files.len() != manifest.versions.len() + 1 {
        return Err(CatalogError::InvalidPackage(
            "unlisted or missing version document".into(),
        ));
    }
    let mut definitions = Vec::with_capacity(manifest.versions.len());
    let mut ordered_files = vec![(**manifest_file).clone()];
    for (key, path) in &manifest.versions {
        let file = files.get(path.as_str()).ok_or_else(|| {
            CatalogError::InvalidPackage(format!("listed version {key} is missing: {path}"))
        })?;
        let definition = parse_version(&package.name, key, path, &file.contents)
            .map_err(CatalogError::Template)?;
        definitions.push((key.clone(), definition));
        ordered_files.push((**file).clone());
    }
    let resolved = resolve_template(manifest, &definitions).map_err(CatalogError::Template)?;
    let canonical_json = canonical_json(&resolved);
    let semantic_hash = format!("{:x}", Sha256::digest(canonical_json.as_bytes()));
    let id = format!(
        "{}:{}:{}",
        resolved.manifest.id, resolved.manifest.template_version, semantic_hash
    );
    Ok(TemplateRevision {
        id,
        template_id: resolved.manifest.id,
        version: resolved.manifest.template_version,
        normalization: "template-normalization-v1".into(),
        semantic_hash,
        canonical_json,
        origin: package.origin.as_str().into(),
        files: ordered_files,
    })
}

fn canonical_json(template: &ResolvedTemplate) -> String {
    let mut manifest = canonical_node(&template.manifest.document, "");
    if let JsonValue::Object(fields) = &mut manifest {
        fields.insert(
            "versions".into(),
            JsonValue::Array(
                template
                    .manifest
                    .versions
                    .iter()
                    .map(|(key, _)| JsonValue::String(key.clone()))
                    .collect(),
            ),
        );
        fields.remove("versionClone");
        fields.remove("storageClone");
    }
    let versions: Vec<_> = template
        .versions
        .iter()
        .map(|version| {
            json!({
                "key": version.key,
                "definition": canonical_node(&version.definition.document, ""),
                "inputPolicies": version.input_policies,
            })
        })
        .collect();
    json!({
        "normalization": "template-normalization-v1",
        "manifest": manifest,
        "versions": versions,
        "rules": {
            "clonePolicy": "1",
            "identity": "identity-v1",
            "project": "project-v1",
            "storage": "storage-v1",
            "secret": "secret-v1",
            "versionClone": template.version_clone,
            "storageClone": template.storage_clone,
        }
    })
    .to_string()
}

fn canonical_node(node: &Node, parent: &str) -> JsonValue {
    match &node.value {
        Value::String(value) => json!(value),
        Value::Integer(value) => json!(value),
        Value::Boolean(value) => json!(value),
        Value::Sequence(items) => JsonValue::Array(
            items
                .iter()
                .map(|item| canonical_node(item, parent))
                .collect(),
        ),
        Value::Map(items) => {
            let ordered = matches!(parent, "inputs" | "connections" | "ports" | "storage");
            let values: Map<_, _> = items
                .iter()
                .map(|(key, value)| (key.clone(), canonical_node(value, key)))
                .collect();
            if ordered {
                let order: Vec<_> = items.iter().map(|(key, _)| key.as_str()).collect();
                json!({ "order": order, "values": values })
            } else {
                JsonValue::Object(values)
            }
        }
    }
}
