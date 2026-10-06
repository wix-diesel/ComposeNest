//! Read-only restoration of durable operation progress in the trusted local scope.

use crate::{create_commands::CreateBackend, instance_commands::envelope};
use composenest_application::{
    ResponseEnvelope,
    query_service::{OperationProgressView, OperationRequest},
};
use std::sync::Arc;
use tauri::State;

/// Reads the requested journal and masked target without executing or cancelling work.
#[tauri::command]
pub async fn get_operation(
    request: OperationRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<OperationProgressView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    Ok(envelope(
        request.context,
        composenest_adapters::query_service::view_operation(
            &state.database,
            &state.scope,
            &request.operation_id,
        ),
    ))
}
