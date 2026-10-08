//! Explicit recovery choices under the same instance gate and immutable operation identity.
use crate::{
    artifact_store::ArtifactStore,
    docker_target::DockerProbe,
    external_recovery_state::ExternalRecoveryRequest,
    lifecycle_stages::AdapterLifecycleStages,
    named_volumes::DockerNamedVolumes,
    port_edit_stages::{PortEditError, fail, map_effect},
    port_recovery::{PortRecoveryAction, PortRecoveryResult},
    recovery_view::{RecoverySession, inspect_locked},
    sqlite::DatabaseWorker,
    storage::BindStorage,
};
use composenest_application::{
    create_state::CreateStateStore,
    host_ports::{PortCursor, PortPlan},
    image_resolution::ImageResolutionStore,
    lifecycle_operation::{LifecycleStages, LifecycleState},
    operation_journal::{
        ExpectedResult, OperationJournal, OperationStatus, RequestReceipt, StepCommand, StepIntent,
        StepOutcome,
    },
    operation_recovery::{RecoveryJournal, abandon_operation, retry_operation},
    operation_runner::OperationRunner,
    recovery_view::{RecoverOperationRequest, RecoveryView},
    state_store::{PortAllocation, StoreConflict},
};
use composenest_domain::{clone_policy::RandomSource, instance::RuntimeStatus};

/// Executes only a freshly permitted choice; caller-supplied allow flags are never trusted.
pub async fn recover_operation(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    session: &RecoverySession,
    runner: &OperationRunner,
    scope: &str,
    request: &RecoverOperationRequest,
) -> Result<RecoveryView, PortEditError> {
    runner
        .run_exclusive(&request.instance_id, || {
            recover_locked(database, probe, session, scope, request)
        })
        .await
        .map_err(PortEditError::Runner)?
}

