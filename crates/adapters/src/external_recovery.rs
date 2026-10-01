//! Explicit restoration from saved specs; external Compose is never an execution input.

use std::{collections::BTreeMap, path::Path};

use composenest_application::{
    create_state::CreateStateStore,
    image_resolution::ImageResolutionStore,
    lifecycle_operation::LifecycleStages,
    operation_journal::{
        ExpectedResult, OperationJournal, OperationKind, OperationStatus, StepCommand, StepIntent,
        StepOutcome,
    },
    operation_recovery::{RecoverableOperation, RecoveryJournal, RecoveryProbe},
    operation_runner::OperationRunner,
    state_store::StoreConflict,
};
use composenest_domain::instance::RuntimeStatus;
use rusqlite::OptionalExtension;

use crate::{
    artifact_store::{ArtifactInput, ArtifactStore, ExternalArtifact},
    create_projection::CreateProjection,
    docker_observation::Ownership,
    docker_target::DockerProbe,
    external_recovery_state::{ExternalRecoveryRecord, ExternalRecoveryRequest},
    lifecycle_stages::AdapterLifecycleStages,
    named_volumes::DockerNamedVolumes,
    port_edit_stages::{PortEditError, fail, map_docker, map_effect},
    sqlite::DatabaseWorker,
    storage::BindStorage,
};

/// Hash-only preview and saved revisions required by confirmation.
#[derive(Debug, Clone)]
pub struct ExternalRecoveryPreview {
    /// Instance revision checked again at acceptance.
    pub instance_revision: u64,
    /// Applied immutable spec used for regeneration.
    pub spec_revision: u64,
    /// File changes without values, including credentials.
    pub external: ExternalArtifact,
}

/// Returns the differences against the saved generated files without changing state.
pub fn preview_external_recovery(
    database: &DatabaseWorker,
    root: &Path,
    scope: &str,
    instance: &str,
) -> Result<ExternalRecoveryPreview, PortEditError> {
    let (instance_revision, spec_revision) = database.read(|db| {
        db.query_row("SELECT revision, applied_spec_revision FROM instances WHERE id = ?1 AND scope_id = ?2 AND lifecycle = 'managed'", [instance, scope], |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?))).optional()?.ok_or(crate::sqlite::DatabaseError::Missing)
    }).map_err(|_| PortEditError::Store(StoreConflict::Missing))?;
    let id = database
        .selected_artifact(instance, spec_revision)
        .map_err(PortEditError::Store)?;
    let external = ArtifactStore::new(root, database)
        .inspect_external(&id)
        .map_err(|_| PortEditError::Rejected)?;
    Ok(ExternalRecoveryPreview {
        instance_revision,
        spec_revision,
        external,
    })
}

/// Confirms restoration, including owned-runtime stop and stopped recreation when required.
/// The liveness probe must verify termination of any earlier CLI on the recorded target.
/// Calling again reconciles the same operation; no archive or external file is executed.
pub async fn restore_external_artifact<P: RecoveryProbe>(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    root: &Path,
    runner: &OperationRunner,
    request: &ExternalRecoveryRequest,
    previous_cli: &P,
) -> Result<String, PortEditError> {
    runner
        .run_exclusive(&request.receipt.instance_id, || async {
            database
                .begin_external_recovery(request)
                .map_err(PortEditError::Store)?;
            let result = restore_locked(database, probe, root, request, previous_cli).await;
            result.map_err(|error| fail(database, &request.receipt.operation_id, "restore", error))
        })
        .await
        .map_err(PortEditError::Runner)?
}

