//! Stopped-only Docker application of a durably accepted port edit.

use std::{collections::BTreeMap, ffi::OsString, path::Path};

use composenest_application::{
    create_state::CreateStateStore,
    host_ports::{PortInspector, PortReason},
    image_resolution::ImageResolutionStore,
    lifecycle_operation::{LifecycleEffectError, LifecycleStages},
    operation_journal::{ExpectedResult, OperationJournal, StepCommand, StepIntent, StepOutcome},
    operation_runner::{OperationRunner, RunnerError},
    port_edit::{PortEditRequest, PortEditStore},
    state_store::{RuntimeTarget, StoreConflict},
};
use composenest_domain::instance::{OperationStatus, RuntimeStatus};
use rusqlite::{OptionalExtension, params};

use crate::{
    artifact_store::{ArtifactInput, ArtifactStore},
    create_projection::CreateProjection,
    docker_cli::DockerCli,
    docker_create::{CreateDockerError, DockerCreate},
    docker_observation::Ownership,
    docker_target::{BoundDocker, DockerProbe},
    host_ports::PortSnapshot,
    lifecycle_stages::AdapterLifecycleStages,
    named_volumes::DockerNamedVolumes,
    sqlite::{DatabaseError, DatabaseWorker},
    storage::BindStorage,
};

/// Failure to accept, apply, or verify a stopped port edit.
#[derive(Debug, PartialEq, Eq)]
pub enum PortEditError {
    /// The operation gate rejected concurrent work.
    Runner(RunnerError),
    /// The current spec or request could not be persisted.
    Store(StoreConflict),
    /// The selected port is already used or cannot be checked.
    Port(PortReason),
    /// A prerequisite or verified postcondition failed.
    Rejected,
    /// Docker may have changed state, requiring reconciliation.
    OutcomeUnknown,
}

struct EditContext {
    target: RuntimeTarget,
    project: String,
    recorded_container: Option<String>,
}

fn context(
    database: &DatabaseWorker,
    request: &PortEditRequest,
) -> Result<EditContext, PortEditError> {
    database.read(|db| {
        db.query_row(
            "SELECT t.id, t.scope_id, t.endpoint, t.engine_id, t.platform, i.project_name, r.container_id FROM instances i JOIN runtime_targets t ON t.id = i.target_id AND t.scope_id = i.scope_id LEFT JOIN runtime_observations r ON r.instance_id = i.id WHERE i.id = ?1 AND i.scope_id = ?2",
            params![request.receipt.instance_id, request.receipt.scope_id],
            |row| Ok(EditContext {
                target: RuntimeTarget { id: row.get(0)?, scope_id: row.get(1)?, endpoint: row.get(2)?, engine_id: row.get(3)?, platform: row.get(4)? },
                project: row.get(5)?, recorded_container: row.get(6)?,
            }),
        ).optional()?.ok_or(DatabaseError::Missing)
    }).map_err(|_| PortEditError::Store(StoreConflict::Missing))
}

async fn stopped_before_edit(
    docker: &BoundDocker,
    create: &DockerCreate<'_>,
    context: &EditContext,
    request: &PortEditRequest,
) -> Result<(), PortEditError> {
    let actual = create.project_container_id().await.map_err(map_docker)?;
    match actual {
        None => Ok(()),
        Some(id) if context.recorded_container.as_deref() == Some(&id) => {
            let expected = crate::docker_observation::ExpectedContainer {
                container_id: id,
                project: context.project.clone(),
                scope: request.receipt.scope_id.clone(),
                instance: request.receipt.instance_id.clone(),
                image_id: String::new(),
                mounts: vec![],
                ports: BTreeMap::new(),
                networks: vec![],
                command: vec![],
                environment: vec![],
                healthcheck: None,
                spec_revision: request.old_spec_revision,
            };
            let observed = docker.observe(&expected).await;
            if observed.ownership == Ownership::Verified
                && observed.status == RuntimeStatus::Stopped
            {
                Ok(())
            } else {
                Err(PortEditError::Rejected)
            }
        }
        Some(_) => Err(PortEditError::Rejected),
    }
}

pub(crate) fn map_docker(error: CreateDockerError) -> PortEditError {
    match error {
        CreateDockerError::Unavailable | CreateDockerError::OutcomeUnknown => {
            PortEditError::OutcomeUnknown
        }
        _ => PortEditError::Rejected,
    }
}

