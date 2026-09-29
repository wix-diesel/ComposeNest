//! Docker and storage adapters for an accepted lifecycle operation.

use std::{collections::BTreeMap, ffi::OsString, path::Path, time::Duration};

use composenest_application::{
    create_operation::CreateEffectError,
    create_state::{ConfirmedCreate, CreateStateStore},
    image_resolution::{ImageResolution, ImageResolutionStore},
    lifecycle_operation::{
        LifecycleEffectError, LifecycleObservation, LifecycleOperation, LifecycleOperationError,
        LifecycleStages, LifecycleState,
    },
    named_volumes::NamedVolumePort,
    operation_journal::{OperationKind, RequestReceipt},
    operation_runner::OperationRunner,
    state_store::StorageMethod,
    storage::StoragePort,
};
use composenest_domain::{
    identity::SlotId,
    instance::{ObservationFailure, RuntimeStatus, StoragePresence},
};

use crate::{
    artifact_store::ArtifactStore,
    create_projection::{CreateProjection, parse_instance_id},
    docker_create::{CreateDockerError, DockerCreate},
    docker_observation::{ExpectedContainer, Ownership},
    docker_target::{BoundDocker, DockerProbe},
    named_volumes::DockerNamedVolumes,
    sqlite::DatabaseWorker,
    storage::BindStorage,
};

const CHANGE_DEADLINE: Duration = Duration::from_secs(30);

/// Runs an accepted lifecycle request against its saved Docker Engine.
pub async fn run_confirmed_lifecycle(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    management_root: &Path,
    runner: &OperationRunner,
    receipt: &RequestReceipt,
    kind: OperationKind,
) -> Result<String, LifecycleOperationError> {
    let snapshot = database
        .snapshot(receipt, kind)
        .map_err(LifecycleOperationError::Store)?;
    let confirmed = database
        .confirmed_create(receipt)
        .map_err(LifecycleOperationError::Store)?;
    let docker = probe
        .bind(confirmed.target.clone())
        .map_err(|_| LifecycleOperationError::OutcomeUnknown)?;
    let volume_docker = probe
        .bind(confirmed.target.clone())
        .map_err(|_| LifecycleOperationError::OutcomeUnknown)?;
    let volumes = DockerNamedVolumes::new(volume_docker);
    let binds = BindStorage::new(management_root);
    let artifacts = ArtifactStore::new(management_root, database);
    let image = if kind == OperationKind::Stop {
        None
    } else {
        Some(
            database
                .image_resolution(&confirmed.instance_id, confirmed.spec_revision)
                .map_err(LifecycleOperationError::Store)?
                .ok_or(LifecycleOperationError::Rejected)?,
        )
    };
    let stages = AdapterLifecycleStages {
        database,
        docker: &docker,
        volumes: &volumes,
        binds: &binds,
        artifacts: &artifacts,
        artifact_id: format!("{}-r{}", confirmed.instance_id, confirmed.spec_revision),
        confirmed,
        image,
        kind,
    };
    if snapshot.spec_revision != stages.confirmed.spec_revision {
        return Err(LifecycleOperationError::Store(
            composenest_application::state_store::StoreConflict::InvalidInput,
        ));
    }
    LifecycleOperation {
        state: database,
        journal: database,
        runner,
        stages: &stages,
    }
    .run(receipt, kind)
    .await
}

struct AdapterLifecycleStages<'a> {
    database: &'a DatabaseWorker,
    docker: &'a BoundDocker,
    volumes: &'a DockerNamedVolumes,
    binds: &'a BindStorage,
    artifacts: &'a ArtifactStore<'a>,
    confirmed: ConfirmedCreate,
    artifact_id: String,
    image: Option<ImageResolution>,
    kind: OperationKind,
}