async fn restore_locked<P: RecoveryProbe>(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    root: &Path,
    request: &ExternalRecoveryRequest,
    previous_cli: &P,
) -> Result<String, PortEditError> {
    let receipt = &request.receipt;
    // A completed idempotent request returns its stored result without another effect.
    let completed = database.read(|db| {
        Ok(db.query_row("SELECT s.resource_id FROM operations o JOIN operation_steps s ON s.operation_id = o.id WHERE o.id = ?1 AND o.status = 'Succeeded' AND s.command_kind = 'observe' AND s.outcome = 'succeeded' ORDER BY s.sequence DESC LIMIT 1", [&receipt.operation_id], |row| row.get::<_, String>(0)).optional()?)
    }).map_err(|_| PortEditError::Store(StoreConflict::Backend))?;
    if let Some(id) = completed {
        return Ok(id);
    }
    let operation = database
        .recoverable(&receipt.operation_id)
        .map_err(PortEditError::Store)?;
    let evidence = previous_cli.inspect(&operation).await;
    if !evidence.previous_cli_exited || !evidence.target_matches {
        return Err(PortEditError::OutcomeUnknown);
    }
    let record = database
        .external_recovery(&receipt.operation_id)
        .map_err(PortEditError::Store)?;
    let confirmed = database
        .confirmed_create(receipt)
        .map_err(PortEditError::Store)?;
    let docker = probe
        .bind(confirmed.target.clone())
        .map_err(|_| PortEditError::OutcomeUnknown)?;
    let volumes = DockerNamedVolumes::new(
        probe
            .bind(confirmed.target.clone())
            .map_err(|_| PortEditError::OutcomeUnknown)?,
    );
    let binds = BindStorage::new(root);
    let artifacts = ArtifactStore::new(root, database);
    let image = database
        .image_resolution(&confirmed.instance_id, confirmed.spec_revision)
        .map_err(PortEditError::Store)?
        .ok_or(PortEditError::Rejected)?;
    let stages = AdapterLifecycleStages {
        database,
        docker: &docker,
        volumes: &volumes,
        binds: &binds,
        artifacts: &artifacts,
        confirmed,
        artifact_id: record.replacement_artifact_id.clone(),
        image: Some(image.clone()),
        kind: OperationKind::Recover,
    };
    stages.verify_storage().await.map_err(map_effect)?;
    let create = stages.docker_create().map_err(map_effect)?;
    let actual = create.project_container_id().await.map_err(map_docker)?;
    verify_runtime_identity(&stages, &record, &operation, &actual).await?;
    reconcile_steps(&stages, &record, &operation, &actual).await?;
    let attempt = if operation.status == OperationStatus::Accepted {
        database
            .set_status(&operation.id, OperationStatus::Executing, "restore")
            .map_err(PortEditError::Store)?;
        operation.attempt
    } else {
        database
            .retry(&operation.id, "restore")
            .map_err(PortEditError::Store)?
    };
    let mut effects = Effects {
        database,
        operation_id: &operation.id,
        attempt,
        sequence: operation.steps.last().map_or(1, |s| s.sequence + 1),
    };
    let projection = CreateProjection::new(
        database,
        &docker,
        &binds,
        &stages.confirmed.project_name,
        &stages.confirmed.instance_id,
    );
    let model = projection
        .model(&stages.confirmed, &image)
        .map_err(|_| PortEditError::Rejected)?;
    let yaml = composenest_domain::compose::to_yaml(&model).map_err(|_| PortEditError::Rejected)?;
    effects
        .run(
            StepCommand::GenerateArtifact,
            ExpectedResult::ArtifactReady,
            &stages.artifact_id,
            async {
                artifacts
                    .archive_external(
                        &operation.id,
                        &record.source_artifact_id,
                        &record.confirmation_hash,
                    )
                    .map_err(|_| PortEditError::Rejected)?;
                artifacts
                    .publish(
                        &operation.id,
                        ArtifactInput {
                            id: stages.artifact_id.clone(),
                            instance_id: stages.confirmed.instance_id.clone(),
                            spec_revision: stages.confirmed.spec_revision,
                            generator_version: "compose-v1".into(),
                            files: BTreeMap::from([("compose.yaml".into(), yaml.into_bytes())]),
                        },
                    )
                    .map_err(|_| PortEditError::OutcomeUnknown)?;
                Ok(())
            },
        )
        .await?;
    create.validate_config().await.map_err(map_docker)?;
    let (id, status) = restore_runtime(&stages, &mut effects, actual).await?;
    stages.verify_artifact().map_err(map_effect)?;
    effects
        .run(
            StepCommand::Observe,
            ExpectedResult::StateObserved,
            &id,
            async { Ok(()) },
        )
        .await?;
    database
        .complete_external_recovery(&receipt.operation_id, &id, status)
        .map_err(PortEditError::Store)?;
    Ok(id)
}