async fn recover_locked(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    session: &RecoverySession,
    scope: &str,
    request: &RecoverOperationRequest,
) -> Result<RecoveryView, PortEditError> {
    let known_receipt = if request.action == "restore_external" {
        database
            .receipt(scope, &request.context.request_id)
            .map_err(PortEditError::Store)?
    } else {
        None
    };
    crate::recovery_view::recovery_receipt(
        database,
        scope,
        &request.instance_id,
        &request.operation_id,
    )
    .map_err(PortEditError::Store)?;
    let inspecting_id = known_receipt
        .as_ref()
        .map_or(request.operation_id.as_str(), |r| r.operation_id.as_str());
    let inspected = inspect_locked(
        database,
        probe,
        session,
        scope,
        &request.instance_id,
        inspecting_id,
    )
    .await
    .map_err(PortEditError::Store)?;
    // A lost external-restoration response reuses exactly the accepted hash-bound receipt.
    let known_external = known_receipt.is_some();
    if !known_external {
        if inspected.view.attempt != request.expected_attempt
            || inspected.view.instance_revision != request.expected_revision
            || inspected.view.candidate_revision != request.candidate_revision
        {
            return Err(PortEditError::Store(StoreConflict::StaleRevision));
        }
        let required = if request.action == "confirm_ports" {
            "propose_ports"
        } else {
            &request.action
        };
        if !inspected.view.actions.iter().any(|a| a == required) {
            return Err(PortEditError::Rejected);
        }
    }
    match request.action.as_str() {
        "restore_external" | "retry_external" => {
            let external = if request.action == "retry_external" {
                let record = database
                    .external_recovery(&request.operation_id)
                    .map_err(PortEditError::Store)?;
                ExternalRecoveryRequest {
                    receipt: inspected.receipt.clone(),
                    spec_revision: request.candidate_revision,
                    source_artifact_id: record.source_artifact_id,
                    confirmation_hash: record.confirmation_hash,
                }
            } else {
                let recovery_id = if let Some(receipt) = &known_receipt {
                    receipt.operation_id.clone()
                } else {
                    let mut bytes = [0_u8; 16];
                    crate::SystemRandom
                        .fill_bytes(&mut bytes)
                        .map_err(|_| PortEditError::Store(StoreConflict::Backend))?;
                    format!("recover-{:032x}", u128::from_be_bytes(bytes))
                };
                let artifact = request.artifact_id.clone().ok_or(PortEditError::Rejected)?;
                let hash = request
                    .confirmation_hash
                    .clone()
                    .ok_or(PortEditError::Rejected)?;
                if !known_external
                    && (inspected.view.artifact_id.as_ref() != Some(&artifact)
                        || inspected.view.confirmation_hash.as_ref() != Some(&hash))
                {
                    return Err(PortEditError::Rejected);
                }
                let mut external = ExternalRecoveryRequest {
                    receipt: RequestReceipt {
                        scope_id: scope.into(),
                        request_id: request.context.request_id.clone(),
                        plan_id: None,
                        confirmed_revision: request.expected_revision,
                        request_hash: String::new(),
                        instance_id: request.instance_id.clone(),
                        operation_id: recovery_id,
                    },
                    spec_revision: request.candidate_revision,
                    source_artifact_id: artifact,
                    confirmation_hash: hash,
                };
                external.receipt.request_hash = external.request_hash();
                external
            };
            database
                .begin_external_recovery(&external)
                .map_err(PortEditError::Store)?;
            // Reinspect the accepted recovery identity: an inherited unfinished CLI remains held.
            let prior = inspect_locked(
                database,
                probe,
                session,
                scope,
                &request.instance_id,
                &external.receipt.operation_id,
            )
            .await
            .map_err(PortEditError::Store)?;
            crate::external_recovery::restore_locked(
                database,
                probe,
                database.management_root(),
                &external,
                &prior,
            )
            .await
            .map_err(|error| fail(database, &external.receipt.operation_id, "restore", error))?;
            return inspect_locked(
                database,
                probe,
                session,
                scope,
                &request.instance_id,
                &external.receipt.operation_id,
            )
            .await
            .map(|i| i.view)
            .map_err(PortEditError::Store);
        }
        "abandon" => abandon_operation(database, &request.operation_id, &inspected)
            .await
            .map_err(PortEditError::Store)?,
        "complete" | "retry" => lifecycle(database, probe, request, &inspected).await?,
        "propose_ports" | "confirm_ports" | "retry_ports" | "restore_ports" | "reconcile_ports" => {
            let action = match request.action.as_str() {
                "propose_ports" => PortRecoveryAction::Propose(PortCursor::default()),
                "retry_ports" => PortRecoveryAction::Retry,
                "restore_ports" => PortRecoveryAction::Restore,
                "reconcile_ports" => PortRecoveryAction::Reconcile,
                _ => {
                    if request.ports.len() != inspected.view.ports.len() {
                        return Err(PortEditError::Rejected);
                    }
                    let ports = inspected
                        .view
                        .ports
                        .iter()
                        .map(|p| {
                            Ok(PortAllocation {
                                slot: p.slot.clone(),
                                host_ip: p.host_ip.clone(),
                                container_port: p.container_port,
                                host_port: *request
                                    .ports
                                    .get(&p.slot)
                                    .ok_or(PortEditError::Rejected)?,
                            })
                        })
                        .collect::<Result<Vec<_>, PortEditError>>()?;
                    PortRecoveryAction::Confirm {
                        candidate_revision: request.candidate_revision,
                        ports,
                    }
                }
            };
            let operation = database
                .recoverable(&request.operation_id)
                .map_err(PortEditError::Store)?;
            let result = crate::port_recovery::recover_locked(
                database,
                probe,
                database.management_root(),
                &inspected.receipt,
                operation,
                action,
            )
            .await?;
            match result {
                PortRecoveryResult::Proposal(PortPlan::Complete(ports)) => {
                    let mut view = inspect_locked(
                        database,
                        probe,
                        session,
                        scope,
                        &request.instance_id,
                        &request.operation_id,
                    )
                    .await
                    .map_err(PortEditError::Store)?
                    .view;
                    view.proposed_ports = view
                        .ports
                        .iter()
                        .map(|p| {
                            let mut proposed = p.clone();
                            proposed.host_port = ports[&p.slot];
                            proposed
                        })
                        .collect();
                    return Ok(view);
                }
                PortRecoveryResult::Proposal(_) => {
                    return Err(PortEditError::Port(
                        composenest_application::host_ports::PortReason::Unavailable,
                    ));
                }
                PortRecoveryResult::Held => return Err(PortEditError::Rejected),
                PortRecoveryResult::Completed(_) => {}
            }
        }
        _ => return Err(PortEditError::Rejected),
    }
    inspect_locked(
        database,
        probe,
        session,
        scope,
        &request.instance_id,
        &request.operation_id,
    )
    .await
    .map(|i| i.view)
    .map_err(PortEditError::Store)
}

