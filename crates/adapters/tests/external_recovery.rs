#![cfg(unix)]

mod support;

use composenest_adapters::{
    artifact_store::{ArtifactInput, ArtifactStore},
    create_projection::CreateProjection,
    docker_observation::ExpectedContainer,
    docker_target::DockerProbe,
    external_recovery::{preview_external_recovery, restore_external_artifact},
    external_recovery_state::ExternalRecoveryRequest,
    sqlite::DatabaseWorker,
    storage::BindStorage,
};
use composenest_application::{
    create_state::CreateStateStore,
    image_resolution::ImageResolutionStore,
    operation_journal::{
        ExpectedResult, OperationIntent, OperationJournal, OperationKind, OperationStatus,
        RequestReceipt, StepCommand, StepIntent,
    },
    operation_recovery::{
        CurrentRuntime, RecoverableOperation, RecoveryEvidence, RecoveryJournal, RecoveryProbe,
    },
    operation_runner::OperationRunner,
    state_store::StateStore,
    storage::StoragePort,
};
use composenest_domain::identity::{InstanceId, SlotId};
use serde_json::json;
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt};

const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CONTAINER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NEW: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

struct PriorCli(bool);
impl RecoveryProbe for PriorCli {
    async fn inspect(&self, _: &RecoverableOperation) -> RecoveryEvidence {
        RecoveryEvidence {
            previous_cli_exited: self.0,
            target_matches: true,
            artifact_matches: false,
            storage_verified: false,
            docker_verified: false,
            runtime: CurrentRuntime::Unknown,
        }
    }
}

struct Fixture {
    root: tempfile::TempDir,
    db: DatabaseWorker,
    probe: DockerProbe,
    request: ExternalRecoveryRequest,
}
impl Fixture {
    async fn new() -> Self {
        let (root, db, template) = support::store();
        db.create_target(
            "target",
            "scope",
            "unix:///tmp/composenest-test.sock",
            "engine",
            "linux/amd64",
        )
        .unwrap();
        let binds = BindStorage::new(root.path());
        let mut instance = support::source_instance(&template);
        instance.id = ID.into();
        instance.project_name = format!("cn-{ID}");
        instance.storage = binds
            .create_bind_set(
                InstanceId::from_u128(u128::from_str_radix(ID, 16).unwrap()),
                &[SlotId::parse("data").unwrap()],
            )
            .unwrap();
        db.commit_instance(&instance).unwrap();
        let receipt = RequestReceipt {
            scope_id: "scope".into(),
            request_id: "create".into(),
            plan_id: None,
            confirmed_revision: 1,
            request_hash: "a".repeat(64),
            instance_id: ID.into(),
            operation_id: "create".into(),
        };
        db.accept(
            &OperationIntent {
                id: "create".into(),
                instance_id: ID.into(),
                kind: OperationKind::Create,
                phase: "artifact".into(),
                expected_revision: 1,
                old_spec_revision: None,
                new_spec_revision: Some(1),
            },
            &receipt,
        )
        .unwrap();
        db.write(|db| {
            db.execute_batch(&format!("UPDATE storage_allocations SET presence = 'present'; INSERT INTO image_resolutions (instance_id, spec_revision, image_ref, digest, image_id, platform, first_operation_id) VALUES ('{ID}', 1, 'example:1', 'example@sha256:{}', 'sha256:{}', 'linux/amd64', 'create');", "c".repeat(64), "d".repeat(64)))?;
            Ok(())
        }).unwrap();
        let executable = root.path().join("docker");
        fs::write(
            &executable,
            format!(
                r#"#!/bin/sh
shift 2
if [ "$1" = info ]; then
  if [ -f wrong-engine ]; then printf '{{"ID":"other"}}\n'; else printf '{{"ID":"engine"}}\n'; fi
  exit 0
fi
printf '%s\n' "$*" >> calls
if [ "$1" = image ]; then printf '{{"Env":null,"Cmd":null}}\n'; exit 0; fi
if [ "$1" = container ] && [ "$2" = ls ]; then
  case "$*" in *'id={CONTAINER}'*) if [ -f recreated ]; then exit 0; fi ;; esac
  if [ -f recreated ]; then printf '{NEW}\n'; else printf '{CONTAINER}\n'; fi
  exit 0
fi
if [ "$1" = container ] && [ "$2" = inspect ]; then
  case "$*" in *'{CONTAINER}'*) if [ -f recreated ]; then exit 1; fi ;; esac
  /bin/cat current.json; exit 0
fi
if [ "$1" = container ] && [ "$2" = stop ]; then /bin/cp stopped.json current.json; exit 0; fi
if [ "$1" = compose ]; then
  case "$*" in
    *' config --quiet') exit 0 ;;
    *' create --force-recreate --no-build --pull never main')
      /bin/cp target.json current.json; /usr/bin/touch recreated
      if [ -f create-fail ]; then exit 1; fi
      exit 0 ;;
  esac
