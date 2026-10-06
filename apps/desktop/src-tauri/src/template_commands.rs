//! Fixed-root catalog transport; reload never contacts Docker.

use crate::create_commands::{CreateBackend, envelope};
use composenest_adapters::template_catalog_view::{catalog_view, reload_results};
use composenest_application::{
    RequestContext, ResponseEnvelope,
    create_plan::PlanError,
    template_catalog_view::{TemplateCatalogView, TemplateLoadResult},
};
use std::{path::PathBuf, sync::Arc};
use tauri::State;
use tokio::sync::Mutex;

/// Trusted resource location and serialized latest reload outcomes.
pub struct TemplateBackend {
    /// Application resource directory, never supplied by the WebView.
    pub bundled: PathBuf,
    /// Last reload; None triggers initial discovery without startup failure.
    pub results: Mutex<Option<Vec<TemplateLoadResult>>>,
}

async fn catalog(
    request: RequestContext,
    state: &CreateBackend,
    templates: &TemplateBackend,
    reload: bool,
) -> ResponseEnvelope<TemplateCatalogView> {
    if let Err(error) = request.validate() {
        return ResponseEnvelope::failure(request.request_id, *error);
    }
    let mut results = templates.results.lock().await;
    let local = state.database.management_root().join("templates/local");
    let result = (|| {
        if reload || results.is_none() {
            *results = None;
            *results = Some(reload_results(
                &*state.database,
                &templates.bundled,
                &local,
            )?);
        }
        catalog_view(
            &*state.database,
            &local,
            results.clone().unwrap_or_default(),
        )
    })()
    .map_err(|_| PlanError {
        code: "TEMPLATE_CATALOG_UNAVAILABLE",
        field_path: None,
    });
    envelope(request, result)
}

/// Reads usable definitions and the initial or latest discovery report.
#[tauri::command]
pub async fn list_templates(
    request: RequestContext,
    state: State<'_, Arc<CreateBackend>>,
    templates: State<'_, TemplateBackend>,
) -> Result<ResponseEnvelope<TemplateCatalogView>, ()> {
    Ok(catalog(request, &state, &templates, false).await)
}

/// Reloads bundled and local definitions without executing runtime operations.
#[tauri::command]
pub async fn reload_templates(
    request: RequestContext,
    state: State<'_, Arc<CreateBackend>>,
    templates: State<'_, TemplateBackend>,
) -> Result<ResponseEnvelope<TemplateCatalogView>, ()> {
    Ok(catalog(request, &state, &templates, true).await)
}
