#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt};

use composenest_adapters::{
    artifact_store::ArtifactStore,
    create_projection::CreateProjection,
    create_stages::AdapterCreateStages,
    docker_target::{BoundDocker, DockerProbe},
    named_volumes::DockerNamedVolumes,
    sqlite::DatabaseWorker,
    storage::BindStorage,
};
use composenest_application::{
    create_operation::{CreateEffectError, CreateStages},
    create_state::ConfirmedCreate,
    image_resolution::ImageResolution,
    operation_journal::OperationJournal,
    state_store::{
        PortAllocation, RuntimeTarget, StateStore, StorageAllocation, StorageMethod, TemplateFile,
    },
};
use composenest_domain::instance::{Initialization, StorageOwnership, StoragePresence};
use tempfile::TempDir;

const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn stages<'a>(
    database: &'a DatabaseWorker,
    docker: &'a BoundDocker,
    volumes: &'a DockerNamedVolumes,
    binds: &'a BindStorage,
    artifacts: &'a ArtifactStore<'a>,
) -> AdapterCreateStages<'a> {
    AdapterCreateStages::new(database, docker, volumes, binds, artifacts, "operation", ID).unwrap()
}

fn fixture() -> (TempDir, DatabaseWorker, ConfirmedCreate, DockerProbe) {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    for name in ["state", "locks"] {
        let path = root.path().join(name);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let database = DatabaseWorker::start(root.path()).unwrap();
    database.write(|db| {
        db.execute_batch(&format!("INSERT INTO management_scopes (id, owner_id, root_identity) VALUES ('scope', 'owner', 'root');
            INSERT INTO runtime_targets (id, scope_id, endpoint, engine_id, platform) VALUES ('target', 'scope', 'unix:///tmp/composenest-test.sock', 'engine-a', 'linux/amd64');
            INSERT INTO instances (id, scope_id, target_id, display_name, normalized_name, project_name) VALUES ('{ID}', 'scope', 'target', 'Test', 'Test', 'cn-{ID}');
            INSERT INTO instance_specs (instance_id, revision, selected_version, storage_method, inputs_json) VALUES ('{ID}', 1, '1', 'bind', '{{}}');
            INSERT INTO operations (id, instance_id, kind, status, phase, expected_instance_revision, new_spec_revision) VALUES ('operation', '{ID}', 'create', 'Executing', 'storage', 1, 1);
            INSERT INTO request_receipts (scope_id, request_id, confirmed_revision, request_hash, instance_id, operation_id) VALUES ('scope', 'request', 1, 'hash', '{ID}', 'operation');
            INSERT INTO storage_allocations (instance_id, slot, method, resource_identity, ownership_evidence, ownership, presence, initialization)
            VALUES ('{ID}', 'data', 'bind', 'data/{ID}/data', 'pending-operation', 'assigned', 'not_materialized', 'not_attempted');"))?;
        Ok(())
    }).unwrap();
    let target = RuntimeTarget {
        id: "target".into(),
        scope_id: "scope".into(),
        endpoint: "unix:///tmp/composenest-test.sock".into(),
        engine_id: "engine-a".into(),
        platform: "linux/amd64".into(),
    };
    let confirmed = ConfirmedCreate {
        instance_id: ID.into(),
        scope_id: "scope".into(),
        project_name: format!("cn-{ID}"),
        target,
        spec_revision: 1,
        selected_version: "1".into(),
        snapshot_files: vec![],
        inputs_json: "{}".into(),
        ports: vec![],
        storage: vec![composenest_application::state_store::StorageLedgerEntry {
            instance_id: ID.into(),
            scope_id: "scope".into(),
            slot: "data".into(),
            method: StorageMethod::Bind,
            allocation: StorageAllocation {
                slot: "data".into(),
                resource_identity: format!("data/{ID}/data"),
                ownership_evidence: "pending-operation".into(),
            },
            ownership: StorageOwnership::Assigned,
            presence: StoragePresence::NotMaterialized,
            initialization: Initialization::NotAttempted,
        }],
    };
    let probe = DockerProbe {
        executable: "/bin/true".into(),
        directory: root.path().into(),
        config_directory: root.path().into(),
    };
    (root, database, confirmed, probe)
}

#[tokio::test]
async fn bind_creation_records_proof_and_journal_before_mounting() {
    let (root, database, mut confirmed, probe) = fixture();
    database
        .write(|db| {
            db.execute(
                &format!(
                    "INSERT INTO storage_allocations (instance_id, slot, method, resource_identity,
            ownership_evidence, ownership, presence, initialization)
            VALUES ('{ID}', 'logs', 'bind', 'data/{ID}/logs', 'pending-operation',
                'assigned', 'not_materialized', 'not_attempted')"
                ),
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let mut logs = confirmed.storage[0].clone();
    logs.slot = "logs".into();
    logs.allocation.slot = "logs".into();
    logs.allocation.resource_identity = format!("data/{ID}/logs");
    confirmed.storage.push(logs);
    let docker = probe.bind(confirmed.target.clone()).unwrap();
    let volumes = DockerNamedVolumes::new(probe.bind(confirmed.target.clone()).unwrap());
    let binds = BindStorage::new(root.path());
    let artifacts = ArtifactStore::new(root.path(), &database);
    let stages = stages(&database, &docker, &volumes, &binds, &artifacts);
    assert_eq!(
        stages.prepare_storage(&confirmed, "operation", 1).await,
        Ok(2)
    );
    let saved = database.storage_allocation(ID, "data").unwrap().unwrap();
    assert_eq!(saved.presence, StoragePresence::Present);
    assert_eq!(saved.allocation.ownership_evidence.len(), 64);
    assert_eq!(
        database
            .storage_allocation(ID, "logs")
            .unwrap()
            .unwrap()
            .presence,
        StoragePresence::Present
    );
    let steps = database.steps_for_resource("operation", ID).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(
        steps[0].outcome,
        Some(composenest_application::operation_journal::StepOutcome::Succeeded)
    );
}

#[tokio::test]
async fn preexisting_unproven_bind_is_not_adopted() {
    let (root, database, confirmed, probe) = fixture();
    let path = root.path().join(format!("data/{ID}/data"));
    fs::create_dir_all(&path).unwrap();
    let docker = probe.bind(confirmed.target.clone()).unwrap();
    let volumes = DockerNamedVolumes::new(probe.bind(confirmed.target.clone()).unwrap());
    let binds = BindStorage::new(root.path());
    let artifacts = ArtifactStore::new(root.path(), &database);
    let stages = stages(&database, &docker, &volumes, &binds, &artifacts);
    assert_eq!(
        stages.prepare_storage(&confirmed, "operation", 1).await,
        Err(CreateEffectError::OutcomeUnknown)
    );
    assert_eq!(
        database
            .storage_allocation(ID, "data")
            .unwrap()
            .unwrap()
            .presence,
        StoragePresence::NotMaterialized
    );
    assert!(path.exists());
}

#[tokio::test]
async fn compose_projection_uses_verified_storage_and_saved_inputs() {
    let (root, database, mut confirmed, probe) = fixture();
    let docker = probe.bind(confirmed.target.clone()).unwrap();
    let volumes = DockerNamedVolumes::new(probe.bind(confirmed.target.clone()).unwrap());
    let binds = BindStorage::new(root.path());
    let artifacts = ArtifactStore::new(root.path(), &database);
    let stages = stages(&database, &docker, &volumes, &binds, &artifacts);
    stages
        .prepare_storage(&confirmed, "operation", 1)
        .await
        .unwrap();
    confirmed.snapshot_files = [
        (
            "template.yaml",
            include_bytes!("../../../docs/template-examples/redis/template.yaml").as_slice(),
        ),
        (
            "versions/8.2.yaml",
            include_bytes!("../../../docs/template-examples/redis/versions/8.2.yaml").as_slice(),
        ),
    ]
    .into_iter()
    .map(|(path, bytes)| TemplateFile {
        relative_path: path.into(),
        contents: bytes.to_vec(),
    })
    .collect();
    confirmed.selected_version = "8.2".into();
    confirmed.inputs_json = r#"{"password":"saved-secret"}"#.into();
    confirmed.ports.push(PortAllocation {
        slot: "redis".into(),
        host_ip: "127.0.0.1".into(),
        host_port: 16379,
        container_port: 6379,
    });
    let image = ImageResolution {
        instance_id: ID.into(),
        spec_revision: 1,
        requested: "redis:8.2".into(),
        digest: format!("redis@sha256:{}", "a".repeat(64)),
        image_id: format!("sha256:{}", "b".repeat(64)),
        platform: "linux/amd64".into(),
        operation_id: "operation".into(),
    };
    let projection = CreateProjection::new(&database, &docker, &binds, &confirmed.project_name, ID);
    let model = projection.model(&confirmed, &image).unwrap();
    assert_eq!(model.service.mounts[0].1, "/data");
    assert_eq!(model.service.environment["REDISCLI_AUTH"], "saved-secret");
    assert!(
        matches!(&model.service.mounts[0].0, composenest_domain::compose::Storage::Bind(path)
        if path.ends_with(&format!("data/{ID}/data")))
    );
}
