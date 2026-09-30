//! Confirmed port recovery against immutable specs, artifacts and the fixed Engine.

use std::{collections::BTreeMap, ffi::OsString, path::Path};

use composenest_application::{
    create_state::CreateStateStore,
    host_ports::{PortCursor, PortInspector, PortPlan, PortReason, PortSlot, plan_ports},
    image_resolution::ImageResolutionStore,
    lifecycle_operation::LifecycleStages,
    operation_journal::{
        ExpectedResult, OperationJournal, OperationKind, OperationStatus, RequestReceipt,
        StepCommand, StepIntent, StepOutcome,
    },
    operation_recovery::{RecoverableOperation, RecoveryJournal, RecoveryProbe},
    operation_runner::OperationRunner,
    port_edit::{PortEditStore, PortRecoveryRequest, PortRecoveryStore},
    state_store::{PortAllocation, StoreConflict},
};
use composenest_domain::instance::RuntimeStatus;
use rusqlite::params;

use crate::{
    artifact_store::{ArtifactInput, ArtifactStore},
    create_projection::CreateProjection,
    docker_cli::DockerCli,
    docker_observation::{ExpectedContainer, Ownership},
    docker_target::DockerProbe,
    host_ports::PortSnapshot,
    lifecycle_stages::AdapterLifecycleStages,
    named_volumes::DockerNamedVolumes,
    port_edit_stages::{PortEditError, fail, map_docker, map_effect},
    sqlite::DatabaseWorker,
    storage::BindStorage,
};

/// Explicit recovery intent; proposals never implicitly authorize a configuration change.
pub enum PortRecoveryAction {
    /// Observe and commit an already applied change without sending a change command.
    Reconcile,
    /// Retry the currently confirmed candidate in the same operation.
    Retry,
    /// Restore the original artifact of an edit, keeping the container stopped.
    Restore,
    /// Suggest alternatives only for ports freshly observed to conflict.
    Propose(PortCursor),
    /// Apply the complete candidate set explicitly confirmed by the user.
    Confirm {
        candidate_revision: u64,
        ports: Vec<PortAllocation>,
    },
}

/// A non-sensitive recovery result without secret values or storage paths.
pub enum PortRecoveryResult {
    /// Full configuration and the original runtime intent were verified and committed.
    Completed(String),
    /// Complete port candidates, or the bounded search's continuation/input requirement.
    Proposal(PortPlan),
    /// Inconclusive evidence retains every reservation and sends no change command.
    Held,
}

/// Recovers one operation under the instance gate after a fresh prior-CLI liveness probe.
/// The probe must establish termination for the recorded attempt, including after restart.
/// Artifact, storage, ownership and complete configuration are independently checked here.
pub async fn recover_port_change<P: RecoveryProbe>(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    root: &Path,
    runner: &OperationRunner,
    receipt: &RequestReceipt,
    previous_cli: &P,
    action: PortRecoveryAction,
) -> Result<PortRecoveryResult, PortEditError> {
    runner
        .run_exclusive(&receipt.instance_id, || async {
            let operation = database
                .recoverable(&receipt.operation_id)
                .map_err(PortEditError::Store)?;
            let evidence = previous_cli.inspect(&operation).await;
            if !evidence.previous_cli_exited
                || !evidence.target_matches
                || !matches!(
                    operation.kind,
                    OperationKind::Create | OperationKind::Clone | OperationKind::EditPort
                )
                || !matches!(
                    operation.status,
                    OperationStatus::Failed
                        | OperationStatus::AwaitingDecision
                        | OperationStatus::OutcomeUnknown
                )
            {
                return Ok(PortRecoveryResult::Held);
            }
            recover_locked(database, probe, root, receipt, operation, action).await
        })
        .await
        .map_err(PortEditError::Runner)?
}