fi
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
        let confirmed = db.confirmed_create(&receipt).unwrap();
        let image = db.image_resolution(ID, 1).unwrap().unwrap();
        let docker = probe.bind(confirmed.target.clone()).unwrap();
        let projection = CreateProjection::new(&db, &docker, &binds, &confirmed.project_name, ID);
        let model = projection.model(&confirmed, &image).unwrap();
        let artifacts = ArtifactStore::new(root.path(), &db);
        let path = artifacts
            .publish(
                "create",
                ArtifactInput {
                    id: format!("{ID}-r1"),
                    instance_id: ID.into(),
                    spec_revision: 1,
                    generator_version: "compose-v1".into(),
                    files: BTreeMap::from([(
                        "compose.yaml".into(),
                        composenest_domain::compose::to_yaml(&model)
                            .unwrap()
                            .into_bytes(),
                    )]),
                },
            )
            .unwrap();
        for (id, name, ready) in [
            (CONTAINER, "current.json", true),
            (NEW, "target.json", false),
        ] {
            let expected = projection
                .expected(&model, &image, id, &confirmed)
                .await
                .unwrap();
            fs::write(root.path().join(name), inspection(&expected, ready)).unwrap();
        }
        db.write(|db| {
            db.execute_batch(&format!("UPDATE instances SET applied_spec_revision = 1; UPDATE operations SET status = 'Succeeded', completed_at = CURRENT_TIMESTAMP WHERE id = 'create'; INSERT INTO runtime_observations (instance_id, container_id, runtime_state, freshness) VALUES ('{ID}', '{CONTAINER}', 'running', 'fresh');"))?;
            Ok(())
        }).unwrap();
        fs::write(
            path.join("compose.yaml"),
            "external-secret-and-arbitrary-command",
        )
        .unwrap();
        let preview = preview_external_recovery(&db, root.path(), "scope", ID).unwrap();
        let mut request = ExternalRecoveryRequest {
            receipt: RequestReceipt {
                scope_id: "scope".into(),
                request_id: "restore".into(),
                plan_id: None,
                confirmed_revision: preview.instance_revision,
                request_hash: String::new(),
                instance_id: ID.into(),
                operation_id: "restore".into(),
            },
            spec_revision: preview.spec_revision,
            source_artifact_id: preview.external.artifact_id,
            confirmation_hash: preview.external.confirmation_hash,
        };
        request.receipt.request_hash = request.request_hash();
        Self {
            root,
            db,
            probe,
            request,
        }
    }
    fn drift(&self, foreign: bool) {
        let path = self.root.path().join("current.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["Image"] = json!("wrong-image");
        if foreign {
            value["Config"]["Labels"]["io.composenest.scope"] = json!("foreign");
        }
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        value["State"]["Status"] = json!("exited");
        fs::write(
            self.root.path().join("stopped.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }
    async fn run(
        &self,
        exited: bool,
    ) -> Result<String, composenest_adapters::port_edit_stages::PortEditError> {
        restore_external_artifact(
            &self.db,
            &self.probe,
            self.root.path(),
            &OperationRunner::new(),
            &self.request,
            &PriorCli(exited),
        )
        .await
    }
    fn calls(&self) -> String {
        fs::read_to_string(self.root.path().join("calls")).unwrap()
    }
}

#[tokio::test]
async fn scoped_external_confirmation_rejects_changed_hash_and_reconciles_same_receipt() {
    use composenest_adapters::{
        recovery_actions::recover_operation,
        recovery_view::{RecoverySession, inspect_recovery},
    };
    use composenest_application::{RequestContext, recovery_view::RecoverOperationRequest};
    let f = Fixture::new().await;
    let session = RecoverySession::new(&f.db).unwrap();
    let view = inspect_recovery(&f.db, &f.probe, &session, "scope", ID, "create")
        .await
        .unwrap();
    assert!(view.actions.contains(&"restore_external".into()));
    assert!(!view.files.is_empty());
    let mut request = RecoverOperationRequest {
        context: RequestContext {
            api_version: 1,
            request_id: "explicit-restore".into(),
        },
        instance_id: ID.into(),
        operation_id: "create".into(),
        expected_attempt: view.attempt,
        expected_revision: view.instance_revision,
        candidate_revision: view.candidate_revision,
        action: "restore_external".into(),
        ports: BTreeMap::new(),
        artifact_id: view.artifact_id,
        confirmation_hash: view.confirmation_hash.clone(),
    };
    let confirmed_hash = request.confirmation_hash.clone().unwrap();
    let mut different_hash = confirmed_hash.clone();
    different_hash.replace_range(
        0..1,
        if &confirmed_hash[0..1] == "0" {
            "1"
        } else {
            "0"
        },
    );
    request.confirmation_hash = Some(different_hash);
    let runner = OperationRunner::new();
    assert!(matches!(
        recover_operation(&f.db, &f.probe, &session, &runner, "scope", &request).await,
        Err(composenest_adapters::port_edit_stages::PortEditError::Rejected)
    ));
    assert!(
        f.db.receipt("scope", &request.context.request_id)
            .unwrap()
            .is_none(),
        "hash rejection must precede acceptance"
    );
    request.confirmation_hash = Some(confirmed_hash);
    let restored = recover_operation(&f.db, &f.probe, &session, &runner, "scope", &request)
        .await
        .unwrap();
    assert_ne!(restored.operation_id, "recover-explicit-restore");
    assert_eq!(restored.receipt_request_id, request.context.request_id);
    use composenest_adapters::recovery_view::inspection_operation;
    assert_eq!(
        inspection_operation(&f.db, "scope", ID, "create", Some("explicit-restore")).unwrap(),
        restored.operation_id
    );
    assert_eq!(
        inspection_operation(&f.db, "scope", ID, "create", Some("not-accepted")).unwrap(),
        "create"
    );
    assert!(
        inspection_operation(&f.db, "foreign", ID, "create", Some("explicit-restore")).is_err()
    );
    assert!(
        f.root
            .path()
            .join(format!(
                "instances/{ID}/recovery/{}/compose.yaml",
                restored.operation_id
            ))
            .exists()
    );
    let again = recover_operation(&f.db, &f.probe, &session, &runner, "scope", &request)
        .await
        .unwrap();
    assert_eq!(again.operation_id, restored.operation_id);
    assert!(!f.calls().contains("container stop"));
}

#[tokio::test]
async fn abandoned_unapplied_create_or_clone_does_not_offer_external_restoration() {
    use composenest_adapters::recovery_view::{RecoverySession, inspect_recovery};
    for kind in ["create", "clone"] {
        let f = Fixture::new().await;
        f.db.write(move |db| {
            db.execute("UPDATE instances SET applied_spec_revision=NULL", [])?;
            db.execute(
                "UPDATE operations SET status='Abandoned', kind=?1 WHERE id='create'",
                [kind],
            )?;
            Ok(())
        })
        .unwrap();
        let view = inspect_recovery(
            &f.db,
            &f.probe,
            &RecoverySession::new(&f.db).unwrap(),
            "scope",
            ID,
            "create",
        )
        .await
        .unwrap();
        assert!(!view.actions.contains(&"restore_external".into()));
        assert!(view.hold_reasons.contains(&"SPEC_NOT_APPLIED".into()));
        assert!(!f.calls().contains("container stop"));
    }
}

#[tokio::test]
async fn external_restoration_allocates_internal_ids_for_all_valid_request_ids() {
    use composenest_adapters::{
        recovery_actions::recover_operation,
        recovery_view::{RecoverySession, inspect_recovery},
    };
    use composenest_application::{RequestContext, recovery_view::RecoverOperationRequest};
    for request_id in [
        "request.with.dots".to_owned(),
        "日本語の受付".to_owned(),
        "x".repeat(128),
    ] {
        let f = Fixture::new().await;
        let session = RecoverySession::new(&f.db).unwrap();
        let view = inspect_recovery(&f.db, &f.probe, &session, "scope", ID, "create")
            .await
            .unwrap();
        let request = RecoverOperationRequest {
            context: RequestContext {
                api_version: 1,
                request_id,
            },
            instance_id: ID.into(),
            operation_id: "create".into(),
            expected_attempt: view.attempt,
            expected_revision: view.instance_revision,
            candidate_revision: view.candidate_revision,
            action: "restore_external".into(),
            ports: BTreeMap::new(),
            artifact_id: view.artifact_id,
            confirmation_hash: view.confirmation_hash,
        };
        request.context.validate().unwrap();
        let runner = OperationRunner::new();
        let restored = recover_operation(&f.db, &f.probe, &session, &runner, "scope", &request)
            .await
            .unwrap();
        assert_eq!(restored.operation_id.len(), 40);
        assert!(
            restored.operation_id[8..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        );
        assert_eq!(
            f.db.receipt("scope", &request.context.request_id)
                .unwrap()
                .unwrap()
                .operation_id,
            restored.operation_id
        );
        let repeated = recover_operation(&f.db, &f.probe, &session, &runner, "scope", &request)
            .await
            .unwrap();
        assert_eq!(repeated.operation_id, restored.operation_id);
    }
}

fn inspection(expected: &ExpectedContainer, ready: bool) -> Vec<u8> {
    let health = expected.healthcheck.as_ref().unwrap();
    serde_json::to_vec(&json!({
        "Id": expected.container_id, "Image": expected.image_id,
        "Config": { "Labels": { "com.docker.compose.project": expected.project, "com.docker.compose.service": "main", "io.composenest.scope": "scope", "io.composenest.instance": ID, "io.composenest.spec-revision": expected.spec_revision.to_string() }, "Env": expected.environment, "Cmd": expected.command, "Healthcheck": { "Test": health.test, "Interval": health.interval, "Timeout": health.timeout, "StartPeriod": health.start_period, "StartInterval": health.start_interval, "Retries": health.retries } },
        "HostConfig": { "PortBindings": expected.ports.iter().map(|(key, hosts)| (key.clone(), hosts.iter().map(|host| json!({"HostIp": host.host_ip, "HostPort": host.host_port})).collect::<Vec<_>>())).collect::<BTreeMap<_, _>>() },
        "Mounts": expected.mounts.iter().map(|mount| json!({ "Type": mount.kind, "Source": mount.source, "Destination": mount.destination, "RW": mount.read_write })).collect::<Vec<_>>(),
        "NetworkSettings": { "Networks": expected.networks.iter().map(|name| (name, json!({}))).collect::<BTreeMap<_, _>>() },
        "State": { "Status": if ready { "running" } else { "created" }, "Health": { "Status": if ready { "healthy" } else { "starting" } } }
    })).unwrap()
}

#[tokio::test]
async fn matching_runtime_is_preserved_and_saved_secrets_are_regenerated() {
    let f = Fixture::new().await;
    assert_eq!(f.run(true).await.unwrap(), CONTAINER);
    let selected = f.db.selected_artifact(ID, 1).unwrap();
    let path = ArtifactStore::new(f.root.path(), &f.db)
        .verified_compose_path(&selected)
        .unwrap();
    let yaml = fs::read_to_string(path).unwrap();
    assert!(yaml.contains(&"p".repeat(32)));
    assert!(!yaml.contains("external-secret-and-arbitrary-command"));
    assert!(!f.calls().contains("container stop"));
    assert!(!f.calls().contains(" create "));
    assert!(
        f.root
            .path()
            .join(format!("instances/{ID}/recovery/restore/compose.yaml"))
            .exists()
    );
    f.db.write(|db| {
        db.execute(
            "UPDATE runtime_observations SET operation_id = 'create'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(f.run(true).await.unwrap(), CONTAINER);
}

#[tokio::test]
async fn changed_runtime_is_stopped_and_recreated_without_starting() {
    let f = Fixture::new().await;
    f.drift(false);
    assert_eq!(f.run(true).await.unwrap(), NEW);
    let calls = f.calls();
    assert!(calls.find("container stop").unwrap() < calls.find(" create ").unwrap());
    assert!(!calls.contains("container start") && !calls.contains("recovery/"));
    let runtime: String =
        f.db.read(|db| {
            Ok(
                db.query_row("SELECT runtime_state FROM runtime_observations", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(runtime, "stopped");
}

#[tokio::test]
async fn uncertain_prerequisites_do_not_archive_stop_or_recreate() {
    for failure in ["owner", "engine", "cli", "reedit", "storage"] {
        let f = Fixture::new().await;
        match failure {
            "owner" => f.drift(true),
            "engine" => fs::write(f.root.path().join("wrong-engine"), "").unwrap(),
            "reedit" => fs::write(
                f.root
                    .path()
                    .join(format!("instances/{ID}/artifacts/{ID}-r1/compose.yaml")),
                "edited-after-confirmation",
            )
            .unwrap(),
            "storage" => fs::remove_dir_all(f.root.path().join(format!("data/{ID}/data"))).unwrap(),
            _ => {}
        }
        assert!(f.run(failure != "cli").await.is_err(), "{failure}");
        assert!(
            !f.calls().contains("container stop") && !f.calls().contains(" create "),
            "{failure}"
        );
        assert!(
            f.root
                .path()
                .join(format!("instances/{ID}/artifacts/{ID}-r1/compose.yaml"))
                .exists(),
            "{failure}"
        );
    }
}

#[tokio::test]
async fn interrupted_archive_and_unknown_recreate_resume_the_original_operation() {
    for interrupted_archive in [true, false] {
        let f = Fixture::new().await;
        if interrupted_archive {
            f.db.begin_external_recovery(&f.request).unwrap();
            f.db.set_status("restore", OperationStatus::Executing, "generate")
                .unwrap();
            f.db.record_step(&StepIntent {
                operation_id: "restore".into(),
                sequence: 1,
                attempt: 1,
                command_kind: StepCommand::GenerateArtifact,
                resource_id: "restore-restored".into(),
                expected_result: ExpectedResult::ArtifactReady,
            })
            .unwrap();
            ArtifactStore::new(f.root.path(), &f.db)
                .archive_external(
                    "restore",
                    &f.request.source_artifact_id,
                    &f.request.confirmation_hash,
                )
                .unwrap();
            f.db.recover_on_startup().unwrap();
        } else {
            f.drift(false);
            fs::write(f.root.path().join("create-fail"), "").unwrap();
            assert!(f.run(true).await.is_err());
            fs::remove_file(f.root.path().join("create-fail")).unwrap();
        }
        let before = f.calls().matches(" create ").count();
        assert_eq!(
            f.run(true).await.unwrap(),
            if interrupted_archive { CONTAINER } else { NEW }
        );
        assert_eq!(f.calls().matches(" create ").count(), before);
        assert_eq!(
            f.db.external_recovery("restore")
                .unwrap()
                .replacement_artifact_id,
            "restore-restored"
        );
    }
}

struct RealDocker {
    probe: DockerProbe,
    endpoint: String,
    project: String,
    armed: bool,
}
impl RealDocker {
    fn call(&self, args: &[&str]) -> String {
        let output = std::process::Command::new(&self.probe.executable)
            .args(["--host", &self.endpoint])
            .args(args)
            .env("DOCKER_CONFIG", &self.probe.config_directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "test Docker command failed: {}",
            args[0]
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
}
impl Drop for RealDocker {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // The test first proves this project absent; remove only its exact scope-labelled IDs.
        let ids = self.call(&[
            "container",
            "ls",
            "--all",
            "--quiet",
            "--no-trunc",
            "--filter",
            &format!("label=com.docker.compose.project={}", self.project),
            "--filter",
            "label=io.composenest.scope=scope",
        ]);
        for id in ids.lines() {
            self.call(&["container", "rm", "--force", id]);
        }
        let network = format!("{}_default", self.project);
        let ids = self.call(&[
            "network",
            "ls",
            "--quiet",
            "--filter",
            &format!("name=^{network}$"),
            "--filter",
            "label=io.composenest.scope=scope",
        ]);
        for id in ids.lines() {
            self.call(&["network", "rm", id]);
        }
    }
}

#[tokio::test]
#[ignore = "requires a local Docker Engine, cached alpine:latest, COMPOSENEST_TEST_DOCKER and COMPOSENEST_TEST_DOCKER_CONFIG"]
async fn actual_docker_restores_owned_drift_and_keeps_data_and_secrets() {
    let f = Fixture::new().await;
    let probe = DockerProbe {
        executable: std::env::var_os("COMPOSENEST_TEST_DOCKER").unwrap().into(),
        directory: f.root.path().into(),
        config_directory: std::env::var_os("COMPOSENEST_TEST_DOCKER_CONFIG")
            .unwrap()
            .into(),
    };
    let report = probe.diagnose(None).await;
    let endpoint = report.resolved_endpoint.unwrap();
    let project = format!("cn-{ID}");
    let mut real = RealDocker {
        probe,
        endpoint: endpoint.clone(),
        project: project.clone(),
        armed: false,
    };
    assert!(
        real.call(&[
            "container",
            "ls",
            "--all",
            "--quiet",
            "--filter",
            &format!("label=com.docker.compose.project={project}")
        ])
        .is_empty()
    );
    assert!(
        real.call(&[
            "network",
            "ls",
            "--quiet",
            "--filter",
            &format!("name=^{project}_default$")
        ])
        .is_empty()
    );
    real.armed = true;
    let image: serde_json::Value =
        serde_json::from_str(&real.call(&["image", "inspect", "alpine:latest"])).unwrap();
    let digest = image[0]["RepoDigests"][0].as_str().unwrap().to_owned();
    let image_id = image[0]["Id"].as_str().unwrap().to_owned();
    let engine = report.engine_id.unwrap();
    let platform = report.observed_platform.unwrap();
    let contents = support::V1
        .replace("example:1", "alpine:latest")
        .into_bytes();
    f.db.write(move |db| {
        db.execute("UPDATE runtime_targets SET endpoint = ?1, engine_id = ?2, platform = ?3", rusqlite::params![endpoint, engine, platform])?;
        db.execute("UPDATE image_resolutions SET image_ref = 'alpine:latest', digest = ?1, image_id = ?2, platform = ?3", rusqlite::params![digest, image_id, platform])?;
        // Replace only the test fixture's snapshot image with the cached integration image.
        db.execute("UPDATE template_snapshot_files SET contents = ?1 WHERE relative_path = 'versions/1.yaml'", [contents])?;
        Ok(())
    }).unwrap();
    let old = real.call(&[
        "run",
        "--detach",
        "--label",
        &format!("com.docker.compose.project={project}"),
        "--label",
        "com.docker.compose.service=main",
        "--label",
        "com.docker.compose.container-number=1",
        "--label",
        "com.docker.compose.oneoff=False",
        "--label",
        "com.docker.compose.config-hash=external-drift",
        "--label",
        "io.composenest.scope=scope",
        "--label",
        &format!("io.composenest.instance={ID}"),
        "alpine:latest",
        "sleep",
        "600",
    ]);
    f.db.write(move |db| {
        db.execute("UPDATE runtime_observations SET container_id = ?1", [old])?;
        Ok(())
    })
    .unwrap();
    let data = f.root.path().join(format!("data/{ID}/data/retained"));
    fs::write(&data, "preserved-data").unwrap();
    let id = restore_external_artifact(
        &f.db,
        &real.probe,
        f.root.path(),
        &OperationRunner::new(),
        &f.request,
        &PriorCli(true),
    )
    .await
    .unwrap_or_else(|error| {
        let ids = real.call(&[
            "container",
            "ls",
            "--all",
            "--quiet",
            "--filter",
            &format!("label=com.docker.compose.project={project}"),
        ]);
        let health = real.call(&[
            "container",
            "inspect",
            "--format",
            "{{json (index .Config.Healthcheck \"StartInterval\")}}",
            ids.trim(),
        ]);
        panic!(
            "{error:?}; health start interval: {health}; journal: {:?}",
            f.db.recoverable("restore")
        )
    });
    let observed: serde_json::Value =
        serde_json::from_str(&real.call(&["container", "inspect", &id])).unwrap();
    assert!(!observed[0]["State"]["Running"].as_bool().unwrap());
    assert!(
        observed[0]["Config"]["Env"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some(&format!("PASSWORD={}", "p".repeat(32))))
    );
    assert_eq!(fs::read_to_string(data).unwrap(), "preserved-data");
    assert_eq!(
        fs::read_to_string(
            f.root
                .path()
                .join(format!("instances/{ID}/recovery/restore/compose.yaml"))
        )
        .unwrap(),
        "external-secret-and-arbitrary-command"
    );
}