pub(crate) fn map_effect(error: LifecycleEffectError) -> PortEditError {
    match error {
        LifecycleEffectError::Rejected => PortEditError::Rejected,
        LifecycleEffectError::OutcomeUnknown => PortEditError::OutcomeUnknown,
    }
}

pub(crate) fn fail(
    database: &DatabaseWorker,
    operation_id: &str,
    phase: &str,
    error: PortEditError,
) -> PortEditError {
    let status = if error == PortEditError::OutcomeUnknown {
        OperationStatus::OutcomeUnknown
    } else if matches!(error, PortEditError::Port(_)) {
        OperationStatus::AwaitingDecision
    } else {
        OperationStatus::Failed
    };
    database
        .set_status(operation_id, status, phase)
        .map_or_else(PortEditError::Store, |_| error)
}

fn record(
    database: &DatabaseWorker,
    operation_id: &str,
    sequence: u64,
    command: StepCommand,
    result: ExpectedResult,
    resource: &str,
) -> Result<(), PortEditError> {
    database
        .record_step(&StepIntent {
            operation_id: operation_id.into(),
            sequence,
            attempt: 1,
            command_kind: command,
            resource_id: resource.into(),
            expected_result: result,
        })
        .map_err(PortEditError::Store)
}

/// Applies a complete candidate port set and leaves the verified container stopped.
/// The request must contain the current revision and bindings; the database rechecks both.
pub async fn run_port_edit(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    management_root: &Path,
    runner: &OperationRunner,
    request: &PortEditRequest,
) -> Result<String, PortEditError> {
    runner
        .run_exclusive(&request.receipt.instance_id, || async {
            run_locked(database, probe, management_root, request).await
        })
        .await
        .map_err(PortEditError::Runner)?
}

