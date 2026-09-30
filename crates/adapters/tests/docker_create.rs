#![cfg(unix)]

use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt};

use composenest_adapters::{
    artifact_store::{ArtifactInput, ArtifactStore},
    docker_create::{CreateDockerError, DockerCreate},
    docker_observation::ExpectedContainer,
    docker_target::DockerProbe,
    sqlite::DatabaseWorker,
};
use composenest_application::state_store::RuntimeTarget;
use serde_json::{Value, json};
use tempfile::TempDir;

const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CONTAINER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn fixture() -> (TempDir, DatabaseWorker, DockerProbe) {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    for name in ["state", "locks"] {
        let path = root.path().join(name);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let database = DatabaseWorker::start(root.path()).unwrap();
    database.write(|db| {
        db.execute_batch(&format!("INSERT INTO management_scopes (id, owner_id, root_identity)
                VALUES ('scope', 'owner', 'root');
            INSERT INTO runtime_targets (id, scope_id, endpoint, engine_id, platform)
                VALUES ('target', 'scope', 'unix:///tmp/composenest-test.sock', 'engine-a', 'linux/amd64');
            INSERT INTO instances (id, scope_id, target_id, display_name, normalized_name, project_name)
                VALUES ('{ID}', 'scope', 'target', 'Test', 'Test', 'cn-{ID}');
            INSERT INTO instance_specs (instance_id, revision, selected_version, storage_method, inputs_json)
                VALUES ('{ID}', 1, '1', 'bind', '{{}}');
            INSERT INTO operations (id, instance_id, kind, phase, expected_instance_revision)
                VALUES ('operation', '{ID}', 'create', 'artifact', 1);"))?;
        Ok(())
    }).unwrap();
    let executable = root.path().join("docker");
    fs::write(
        &executable,
        format!(
            r#"#!/bin/sh
if [ "$1" != '--host' ] || [ "$2" != 'unix:///tmp/composenest-test.sock' ]; then exit 1; fi
shift 2
if [ "$1" = info ]; then printf '{{"ID":"engine-a"}}\n'; exit 0; fi
printf '%s\n' "$*" >> calls
if [ "$1" = compose ]; then
  case "$*" in
    *' config --quiet') if [ -f config-fail ]; then exit 1; fi; exit 0 ;;
    *' create --no-build --pull never main') /usr/bin/touch created; exit 0 ;;
    *' create --force-recreate --no-build --pull never main') /usr/bin/touch created; exit 0 ;;
  esac
fi
if [ "$1" = container ] && [ "$2" = ls ]; then
  if [ -f foreign ]; then
    case "$*" in
      *io.composenest.instance*) exit 0 ;;
      *) printf '{CONTAINER}\n'; exit 0 ;;
    esac
  fi
  if [ -f created ]; then printf '{CONTAINER}\n'; fi
  exit 0
fi
if [ "$1" = container ] && [ "$2" = inspect ]; then
  if [ -f started ]; then /bin/cat inspect-ready.json; else /bin/cat inspect-stopped.json; fi
  exit 0
fi
if [ "$1" = container ] && [ "$2" = start ]; then /usr/bin/touch started; exit 0; fi
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
    let expected = expected();
    fs::write(
        root.path().join("inspect-stopped.json"),
        serde_json::to_vec(&inspect(&expected, "created")).unwrap(),
    )
    .unwrap();
    fs::write(
        root.path().join("inspect-ready.json"),
        serde_json::to_vec(&inspect(&expected, "running")).unwrap(),
    )
    .unwrap();
    (root, database, probe)
}

fn expected() -> ExpectedContainer {
    ExpectedContainer {
        container_id: CONTAINER.into(),
        project: format!("cn-{ID}"),
        scope: "scope".into(),
        instance: ID.into(),
        image_id: "sha256:image".into(),
        mounts: vec![],
        ports: BTreeMap::new(),
        networks: vec![],
        command: vec![],
        environment: vec![],
        healthcheck: None,
        spec_revision: 1,
    }
}

fn inspect(expected: &ExpectedContainer, state: &str) -> Value {
    json!({
        "Id": expected.container_id, "Image": expected.image_id,
        "Config": {"Labels": {"com.docker.compose.project": expected.project,
            "com.docker.compose.service":"main", "io.composenest.scope":expected.scope,
            "io.composenest.instance":expected.instance, "io.composenest.spec-revision":"1"},
            "Cmd":null, "Env":null, "Healthcheck":null},
        "HostConfig":{"PortBindings":null}, "Mounts":[], "NetworkSettings":{"Networks":{}},
        "State":{"Status":state,"Health":{"Status":if state == "running" {"healthy"} else {"starting"}}}
    })
}

fn target() -> RuntimeTarget {
    RuntimeTarget {
        id: "target".into(),
        scope_id: "scope".into(),
        endpoint: "unix:///tmp/composenest-test.sock".into(),
        engine_id: "engine-a".into(),
        platform: "linux/amd64".into(),
    }
}

fn publish(root: &TempDir, database: &DatabaseWorker) {
    ArtifactStore::new(root.path(), database)
        .publish(
            "operation",
            ArtifactInput {
                id: "artifact".into(),
                instance_id: ID.into(),
                spec_revision: 1,
                generator_version: "compose-v1".into(),
                files: BTreeMap::from([("compose.yaml".into(), b"services: {}\n".to_vec())]),
            },
        )
        .unwrap();
}