async fn restore_runtime(
    stages: &AdapterLifecycleStages<'_>,
    effects: &mut Effects<'_>,
    actual: Option<String>,
) -> Result<(String, RuntimeStatus), PortEditError> {
    let create = stages.docker_create().map_err(map_effect)?;
    let mut final_id = actual;
    let mut matching = false;
    if let Some(id) = &final_id {
        let observed = stages.observe(id).await;
        if !observed.owned
            || matches!(
                observed.status,
                RuntimeStatus::Unknown(_) | RuntimeStatus::Absent
            )
        {
            return Err(PortEditError::OutcomeUnknown);
        }
        matching = observed.configuration_matches;
        if !matching {
            effects
                .run(
                    StepCommand::ComposeStop,
                    ExpectedResult::ContainerStopped,
                    id,
                    async { stages.stop(id).await.map_err(map_effect) },
                )
                .await?;
            let stopped = stages.docker.observe(&stages.ownership_only(id)).await;
            if stopped.ownership != Ownership::Verified || stopped.status != RuntimeStatus::Stopped
            {
                return Err(PortEditError::OutcomeUnknown);
            }
        }
    }
    if !matching {
        // Refresh ownership and the project membership immediately before force-recreate.
        if create.project_container_id().await.map_err(map_docker)? != final_id {
            return Err(PortEditError::OutcomeUnknown);
        }
        if let Some(id) = &final_id {
            let stopped = stages.docker.observe(&stages.ownership_only(id)).await;
            if stopped.ownership != Ownership::Verified || stopped.status != RuntimeStatus::Stopped
            {
                return Err(PortEditError::OutcomeUnknown);
            }
        }
        stages.verify_storage().await.map_err(map_effect)?;
        effects
            .run(
                StepCommand::ComposeCreate,
                ExpectedResult::ContainerCreated,
                &stages.confirmed.instance_id,
                async { create.recreate_stopped().await.map_err(map_docker) },
            )
            .await?;
        final_id = Some(create.created_container_id().await.map_err(map_docker)?);
    }
    let id = final_id.ok_or(PortEditError::OutcomeUnknown)?;
    let observed = stages.observe(&id).await;
    if !observed.owned
        || !observed.configuration_matches
        || matches!(
            observed.status,
            RuntimeStatus::Unknown(_) | RuntimeStatus::Absent
        )
        || (!matching && observed.status != RuntimeStatus::Stopped)
    {
        return Err(PortEditError::OutcomeUnknown);
    }
    Ok((id, observed.status))
}

async fn verify_runtime_identity(
    stages: &AdapterLifecycleStages<'_>,
    record: &ExternalRecoveryRecord,
    operation: &RecoverableOperation,
    actual: &Option<String>,
) -> Result<(), PortEditError> {
    if let Some(id) = &actual {
        let owned = stages.docker.observe(&stages.ownership_only(id)).await;
        if owned.ownership != Ownership::Verified
            || matches!(
                owned.status,
                RuntimeStatus::Unknown(_) | RuntimeStatus::Absent
            )
        {
            return Err(PortEditError::OutcomeUnknown);
        }
        if record.original_container_id.as_ref() != Some(id) {
            let has_create = operation
                .steps
                .iter()
                .any(|s| s.command_kind == StepCommand::ComposeCreate);
            if !has_create {
                return Err(PortEditError::Rejected);
            }
            if let Some(original) = &record.original_container_id
                && stages
                    .docker
                    .observe(&stages.ownership_only(original))
                    .await
                    .status
                    != RuntimeStatus::Absent
            {
                return Err(PortEditError::OutcomeUnknown);
            }
            let observed = stages.observe(id).await;
            if !observed.owned
                || !observed.configuration_matches
                || observed.status != RuntimeStatus::Stopped
            {
                return Err(PortEditError::OutcomeUnknown);
            }
        }
    } else if let Some(original) = &record.original_container_id
        && stages
            .docker
            .observe(&stages.ownership_only(original))
            .await
            .status
            != RuntimeStatus::Absent
    {
        return Err(PortEditError::OutcomeUnknown);
    }
    Ok(())
}

