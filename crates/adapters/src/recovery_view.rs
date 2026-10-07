//! Fresh scoped recovery evidence. Unknown process history never authorizes a change.
use crate::{
    artifact_store::ArtifactStore, docker_cli::DockerCli, docker_observation::Ownership,
    docker_target::DockerProbe, lifecycle_stages::AdapterLifecycleStages,
    named_volumes::DockerNamedVolumes, sqlite::DatabaseWorker, storage::BindStorage,
};
use composenest_application::{
    create_state::CreateStateStore,
    image_resolution::ImageResolutionStore,
    lifecycle_operation::LifecycleStages,
    named_volumes::NamedVolumePort,
    operation_journal::{OperationJournal, OperationKind, RequestReceipt, StepOutcome},
    operation_recovery::{
        CurrentRuntime, RecoverableOperation, RecoveryDecision, RecoveryEvidence, RecoveryJournal,
        RecoveryProbe, classify_recovery,
    },
    port_edit::PortRecoveryStore,
    query_service::PortView,
    recovery_view::{RecoveryFileDiff, RecoveryView},
    state_store::StoreConflict,
};
use composenest_domain::instance::{RuntimeStatus, StoragePresence};
use rusqlite::{OptionalExtension, params};
use std::collections::HashSet;

/// Records attempts inherited from another process, for which CLI liveness is not proven.
pub struct RecoverySession {
    inherited: HashSet<String>,
}
impl RecoverySession {
    /// Captures unresolved identities before this process admits any new work.
    pub fn new(database: &DatabaseWorker) -> Result<Self, StoreConflict> {
        let inherited = database
            .read(|db| {
                let mut q = db.prepare(
                    "SELECT id FROM operations WHERE status NOT IN ('Succeeded','Abandoned')",
                )?;
                Ok(q.query_map([], |r| r.get(0))?
                    .collect::<Result<HashSet<String>, _>>()?)
            })
            .map_err(|_| StoreConflict::Backend)?;
        Ok(Self { inherited })
    }
    fn permits(&self, operation: &RecoverableOperation) -> bool {
        !self.inherited.contains(&operation.id)
            || operation.steps.iter().all(|s| {
                matches!(
                    s.outcome,
                    Some(StepOutcome::Succeeded | StepOutcome::Failed)
                )
            })
    }
}

/// Fixed evidence reused only inside a held instance gate.
pub(crate) struct Inspection {
    pub(crate) view: RecoveryView,
    pub(crate) evidence: RecoveryEvidence,
}
impl RecoveryProbe for Inspection {
    async fn inspect(&self, _: &RecoverableOperation) -> RecoveryEvidence {
        self.evidence
    }
}

/// Reads one original receipt after verifying scope, instance and operation identity.
pub fn recovery_receipt(
    database: &DatabaseWorker,
    scope: &str,
    instance: &str,
    operation: &str,
) -> Result<RequestReceipt, StoreConflict> {
    let request: String = database.read(|db| db.query_row(
        "SELECT r.request_id FROM request_receipts r JOIN operations o ON o.id=r.operation_id JOIN instances i ON i.id=o.instance_id WHERE o.id=?1 AND i.id=?2 AND i.scope_id=?3 AND r.scope_id=?3",
        params![operation, instance, scope], |r| r.get(0)).optional()?.ok_or(crate::sqlite::DatabaseError::Missing))
        .map_err(|_| StoreConflict::Missing)?;
    database
        .receipt(scope, &request)?
        .ok_or(StoreConflict::Missing)
}

/// Inspects without changing files, Docker, reservations or historical outcome.
/// The caller holds the instance gate until this evidence has been consumed.
pub async fn inspect_recovery(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    session: &RecoverySession,
    scope: &str,
    instance: &str,
    operation: &str,
) -> Result<RecoveryView, StoreConflict> {
    Ok(
        inspect_locked(database, probe, session, scope, instance, operation)
            .await?
            .view,
    )
}