async fn lifecycle(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    request: &RecoverOperationRequest,
    inspected: &crate::recovery_view::Inspection,
) -> Result<(), PortEditError> {
    let operation = database
        .recoverable(&request.operation_id)
        .map_err(PortEditError::Store)?;
    let id = inspected
        .container_id
        .clone()
        .or_else(|| {
            database
                .snapshot(&inspected.receipt, operation.kind)
                .ok()
                .map(|s| s.container_id)
        })
        .ok_or(PortEditError::Rejected)?;
    if request.action == "complete" {
        let status = match inspected.evidence.runtime {
            composenest_application::operation_recovery::CurrentRuntime::Ready => {
                RuntimeStatus::Ready
            }
            composenest_application::operation_recovery::CurrentRuntime::Stopped => {
                RuntimeStatus::Stopped
            }
            composenest_application::operation_recovery::CurrentRuntime::Absent => {
                RuntimeStatus::Absent
            }
            _ => return Err(PortEditError::Rejected),
        };
        database
            .set_status(
                &request.operation_id,
                OperationStatus::Executing,
                "reconcile",
            )
            .map_err(PortEditError::Store)?;
        return database
            .complete(&request.operation_id, &id, status)
            .map_err(PortEditError::Store);
    }
    let confirmed = database
        .confirmed_create(&inspected.receipt)
        .map_err(PortEditError::Store)?;
    let docker = probe
        .bind(confirmed.target.clone())
        .map_err(|_| PortEditError::OutcomeUnknown)?;
    let volumes = DockerNamedVolumes::new(
        probe
            .bind(confirmed.target.clone())
            .map_err(|_| PortEditError::OutcomeUnknown)?,
    );
    let root = database.management_root();
    let binds = BindStorage::new(root);
    let artifacts = ArtifactStore::new(root, database);
    let stages = AdapterLifecycleStages {
        database,
        docker: &docker,
        volumes: &volumes,
        binds: &binds,
        artifacts: &artifacts,
        image: database
            .image_resolution(&request.instance_id, confirmed.spec_revision)
            .map_err(PortEditError::Store)?,
        artifact_id: database
            .selected_artifact(&request.instance_id, confirmed.spec_revision)
            .map_err(PortEditError::Store)?,
        confirmed,
        kind: operation.kind,
    };
    let attempt = retry_operation(database, &request.operation_id, inspected)
        .await
        .map_err(PortEditError::Store)?;
    let sequence = operation.steps.last().map_or(1, |s| s.sequence + 1);
    database
        .record_step(&StepIntent {
            operation_id: request.operation_id.clone(),
            sequence,
            attempt,
            command_kind: StepCommand::ComposeStart,
            resource_id: id.clone(),
            expected_result: ExpectedResult::ContainerRunning,
        })
        .map_err(PortEditError::Store)?;
    let result = stages.start(&id).await;
    let result = match result {
        Ok(()) => stages.wait_ready(&id).await,
        Err(e) => Err(e),
    };
    database
        .finish_step(
            &request.operation_id,
            sequence,
            if result.is_ok() {
                StepOutcome::Succeeded
            } else {
                StepOutcome::Unknown
            },
        )
        .map_err(PortEditError::Store)?;
    result.map_err(|e| fail(database, &request.operation_id, "start", map_effect(e)))?;
    database
        .complete(&request.operation_id, &id, RuntimeStatus::Ready)
        .map_err(PortEditError::Store)
}
