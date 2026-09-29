//! Concrete create stages built from committed records and fixed adapters.

use std::{cell::RefCell, collections::BTreeMap};

use composenest_application::{
    create_operation::{CreateEffectError, CreateOperation, CreateOperationError, CreateStages},
    create_state::{ConfirmedCreate, CreateStateStore},
    image_resolution::ImageResolution,
    named_volumes::NamedVolumePort,
    operation_journal::{
        ExpectedResult, OperationJournal, RequestReceipt, StepCommand, StepIntent, StepOutcome,
    },
    operation_runner::{OperationRunner, ProgressSink},
    state_store::StorageMethod,
    storage::StoragePort,
};
use composenest_domain::{
    compose,
    identity::{InstanceId, SlotId},
    instance::StoragePresence,
};

use crate::{
    artifact_store::{ArtifactInput, ArtifactStore},
    create_projection::{CreateProjection, parse_instance_id, storage_destinations},
    docker_create::{CreateDockerError, DockerCreate},
    docker_observation::ExpectedContainer,
    docker_target::{BoundDocker, DockerProbe},
    image_resolution::{ImageRequest, ensure_image},
    named_volumes::DockerNamedVolumes,
    sqlite::DatabaseWorker,
    storage::BindStorage,
};

/// Executes one accepted create or clone receipt through the fixed local adapters.
pub async fn run_confirmed_create<P: ProgressSink>(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    management_root: &std::path::Path,
    runner: &OperationRunner,
    receipt: &RequestReceipt,
    progress: &P,
) -> Result<String, CreateOperationError> {
    let confirmed = database
        .confirmed_create(receipt)
        .map_err(CreateOperationError::Store)?;
    let docker = probe
        .bind(confirmed.target.clone())
        .map_err(|_| CreateOperationError::OutcomeUnknown)?;
    let volume_docker = probe
        .bind(confirmed.target)
        .map_err(|_| CreateOperationError::OutcomeUnknown)?;
    let volumes = DockerNamedVolumes::new(volume_docker);
    let binds = BindStorage::new(management_root);
    let artifacts = ArtifactStore::new(management_root, database);
    let stages = AdapterCreateStages::new(
        database,
        &docker,
        &volumes,
        &binds,
        &artifacts,
        &receipt.operation_id,
        &receipt.instance_id,
    )
    .map_err(|_| CreateOperationError::Failed)?;
    CreateOperation {
        state: database,
        journal: database,
        runner,
        stages: &stages,
        progress,
    }
    .run(receipt)
    .await
}

struct Prepared {
    confirmed: Option<ConfirmedCreate>,
    image: Option<ImageResolution>,
    model: Option<compose::ComposeModel>,
    expected: Option<ExpectedContainer>,
}

/// Connects a confirmed create or clone operation to the existing adapters.
pub struct AdapterCreateStages<'a> {
    database: &'a DatabaseWorker,
    docker: &'a BoundDocker,
    volumes: &'a DockerNamedVolumes,
    binds: &'a BindStorage,
    artifacts: &'a ArtifactStore<'a>,
    operation_id: String,
    instance_id: String,
    project: String,
    artifact_id: String,
    prepared: RefCell<Prepared>,
}

impl<'a> AdapterCreateStages<'a> {
    /// Binds one accepted operation to its fixed target and management root.
    pub fn new(
        database: &'a DatabaseWorker,
        docker: &'a BoundDocker,
        volumes: &'a DockerNamedVolumes,
        binds: &'a BindStorage,
        artifacts: &'a ArtifactStore<'a>,
        operation_id: &str,
        instance_id: &str,
    ) -> Result<Self, CreateEffectError> {
        let id = parse_instance_id(instance_id)?;
        let project = id.compose_project_name();
        Ok(Self {
            database,
            docker,
            volumes,
            binds,
            artifacts,
            operation_id: operation_id.into(),
            instance_id: instance_id.into(),
            artifact_id: format!("{instance_id}-r1"),
            project,
            prepared: RefCell::new(Prepared {
                confirmed: None,
                image: None,
                model: None,
                expected: None,
            }),
        })
    }