async fn recover_locked(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    root: &Path,
    receipt: &RequestReceipt,
    operation: RecoverableOperation,
    action: PortRecoveryAction,
) -> Result<PortRecoveryResult, PortEditError> {
    let mut confirmed = database
        .confirmed_create(receipt)
        .map_err(PortEditError::Store)?;
    let pending = match database.port_change_revisions(&operation.id) {
        Ok(revisions) => Some(revisions),
        Err(StoreConflict::Missing) if operation.kind != OperationKind::EditPort => None,
        Err(error) => return Err(PortEditError::Store(error)),
    };
    let original = pending.map_or(confirmed.spec_revision, |(old, _)| old);
    let candidate = confirmed.spec_revision;
    let restore = matches!(action, PortRecoveryAction::Restore)
        || (matches!(action, PortRecoveryAction::Reconcile) && operation.phase == "restore");
    if restore && operation.kind != OperationKind::EditPort {
        return Err(PortEditError::Rejected);
    }
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
        .image_resolution(&confirmed.instance_id, original)
        .map_err(PortEditError::Store)?
        .ok_or(PortEditError::Rejected)?;
    let mut stages = AdapterLifecycleStages {
        database,
        docker: &docker,
        volumes: &volumes,
        binds: &binds,
        artifacts: &artifacts,
        artifact_id: format!("{}-r{candidate}", confirmed.instance_id),
        confirmed: confirmed.clone(),
        image: Some(image.clone()),
        kind: operation.kind,
    };
    stages.verify_storage().await.map_err(map_effect)?;
    let actual = stages
        .docker_create()
        .map_err(map_effect)?
        .project_container_id()
        .await
        .map_err(map_docker)?;
    let mut matching_revision = None;
    let mut verified = None;
    let mut runtime = RuntimeStatus::Absent;
    if let Some(id) = &actual {
        // Older confirmed candidates remain possible after a failed replacement.
        // Check full configuration, never just the new container ID or revision label.
        for revision in original..=candidate {
            select_revision(database, &mut stages, revision)?;
            if stages.verify_artifact().is_err() {
                continue;
            }
            let observed = stages.observe(id).await;
            if observed.owned && observed.configuration_matches {
                matching_revision = Some(revision);
                runtime = observed.status;
                verified = Some(stages.expected(id).await.map_err(map_effect)?);
                break;
            }
        }
        if matching_revision.is_none() {
            return Ok(PortRecoveryResult::Held);
        }
    }
    let target_revision = if restore { original } else { candidate };
    select_revision(database, &mut stages, target_revision)?;
    let ready = operation.kind != OperationKind::EditPort;
    let complete = matching_revision == Some(target_revision)
        && runtime
            == if ready {
                RuntimeStatus::Ready
            } else {
                RuntimeStatus::Stopped
            };
    if matches!(action, PortRecoveryAction::Reconcile) && !complete {
        return Ok(PortRecoveryResult::Held);
    }
    if !complete && !matches!(runtime, RuntimeStatus::Stopped | RuntimeStatus::Absent) {
        return Ok(PortRecoveryResult::Held);
    }

    let cli = DockerCli::new(
        probe.executable.clone(),
        probe.directory.clone(),
        probe.config_directory.clone(),
        OsString::from(&confirmed.target.endpoint),
    )
    .map_err(|_| PortEditError::OutcomeUnknown)?;
    if let PortRecoveryAction::Propose(cursor) = action {
        let snapshot =
            PortSnapshot::observe_recovery(database, &cli, &receipt.scope_id, actual.as_deref())
                .await
                .map_err(PortEditError::Port)?;
        let mut affected = Vec::new();
        for port in &confirmed.ports {
            match snapshot.inspect_reserved_candidate(port.host_port) {
                Ok(()) => {}
                Err(PortReason::Host | PortReason::Docker) => affected.push(PortSlot {
                    key: port.slot.clone(),
                    source: Some(port.host_port),
                    recommended: None,
                    explicit: None,
                    min: 1024,
                    max: 65535,
                }),
                Err(reason) => return Err(PortEditError::Port(reason)),
            }
        }
        if affected.is_empty() {
            return Ok(PortRecoveryResult::Held);
        }
        database
            .set_status(
                &operation.id,
                OperationStatus::AwaitingDecision,
                "port_conflict",
            )
            .map_err(PortEditError::Store)?;
        let mut plan = plan_ports(&affected, &snapshot, cursor);
        if let PortPlan::Complete(ports) = &mut plan {
            for port in &confirmed.ports {
                ports.entry(port.slot.clone()).or_insert(port.host_port);
            }
        }
        return Ok(PortRecoveryResult::Proposal(plan));
    }
    let confirming = matches!(action, PortRecoveryAction::Confirm { .. });
    if let PortRecoveryAction::Confirm {
        candidate_revision,
        ports,
    } = action
    {
        if complete || candidate_revision != candidate {
            return Err(PortEditError::Rejected);
        }
        let snapshot =
            PortSnapshot::observe_recovery(database, &cli, &receipt.scope_id, actual.as_deref())
                .await
                .map_err(PortEditError::Port)?;
        for port in &ports {
            if !confirmed
                .ports
                .iter()
                .any(|previous| previous.slot == port.slot && previous.host_port == port.host_port)
            {
                snapshot.inspect(port.host_port).result.map_err(|reason| {
                    fail(
                        database,
                        &operation.id,
                        "port_conflict",
                        PortEditError::Port(reason),
                    )
                })?;
            }
        }
        let revision = database
            .confirm_port_recovery(&PortRecoveryRequest {
                receipt: receipt.clone(),
                expected_candidate_revision: candidate_revision,
                ports,
            })
            .map_err(PortEditError::Store)?;
        confirmed = database
            .confirmed_create(receipt)
            .map_err(PortEditError::Store)?;
        stages.confirmed = confirmed;
        select_revision(database, &mut stages, revision)?;
    }
    if pending.is_none() && !confirming {
        return Ok(PortRecoveryResult::Held);
    }
    for step in &operation.steps {
        if step.outcome.is_none() || step.outcome == Some(StepOutcome::Unknown) {
            database
                .reconcile_step(&operation.id, step.sequence, StepOutcome::Unknown)
                .map_err(PortEditError::Store)?;
        }
    }
    let phase = if restore { "restore" } else { "ports" };
    let attempt = database
        .retry(&operation.id, phase)
        .map_err(PortEditError::Store)?;
    let mut effects = Effects {
        database,
        operation_id: &operation.id,
        attempt,
        sequence: operation.steps.last().map_or(1, |step| step.sequence + 1),
    };
    let result = apply(
        &mut effects,
        &mut stages,
        verified.as_ref(),
        complete,
        restore,
        ready,
        &cli,
    )
    .await;
    result.map_err(|error| fail(database, &operation.id, phase, error))
}

