//! Combines filesystem reload outcomes with persisted, usable package metadata.

use crate::template_package::{ReloadEntry, reload_catalog};
use composenest_application::{
    state_store::StateStore,
    template_catalog::CatalogError,
    template_catalog_view::{TemplateCatalogView, TemplateLoadResult, project_card},
    template_diagnostics::format_template_error,
};
use std::{io, path::Path};

/// Reloads definitions only; no Docker, image pull or instance creation is involved.
pub fn reload_results(
    store: &impl StateStore,
    bundled: &Path,
    local: &Path,
) -> io::Result<Vec<TemplateLoadResult>> {
    Ok(reload_catalog(store, bundled, local)?.into_iter().map(|entry| match entry {
        ReloadEntry::ReadFailure(error) => TemplateLoadResult {
            package: error.package, origin: error.origin.as_str().into(),
            revision_id: None, error: Some(error.reason), warnings: vec![],
        },
        ReloadEntry::Registration(entry) => {
            let (revision_id, error) = match entry.result {
                Ok(revision) => (Some(revision.id), None),
                Err(error) => (None, Some(match error {
                    CatalogError::Template(error) => format_template_error(&error),
                    CatalogError::InvalidPackage(reason) => reason,
                    CatalogError::AmbiguousRevision => "同じID・Template版のパッケージが重複しています。".into(),
                    CatalogError::Store(_) => "登録できません。登録済みの同じTemplate版と内容が異なる場合は、Template版を更新してください。".into(),
                })),
            };
            TemplateLoadResult { package: entry.package, origin: entry.origin.as_str().into(), revision_id, error, warnings: entry.warnings }
        }
    }).collect())
}

/// Reads registered revisions without confusing retained definitions with reload successes.
pub fn catalog_view(
    store: &impl StateStore,
    local: &Path,
    results: Vec<TemplateLoadResult>,
) -> io::Result<TemplateCatalogView> {
    let revisions = store
        .list_templates()
        .map_err(|_| io::Error::other("catalog store unavailable"))?;
    let mut templates = revisions
        .into_iter()
        .map(|revision| {
            let loaded = results
                .iter()
                .any(|result| result.revision_id.as_deref() == Some(&revision.id));
            project_card(revision, loaded)
                .map_err(|_| io::Error::other("catalog metadata unavailable"))
        })
        .collect::<io::Result<Vec<_>>>()?;
    templates.sort_by(|a, b| {
        (&a.name, &a.template_version, &a.revision_id).cmp(&(
            &b.name,
            &b.template_version,
            &b.revision_id,
        ))
    });
    Ok(TemplateCatalogView {
        local_root: local.to_string_lossy().into_owned(),
        templates,
        results,
    })
}