impl AdapterLifecycleStages<'_> {
    fn docker_create(&self) -> Result<DockerCreate<'_>, LifecycleEffectError> {
        DockerCreate::new(
            self.docker,
            self.artifacts,
            &self.artifact_id,
            &self.confirmed.project_name,
            &self.confirmed.instance_id,
        )
        .map_err(map_create)
    }

    async fn expected(
        &self,
        container_id: &str,
    ) -> Result<ExpectedContainer, LifecycleEffectError> {
        let image = self.image.as_ref().ok_or(LifecycleEffectError::Rejected)?;
        let projection = CreateProjection::new(
            self.database,
            self.docker,
            self.binds,
            &self.confirmed.project_name,
            &self.confirmed.instance_id,
        );
        let model = projection
            .model(&self.confirmed, image)
            .map_err(map_effect)?;
        projection
            .expected(&model, image, container_id, &self.confirmed)
            .await
            .map_err(map_effect)
    }

    fn ownership_only(&self, container_id: &str) -> ExpectedContainer {
        ExpectedContainer {
            container_id: container_id.into(),
            project: self.confirmed.project_name.clone(),
            scope: self.confirmed.scope_id.clone(),
            instance: self.confirmed.instance_id.clone(),
            image_id: String::new(),
            mounts: vec![],
            ports: BTreeMap::new(),
            networks: vec![],
            command: vec![],
            environment: vec![],
            healthcheck: None,
            spec_revision: self.confirmed.spec_revision,
        }
    }

    async fn change(&self, action: &str, container_id: &str) -> Result<(), LifecycleEffectError> {
        let args: [OsString; 3] = ["container".into(), action.into(), container_id.into()];
        let outcome = self
            .docker
            .change(&args, CHANGE_DEADLINE)
            .await
            .map_err(|_| LifecycleEffectError::OutcomeUnknown)?;
        if outcome.outcome_unknown || !outcome.status.is_some_and(|status| status.success()) {
            return Err(LifecycleEffectError::OutcomeUnknown);
        }
        Ok(())
    }
}

impl LifecycleStages for AdapterLifecycleStages<'_> {
    async fn observe(&self, container_id: &str) -> LifecycleObservation {
        let expected = if self.kind == OperationKind::Stop {
            Ok(self.ownership_only(container_id))
        } else {
            self.expected(container_id).await
        };
        let Ok(expected) = expected else {
            return LifecycleObservation {
                status: RuntimeStatus::Unknown(ObservationFailure::Inconclusive),
                owned: false,
                configuration_matches: false,
            };
        };
        let actual = self.docker.observe(&expected).await;
        LifecycleObservation {
            status: actual.status,
            owned: actual.ownership == Ownership::Verified,
            configuration_matches: actual.configuration_matches == Some(true),
        }
    }

    async fn verify_storage(&self) -> Result<(), LifecycleEffectError> {
        let id = parse_instance_id(&self.confirmed.instance_id).map_err(map_effect)?;
        for saved in &self.confirmed.storage {
            if saved.presence != StoragePresence::Present {
                return Err(LifecycleEffectError::Rejected);
            }
            let presence = match saved.method {
                StorageMethod::Bind => self.binds.inspect_bind(id, &saved.allocation),
                StorageMethod::Volume => {
                    let slot =
                        SlotId::parse(&saved.slot).map_err(|_| LifecycleEffectError::Rejected)?;
                    self.volumes
                        .inspect_named_volume(self.database, self.database, id, &slot)
                        .await
                        .map_err(|_| LifecycleEffectError::OutcomeUnknown)?
                        .presence
                }
            };
            if presence != StoragePresence::Present {
                return Err(LifecycleEffectError::Rejected);
            }
        }
        Ok(())
    }

    fn verify_artifact(&self) -> Result<(), LifecycleEffectError> {
        self.artifacts
            .verified_compose_path(&self.artifact_id)
            .map(|_| ())
            .map_err(|_| LifecycleEffectError::Rejected)
    }

    async fn recreate(&self) -> Result<String, LifecycleEffectError> {
        let docker = self.docker_create()?;
        docker.create_stopped().await.map_err(map_create)?;
        let container_id = docker.created_container_id().await.map_err(map_create)?;
        let expected = self.expected(&container_id).await?;
        docker.verify_stopped(&expected).await.map_err(map_create)?;
        Ok(container_id)
    }

    async fn start(&self, container_id: &str) -> Result<(), LifecycleEffectError> {
        let expected = self.expected(container_id).await?;
        self.docker_create()?
            .start_verified(&expected)
            .await
            .map_err(map_create)
    }

    async fn stop(&self, container_id: &str) -> Result<(), LifecycleEffectError> {
        self.change("stop", container_id).await
    }

    async fn restart(&self, container_id: &str) -> Result<(), LifecycleEffectError> {
        self.change("restart", container_id).await
    }

    async fn wait_ready(&self, container_id: &str) -> Result<(), LifecycleEffectError> {
        let expected = self.expected(container_id).await?;
        self.docker_create()?
            .wait_ready(&expected)
            .await
            .map_err(map_create)
    }
}

fn map_effect(error: CreateEffectError) -> LifecycleEffectError {
    match error {
        CreateEffectError::Failed => LifecycleEffectError::Rejected,
        CreateEffectError::OutcomeUnknown => LifecycleEffectError::OutcomeUnknown,
    }
}

fn map_create(error: CreateDockerError) -> LifecycleEffectError {
    match error {
        CreateDockerError::OutcomeUnknown | CreateDockerError::Unavailable => {
            LifecycleEffectError::OutcomeUnknown
        }
        _ => LifecycleEffectError::Rejected,
    }
}
