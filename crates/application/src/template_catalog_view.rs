//! Secret-free package cards and outcomes for the desktop catalog.

use crate::state_store::{StorageMethod, TemplateCatalogItem};
use serde::Serialize;

/// One immutable registered package, with all service Versions on the same card.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateCard {
    /// Exact revision passed to the create Plan.
    pub revision_id: String,
    /// Stable service family identifier.
    pub template_id: String,
    /// Display name from the validated manifest.
    pub name: String,
    /// Plain-text service description.
    pub description: String,
    /// Package revision, distinct from service Versions.
    pub template_version: String,
    /// Service Versions in manifest order.
    pub versions: Vec<String>,
    /// Supported Core storage methods when writable slots exist.
    pub storage_methods: Vec<StorageMethod>,
    /// Persisted application-assigned source; never author-supplied metadata.
    pub origin: String,
    /// Whether this revision succeeded in the latest reload.
    pub loaded: bool,
}

/// One discovered package's outcome, independent of retained database revisions.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateLoadResult {
    /// Discovery directory name.
    pub package: String,
    /// Application-assigned source of this attempt.
    pub origin: String,
    /// Registered revision, absent for every failed attempt.
    pub revision_id: Option<String>,
    /// Safe file, Version, item path and reason, absent on success.
    pub error: Option<String>,
    /// Unlisted files and other discovery notices.
    pub warnings: Vec<String>,
}

/// Current usable revisions plus the last actual reload results.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateCatalogView {
    /// Trusted management-root local package directory.
    pub local_root: String,
    /// Registered revisions, including retained older definitions.
    pub templates: Vec<TemplateCard>,
    /// One result per discovered package, never per Version file.
    pub results: Vec<TemplateLoadResult>,
}

/// Projects only public metadata; raw definitions and input defaults never leave Core.
pub fn project_card(
    revision: TemplateCatalogItem,
    loaded: bool,
) -> Result<TemplateCard, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(&revision.canonical_json)?;
    let manifest = &value["manifest"];
    let storage = value["versions"].as_array().is_some_and(|versions| {
        versions.iter().any(|version| {
            version["definition"]["service"]["storage"]["order"]
                .as_array()
                .is_some_and(|slots| !slots.is_empty())
        })
    });
    Ok(TemplateCard {
        revision_id: revision.id,
        template_id: revision.template_id,
        name: manifest["name"].as_str().unwrap_or_default().into(),
        description: manifest["description"].as_str().unwrap_or_default().into(),
        template_version: revision.version,
        versions: manifest["versions"]
            .as_array()
            .map_or_else(Vec::new, |versions| {
                versions
                    .iter()
                    .filter_map(|version| version.as_str().map(str::to_owned))
                    .collect()
            }),
        storage_methods: if storage {
            vec![StorageMethod::Bind, StorageMethod::Volume]
        } else {
            vec![]
        },
        origin: revision.origin,
        loaded,
    })
}
