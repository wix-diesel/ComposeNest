//! Desktop-only transport for scoped create requests and detached accepted work.

use composenest_adapters::{
    SystemClock, SystemRandom, create_stages::run_confirmed_create, docker_cli::DockerCli,
    docker_target::DockerProbe, host_ports::PortSnapshot, sqlite::DatabaseWorker,
};
use composenest_application::{
    ErrorDto, OperationEvent, RequestContext, ResponseEnvelope, Retryability,
    create_plan::{CreatePlanView, PlanError},
    create_session::{
        ConfirmCreateRequest, CreatePlanRequest, CreateReceipt, CreateSession,
        PrepareCreateRequest, UpdateCreateRequest,
    },
    operation_journal::RequestReceipt,
    operation_runner::{OperationReservation, OperationRunner, ProgressSink, RunnerError},
    state_store::StateStore,
};
use std::{path::PathBuf, sync::Arc};
use tauri::{Emitter, State};
use tokio::sync::Mutex;

/// Trusted runtime composition shared by the desktop create commands.
pub struct CreateBackend {
    pub(super) database: Arc<DatabaseWorker>,
    pub(super) runner: Arc<OperationRunner>,
    scope: String,
    session: Mutex<CreateSession>,
    pub(super) clones: Mutex<composenest_application::clone_session::CloneSession>,
    probe: Option<DockerProbe>,
}