async fn run_locked(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    management_root: &Path,
    request: &PortEditRequest,
) -> Result<String, PortEditError> {
    let context = context(database, request)?;
    let docker = probe
        .bind(context.target.clone())
        .map_err(|_| PortEditError::OutcomeUnknown)?;
    let artifacts = ArtifactStore::new(management_root, database);
    let candidate_revision = request
        .old_spec_revision
        .checked_add(1)
        .ok_or(PortEditError::Store(StoreConflict::InvalidInput))?;
    let artifact_id = format!("{}-r{candidate_revision}", request.receipt.instance_id);
    let create = DockerCreate::new(
        &docker,
        &artifacts,
        &artifact_id,
        &context.project,
        &request.receipt.instance_id,
    )
    .map_err(map_docker)?;
    stopped_before_edit(&docker, &create, &context, request).await?;

    let cli = DockerCli::new(
        probe.executable.clone(),
        probe.directory.clone(),
        probe.config_directory.clone(),
        OsString::from(&context.target.endpoint),
    )
    .map_err(|_| PortEditError::OutcomeUnknown)?;
    let snapshot = PortSnapshot::observe(database, &cli, &request.receipt.scope_id)
        .await
        .map_err(PortEditError::Port)?;
    let old = database.read(|db| {
        let mut statement = db.prepare("SELECT slot, host_port FROM port_bindings WHERE instance_id = ?1 AND spec_revision = ?2")?;
        let rows = statement.query_map(params![request.receipt.instance_id, request.old_spec_revision], |row| Ok((row.get::<_, String>(0)?, row.get::<_, u16>(1)?)))?;
        rows.collect::<Result<BTreeMap<_, _>, _>>().map_err(Into::into)
    }).map_err(|_| PortEditError::Store(StoreConflict::Backend))?;
    for port in &request.ports {
        if old.get(&port.slot) != Some(&port.host_port) {
            snapshot
                .inspect(port.host_port)
                .result
                .map_err(PortEditError::Port)?;
        }
    }
    let receipt = database
        .begin_port_edit(request)
        .map_err(PortEditError::Store)?;
    let confirmed = database
        .confirmed_create(&receipt)
        .map_err(PortEditError::Store)?;
    let expected_ports: BTreeMap<_, _> = request
        .ports
        .iter()
        .map(|port| {
            (
                &port.slot,
                (&port.host_ip, port.host_port, port.container_port),
            )
        })
        .collect();
    let saved_ports: BTreeMap<_, _> = confirmed
        .ports
        .iter()
        .map(|port| {
            (
                &port.slot,
                (&port.host_ip, port.host_port, port.container_port),
            )
        })
        .collect();
    if expected_ports != saved_ports {
        return Err(fail(
            database,
            &receipt.operation_id,
            "candidate",
            PortEditError::Rejected,
        ));
    }
    let image = database
        .image_resolution(&confirmed.instance_id, confirmed.spec_revision)
        .map_err(PortEditError::Store)?
        .ok_or_else(|| {
            fail(
                database,
                &receipt.operation_id,
                "image",
                PortEditError::Rejected,
            )
        })?;
    let volume_docker = probe.bind(context.target.clone()).map_err(|_| {
        fail(
            database,
            &receipt.operation_id,
            "target",
            PortEditError::OutcomeUnknown,
        )
    })?;
    let volumes = DockerNamedVolumes::new(volume_docker);
    let binds = BindStorage::new(management_root);
    let stages = AdapterLifecycleStages {
        database,
        docker: &docker,
        volumes: &volumes,
        binds: &binds,
        artifacts: &artifacts,
        confirmed,
        artifact_id: artifact_id.clone(),
        image: Some(image.clone()),
        kind: composenest_application::operation_journal::OperationKind::EditPort,
    };
    let operation_id = &receipt.operation_id;
    database
        .set_status(operation_id, OperationStatus::Executing, "prepare")
        .map_err(PortEditError::Store)?;
    stages
        .verify_storage()
        .await
        .map_err(|error| fail(database, operation_id, "storage", map_effect(error)))?;
    let projection = CreateProjection::new(
        database,
        &docker,
        &binds,
        &stages.confirmed.project_name,
        &stages.confirmed.instance_id,
    );
    let model = projection
        .model(&stages.confirmed, &image)
        .map_err(|_| fail(database, operation_id, "compose", PortEditError::Rejected))?;
    let yaml = composenest_domain::compose::to_yaml(&model)
        .map_err(|_| fail(database, operation_id, "compose", PortEditError::Rejected))?;
    record(
        database,
        operation_id,
        1,
        StepCommand::GenerateArtifact,
        ExpectedResult::ArtifactReady,
        &stages.artifact_id,
    )?;
    let published = artifacts.publish(
        operation_id,
        ArtifactInput {
            id: stages.artifact_id.clone(),
            instance_id: stages.confirmed.instance_id.clone(),
            spec_revision: stages.confirmed.spec_revision,
            generator_version: "compose-v1".into(),
            files: BTreeMap::from([("compose.yaml".into(), yaml.into_bytes())]),
        },
    );
    database
        .finish_step(
            operation_id,
            1,
            if published.is_ok() {
                StepOutcome::Succeeded
            } else {
                StepOutcome::Unknown
            },
        )
        .map_err(PortEditError::Store)?;
    published.map_err(|_| {
        fail(
            database,
            operation_id,
            "artifact",
            PortEditError::OutcomeUnknown,
        )
    })?;
    create
        .validate_config()
        .await
        .map_err(|error| fail(database, operation_id, "config", map_docker(error)))?;
    stopped_before_edit(&docker, &create, &context, request)
        .await
        .map_err(|error| fail(database, operation_id, "inspect", error))?;
    record(
        database,
        operation_id,
        2,
        StepCommand::ComposeCreate,
        ExpectedResult::ContainerCreated,
        &receipt.instance_id,
    )?;
    let created = create.recreate_stopped().await;
    database
        .finish_step(
            operation_id,
            2,
            if created.is_ok() {
                StepOutcome::Succeeded
            } else {
                StepOutcome::Unknown
            },
        )
        .map_err(PortEditError::Store)?;
    created.map_err(|error| fail(database, operation_id, "recreate", map_docker(error)))?;
    let id = create
        .created_container_id()
        .await
        .map_err(|error| fail(database, operation_id, "observe", map_docker(error)))?;
    let expected = stages
        .expected(&id)
        .await
        .map_err(|error| fail(database, operation_id, "expected", map_effect(error)))?;
    create
        .verify_stopped(&expected)
        .await
        .map_err(|error| fail(database, operation_id, "observe", map_docker(error)))?;
    record(
        database,
        operation_id,
        3,
        StepCommand::Observe,
        ExpectedResult::ContainerStopped,
        &id,
    )?;
    database
        .finish_step(operation_id, 3, StepOutcome::Succeeded)
        .map_err(PortEditError::Store)?;
    database
        .complete_port_edit(operation_id, &id)
        .map_err(PortEditError::Store)?;
    Ok(id)
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use composenest_application::{operation_journal::RequestReceipt, state_store::PortAllocation};

    use super::*;

    const INSTANCE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const CONTAINER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[tokio::test]
    async fn accepts_only_fresh_stopped_or_absent_observations() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        for name in ["state", "locks"] {
            fs::create_dir(root.path().join(name)).unwrap();
            fs::set_permissions(root.path().join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let executable = root.path().join("docker");
        fs::write(
            &executable,
            format!(
                r#"#!/bin/sh
if [ "$1" != '--host' ]; then exit 1; fi
shift 2
if [ "$1" = info ]; then printf '{{"ID":"engine"}}\n'; exit 0; fi
printf '%s\n' "$*" >> calls
if [ "$1" = container ] && [ "$2" = ls ]; then
  if [ ! -f absent ]; then printf '{CONTAINER}\n'; fi
  exit 0
fi
if [ "$1" = container ] && [ "$2" = inspect ]; then /bin/cat inspect.json; exit 0; fi
exit 1
"#
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let probe = DockerProbe {
            executable,
            directory: root.path().into(),
            config_directory: root.path().into(),
        };
        let target = RuntimeTarget {
            id: "target".into(),
            scope_id: "scope".into(),
            endpoint: "unix:///tmp/composenest-test.sock".into(),
            engine_id: "engine".into(),
            platform: "linux/amd64".into(),
        };
        let context = EditContext {
            target: target.clone(),
            project: format!("cn-{INSTANCE}"),
            recorded_container: Some(CONTAINER.into()),
        };
        let request = PortEditRequest {
            receipt: RequestReceipt {
                scope_id: "scope".into(),
                request_id: "edit".into(),
                plan_id: None,
                confirmed_revision: 1,
                request_hash: "a".repeat(64),
                instance_id: INSTANCE.into(),
                operation_id: "operation".into(),
            },
            expected_instance_revision: 1,
            old_spec_revision: 1,
            ports: vec![PortAllocation {
                slot: "db".into(),
                host_ip: "127.0.0.1".into(),
                host_port: 15433,
                container_port: 5432,
            }],
        };
        let database = DatabaseWorker::start(root.path()).unwrap();
        let docker = probe.bind(target).unwrap();
        let artifacts = ArtifactStore::new(root.path(), &database);
        let create =
            DockerCreate::new(&docker, &artifacts, "candidate", &context.project, INSTANCE)
                .unwrap();
        let inspect = |status: &str, scope: &str| {
            format!(
                r#"{{"Id":"{CONTAINER}","Config":{{"Labels":{{"com.docker.compose.project":"cn-{INSTANCE}","com.docker.compose.service":"main","io.composenest.scope":"{scope}","io.composenest.instance":"{INSTANCE}"}}}},"State":{{"Status":"{status}","Health":{{"Status":null}}}}}}"#
            )
        };
        fs::write(
            root.path().join("inspect.json"),
            inspect("running", "scope"),
        )
        .unwrap();
        assert_eq!(
            stopped_before_edit(&docker, &create, &context, &request).await,
            Err(PortEditError::Rejected)
        );
        fs::write(root.path().join("inspect.json"), "invalid").unwrap();
        assert_eq!(
            stopped_before_edit(&docker, &create, &context, &request).await,
            Err(PortEditError::Rejected)
        );
        fs::write(
            root.path().join("inspect.json"),
            inspect("exited", "foreign"),
        )
        .unwrap();
        assert_eq!(
            stopped_before_edit(&docker, &create, &context, &request).await,
            Err(PortEditError::Rejected)
        );
        fs::write(root.path().join("inspect.json"), inspect("exited", "scope")).unwrap();
        assert_eq!(
            stopped_before_edit(&docker, &create, &context, &request).await,
            Ok(())
        );
        fs::write(root.path().join("absent"), "").unwrap();
        assert_eq!(
            stopped_before_edit(&docker, &create, &context, &request).await,
            Ok(())
        );
        let calls = fs::read_to_string(root.path().join("calls")).unwrap();
        assert!(!calls.contains(" create "));
        assert!(!calls.contains(" start "));
    }
}