async fn reconcile_steps(
    stages: &AdapterLifecycleStages<'_>,
    record: &ExternalRecoveryRecord,
    operation: &RecoverableOperation,
    actual: &Option<String>,
) -> Result<(), PortEditError> {
    // Recheck archived bytes before resuming any journaled generation.
    if operation.status != OperationStatus::Accepted {
        stages
            .database
            .set_status(&operation.id, OperationStatus::OutcomeUnknown, "reconcile")
            .map_err(PortEditError::Store)?;
    }
    for step in operation
        .steps
        .iter()
        .filter(|s| matches!(s.outcome, None | Some(StepOutcome::Unknown)))
    {
        let outcome = match step.command_kind {
            StepCommand::GenerateArtifact => {
                stages
                    .artifacts
                    .archive_external(
                        &operation.id,
                        &record.source_artifact_id,
                        &record.confirmation_hash,
                    )
                    .map_err(|_| PortEditError::Rejected)?;
                if stages.verify_artifact().is_ok() {
                    StepOutcome::Succeeded
                } else {
                    StepOutcome::Failed
                }
            }
            StepCommand::ComposeStop => {
                if actual.as_ref() != Some(&step.resource_id) {
                    return Err(PortEditError::OutcomeUnknown);
                }
                if stages
                    .docker
                    .observe(&stages.ownership_only(&step.resource_id))
                    .await
                    .status
                    == RuntimeStatus::Stopped
                {
                    StepOutcome::Succeeded
                } else {
                    StepOutcome::Failed
                }
            }
            StepCommand::ComposeCreate | StepCommand::Observe => {
                let observed = if let Some(id) = &actual {
                    Some(stages.observe(id).await)
                } else {
                    None
                };
                if observed.is_some_and(|o| {
                    o.owned && o.configuration_matches && o.status == RuntimeStatus::Stopped
                }) {
                    StepOutcome::Succeeded
                } else {
                    StepOutcome::Failed
                }
            }
            _ => return Err(PortEditError::Rejected),
        };
        stages
            .database
            .reconcile_step(&operation.id, step.sequence, outcome)
            .map_err(PortEditError::Store)?;
    }
    Ok(())
}

struct Effects<'a> {
    database: &'a DatabaseWorker,
    operation_id: &'a str,
    attempt: u64,
    sequence: u64,
}
impl Effects<'_> {
    async fn run(
        &mut self,
        command: StepCommand,
        expected: ExpectedResult,
        resource: &str,
        effect: impl std::future::Future<Output = Result<(), PortEditError>>,
    ) -> Result<(), PortEditError> {
        self.database
            .record_step(&StepIntent {
                operation_id: self.operation_id.into(),
                sequence: self.sequence,
                attempt: self.attempt,
                command_kind: command,
                expected_result: expected,
                resource_id: resource.into(),
            })
            .map_err(PortEditError::Store)?;
        let result = effect.await;
        self.database
            .finish_step(
                self.operation_id,
                self.sequence,
                if result.is_ok() {
                    StepOutcome::Succeeded
                } else {
                    StepOutcome::Unknown
                },
            )
            .map_err(PortEditError::Store)?;
        self.sequence += 1;
        result
    }
}
