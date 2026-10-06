mod support;
use composenest_adapters::{
    delete_stages::{refresh_retained_storage, run_delete},
    docker_target::DockerProbe,
    named_volumes::DockerNamedVolumes,
    storage::BindStorage,
};
use composenest_application::{
    named_volumes::NamedVolumePort,
    operation_journal::{
        OperationIntent, OperationJournal, OperationKind, OperationStatus, RequestReceipt,
    },
    operation_runner::OperationRunner,
    retained_storage::RetainedStore,
    state_store::{StateStore, StorageAllocation, StorageMethod},
    storage::StoragePort,
};
use composenest_domain::{
    identity::{InstanceId, SlotId},
    instance::StoragePresence,
};
use std::{
    env, fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn request(id: &str, operation: &str, kind: OperationKind) -> (OperationIntent, RequestReceipt) {
    (
        OperationIntent {
            id: operation.into(),
            instance_id: id.into(),
            kind,
            phase: "accepted".into(),
            expected_revision: 1,
            old_spec_revision: None,
            new_spec_revision: (kind == OperationKind::Create).then_some(1),
        },
        RequestReceipt {
            scope_id: "scope".into(),
            request_id: operation.into(),
            plan_id: None,
            confirmed_revision: 1,
            request_hash: "a".repeat(64),
            instance_id: id.into(),
            operation_id: operation.into(),
        },
    )
}

#[tokio::test]
async fn unavailable_engine_accepts_retiring_and_keeps_the_committed_port() {
    let (root, db, template) = support::store();
    db.create_target(
        "target",
        "scope",
        "unix:///tmp/delete-test.sock",
        "engine",
        "linux/amd64",
    )
    .unwrap();
    let mut record = support::source_instance(&template);
    record.id = "a".repeat(32);
    record.project_name = format!("cn-{}", record.id);
    db.commit_instance(&record).unwrap();
    let probe = DockerProbe {
        executable: root.path().join("missing-docker.exe"),
        directory: root.path().into(),
        config_directory: root.path().into(),
    };
    let (intent, receipt) = request(&record.id, "delete", OperationKind::Delete);
    assert!(
        run_delete(
            &db,
            &probe,
            &OperationRunner::new(),
            &intent,
            &receipt,
            true
        )
        .await
        .is_err()
    );
    db.read(|db| {
        assert_eq!(
            db.query_row("SELECT lifecycle FROM instances", [], |r| r
                .get::<_, String>(0))?,
            "retiring"
        );
        assert_eq!(
            db.query_row("SELECT status FROM port_reservations", [], |r| r
                .get::<_, String>(0))?,
            "committed"
        );
        Ok(())
    })
    .unwrap();
}

struct DockerFixture {
    executable: PathBuf,
    endpoint: String,
    containers: Vec<String>,
    networks: Vec<String>,
    volumes: Vec<String>,
}
impl DockerFixture {
    fn call(&self, args: &[&str]) -> String {
        let output = Command::new(&self.executable)
            .args(["--host", &self.endpoint])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "test Docker command failed: {} {}",
            args[0],
            args.get(1).unwrap_or(&"")
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
}
impl Drop for DockerFixture {
    fn drop(&mut self) {
        // Clean up only IDs and names created by this test; application Delete never removes volumes.
        for id in &self.containers {
            let _ = Command::new(&self.executable)
                .args([
                    "--host",
                    &self.endpoint,
                    "container",
                    "stop",
                    "--time",
                    "1",
                    id,
                ])
                .output();
            let _ = Command::new(&self.executable)
                .args(["--host", &self.endpoint, "container", "rm", id])
                .output();
        }
        for id in &self.networks {
            let _ = Command::new(&self.executable)
                .args(["--host", &self.endpoint, "network", "rm", id])
                .output();
        }
        for name in &self.volumes {
            let _ = Command::new(&self.executable)
                .args(["--host", &self.endpoint, "volume", "rm", name])
                .output();
        }
    }
}

#[tokio::test]
#[ignore = "requires a local Docker Engine, cached alpine:latest, COMPOSENEST_TEST_DOCKER and COMPOSENEST_TEST_DOCKER_CONFIG"]
async fn docker_delete_preserves_bind_and_volume_data_and_rejects_foreign_network_endpoints() {
    for method in [StorageMethod::Bind, StorageMethod::Volume] {
        let (root, db, template) = support::store();
        let probe = DockerProbe {
            executable: PathBuf::from(env::var_os("COMPOSENEST_TEST_DOCKER").unwrap()),
            directory: root.path().into(),
            config_directory: PathBuf::from(env::var_os("COMPOSENEST_TEST_DOCKER_CONFIG").unwrap()),
        };
        let report = probe.diagnose(None).await;
        db.create_target(
            "target",
            "scope",
            report.resolved_endpoint.as_ref().unwrap(),
            report.engine_id.as_ref().unwrap(),
            report.observed_platform.as_ref().unwrap(),
        )
        .unwrap();
        let target = db.runtime_target("scope").unwrap().unwrap();
        let id_number = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let id = format!("{id_number:032x}");
        let project = format!("cn-{id}");
        let slot = SlotId::parse("data").unwrap();
        let instance_id = InstanceId::from_u128(id_number);
        let volume_name = format!("cn-{id}-data");
        let bind = BindStorage::new(root.path());
        let allocation = if method == StorageMethod::Bind {
            bind.create_bind_set(instance_id, std::slice::from_ref(&slot))
                .unwrap()
                .remove(0)
        } else {
            StorageAllocation {
                slot: "data".into(),
                resource_identity: volume_name.clone(),
                ownership_evidence: "create".into(),
            }
        };
        let mut record = support::source_instance(&template);
        record.id = id.clone();
        record.project_name = project.clone();
        record.storage_method = method;
        record.storage = vec![allocation];
        db.commit_instance(&record).unwrap();
        let (create, create_receipt) = request(&id, "create", OperationKind::Create);
        db.accept(&create, &create_receipt).unwrap();
        let mut docker = DockerFixture {
            executable: probe.executable.clone(),
            endpoint: target.endpoint.clone(),
            containers: vec![],
            networks: vec![],
            volumes: vec![],
        };
        if method == StorageMethod::Volume {
            docker.volumes.push(volume_name.clone());
            DockerNamedVolumes::new(probe.bind(target.clone()).unwrap())
                .ensure_named_volume(&db, &db, instance_id, &slot, 1, 1)
                .await
                .unwrap();
        } else {
            db.set_storage_presence(&id, "data", StoragePresence::Present)
                .unwrap();
        }
        db.set_status("create", OperationStatus::Succeeded, "fixture")
            .unwrap();
        let project_label = format!("com.docker.compose.project={project}");
        let instance_label = format!("io.composenest.instance={id}");
        let network_name = format!("{project}_default");
        let network_id = docker.call(&[
            "network",
            "create",
            "--label",
            &project_label,
            "--label",
            "io.composenest.scope=scope",
            "--label",
            &instance_label,
            "--label",
            "com.docker.compose.network=default",
            &network_name,
        ]);
        docker.networks.push(network_id.clone());
        let mount = if method == StorageMethod::Bind {
            format!(
                "type=bind,source={},target=/data",
                root.path()
                    .join(&record.storage[0].resource_identity)
                    .display()
            )
        } else {
            format!("type=volume,source={volume_name},target=/data")
        };
        let container = docker.call(&[
            "container",
            "create",
            "--pull",
            "never",
            "--network",
            &network_id,
            "--mount",
            &mount,
            "--label",
            &project_label,
            "--label",
            "io.composenest.scope=scope",
            "--label",
            &instance_label,
            "--label",
            "com.docker.compose.service=main",
            "alpine:latest",
            "sleep",
            "600",
        ]);
        docker.containers.push(container.clone());
        docker.call(&["container", "start", &container]);
        docker.call(&[
            "container",
            "exec",
            &container,
            "sh",
            "-c",
            "printf keep > /data/sentinel",
        ]);
        let artifact = root
            .path()
            .join("instances")
            .join(&id)
            .join("artifacts/saved");
        fs::create_dir_all(&artifact).unwrap();
        fs::write(artifact.join("compose.yaml"), "# preserved").unwrap();
        let saved_id = id.clone();
        let saved_container = container.clone();
        db.write(move |db| {
            db.execute("INSERT INTO runtime_observations(instance_id, container_id, runtime_state, freshness) VALUES (?1, ?2, 'running', 'fresh')", rusqlite::params![saved_id, saved_container])?;
            db.execute("INSERT INTO artifacts(id, instance_id, spec_revision, generator_version, manifest_hash, placement) VALUES ('saved', ?1, 1, 'v1', 'hash', 'published')", [&saved_id])?; Ok(())
        }).unwrap();
        let (delete, receipt) = request(&id, "delete", OperationKind::Delete);
        let runner = OperationRunner::new();
        if method == StorageMethod::Bind {
            let outsider = docker.call(&[
                "container",
                "create",
                "--pull",
                "never",
                "--network",
                &network_id,
                "alpine:latest",
                "sleep",
                "600",
            ]);
            docker.containers.push(outsider.clone());
            docker.call(&["container", "start", &outsider]);
            assert!(
                run_delete(&db, &probe, &runner, &delete, &receipt, true)
                    .await
                    .is_err()
            );
            assert_eq!(
                docker.call(&[
                    "container",
                    "inspect",
                    "--format",
                    "{{.State.Running}}",
                    &container
                ]),
                "true"
            );
            docker.call(&["network", "disconnect", &network_id, &outsider]);
            db.retry("delete", "retry").unwrap();
        }
        run_delete(&db, &probe, &runner, &delete, &receipt, true)
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(artifact.join("compose.yaml")).unwrap(),
            "# preserved"
        );
        let retained = refresh_retained_storage(&db, Some(&probe), "scope", &id)
            .await
            .unwrap();
        assert_eq!(retained.locations[0].storage.presence, "present");
        assert!(retained.locations[0].ownership_verified);
        assert_eq!(retained.instance.lifecycle, "retired");
        assert_eq!(retained.instance.runtime_status, "absent");
        assert_eq!(retained.artifacts[0].placement, "retained");
        if method == StorageMethod::Bind {
            assert_eq!(
                fs::read_to_string(
                    root.path()
                        .join(&record.storage[0].resource_identity)
                        .join("sentinel")
                )
                .unwrap(),
                "keep"
            );
        } else {
            let reader = docker.call(&[
                "container",
                "create",
                "--pull",
                "never",
                "--mount",
                &mount,
                "alpine:latest",
                "cat",
                "/data/sentinel",
            ]);
            docker.containers.push(reader.clone());
            assert_eq!(
                docker.call(&["container", "start", "--attach", &reader]),
                "keep"
            );
        }
        assert_eq!(db.list_retained_storage("scope").unwrap().len(), 1);
        assert_eq!(
            run_delete(&db, &probe, &runner, &delete, &receipt, true).await,
            Ok(receipt)
        );
    }
}