#[tokio::test]
async fn config_failure_never_creates_or_starts() {
    let (root, database, probe) = fixture();
    publish(&root, &database);
    fs::write(root.path().join("config-fail"), "").unwrap();
    let docker = probe.bind(target()).unwrap();
    let artifacts = ArtifactStore::new(root.path(), &database);
    let project = format!("cn-{ID}");
    let create = DockerCreate::new(&docker, &artifacts, "artifact", &project, ID).unwrap();
    assert_eq!(
        create.create_stopped().await,
        Err(CreateDockerError::Unavailable)
    );
    assert!(!root.path().join("created").exists());
    assert!(!root.path().join("started").exists());
}

#[tokio::test]
async fn force_recreate_uses_stopped_compose_create_without_start() {
    let (root, database, probe) = fixture();
    publish(&root, &database);
    fs::write(root.path().join("created"), "").unwrap();
    let docker = probe.bind(target()).unwrap();
    let artifacts = ArtifactStore::new(root.path(), &database);
    let project = format!("cn-{ID}");
    let create = DockerCreate::new(&docker, &artifacts, "artifact", &project, ID).unwrap();
    create.recreate_stopped().await.unwrap();
    create.verify_stopped(&expected()).await.unwrap();
    let calls = fs::read_to_string(root.path().join("calls")).unwrap();
    assert!(calls.contains("create --force-recreate --no-build --pull never main"));
    assert!(!calls.contains("container start"));
}

#[tokio::test]
async fn foreign_project_container_blocks_recreation() {
    let (root, database, probe) = fixture();
    publish(&root, &database);
    fs::write(root.path().join("foreign"), "").unwrap();
    let docker = probe.bind(target()).unwrap();
    let artifacts = ArtifactStore::new(root.path(), &database);
    let project = format!("cn-{ID}");
    let create = DockerCreate::new(&docker, &artifacts, "artifact", &project, ID).unwrap();
    assert_eq!(
        create.create_stopped().await,
        Err(CreateDockerError::ExistingContainer)
    );
    assert!(!root.path().join("created").exists());
}

#[tokio::test]
async fn unknown_mount_or_foreign_owner_blocks_start() {
    for foreign in [false, true] {
        let (root, database, probe) = fixture();
        publish(&root, &database);
        let docker = probe.bind(target()).unwrap();
        let artifacts = ArtifactStore::new(root.path(), &database);
        let project = format!("cn-{ID}");
        let create = DockerCreate::new(&docker, &artifacts, "artifact", &project, ID).unwrap();
        create.create_stopped().await.unwrap();
        assert_eq!(create.created_container_id().await.unwrap(), CONTAINER);
        let mut actual: Value =
            serde_json::from_slice(&fs::read(root.path().join("inspect-stopped.json")).unwrap())
                .unwrap();
        if foreign {
            actual["Config"]["Labels"]["io.composenest.scope"] = json!("foreign");
        } else {
            actual["Mounts"] =
                json!([{"Type":"bind","Source":"/unknown","Destination":"/extra","RW":true}]);
        }
        fs::write(
            root.path().join("inspect-stopped.json"),
            serde_json::to_vec(&actual).unwrap(),
        )
        .unwrap();
        assert_eq!(
            create.start_verified(&expected()).await,
            Err(CreateDockerError::Mismatch)
        );
        assert!(!root.path().join("started").exists());
    }
}

#[tokio::test]
async fn starts_recorded_id_and_requires_healthy_observation() {
    let (root, database, probe) = fixture();
    publish(&root, &database);
    let docker = probe.bind(target()).unwrap();
    let artifacts = ArtifactStore::new(root.path(), &database);
    let project = format!("cn-{ID}");
    let create = DockerCreate::new(&docker, &artifacts, "artifact", &project, ID).unwrap();
    create.create_stopped().await.unwrap();
    create.verify_stopped(&expected()).await.unwrap();
    create.start_verified(&expected()).await.unwrap();
    create.wait_ready(&expected()).await.unwrap();
    let calls = fs::read_to_string(root.path().join("calls")).unwrap();
    assert!(calls.contains(&format!("container start {CONTAINER}")));
    assert!(calls.contains("create --no-build --pull never main"));
}

#[tokio::test]
async fn cli_completion_does_not_prove_container_or_readiness() {
    let (root, database, probe) = fixture();
    publish(&root, &database);
    let docker = probe.bind(target()).unwrap();
    let artifacts = ArtifactStore::new(root.path(), &database);
    let project = format!("cn-{ID}");
    let create = DockerCreate::new(&docker, &artifacts, "artifact", &project, ID).unwrap();
    create.create_stopped().await.unwrap();
    fs::remove_file(root.path().join("created")).unwrap();
    assert_eq!(
        create.created_container_id().await,
        Err(CreateDockerError::OutcomeUnknown)
    );
    fs::write(root.path().join("created"), "").unwrap();
    create.start_verified(&expected()).await.unwrap();
    let mut actual: Value =
        serde_json::from_slice(&fs::read(root.path().join("inspect-ready.json")).unwrap()).unwrap();
    actual["State"]["Health"]["Status"] = json!("unhealthy");
    fs::write(
        root.path().join("inspect-ready.json"),
        serde_json::to_vec(&actual).unwrap(),
    )
    .unwrap();
    assert_eq!(
        create.wait_ready(&expected()).await,
        Err(CreateDockerError::NotReady)
    );
}
