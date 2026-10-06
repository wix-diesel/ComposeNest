//! Scoped, read-only retained storage queries and persisted physical observations.

use crate::{create_commands::CreateBackend, instance_commands::envelope};
use composenest_application::{
    RequestContext, ResponseEnvelope,
    instance_actions::InstanceActionRequest,
    retained_storage::{RetainedInstance, RetainedStore},
};
use std::sync::Arc;
use tauri::State;

/// Lists original settings references and saved observations without contacting Docker.
#[tauri::command]
pub async fn list_retained_storage(
    request: RequestContext,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<Vec<RetainedInstance>>, ()> {
    if let Err(error) = request.validate() {
        return Ok(ResponseEnvelope::failure(request.request_id, *error));
    }
    Ok(envelope(
        request,
        state.database.list_retained_storage(&state.scope),
    ))
}

/// Rechecks only the saved instance in the backend scope using Core's ownership checks.
#[tauri::command]
pub async fn refresh_retained_storage(
    request: InstanceActionRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<RetainedInstance>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let result = composenest_adapters::delete_stages::refresh_retained_storage(
        &state.database,
        state.probe.as_ref(),
        &state.scope,
        &request.instance_id,
    )
    .await;
    Ok(envelope(request.context, result))
}
