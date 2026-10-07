//! Local scoped recovery transport; work continues if its requesting screen disappears.
use crate::create_commands::CreateBackend;
use composenest_adapters::port_edit_stages::PortEditError;
use composenest_application::{
    ErrorDto, RequestContext, ResponseEnvelope, Retryability,
    recovery_view::{RecoverOperationRequest, RecoveryRequest, RecoveryView},
};
use std::sync::Arc;
use tauri::State;

fn response(
    context: RequestContext,
    result: Result<RecoveryView, PortEditError>,
) -> ResponseEnvelope<RecoveryView> {
    match result {
        Ok(view) => ResponseEnvelope::success(context.request_id, view),
        Err(PortEditError::Store(error)) => crate::instance_commands::envelope(context, Err(error)),
        Err(_) => ResponseEnvelope::failure(
            context.request_id,
            ErrorDto {
                code: "RECOVERY_HELD".into(),
                field_path: None,
                reason: "復旧の安全条件を確認できません。実体を再確認してください。".into(),
                retryability: Retryability::Unknown,
                operation_id: None,
                safe_details: None,
            },
        ),
    }
}

/// Collects fresh read-only evidence under the instance gate without completing an operation.
#[tauri::command]
pub async fn resolve_operation(
    request: RecoveryRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<RecoveryView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let context = request.context.clone();
    let backend = Arc::clone(state.inner());
    let result = tauri::async_runtime::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| PortEditError::OutcomeUnknown)?;
        let probe = backend
            .probe
            .as_ref()
            .ok_or(PortEditError::OutcomeUnknown)?;
        runtime
            .block_on(
                backend
                    .runner
                    .run_exclusive(&request.instance_id, || async {
                        let operation_id =
                            composenest_adapters::recovery_view::inspection_operation(
                                &backend.database,
                                &backend.scope,
                                &request.instance_id,
                                &request.operation_id,
                                request.recovery_request_id.as_deref(),
                            )
                            .map_err(PortEditError::Store)?;
                        composenest_adapters::recovery_view::inspect_recovery(
                            &backend.database,
                            probe,
                            &backend.recovery,
                            &backend.scope,
                            &request.instance_id,
                            &operation_id,
                        )
                        .await
                        .map_err(PortEditError::Store)
                    }),
            )
            .map_err(PortEditError::Runner)?
    })
    .await
    .unwrap_or(Err(PortEditError::OutcomeUnknown));
    Ok(response(context, result))
}

/// Rechecks the confirmed choice and revisions, then applies the existing Core recovery path.
#[tauri::command]
pub async fn retry_operation(
    request: RecoverOperationRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<RecoveryView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let context = request.context.clone();
    let backend = Arc::clone(state.inner());
    let result = tauri::async_runtime::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| PortEditError::OutcomeUnknown)?;
        let probe = backend
            .probe
            .as_ref()
            .ok_or(PortEditError::OutcomeUnknown)?;
        runtime.block_on(composenest_adapters::recovery_actions::recover_operation(
            &backend.database,
            probe,
            &backend.recovery,
            &backend.runner,
            &backend.scope,
            &request,
        ))
    })
    .await
    .unwrap_or(Err(PortEditError::OutcomeUnknown));
    Ok(response(context, result))
}