    fn docker_create(&self) -> Result<DockerCreate<'_>, CreateEffectError> {
        DockerCreate::new(
            self.docker,
            self.artifacts,
            &self.artifact_id,
            &self.project,
            &self.instance_id,
        )
        .map_err(map_docker)
    }

    fn check_confirmed(
        &self,
        confirmed: &ConfirmedCreate,
    ) -> Result<InstanceId, CreateEffectError> {
        if confirmed.instance_id != self.instance_id || confirmed.project_name != self.project {
            return Err(CreateEffectError::Failed);
        }
        parse_instance_id(&confirmed.instance_id)
    }

    fn projection(&self) -> CreateProjection<'_> {
        CreateProjection::new(
            self.database,
            self.docker,
            self.binds,
            &self.project,
            &self.instance_id,
        )
    }
}

impl CreateStages for AdapterCreateStages<'_> {
    async fn prepare_storage(
        &self,
        confirmed: &ConfirmedCreate,
        operation_id: &str,
        first_sequence: u64,
    ) -> Result<u64, CreateEffectError> {
        let id = self.check_confirmed(confirmed)?;
        if operation_id != self.operation_id {
            return Err(CreateEffectError::Failed);
        }
        self.prepared.borrow_mut().confirmed = Some(confirmed.clone());
        let mut sequence = first_sequence;
        let bind_entries = confirmed
            .storage
            .iter()
            .filter(|entry| entry.method == StorageMethod::Bind)
            .collect::<Vec<_>>();
        if !bind_entries.is_empty() {
            if bind_entries
                .iter()
                .any(|entry| entry.presence != StoragePresence::NotMaterialized)
            {
                return Err(CreateEffectError::Failed);
            }
            let slots = bind_entries
                .iter()
                .map(|entry| SlotId::parse(&entry.slot).map_err(|_| CreateEffectError::Failed))
                .collect::<Result<Vec<_>, _>>()?;
            self.database
                .record_step(&StepIntent {
                    operation_id: operation_id.into(),
                    sequence,
                    attempt: 1,
                    command_kind: StepCommand::CreateBind,
                    resource_id: confirmed.instance_id.clone(),
                    expected_result: ExpectedResult::BindCreated,
                })
                .map_err(|_| CreateEffectError::OutcomeUnknown)?;
            let created = self
                .binds
                .create_bind_set(id, &slots)
                .map_err(|_| CreateEffectError::OutcomeUnknown)?;
            if created.len() != bind_entries.len()
                || created.iter().any(|allocation| {
                    !bind_entries.iter().any(|entry| {
                        entry.slot == allocation.slot
                            && entry.allocation.resource_identity == allocation.resource_identity
                    })
                })
            {
                return Err(CreateEffectError::OutcomeUnknown);
            }
            for allocation in &created {
                self.database
                    .record_bind_materialization(operation_id, allocation)
                    .map_err(|_| CreateEffectError::OutcomeUnknown)?;
            }
            self.database
                .finish_step(operation_id, sequence, StepOutcome::Succeeded)
                .map_err(|_| CreateEffectError::OutcomeUnknown)?;
            sequence += 1;
        }
        for entry in confirmed
            .storage
            .iter()
            .filter(|entry| entry.method == StorageMethod::Volume)
        {
            let slot = SlotId::parse(&entry.slot).map_err(|_| CreateEffectError::Failed)?;
            self.volumes
                .ensure_named_volume(self.database, self.database, id, &slot, 1, sequence)
                .await
                .map_err(|_| CreateEffectError::OutcomeUnknown)?;
            sequence += 1;
        }
        Ok(sequence)
    }

    async fn resolve_image(&self, confirmed: &ConfirmedCreate) -> Result<(), CreateEffectError> {
        self.check_confirmed(confirmed)?;
        let snapshot = CreateProjection::snapshot(confirmed)?;
        let version = snapshot
            .versions
            .iter()
            .find(|version| version.key == confirmed.selected_version)
            .ok_or(CreateEffectError::Failed)?;
        let destinations = storage_destinations(&snapshot, &confirmed.selected_version)?;
        let resolution = ensure_image(
            self.docker,
            self.database,
            ImageRequest {
                instance_id: &confirmed.instance_id,
                revision: confirmed.spec_revision,
                requested: &version.definition.image,
                platform: &confirmed.target.platform,
                storage_destinations: &destinations,
                operation_id: &self.operation_id,
            },
        )
        .await
        .map_err(|_| CreateEffectError::OutcomeUnknown)?;
        self.prepared.borrow_mut().image = Some(resolution);
        Ok(())
    }

    async fn publish_artifact(&self, confirmed: &ConfirmedCreate) -> Result<(), CreateEffectError> {
        let image = self
            .prepared
            .borrow()
            .image
            .clone()
            .ok_or(CreateEffectError::Failed)?;
        let model = self.projection().model(confirmed, &image)?;
        let yaml = compose::to_yaml(&model).map_err(|_| CreateEffectError::Failed)?;
        self.artifacts
            .publish(
                &self.operation_id,
                ArtifactInput {
                    id: self.artifact_id.clone(),
                    instance_id: self.instance_id.clone(),
                    spec_revision: confirmed.spec_revision,
                    generator_version: "compose-v1".into(),
                    files: BTreeMap::from([("compose.yaml".into(), yaml.into_bytes())]),
                },
            )
            .map_err(|_| CreateEffectError::OutcomeUnknown)?;
        self.prepared.borrow_mut().model = Some(model);
        Ok(())
    }

    async fn validate_config(&self) -> Result<(), CreateEffectError> {
        self.docker_create()?
            .validate_config()
            .await
            .map_err(map_docker)
    }

    async fn create_stopped(&self) -> Result<(), CreateEffectError> {
        self.docker_create()?
            .create_stopped()
            .await
            .map_err(map_docker)
    }

    async fn inspect_created(&self) -> Result<String, CreateEffectError> {
        let docker = self.docker_create()?;
        let container_id = docker.created_container_id().await.map_err(map_docker)?;
        let (model, image) = {
            let prepared = self.prepared.borrow();
            (
                prepared.model.clone().ok_or(CreateEffectError::Failed)?,
                prepared.image.clone().ok_or(CreateEffectError::Failed)?,
            )
        };
        let confirmed = self
            .prepared
            .borrow()
            .confirmed
            .clone()
            .ok_or(CreateEffectError::Failed)?;
        let expected = self
            .projection()
            .expected(&model, &image, &container_id, &confirmed)
            .await?;
        docker.verify_stopped(&expected).await.map_err(map_docker)?;
        self.prepared.borrow_mut().expected = Some(expected);
        Ok(container_id)
    }

    async fn start(&self, container_id: &str) -> Result<(), CreateEffectError> {
        let expected = self
            .prepared
            .borrow()
            .expected
            .clone()
            .ok_or(CreateEffectError::Failed)?;
        if expected.container_id != container_id {
            return Err(CreateEffectError::Failed);
        }
        self.docker_create()?
            .start_verified(&expected)
            .await
            .map_err(map_docker)
    }

    async fn wait_ready(&self, container_id: &str) -> Result<(), CreateEffectError> {
        let expected = self
            .prepared
            .borrow()
            .expected
            .clone()
            .ok_or(CreateEffectError::Failed)?;
        if expected.container_id != container_id {
            return Err(CreateEffectError::Failed);
        }
        self.docker_create()?
            .wait_ready(&expected)
            .await
            .map_err(map_docker)
    }
}

fn map_docker(error: CreateDockerError) -> CreateEffectError {
    match error {
        CreateDockerError::OutcomeUnknown | CreateDockerError::Unavailable => {
            CreateEffectError::OutcomeUnknown
        }
        _ => CreateEffectError::Failed,
    }
}