pub(crate) async fn inspect_locked(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    session: &RecoverySession,
    scope: &str,
    instance: &str,
    operation_id: &str,
) -> Result<Inspection, StoreConflict> {
    let receipt = recovery_receipt(database, scope, instance, operation_id)?;
    let progress = crate::query_service::view_operation(database, scope, operation_id)?;
    let unresolved = !matches!(
        progress.operation.status.as_str(),
        "Succeeded" | "Abandoned"
    );
    let operation = database.recoverable(operation_id).ok();
    let mut confirmed = database.confirmed_create(&receipt)?;
    let original = database
        .port_change_revisions(operation_id)
        .ok()
        .map(|p| p.0);
    let candidate_revision = confirmed.spec_revision;
    let kind = operation
        .as_ref()
        .map_or(OperationKind::Recover, |o| o.kind);
    let artifact_id = database.selected_artifact(instance, candidate_revision)?;
    let root = database.management_root();
    let artifacts = ArtifactStore::new(root, database);
    let external = artifacts.inspect_external(&artifact_id).ok();
    let docker = probe
        .bind(confirmed.target.clone())
        .map_err(|_| StoreConflict::Backend)?;
    let volumes = DockerNamedVolumes::new(
        probe
            .bind(confirmed.target.clone())
            .map_err(|_| StoreConflict::Backend)?,
    );
    let binds = BindStorage::new(root);
    let cli = DockerCli::new(
        probe.executable.clone(),
        probe.directory.clone(),
        probe.config_directory.clone(),
        confirmed.target.endpoint.clone().into(),
    )
    .map_err(|_| StoreConflict::Backend)?;
    let previous_cli_exited =
        operation.as_ref().is_none_or(|o| session.permits(o)) && cli.termination_verified();
    // A fixed Engine read is separate from current-context display metadata.
    let target_matches = docker
        .read(&["info".into(), "--format".into(), "{{json .ID}}".into()])
        .await
        .is_ok();
    let mut reasons = Vec::new();
    if !previous_cli_exited {
        reasons.push("CLI_TERMINATION_UNCONFIRMED".into());
    }
    if !target_matches {
        reasons.push("RUNTIME_TARGET_MISMATCH".into());
    }
    let mut stages = AdapterLifecycleStages {
        database,
        docker: &docker,
        volumes: &volumes,
        binds: &binds,
        artifacts: &artifacts,
        image: database.image_resolution(instance, original.unwrap_or(candidate_revision))?,
        confirmed: confirmed.clone(),
        artifact_id: artifact_id.clone(),
        kind,
    };
    let mut missing = confirmed
        .storage
        .iter()
        .any(|s| s.presence == StoragePresence::Missing);
    let instance_identity = composenest_domain::identity::InstanceId::from_u128(
        u128::from_str_radix(instance, 16).map_err(|_| StoreConflict::InvalidInput)?,
    );
    for saved in &confirmed.storage {
        use composenest_application::storage::StoragePort;
        let actual = match saved.method {
            composenest_application::state_store::StorageMethod::Bind => {
                binds.inspect_bind(instance_identity, &saved.allocation)
            }
            composenest_application::state_store::StorageMethod::Volume => {
                let slot = composenest_domain::identity::SlotId::parse(&saved.slot)
                    .map_err(|_| StoreConflict::InvalidInput)?;
                volumes
                    .inspect_named_volume(database, database, instance_identity, &slot)
                    .await
                    .map_or(StoragePresence::Unverified, |v| v.presence)
            }
        };
        missing |= actual == StoragePresence::Missing;
    }
    let storage_verified = !missing && stages.verify_storage().await.is_ok();
    if !storage_verified {
        reasons.push(
            if missing {
                "STORAGE_MISSING"
            } else {
                "STORAGE_UNVERIFIED"
            }
            .into(),
        );
    }
    let artifact_matches = stages.verify_artifact().is_ok();
    if !artifact_matches {
        reasons.push("ARTIFACT_MODIFIED_OR_MISSING".into());
    }
    let actual = find_container(&docker, &confirmed.project_name).await;
    let container_id = actual.as_ref().ok().cloned().flatten();
    let mut runtime = CurrentRuntime::Unknown;
    let mut owned = false;
    let mut matching = false;
    let mut known_configuration = false;
    if let Ok(None) = &actual {
        // Verify a formerly recorded ID is absent as well as the project's current membership.
        let recorded: Option<String> = database
            .read(|db| {
                Ok(db
                    .query_row(
                        "SELECT container_id FROM runtime_observations WHERE instance_id=?1",
                        [instance],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten())
            })
            .map_err(|_| StoreConflict::Backend)?;
        if let Some(id) = recorded {
            if docker.observe(&stages.ownership_only(&id)).await.status == RuntimeStatus::Absent {
                runtime = CurrentRuntime::Absent;
                owned = true;
            }
        } else {
            runtime = CurrentRuntime::Absent;
            owned = true;
        }
    } else if let Some(id) = &container_id {
        let observation = docker.observe(&stages.ownership_only(id)).await;
        owned = observation.ownership == Ownership::Verified;
        if owned {
            runtime = current_runtime(&observation.status);
            matching = stages.observe(id).await.configuration_matches;
            known_configuration = matching;
            if !matching && let Some(old) = original {
                confirmed.spec_revision = old;
                confirmed.ports = database.read(|db| {
                    let mut q = db.prepare("SELECT slot,host_ip,host_port,container_port FROM port_bindings WHERE instance_id=?1 AND spec_revision=?2 ORDER BY slot")?;
                    Ok(q.query_map(params![instance,old], |r| Ok(composenest_application::state_store::PortAllocation { slot:r.get(0)?,host_ip:r.get(1)?,host_port:r.get(2)?,container_port:r.get(3)? }))?.collect::<Result<Vec<_>,_>>()?)
                }).map_err(|_| StoreConflict::Backend)?;
                stages.confirmed = confirmed;
                stages.artifact_id = database.selected_artifact(instance, old)?;
                known_configuration = stages.verify_artifact().is_ok()
                    && stages.observe(id).await.configuration_matches;
            }
        }
    }
    if !owned {
        reasons.push("OWNERSHIP_UNKNOWN".into());
    }
    if owned
        && runtime != CurrentRuntime::Absent
        && !known_configuration
        && kind != OperationKind::Stop
    {
        reasons.push("CONFIGURATION_MISMATCH".into());
    }
    let evidence = RecoveryEvidence {
        previous_cli_exited,
        target_matches,
        artifact_matches,
        storage_verified,
        docker_verified: owned
            && (matching || runtime == CurrentRuntime::Absent || kind == OperationKind::Stop),
        runtime,
    };
    let mut actions = Vec::new();
    if let Some(operation) = &operation {
        let paused = matches!(
            operation.status,
            composenest_application::operation_journal::OperationStatus::Failed
                | composenest_application::operation_journal::OperationStatus::OutcomeUnknown
                | composenest_application::operation_journal::OperationStatus::AwaitingDecision
        );
        if paused {
            match classify_recovery(operation, evidence) {
                RecoveryDecision::Complete
                    if matches!(
                        kind,
                        OperationKind::Start | OperationKind::Stop | OperationKind::Restart
                    ) =>
                {
                    actions.push("complete".into())
                }
                RecoveryDecision::RetryAllowed => actions.push("retry".into()),
                _ => {}
            }
            if previous_cli_exited
                && target_matches
                && artifact_matches
                && storage_verified
                && owned
            {
                if runtime == CurrentRuntime::Absent
                    && operation.steps.iter().all(|s| {
                        matches!(
                            s.outcome,
                            Some(StepOutcome::Succeeded | StepOutcome::Failed)
                        )
                    })
                {
                    actions.push("abandon".into());
                }
                if matches!(
                    kind,
                    OperationKind::Create | OperationKind::Clone | OperationKind::EditPort
                ) && (known_configuration || runtime == CurrentRuntime::Absent)
                {
                    if matches!(runtime, CurrentRuntime::Stopped | CurrentRuntime::Absent) {
                        actions.push("propose_ports".into());
                        if original.is_some() {
                            actions.extend(["retry_ports".into(), "restore_ports".into()]);
                        }
                    }
                    if matching
                        && ((kind == OperationKind::EditPort && runtime == CurrentRuntime::Stopped)
                            || (kind != OperationKind::EditPort
                                && runtime == CurrentRuntime::Ready))
                    {
                        actions.push("reconcile_ports".into());
                    }
                }
            }
        } else {
            reasons.push("OPERATION_EXECUTING".into());
        }
    }
    let files = external.as_ref().map_or(Vec::new(), |e| {
        e.differences
            .iter()
            .map(|d| RecoveryFileDiff {
                path: d.path.clone(),
                recorded_hash: d.recorded_hash.clone(),
                observed_hash: d.observed_hash.clone(),
            })
            .collect()
    });
    if !unresolved
        && previous_cli_exited
        && target_matches
        && storage_verified
        && owned
        && !files.is_empty()
    {
        actions.push("restore_external".into());
    }
    if progress
        .instance
        .last_operation
        .as_ref()
        .is_none_or(|o| o.id != operation_id)
    {
        actions.clear();
        reasons.push("OPERATION_TARGET_STALE".into());
    }
    if unresolved && actions.is_empty() && reasons.is_empty() {
        reasons.push("RECOVERY_HELD".into());
    }
    let ports = ports_for(database, instance, candidate_revision)?;
    let original_ports = original.map_or(Ok(Vec::new()), |revision| {
        ports_for(database, instance, revision)
    })?;
    Ok(Inspection {
        evidence,
        view: RecoveryView {
            instance_id: instance.into(),
            operation_id: operation_id.into(),
            attempt: operation.as_ref().map_or(0, |o| o.attempt),
            instance_revision: progress.instance.revision,
            candidate_revision,
            previous_status: progress
                .last_failure_status
                .unwrap_or(progress.operation.status),
            current_runtime: runtime_label(runtime).into(),
            actions,
            hold_reasons: reasons,
            ports,
            original_ports,
            proposed_ports: Vec::new(),
            artifact_id: external.as_ref().map(|e| e.artifact_id.clone()),
            confirmation_hash: external.map(|e| e.confirmation_hash),
            files,
        },
    })
}

pub(crate) fn ports_for(
    database: &DatabaseWorker,
    instance: &str,
    revision: u64,
) -> Result<Vec<PortView>, StoreConflict> {
    database.read(|db| {
        let mut q = db.prepare("SELECT slot,host_ip,host_port,container_port FROM port_bindings WHERE instance_id=?1 AND spec_revision=?2 ORDER BY slot")?;
        Ok(q.query_map(params![instance,revision], |r| Ok(PortView { slot:r.get(0)?,host_ip:r.get(1)?,host_port:r.get(2)?,container_port:r.get(3)? }))?.collect::<Result<Vec<_>,_>>()?)
    }).map_err(|_| StoreConflict::Backend)
}
async fn find_container(
    docker: &crate::docker_target::BoundDocker,
    project: &str,
) -> Result<Option<String>, ()> {
    let result = docker
        .read(&[
            "container".into(),
            "ls".into(),
            "--all".into(),
            "--no-trunc".into(),
            "--quiet".into(),
            "--filter".into(),
            format!("label=com.docker.compose.project={project}").into(),
            "--filter".into(),
            "label=com.docker.compose.service=main".into(),
        ])
        .await
        .map_err(|_| ())?;
    if !result.status.is_some_and(|s| s.success())
        || result.stdout.truncated
        || result.outcome_unknown
    {
        return Err(());
    }
    let value = std::str::from_utf8(&result.stdout.bytes).map_err(|_| ())?;
    let ids: Vec<_> = value.lines().collect();
    match ids.as_slice() {
        [] => Ok(None),
        [id] if id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Ok(Some((*id).into()))
        }
        _ => Err(()),
    }
}
fn current_runtime(status: &RuntimeStatus) -> CurrentRuntime {
    match status {
        RuntimeStatus::Ready => CurrentRuntime::Ready,
        RuntimeStatus::Stopped => CurrentRuntime::Stopped,
        RuntimeStatus::Absent => CurrentRuntime::Absent,
        RuntimeStatus::Preparing | RuntimeStatus::Unhealthy => CurrentRuntime::Present,
        _ => CurrentRuntime::Unknown,
    }
}
fn runtime_label(runtime: CurrentRuntime) -> &'static str {
    match runtime {
        CurrentRuntime::Ready => "ready",
        CurrentRuntime::Stopped => "stopped",
        CurrentRuntime::Absent => "absent",
        CurrentRuntime::Present => "present",
        CurrentRuntime::Unknown => "unknown",
    }
}
