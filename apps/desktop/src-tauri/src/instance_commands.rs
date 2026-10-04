//! Scoped desktop transport for instance actions and detached lifecycle execution.
use crate::create_commands::CreateBackend;
use composenest_adapters::{SystemRandom, lifecycle_stages::run_confirmed_lifecycle_reserved};
use composenest_application::{
    ErrorDto, ResponseEnvelope, Retryability,
    instance_actions::{
        self, ChangeInstanceRequest, InstanceActionRequest, InstanceActionView,
        RenameInstanceRequest,
    },
    operation_journal::OperationJournal,
    query_service::QueryService,
    state_store::StoreConflict,
};
use std::sync::Arc;
use tauri::State;

pub(super) fn envelope<T>(
    context: composenest_application::RequestContext,
    result: Result<T, StoreConflict>,
) -> ResponseEnvelope<T> {
    match result {
        Ok(value) => ResponseEnvelope::success(context.request_id, value),
        Err(error) => {
            let (code, reason) = match error {
                StoreConflict::Duplicate => (
                    "NAME_OR_REQUEST_CONFLICT",
                    "同じ名前が使用されています。または要求IDが別の操作に使われています。",
                ),
                StoreConflict::StaleRevision => (
                    "INSTANCE_STALE",
                    "環境が更新されました。現在の状態を再確認してから変更してください。",
                ),
                StoreConflict::Missing => ("INSTANCE_MISSING", "対象の環境が見つかりません。"),
                StoreConflict::InvalidInput => (
                    "INSTANCE_INPUT_INVALID",
                    "環境名または操作の入力を確認してください。",
                ),
                StoreConflict::UnresolvedOperation | StoreConflict::InvalidLifecycle => (
                    "INSTANCE_ACTION_UNAVAILABLE",
                    "この操作は現在利用できません。未解決の処理と実行状態を確認してください。",
                ),
                StoreConflict::Backend => (
                    "INSTANCE_UNAVAILABLE",
                    "接続または処理の受付を確認できませんでした。同じ要求で再確認してください。",
                ),
            };
            ResponseEnvelope::failure(
                context.request_id,
                ErrorDto {
                    code: code.into(),
                    field_path: None,
                    reason: reason.into(),
                    retryability: Retryability::Unknown,
                    operation_id: None,
                    safe_details: None,
                },
            )
        }
    }
}
/// Reads persisted state and Core's available actions in the trusted local scope.
#[tauri::command]
pub async fn get_instance_actions(
    request: InstanceActionRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<InstanceActionView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    Ok(envelope(
        request.context,
        instance_actions::view(&*state.database, &state.scope, &request.instance_id),
    ))
}
/// Renames only the display name; running containers and port allocations are untouched.
#[tauri::command]
pub async fn rename_instance(
    request: RenameInstanceRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<InstanceActionView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let result = QueryService::new(&*state.database)
        .rename(
            &state.scope,
            &request.instance_id,
            request.expected_revision,
            &request.name,
        )
        .and_then(|_| instance_actions::view(&*state.database, &state.scope, &request.instance_id));
    Ok(envelope(request.context, result))
}
/// Durably accepts one lifecycle change and returns acceptance before external execution.
#[tauri::command]
pub async fn change_instance(
    request: ChangeInstanceRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<InstanceActionView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let result = (|| {
        // A known receipt must remain readable even when the runner is full or closing.
        let known = state
            .database
            .receipt(&state.scope, &request.context.request_id)?
            .is_some();
        let reservation = if known {
            None
        } else {
            Some(state.runner.reserve().map_err(|_| StoreConflict::Backend)?)
        };
        if !known && state.probe.is_none() {
            return Err(StoreConflict::Backend);
        }
        let (receipt, kind, fresh) =
            instance_actions::accept(&*state.database, &state.scope, &request, &mut SystemRandom)?;
        if fresh {
            let backend = Arc::clone(state.inner());
            tauri::async_runtime::spawn_blocking(move || {
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()
                    .and_then(|runtime| {
                        let probe = backend.probe.as_ref()?;
                        let reservation = reservation?;
                        Some(runtime.block_on(run_confirmed_lifecycle_reserved(
                            &backend.database,
                            probe,
                            backend.database.management_root(),
                            &backend.runner,
                            &receipt,
                            kind,
                            reservation,
                        )))
                    });
                if !matches!(result, Some(Ok(_))) {
                    // Preserve Core's specific failed phase; only pre-stage failures need fallback.
                    let _ = backend.database.write(move |db| {
                        db.execute("UPDATE operations SET status='OutcomeUnknown', phase='reconcile' WHERE id=?1 AND status IN ('Accepted','Executing')", [&receipt.operation_id])?;
                        Ok(())
                    });
                }
            });
        }
        instance_actions::view(&*state.database, &state.scope, &request.instance_id)
    })();
    Ok(envelope(request.context, result))
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_acl_allows_instance_commands_only_on_the_local_main_window() {
        let mut context: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let authority = context.runtime_authority_mut();
        for command in [
            "get_instance_actions",
            "rename_instance",
            "change_instance",
            "get_instance_edit",
            "edit_instance_ports",
        ] {
            assert!(
                authority
                    .resolve_access(command, "main", "main", &tauri::ipc::Origin::Local)
                    .is_some(),
                "{command}"
            );
            assert!(
                authority
                    .resolve_access(command, "other", "other", &tauri::ipc::Origin::Local)
                    .is_none(),
                "{command}"
            );
            assert!(
                authority
                    .resolve_access(
                        command,
                        "main",
                        "main",
                        &tauri::ipc::Origin::Remote {
                            url: "https://example.com".parse().unwrap()
                        }
                    )
                    .is_none(),
                "{command}"
            );
        }
    }
}