impl CreateBackend {
    /// Resolves one protected scope and local Docker executable without GUI paths.
    pub fn new(
        database: Arc<DatabaseWorker>,
        runner: Arc<OperationRunner>,
        home: PathBuf,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let scopes = database.read(|db| {
            let mut statement =
                db.prepare("SELECT id FROM management_scopes ORDER BY id LIMIT 2")?;
            Ok(statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })?;
        let scope = match scopes.as_slice() {
            [scope] => scope.clone(),
            [] => {
                database
                    .create_scope(
                        "local",
                        &home.to_string_lossy(),
                        &database.management_root().to_string_lossy(),
                    )
                    .map_err(|_| std::io::Error::other("scope initialization failed"))?;
                "local".into()
            }
            _ => return Err(std::io::Error::other("ambiguous management scope").into()),
        };
        let executable = composenest_adapters::docker_executable::discover(&home);
        let probe = executable.map(|executable| DockerProbe {
            executable,
            directory: database.management_root().to_path_buf(),
            config_directory: home.join(".docker"),
        });
        Ok(Self {
            database,
            runner,
            session: Mutex::new(CreateSession::new(scope.clone())),
            clones: Mutex::new(composenest_application::clone_session::CloneSession::new(
                scope.clone(),
            )),
            scope,
            probe,
        })
    }

    pub(super) async fn ports(&self) -> Result<PortSnapshot, PlanError> {
        let probe = self
            .probe
            .as_ref()
            .ok_or_else(|| failure("DOCKER_UNAVAILABLE"))?;
        let registered = self
            .database
            .runtime_target(&self.scope)
            .map_err(|_| failure("STORE_UNAVAILABLE"))?;
        let report = probe.diagnose(registered.as_ref()).await;
        let target = report
            .target("local-target".into(), self.scope.clone())
            .ok_or_else(|| failure("DOCKER_UNAVAILABLE"))?;
        if registered.is_none() {
            self.database
                .create_target(
                    &target.id,
                    &target.scope_id,
                    &target.endpoint,
                    &target.engine_id,
                    &target.platform,
                )
                .map_err(|_| failure("STORE_UNAVAILABLE"))?;
        }
        let cli = DockerCli::new(
            probe.executable.clone(),
            probe.directory.clone(),
            probe.config_directory.clone(),
            target.endpoint.into(),
        )
        .map_err(|_| failure("DOCKER_UNAVAILABLE"))?;
        PortSnapshot::observe(&self.database, &cli, &self.scope)
            .await
            .map_err(|_| failure("PORT_CHECK_UNAVAILABLE"))
    }
}

fn failure(code: &'static str) -> PlanError {
    PlanError {
        code,
        field_path: None,
    }
}

pub(super) fn envelope<T>(
    context: RequestContext,
    result: Result<T, PlanError>,
) -> ResponseEnvelope<T> {
    match result {
        Ok(value) => ResponseEnvelope::success(context.request_id, value),
        Err(error) => ResponseEnvelope::failure(
            context.request_id,
            ErrorDto {
                code: error.code.into(),
                field_path: error.field_path,
                reason: match error.code {
                    "OPERATION_CAPACITY_REACHED" => {
                        "他の操作が完了してから、同じプランで作成を再試行してください。"
                    }
                    "APPLICATION_SHUTTING_DOWN" => {
                        "アプリの終了中です。再起動後に作成してください。"
                    }
                    "DOCKER_UNAVAILABLE" => "Dockerの接続・対応環境を確認してください。",
                    "PLAN_NOT_FOUND" => {
                        "このプランは失効しました。作成・複製画面を開き直してください。"
                    }
                    "PLAN_STALE" | "PLAN_RECONFIRM" | "PORT_CONFLICT" => {
                        "プランまたはポートが変わりました。同じプランの設定を再確認してください。"
                    }
                    "SOURCE_UNAVAILABLE" => "複製元を利用できません。元の環境を確認してください。",
                    "CONFIGURATION_ONLY_CONFIRMATION_REQUIRED" => {
                        "データを複製しないことの確認が必要です。"
                    }
                    "PLAINTEXT_CONFIRMATION_REQUIRED" => "平文保存の確認が必要です。",
                    _ => "要求を処理できませんでした。設定と接続を再確認してください。",
                }
                .into(),
                retryability: if error.code == "OPERATION_CAPACITY_REACHED" {
                    Retryability::Retryable
                } else {
                    Retryability::NotRetryable
                },
                operation_id: None,
                safe_details: None,
            },
        ),
    }
}

/// Prepares candidates from a registered revision without allocating resources.
#[tauri::command]
pub async fn prepare_create(
    request: PrepareCreateRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<CreatePlanView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let mut session = state.session.lock().await;
    let result = state.ports().await.and_then(|ports| {
        session.prepare(
            request.template_revision_id,
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
pub async fn update_create_plan(
    request: UpdateCreateRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<CreatePlanView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let mut session = state.session.lock().await;
    let result = state.ports().await.and_then(|ports| {
        session.update(
            &request.plan_id,
            request.edit,
            &SystemClock,
            &mut SystemRandom,
            &ports,
        )
    });
    Ok(envelope(request.context, result))
}

/// Refreshes candidates on the same scoped plan.
#[tauri::command]
pub async fn view_create_plan(
    request: CreatePlanRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<CreatePlanView>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let mut session = state.session.lock().await;
    let result = state
        .ports()
        .await
        .and_then(|ports| session.view(&request.plan_id, &SystemClock, &ports));
    Ok(envelope(request.context, result))
}

/// Reconciles acceptance from durable state without contacting Docker.
#[tauri::command]
pub async fn get_create_receipt(
    request: CreatePlanRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<Option<CreateReceipt>>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let session = state.session.lock().await;
    Ok(envelope(
        request.context,
        session
            .receipt(&request.plan_id, &*state.database)
            .map(|receipt| receipt.as_ref().map(CreateReceipt::from)),
    ))
}

/// Discards only an unconfirmed in-memory plan.
#[tauri::command]
pub async fn discard_create_plan(
    request: CreatePlanRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<()>, ()> {
    if let Err(error) = request.context.validate() {
        return Ok(ResponseEnvelope::failure(
            request.context.request_id,
            *error,
        ));
    }
    let mut session = state.session.lock().await;
    Ok(envelope(
        request.context,
        session.discard(&request.plan_id, &*state.database, &SystemClock),
    ))
}

struct DesktopProgress(tauri::AppHandle);
impl ProgressSink for DesktopProgress {
    fn send(&self, event: OperationEvent) {
        // Events are hints; the durable journal remains authoritative if emission fails.
        let _ = self.0.emit("operation-progress", event);
    }
}

pub(super) fn execute(
    state: Arc<CreateBackend>,
    receipt: RequestReceipt,
    reservation: OperationReservation,
    app: tauri::AppHandle,
) {
    tauri::async_runtime::spawn_blocking(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| ())
            .and_then(|runtime| {
                let probe = state.probe.as_ref().ok_or(())?;
                runtime
                    .block_on(run_confirmed_create(
                        &state.database,
                        probe,
                        state.database.management_root(),
                        &state.runner,
                        &receipt,
                        &DesktopProgress(app),
                        reservation,
                    ))
                    .map_err(|_| ())
            });
        if result.is_err() {
            // Only an acceptance that never reached a stage needs this fallback status.
            let _ = state.database.write(move |db| {
                db.execute("UPDATE operations SET status = 'OutcomeUnknown', phase = 'reconcile' WHERE id = ?1 AND status = 'Accepted'", [&receipt.operation_id])?;
                Ok(())
            });
        }
    });
}

pub(super) fn accept_with_capacity(
    runner: &Arc<OperationRunner>,
    accept: impl FnOnce() -> Result<(RequestReceipt, bool), PlanError>,
) -> Result<(RequestReceipt, Option<OperationReservation>), PlanError> {
    let reservation = runner.reserve().map_err(|error| {
        failure(match error {
            RunnerError::CapacityReached => "OPERATION_CAPACITY_REACHED",
            _ => "APPLICATION_SHUTTING_DOWN",
        })
    })?;
    let (receipt, fresh) = accept()?;
    Ok((receipt, fresh.then_some(reservation)))
}

/// Accepts explicit confirmation and starts each fresh operation once.
#[tauri::command]
pub async fn confirm_create(
    request: ConfirmCreateRequest,
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
    let mut session = state.session.lock().await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_expose_only_stable_codes_and_japanese_guidance() {
        let context = RequestContext {
            api_version: 1,
            request_id: "request".into(),
        };
        let response = envelope::<()>(context, Err(failure("DOCKER_UNAVAILABLE")));
        assert_eq!(response.request_id, "request");
        assert!(response.result.is_none());
        let error = response.error.unwrap();
        assert_eq!(error.code, "DOCKER_UNAVAILABLE");
        assert!(error.safe_details.is_none());
        assert!(error.reason.contains("Docker"));
    }

    fn receipt() -> RequestReceipt {
        RequestReceipt {
            scope_id: "scope".into(),
            request_id: "request".into(),
            plan_id: Some("plan".into()),
            confirmed_revision: 1,
            request_hash: "a".repeat(64),
            instance_id: "new-instance".into(),
            operation_id: "new-operation".into(),
        }
    }

    #[test]
    fn third_confirmation_is_not_accepted_and_can_retry_after_capacity_is_released() {
        let runner = Arc::new(OperationRunner::new());
        let first = runner.reserve().unwrap();
        let second = runner.reserve().unwrap();
        let acceptances = std::cell::Cell::new(0);
        let accept = || {
            acceptances.set(acceptances.get() + 1);
            Ok((receipt(), true))
        };
        let error = accept_with_capacity(&runner, accept).err().unwrap();
        assert_eq!(error.code, "OPERATION_CAPACITY_REACHED");
        assert_eq!(acceptances.get(), 0);
        drop(first);
        let (accepted, reservation) = accept_with_capacity(&runner, accept).unwrap();
        assert_eq!(accepted.operation_id, "new-operation");
        assert!(reservation.is_some());
        assert_eq!(acceptances.get(), 1);
        assert!(matches!(
            runner.reserve(),
            Err(RunnerError::CapacityReached)
        ));
        drop(reservation);
        drop(second);
        assert!(runner.shutdown(std::time::Duration::ZERO));
    }

    #[test]
    fn rejected_or_duplicate_acceptance_releases_its_reserved_capacity() {
        let runner = Arc::new(OperationRunner::new());
        assert!(accept_with_capacity(&runner, || Err(failure("PLAN_STALE"))).is_err());
        let (_, reservation) = accept_with_capacity(&runner, || Ok((receipt(), false))).unwrap();
        assert!(reservation.is_none());
        assert!(runner.shutdown(std::time::Duration::ZERO));
    }

    #[test]
    fn capacity_rejection_is_retryable_on_the_same_plan() {
        let context = RequestContext {
            api_version: 1,
            request_id: "busy".into(),
        };
        let response =
            envelope::<CreateReceipt>(context, Err(failure("OPERATION_CAPACITY_REACHED")));
        assert!(response.result.is_none());
        let error = response.error.unwrap();
        assert_eq!(error.retryability, Retryability::Retryable);
        assert!(error.operation_id.is_none());
        assert!(error.reason.contains("同じプラン"));
    }

    #[test]
    fn empty_receipt_is_a_successful_reconciliation() {
        let context = RequestContext {
            api_version: 1,
            request_id: "receipt".into(),
        };
        let response = envelope::<Option<CreateReceipt>>(context, Ok(None));
        assert!(response.error.is_none());
        assert_eq!(response.result, Some(None));
    }
}