fn select_revision(
    database: &DatabaseWorker,
    stages: &mut AdapterLifecycleStages<'_>,
    revision: u64,
) -> Result<(), PortEditError> {
    stages.confirmed.ports = database.read(|db| {
        let mut statement = db.prepare("SELECT slot, host_ip, host_port, container_port FROM port_bindings WHERE instance_id = ?1 AND spec_revision = ?2 ORDER BY slot")?;
        statement.query_map(params![stages.confirmed.instance_id, revision], |row| Ok(PortAllocation { slot: row.get(0)?, host_ip: row.get(1)?, host_port: row.get(2)?, container_port: row.get(3)? }))?.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }).map_err(|_| PortEditError::Rejected)?;
    stages.confirmed.spec_revision = revision;
    stages.artifact_id = format!("{}-r{revision}", stages.confirmed.instance_id);
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

async fn apply(
    effects: &mut Effects<'_>,
    stages: &mut AdapterLifecycleStages<'_>,
    verified: Option<&ExpectedContainer>,
    complete: bool,
    restore: bool,
    ready: bool,
    cli: &DockerCli,
) -> Result<PortRecoveryResult, PortEditError> {
    let database = effects.database;
    let create = stages.docker_create().map_err(map_effect)?;
    let actual = verified.map(|expected| expected.container_id.as_str());
    let id = if complete {
        actual.ok_or(PortEditError::Rejected)?.to_owned()
    } else {
        if restore {
            stages.verify_artifact().map_err(map_effect)?;
        } else {
            let image = stages.image.as_ref().ok_or(PortEditError::Rejected)?;
            let projection = CreateProjection::new(
                database,
                stages.docker,
                stages.binds,
                &stages.confirmed.project_name,
                &stages.confirmed.instance_id,
            );
            let model = projection
                .model(&stages.confirmed, image)
                .map_err(|_| PortEditError::Rejected)?;
            let yaml = composenest_domain::compose::to_yaml(&model)
                .map_err(|_| PortEditError::Rejected)?;
            effects
                .run(
                    StepCommand::GenerateArtifact,
                    ExpectedResult::ArtifactReady,
                    &stages.artifact_id,
                    async {
                        stages
                            .artifacts
                            .publish(
                                effects.operation_id,
                                ArtifactInput {
                                    id: stages.artifact_id.clone(),
                                    instance_id: stages.confirmed.instance_id.clone(),
                                    spec_revision: stages.confirmed.spec_revision,
                                    generator_version: "compose-v1".into(),
                                    files: BTreeMap::from([(
                                        "compose.yaml".into(),
                                        yaml.into_bytes(),
                                    )]),
                                },
                            )
                            .map(|_| ())
                            .map_err(|_| PortEditError::OutcomeUnknown)
                    },
                )
                .await?;
        }
        let snapshot =
            PortSnapshot::observe_recovery(database, cli, &stages.confirmed.scope_id, actual)
                .await
                .map_err(PortEditError::Port)?;
        for port in &stages.confirmed.ports {
            snapshot
                .inspect_reserved_candidate(port.host_port)
                .map_err(PortEditError::Port)?;
        }
        if create
            .project_container_id()
            .await
            .map_err(map_docker)?
            .as_deref()
            != actual
        {
            return Err(PortEditError::OutcomeUnknown);
        }
        if let Some(expected) = verified {
            let observed = stages.docker.observe(expected).await;
            if observed.ownership != Ownership::Verified
                || observed.configuration_matches != Some(true)
                || observed.status != RuntimeStatus::Stopped
            {
                return Err(PortEditError::OutcomeUnknown);
            }
        }
        effects
            .run(
                StepCommand::ComposeCreate,
                ExpectedResult::ContainerCreated,
                &stages.confirmed.instance_id,
                async { create.recreate_stopped().await.map_err(map_docker) },
            )
            .await?;
        let id = create.created_container_id().await.map_err(map_docker)?;
        let expected = stages.expected(&id).await.map_err(map_effect)?;
        create.verify_stopped(&expected).await.map_err(map_docker)?;
        if ready {
            let snapshot = PortSnapshot::observe_recovery(
                database,
                cli,
                &stages.confirmed.scope_id,
                Some(&id),
            )
            .await
            .map_err(PortEditError::Port)?;
            for port in &stages.confirmed.ports {
                snapshot
                    .inspect_reserved_candidate(port.host_port)
                    .map_err(PortEditError::Port)?;
            }
            let instance = stages.confirmed.instance_id.clone();
            database.write(move |db| { db.execute("UPDATE storage_allocations SET initialization = 'may_have_initialized' WHERE instance_id = ?1 AND presence = 'present'", [instance])?; Ok(()) }).map_err(|_| PortEditError::Rejected)?;
            effects
                .run(
                    StepCommand::ComposeStart,
                    ExpectedResult::ContainerRunning,
                    &id,
                    async { stages.start(&id).await.map_err(map_effect) },
                )
                .await?;
            stages.wait_ready(&id).await.map_err(map_effect)?;
        }
        id
    };
    effects
        .run(
            StepCommand::Observe,
            if ready {
                ExpectedResult::ContainerRunning
            } else {
                ExpectedResult::ContainerStopped
            },
            &id,
            async {
                stages.verify_artifact().map_err(map_effect)?;
                stages.verify_storage().await.map_err(map_effect)?;
                let observed = stages.observe(&id).await;
                if !observed.owned
                    || !observed.configuration_matches
                    || observed.status
                        != if ready {
                            RuntimeStatus::Ready
                        } else {
                            RuntimeStatus::Stopped
                        }
                {
                    return Err(PortEditError::OutcomeUnknown);
                }
                Ok(())
            },
        )
        .await?;
    if restore {
        database.complete_port_restore(effects.operation_id, &id)
    } else {
        database.complete_port_edit(effects.operation_id, &id)
    }
    .map_err(PortEditError::Store)?;
    Ok(PortRecoveryResult::Completed(id))
}
