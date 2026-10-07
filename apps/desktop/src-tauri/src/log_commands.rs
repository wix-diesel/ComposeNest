//! Log commands expose only masked memory snapshots, scoped to the trusted main window.
use crate::{create_commands::CreateBackend, instance_commands::envelope};
use composenest_adapters::log_subscription::LogSessions;
use composenest_application::{
    ResponseEnvelope,
    log_subscription::{LogSubscriptionRequest, LogsView, SubscribeLogsRequest},
    state_store::StoreConflict,
};
use std::sync::Arc;
use tauri::{State, WebviewWindow};

/// Starts an owned read-only log session; the request ID supports lost-response cleanup.
#[tauri::command]
pub async fn subscribe_logs(
    window: WebviewWindow,
    request: SubscribeLogsRequest,
    backend: State<'_, Arc<CreateBackend>>,
    sessions: State<'_, Arc<LogSessions>>,
) -> Result<ResponseEnvelope<LogsView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let result = match backend.probe.as_ref() {
        Some(probe) => {
            sessions
                .subscribe(
                    &backend.database,
                    probe,
                    &backend.scope,
                    window.label(),
                    &request,
                )
                .await
        }
        None => Err(StoreConflict::Backend),
    };
    Ok(envelope(request.context, result))
}
/// Pulls bounded masked state, without an event queue that can block a Runner.
#[tauri::command]
pub async fn get_logs(
    window: WebviewWindow,
    request: LogSubscriptionRequest,
    backend: State<'_, Arc<CreateBackend>>,
    sessions: State<'_, Arc<LogSessions>>,
) -> Result<ResponseEnvelope<LogsView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    Ok(envelope(
        request.context,
        sessions.get(
            &backend.database,
            &backend.scope,
            window.label(),
            &request.subscription_id,
        ),
    ))
}
/// Reaps only the log CLI, leaving containers and accepted operations running.
#[tauri::command]
pub async fn unsubscribe_logs(
    window: WebviewWindow,
    request: LogSubscriptionRequest,
    sessions: State<'_, Arc<LogSessions>>,
) -> Result<ResponseEnvelope<bool>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    Ok(envelope(
        request.context,
        sessions
            .unsubscribe(window.label(), &request.subscription_id)
            .await
            .map(|_| true),
    ))
}
