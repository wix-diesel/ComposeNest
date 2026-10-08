//! Explicit sensitive reads restricted to the trusted local main window.
use crate::{create_commands::CreateBackend, instance_commands::envelope};
use composenest_application::{
    ResponseEnvelope,
    instance_content::{
        InstanceComposeRequest, InstanceComposeView, InstanceSecretRequest, InstanceSecretView,
    },
};
use std::sync::Arc;
use tauri::State;

/// Returns an actual connection secret only on explicit request.
#[tauri::command]
pub async fn get_instance_secret(
    request: InstanceSecretRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<InstanceSecretView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let result =
        composenest_adapters::instance_content::secret(&state.database, &state.scope, &request);
    Ok(envelope(request.context, result))
}

/// Reads verified Compose on a blocking worker, with masking as the normal UI request.
#[tauri::command]
pub async fn get_instance_compose(
    request: InstanceComposeRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<InstanceComposeView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let backend = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let result = composenest_adapters::instance_content::compose(
            &backend.database,
            &backend.scope,
            &request,
        );
        envelope(request.context, result)
    })
    .await
    .map_err(|_| ())
}
