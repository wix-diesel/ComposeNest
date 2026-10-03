//! Clone transport sharing trusted target checks and detached operation execution.
use super::create_commands::{CreateBackend, accept_with_capacity, envelope, execute};
use composenest_adapters::{SystemClock, SystemRandom};
use composenest_application::{
    ResponseEnvelope,
    clone_plan::ClonePlanView,
    clone_session::{ConfirmCloneRequest, PrepareCloneRequest, UpdateCloneRequest},
    create_session::{CreatePlanRequest, CreateReceipt},
};
use std::sync::Arc;
use tauri::State;

/// Prepares candidates from a registered revision without allocating resources.
#[tauri::command]
pub async fn prepare_clone(
    request: PrepareCloneRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<ClonePlanView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let mut session = state.clones.lock().await;
    let result = state.ports().await.and_then(|ports| {
        session.prepare(
            request.source_id,
            &*state.database,
            &SystemClock,
            &mut SystemRandom,
            &ports,
        )
    });
    Ok(envelope(request.context, result))
}

/// Applies edits against the caller-reviewed plan revision.
#[tauri::command]
pub async fn update_clone_plan(
    request: UpdateCloneRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<ClonePlanView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let mut session = state.clones.lock().await;
    let result = state.ports().await.and_then(|ports| {
        session.update(
            &request.plan_id,
            request.edit,
            &*state.database,
            &SystemClock,
            &mut SystemRandom,
            &ports,
        )
    });
    Ok(envelope(request.context, result))
}

/// Refreshes candidates on the same scoped plan.
#[tauri::command]
pub async fn view_clone_plan(
    request: CreatePlanRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<ClonePlanView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let mut session = state.clones.lock().await;
    let result = state
        .ports()
        .await
        .and_then(|ports| session.view(&request.plan_id, &*state.database, &SystemClock, &ports));
    Ok(envelope(request.context, result))
}

/// Reconciles acceptance from durable state without contacting Docker.
#[tauri::command]
pub async fn get_clone_receipt(
    request: CreatePlanRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<Option<CreateReceipt>>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let session = state.clones.lock().await;
    Ok(envelope(
        request.context,
        session
            .receipt(&request.plan_id, &*state.database)
            .map(|receipt| receipt.as_ref().map(CreateReceipt::from)),
    ))
}

/// Discards only an unconfirmed in-memory plan.
#[tauri::command]
pub async fn discard_clone_plan(
    request: CreatePlanRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<()>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let mut session = state.clones.lock().await;
    Ok(envelope(
        request.context,
        session.discard(&request.plan_id, &*state.database, &SystemClock),
    ))
}

/// Accepts explicit confirmation and starts each fresh operation once.
#[tauri::command]
pub async fn confirm_clone(
    request: ConfirmCloneRequest,
    state: State<'_, Arc<CreateBackend>>,
    app: tauri::AppHandle,
) -> Result<ResponseEnvelope<CreateReceipt>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let context = request.context.clone();
    let mut session = state.clones.lock().await;
    let result = match session.accepted(&request, &*state.database) {
        Ok(Some(receipt)) => Ok((receipt, None)),
        Ok(None) => state.ports().await.and_then(|ports| {
            accept_with_capacity(&state.runner, || {
                session.confirm(
                    request,
                    &*state.database,
                    &SystemClock,
                    &mut SystemRandom,
                    &ports,
                )
            })
        }),
        Err(error) => Err(error),
    };
    Ok(envelope(
        context,
        result.map(|(receipt, reservation)| {
            if let Some(reservation) = reservation {
                execute(Arc::clone(state.inner()), receipt.clone(), reservation, app);
            }
            CreateReceipt::from(&receipt)
        }),
    ))
}
