//! Local-only edit transport; external work survives a screen leaving the request.
use crate::create_commands::CreateBackend;
use composenest_adapters::{
    instance_edit::{AcceptedPortEdit, accept_instance_ports, view_instance_edit},
    port_edit_stages::{PortEditError, run_accepted_port_edit_reserved},
};
use composenest_application::{
    ErrorDto, RequestContext, ResponseEnvelope, Retryability,
    instance_actions::InstanceActionRequest,
    instance_edit::{EditInstancePortsRequest, InstanceEditView},
    operation_runner::RunnerError,
};
use std::sync::Arc;
use tauri::State;

fn response(
    context: RequestContext,
    result: Result<InstanceEditView, PortEditError>,
) -> ResponseEnvelope<InstanceEditView> {
    match result {
        Ok(view) => ResponseEnvelope::success(context.request_id, view),
        Err(PortEditError::Store(error)) => crate::instance_commands::envelope(context, Err(error)),
        Err(error) => {
            let (code, reason, retryability) = match error {
                PortEditError::Rejected => (
                    "PORT_EDIT_REJECTED",
                    "停止またはコンテナ不在と所有情報を確認できませんでした。ポート変更は受け付けていません。",
                    Retryability::NotRetryable,
                ),
                PortEditError::Port(_) => (
                    "PORT_EDIT_CONFLICT",
                    "新しいポートを利用できません。ポートとDockerの接続を確認してください。",
                    Retryability::NotRetryable,
                ),
                PortEditError::Runner(RunnerError::CapacityReached | RunnerError::ShuttingDown) => {
                    (
                        "PORT_EDIT_NOT_ACCEPTED",
                        "他の処理またはアプリの終了により、ポート変更は受け付けていません。",
                        Retryability::Retryable,
                    )
                }
                _ => (
                    "PORT_EDIT_UNCONFIRMED",
                    "ポート変更の受付を確認できません。同じ要求で再確認してください。",
                    Retryability::Unknown,
                ),
            };
            ResponseEnvelope::failure(
                context.request_id,
                ErrorDto {
                    code: code.into(),
                    field_path: None,
                    reason: reason.into(),
                    retryability,
                    operation_id: None,
                    safe_details: None,
                },
            )
        }
    }
}

/// Reads masked immutable settings and durable port application state in the local scope.
#[tauri::command]
pub async fn get_instance_edit(
    request: InstanceActionRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<InstanceEditView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    Ok(response(
        request.context,
        view_instance_edit(&state.database, &state.scope, &request.instance_id)
            .map_err(PortEditError::Store),
    ))
}

fn execute(backend: Arc<CreateBackend>, work: AcceptedPortEdit) {
    tauri::async_runtime::spawn_blocking(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| PortEditError::OutcomeUnknown)
            .and_then(|runtime| {
                let probe = backend
                    .probe
                    .as_ref()
                    .ok_or(PortEditError::OutcomeUnknown)?;
                runtime.block_on(run_accepted_port_edit_reserved(
                    &backend.database,
                    probe,
                    backend.database.management_root(),
                    &backend.runner,
                    &work.request,
                    work.reservation,
                ))
            });
        if result.is_err() {
            let _ = backend.database.write(move |db| {
                db.execute("UPDATE operations SET status='OutcomeUnknown', phase='reconcile' WHERE id=?1 AND status IN ('Accepted','Executing')", [&work.request.receipt.operation_id])?;
                Ok(())
            });
        }
    });
}

/// Returns durable acceptance before applying ports in a detached worker; never starts containers.
#[tauri::command]
pub async fn edit_instance_ports(
    request: EditInstancePortsRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<InstanceEditView>, ()> {
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
        let work = runtime.block_on(accept_instance_ports(
            &backend.database,
            backend.probe.as_ref(),
            backend.database.management_root(),
            &backend.runner,
            &backend.scope,
            &request,
        ))?;
        // Launch from this surviving task before replying, even if the caller has left the screen.
        if let Some(work) = work {
            execute(Arc::clone(&backend), work);
        }
        view_instance_edit(&backend.database, &backend.scope, &request.instance_id)
            .map_err(PortEditError::Store)
    })
    .await
    .unwrap_or(Err(PortEditError::OutcomeUnknown));
    Ok(response(context, result))
}
