use super::*;
use composenest_domain::compose::{ComposeService, valid_container_name};

fn model() -> ComposeModel {
    ComposeModel {
        name: format!("cn-{}", "a".repeat(32)),
        container_name: Some("my-database_1.test".into()),
        volumes: vec![],
        service: ComposeService {
            image: "alpine:latest".into(),
            platform: "linux/amd64".into(),
            environment: BTreeMap::from([(
                "PASSWORD".into(),
                "dollar$ quote\" 日本語\nsecond line".into(),
            )]),
            command: Some(vec!["sleep".into(), "300".into()]),
            healthcheck: vec!["true".into()],
            health_timing: [1, 1, 0, 3],
            ports: vec![],
            mounts: vec![],
            restart: "no".into(),
            mem_limit: None,
            labels: BTreeMap::from([
                ("io.composenest.scope".into(), "scope".into()),
                ("io.composenest.instance".into(), "a".repeat(32)),
                ("io.composenest.spec-revision".into(), "1".into()),
            ]),
        },
    }
}

#[test]
fn standalone_request_preserves_actual_values_and_omits_compose_group_labels() {
    let model = model();
    let files = artifact_files(
        &model,
        composenest_domain::compose::to_yaml(&model).unwrap(),
    );
    let request: Value = serde_json::from_slice(&files["container-create.json"]).unwrap();
    assert_eq!(request["name"], "my-database_1.test");
    assert_eq!(
        request["config"]["Env"][0],
        "PASSWORD=dollar$ quote\" 日本語\nsecond line"
    );
    assert_eq!(request["config"]["Cmd"], json!(["sleep", "300"]));
    assert!(
        request["config"]["Labels"]
            .as_object()
            .unwrap()
            .keys()
            .all(|key| !key.starts_with("com.docker.compose."))
    );
    assert!(crate::docker_observation::owns_project(
        &request["config"]["Labels"],
        &model.name,
        "service"
    ));
    let mut legacy = model.clone();
    legacy.container_name = None;
    assert!(!artifact_files(&legacy, "legacy".into()).contains_key("container-create.json"));
}

#[test]
fn docker_names_are_bounded_and_cannot_inject_paths_flags_or_shell_syntax() {
    for name in ["db", "9", "my-database_1.test"] {
        assert!(valid_container_name(name));
    }
    for name in [
        "",
        "--name",
        "with space",
        "日本語",
        "a/b",
        "a\\b",
        "a;rm",
        "a\n",
    ] {
        assert!(!valid_container_name(name), "{name}");
    }
    assert!(valid_container_name(&"a".repeat(128)));
    assert!(!valid_container_name(&"a".repeat(129)));
}

#[test]
fn standalone_ports_stay_on_localhost_and_join_only_the_dedicated_network() {
    let mut model = model();
    model.service.ports = vec![(15432, 5432), (18080, 8080)];
    let config = configuration(&model);
    assert_eq!(
        config["HostConfig"]["PortBindings"],
        json!({
            "5432/tcp": [{"HostIp": "127.0.0.1", "HostPort": "15432"}],
            "8080/tcp": [{"HostIp": "127.0.0.1", "HostPort": "18080"}]
        })
    );
    assert_eq!(
        config["ExposedPorts"],
        json!({"5432/tcp": {}, "8080/tcp": {}})
    );
    let network = format!("{}_default", model.name);
    assert_eq!(config["HostConfig"]["NetworkMode"], network);
    assert_eq!(
        config["NetworkingConfig"]["EndpointsConfig"],
        json!({network: {}})
    );
}

#[test]
fn standalone_mounts_preserve_bind_paths_and_existing_volume_names() {
    use composenest_domain::instance::StoragePresence;

    for path in [r"C:\Users\test\data 日本語", "/home/test/data 日本語"] {
        let mut model = model();
        model.service.mounts = vec![
            (Storage::Bind(path.into()), "/bind-data".into()),
            (
                Storage::Volume {
                    name: "existing-db-data".into(),
                    presence: StoragePresence::Present,
                },
                "/var/lib/database".into(),
            ),
        ];
        let config = configuration(&model);
        assert_eq!(
            config["HostConfig"]["Mounts"],
            json!([
                {"Type": "bind", "Source": path, "Target": "/bind-data", "ReadOnly": false},
                {"Type": "volume", "Source": "existing-db-data", "Target": "/var/lib/database", "ReadOnly": false}
            ])
        );
    }
}

#[test]
fn omitted_command_preserves_image_defaults_while_runtime_limits_are_applied() {
    let mut model = model();
    model.service.command = None;
    model.service.healthcheck = vec!["check".into(), "value with spaces".into()];
    model.service.health_timing = [30, 2, 10, 4];
    model.service.restart = "unless-stopped".into();
    model.service.mem_limit = Some(536_870_912);
    let config = configuration(&model);
    assert!(config.get("Cmd").is_none());
    assert_eq!(config["HostConfig"]["Memory"], 536_870_912);
    assert_eq!(
        config["HostConfig"]["RestartPolicy"]["Name"],
        "unless-stopped"
    );
    assert_eq!(
        config["Healthcheck"],
        json!({
            "Test": ["CMD", "check", "value with spaces"],
            "Interval": 30_000_000_000_i64, "Timeout": 2_000_000_000_i64,
            "StartPeriod": 10_000_000_000_i64, "StartInterval": 5_000_000_000_i64,
            "Retries": 4
        })
    );
}

#[tokio::test]
#[ignore = "requires Docker Desktop or a local Linux Engine and cached alpine:latest"]
async fn real_standalone_create_has_requested_name_no_group_and_can_be_recreated_and_deleted() {
    use crate::{
        artifact_store::{ArtifactInput, ArtifactStore},
        docker_create::DockerCreate,
        docker_target::DockerProbe,
    };
    use composenest_application::state_store::StateStore;
    use std::{env, path::PathBuf, process::Command};
    let (root, db, template) = crate::test_support::store();
    let home =
        PathBuf::from(env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap());
    let executable = crate::docker_executable::discover_checked(&home)
        .unwrap()
        .unwrap();
    let probe = DockerProbe {
        executable: executable.clone(),
        directory: root.path().into(),
        config_directory: home.join(".docker"),
    };
    let report = probe.diagnose(None).await;
    let target = report.target("target".into(), "scope".into()).unwrap();
    db.create_target(
        "target",
        "scope",
        &target.endpoint,
        &target.engine_id,
        &target.platform,
    )
    .unwrap();
    let mut record = crate::test_support::source_instance(&template);
    let mut random = [0; 16];
    getrandom::fill(&mut random).unwrap();
    record.id = format!("{:032x}", u128::from_be_bytes(random));
    record.project_name = format!("cn-{}", record.id);
    record.name = record.id.clone();
    record.container_name = Some(format!("cn-test-{}", record.id));
    db.commit_instance(&record).unwrap();
    let operation_instance = record.id.clone();
    db.write(move |connection| { connection.execute("INSERT INTO operations (id,instance_id,kind,phase,expected_instance_revision) VALUES ('standalone-test',?1,'create','artifact',1)", [&operation_instance])?; Ok(()) }).unwrap();
    let mut model = model();
    model.name = record.project_name.clone();
    model.container_name = record.container_name.clone();
    model.service.platform = target.platform.clone();
    let bind = root.path().join("bind data 日本語");
    std::fs::create_dir(&bind).unwrap();
    let volume = format!("cn-test-data-{}", record.id);
    model.service.mounts = vec![
        (
            Storage::Bind(bind.to_string_lossy().into_owned()),
            "/bind-data".into(),
        ),
        (
            Storage::Volume {
                name: volume.clone(),
                presence: composenest_domain::instance::StoragePresence::Present,
            },
            "/volume-data".into(),
        ),
    ];
    model.volumes = vec![volume.clone()];
    model
        .service
        .labels
        .insert("io.composenest.instance".into(), record.id.clone());
    let artifacts = ArtifactStore::new(root.path(), &db);
    artifacts
        .publish(
            "standalone-test",
            ArtifactInput {
                id: "standalone-test".into(),
                instance_id: record.id.clone(),
                spec_revision: 1,
                generator_version: "standalone-v1".into(),
                files: artifact_files(
                    &model,
                    composenest_domain::compose::to_yaml(&model).unwrap(),
                ),
            },
        )
        .unwrap();
    let docker = probe.bind(target.clone()).unwrap();
    let create = DockerCreate::new(
        &docker,
        &artifacts,
        "standalone-test",
        &record.project_name,
        &record.id,
    )
    .unwrap();
    struct Cleanup {
        executable: PathBuf,
        endpoint: String,
        name: String,
        network: String,
        volume: String,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = Command::new(&self.executable)
                .args([
                    "--host",
                    &self.endpoint,
                    "container",
                    "rm",
                    "--force",
                    &self.name,
                ])
                .output();
            let _ = Command::new(&self.executable)
                .args(["--host", &self.endpoint, "network", "rm", &self.network])
                .output();
            let _ = Command::new(&self.executable)
                .args(["--host", &self.endpoint, "volume", "rm", &self.volume])
                .output();
        }
    }
    let _cleanup = Cleanup {
        executable: executable.clone(),
        endpoint: target.endpoint.clone(),
        name: record.container_name.clone().unwrap(),
        network: format!("{}_default", record.project_name),
        volume: volume.clone(),
    };
    let created_volume = docker
        .change(
            &["volume".into(), "create".into(), volume.clone().into()],
            std::time::Duration::from_secs(30),
        )
        .await
        .unwrap();
    assert!(created_volume.status.unwrap().success());
    assert!(
        docker
            .container_name_available(record.container_name.as_ref().unwrap())
            .await
            .unwrap()
    );
    create.create_stopped().await.unwrap();
    let id = create.created_container_id().await.unwrap();
    assert!(
        !docker
            .container_name_available(record.container_name.as_ref().unwrap())
            .await
            .unwrap()
    );
    let inspected = Command::new(&executable)
        .args(["--host", &target.endpoint, "container", "inspect", &id])
        .output()
        .unwrap();
    assert!(inspected.status.success());
    let value: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(
        value[0]["Name"],
        format!("/{}", record.container_name.as_ref().unwrap())
    );
    assert!(value[0]["Config"]["Labels"]["com.docker.compose.project"].is_null());
    assert!(
        value[0]["Config"]["Env"]
            .as_array()
            .unwrap()
            .contains(&json!("PASSWORD=dollar$ quote\" 日本語\nsecond line"))
    );
    let mut expected = crate::docker_observation::ExpectedContainer {
        container_id: id.clone(),
        project: record.project_name.clone(),
        scope: "scope".into(),
        instance: record.id.clone(),
        image_id: value[0]["Image"].as_str().unwrap().into(),
        mounts: vec![
            crate::docker_observation::ExpectedMount {
                kind: "bind".into(),
                source: bind.to_string_lossy().into_owned(),
                destination: "/bind-data".into(),
                read_write: true,
            },
            crate::docker_observation::ExpectedMount {
                kind: "volume".into(),
                source: volume.clone(),
                destination: "/volume-data".into(),
                read_write: true,
            },
        ],
        ports: BTreeMap::new(),
        networks: vec![format!("{}_default", record.project_name)],
        command: model.service.command.clone().unwrap(),
        environment: value[0]["Config"]["Env"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect(),
        healthcheck: Some(crate::docker_observation::ExpectedHealthcheck {
            test: vec!["CMD".into(), "true".into()],
            interval: 1_000_000_000,
            timeout: 1_000_000_000,
            retries: 3,
            start_period: 0,
            start_interval: 5_000_000_000,
        }),
        spec_revision: 1,
    };
    create.verify_stopped(&expected).await.unwrap();
    create.start_verified(&expected).await.unwrap();
    create.wait_ready(&expected).await.unwrap();
    let written = docker.change(&["container".into(), "exec".into(), id.clone().into(), "sh".into(), "-c".into(),
        "printf bind-data > /bind-data/sentinel; printf volume-data > /volume-data/sentinel".into()], std::time::Duration::from_secs(30)).await.unwrap();
    assert!(written.status.unwrap().success());
    let stopped = docker
        .change(
            &[
                "container".into(),
                "stop".into(),
                "--time".into(),
                "1".into(),
                id.clone().into(),
            ],
            std::time::Duration::from_secs(30),
        )
        .await
        .unwrap();
    assert!(stopped.status.unwrap().success());
    create.recreate_stopped().await.unwrap();
    let recreated = create.created_container_id().await.unwrap();
    assert_ne!(recreated, id);
    expected.container_id = recreated.clone();
    create.start_verified(&expected).await.unwrap();
    create.wait_ready(&expected).await.unwrap();
    let retained = docker
        .read(&[
            "container".into(),
            "exec".into(),
            recreated.clone().into(),
            "cat".into(),
            "/bind-data/sentinel".into(),
            "/volume-data/sentinel".into(),
        ])
        .await
        .unwrap();
    assert!(retained.status.unwrap().success());
    assert_eq!(retained.stdout.bytes, b"bind-datavolume-data");
    use composenest_application::operation_journal::{
        OperationIntent, OperationJournal, OperationKind, RequestReceipt,
    };
    let receipt = RequestReceipt {
        scope_id: "scope".into(),
        request_id: "delete-test".into(),
        plan_id: None,
        confirmed_revision: 1,
        request_hash: "a".repeat(64),
        instance_id: record.id.clone(),
        operation_id: "delete-test".into(),
    };
    db.write(|connection| {
        connection.execute(
            "UPDATE operations SET status='Succeeded', completed_at=1 WHERE id='standalone-test'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    db.accept(
        &OperationIntent {
            id: "delete-test".into(),
            instance_id: record.id.clone(),
            kind: OperationKind::Delete,
            phase: "retiring".into(),
            expected_revision: 1,
            old_spec_revision: None,
            new_spec_revision: None,
        },
        &receipt,
    )
    .unwrap();
    let owner = crate::docker_delete::DeletionOwner {
        scope: "scope",
        instance: &record.id,
        project: &record.project_name,
        saved_container: Some(&recreated),
    };
    crate::docker_delete::remove_owned_runtime(&docker, &db, &receipt, &owner)
        .await
        .unwrap();
    crate::docker_delete::verify_runtime_absent(&docker, &owner)
        .await
        .unwrap();
    assert_eq!(std::fs::read(bind.join("sentinel")).unwrap(), b"bind-data");
    let retained_volume = docker
        .read(&["volume".into(), "inspect".into(), volume.into()])
        .await
        .unwrap();
    assert!(retained_volume.status.unwrap().success());
}
