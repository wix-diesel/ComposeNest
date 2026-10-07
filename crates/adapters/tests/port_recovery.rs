#![cfg(unix)]

mod support;

use std::{
    collections::BTreeMap,
    fs,
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicU16, Ordering},
};

use composenest_adapters::{
    artifact_store::{ArtifactInput, ArtifactStore},
    create_projection::CreateProjection,
    docker_observation::ExpectedContainer,
    docker_target::DockerProbe,
    port_recovery::{PortRecoveryAction, PortRecoveryResult, recover_port_change},
    sqlite::DatabaseWorker,
    storage::BindStorage,
};
use composenest_application::{
    create_state::CreateStateStore,
    host_ports::{PortCursor, PortPlan},
    image_resolution::ImageResolutionStore,
    operation_journal::{
        OperationIntent, OperationJournal, OperationKind, OperationStatus, RequestReceipt,
    },
    operation_recovery::{CurrentRuntime, RecoverableOperation, RecoveryEvidence, RecoveryProbe},
    operation_runner::OperationRunner,
    port_edit::{PortEditRequest, PortEditStore},
    state_store::StateStore,
    storage::StoragePort,
};
use composenest_domain::identity::{InstanceId, SlotId};
use serde_json::json;

const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CONTAINER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[tokio::test]
async fn recovery_view_is_scoped_fresh_and_keeps_the_failed_result() {
    use composenest_adapters::recovery_view::{RecoverySession, inspect_recovery};
    let fixture = Fixture::new(OperationKind::EditPort).await;
    fixture.current(1);
    fixture
        .db
        .set_status("operation", OperationStatus::Failed, "ports")
        .unwrap();
    let session = RecoverySession::new(&fixture.db).unwrap();
    let inspect = || {
        inspect_recovery(
            &fixture.db,
            &fixture.probe,
            &session,
            "scope",
            ID,
            "operation",
        )
    };
    let view = inspect().await.unwrap();
    assert_eq!(view.previous_status, "Failed");
    assert_eq!(view.current_runtime, "stopped");
    assert!(view.actions.contains(&"restore_ports".into()));
    assert_ne!(view.ports[0].host_port, view.original_ports[0].host_port);
    assert!(
        inspect_recovery(
            &fixture.db,
            &fixture.probe,
            &session,
            "other",
            ID,
            "operation"
        )
        .await
        .is_err()
    );
    assert!(
        inspect_recovery(
            &fixture.db,
            &fixture.probe,
            &session,
            "scope",
            "other",
            "operation"
        )
        .await
        .is_err()
    );
    fs::copy(
        fixture.root.path().join("ready.json"),
        fixture.root.path().join("current.json"),
    )
    .unwrap();
    let ready = inspect().await.unwrap();
    assert_eq!(ready.previous_status, "Failed");
    assert_eq!(ready.current_runtime, "ready");
    assert!(!ready.actions.contains(&"retry_ports".into()));
    assert!(!ready.actions.contains(&"reconcile_ports".into()));
    assert!(!serde_json::to_string(&ready).unwrap().contains("password"));
    let calls = fs::read_to_string(fixture.root.path().join("calls")).unwrap();
    assert!(!calls.contains("compose create"));
    assert!(!calls.contains("container start"));
}

#[tokio::test]
async fn inherited_unfinished_cli_and_missing_storage_hold_every_change() {
    use composenest_adapters::recovery_view::{RecoverySession, inspect_recovery};
    use composenest_application::operation_journal::{ExpectedResult, StepCommand, StepIntent};
    let fixture = Fixture::new(OperationKind::EditPort).await;
    fixture.current(1);
    fixture
        .db
        .set_status("operation", OperationStatus::Executing, "ports")
        .unwrap();
    fixture
        .db
        .record_step(&StepIntent {
            operation_id: "operation".into(),
            sequence: 1,
            attempt: 1,
            command_kind: StepCommand::ComposeCreate,
            resource_id: ID.into(),
            expected_result: ExpectedResult::ContainerCreated,
        })
        .unwrap();
    fixture
        .db
        .set_status("operation", OperationStatus::OutcomeUnknown, "ports")
        .unwrap();
    let session = RecoverySession::new(&fixture.db).unwrap();
    fixture
        .db
        .write(|db| {
            db.execute("UPDATE storage_allocations SET presence='missing'", [])?;
            Ok(())
        })
        .unwrap();
    let view = inspect_recovery(
        &fixture.db,
        &fixture.probe,
        &session,
        "scope",
        ID,
        "operation",
    )
    .await
    .unwrap();
    assert!(view.actions.is_empty());
    assert!(
        view.hold_reasons
            .contains(&"CLI_TERMINATION_UNCONFIRMED".into())
    );
    assert!(view.hold_reasons.contains(&"STORAGE_MISSING".into()));
    let reservations: u64 = fixture
        .db
        .read(|db| {
            Ok(db.query_row(
                "SELECT count(*) FROM port_reservations WHERE status!='released'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(reservations, 2);
}

fn available_fixture_port() -> u16 {
    // Avoid ephemeral ports reused by child processes, and leave room for each
    // fixture's recovery proposals without overlapping parallel fixtures.
    static NEXT_PORT: AtomicU16 = AtomicU16::new(20_000);
    loop {
        let port = NEXT_PORT.fetch_add(32, Ordering::Relaxed);
        assert!(port < 40_000, "fixture port range exhausted");
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
}

struct PriorCli(bool);
impl RecoveryProbe for PriorCli {
    async fn inspect(&self, _: &RecoverableOperation) -> RecoveryEvidence {
        RecoveryEvidence {
            previous_cli_exited: self.0,
            target_matches: true,
            artifact_matches: true,
            storage_verified: true,
            docker_verified: true,
            runtime: CurrentRuntime::Stopped,
        }
    }
}

struct Fixture {
    root: tempfile::TempDir,
    db: DatabaseWorker,
    probe: DockerProbe,
    receipt: RequestReceipt,
}

impl Fixture {
    async fn new(kind: OperationKind) -> Self {
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
        let storage = binds
            .create_bind_set(
                InstanceId::from_u128(u128::from_str_radix(ID, 16).unwrap()),
                &[SlotId::parse("data").unwrap()],
            )
            .unwrap();
        let mut instance = support::source_instance(&template);
        instance.id = ID.into();
        instance.project_name = format!("cn-{ID}");
        instance.storage = storage;
        instance.ports[0].host_port = available_fixture_port();
        db.commit_instance(&instance).unwrap();
        let receipt = RequestReceipt {
            scope_id: "scope".into(),
            request_id: "request".into(),
            plan_id: None,
            confirmed_revision: 1,
            request_hash: "a".repeat(64),
            instance_id: ID.into(),
            operation_id: "operation".into(),
        };
        db.write(|db| { db.execute("UPDATE storage_allocations SET presence = 'present', initialization = 'may_have_initialized'", [])?; Ok(()) }).unwrap();
        if kind == OperationKind::EditPort {
            db.write(|db| {
                db.execute("UPDATE instances SET applied_spec_revision = 1", [])?;
                Ok(())
            })
            .unwrap();
            let mut ports = instance.ports.clone();
            ports[0].host_port = available_fixture_port();
            db.begin_port_edit(&PortEditRequest {
                receipt: receipt.clone(),
                expected_instance_revision: 1,
                old_spec_revision: 1,
                ports,
            })
            .unwrap();
        } else {
            db.accept(
                &OperationIntent {
                    id: "operation".into(),
                    instance_id: ID.into(),
                    kind,
                    phase: "start".into(),
                    expected_revision: 1,
                    old_spec_revision: None,
                    new_spec_revision: Some(1),
                },
                &receipt,
            )
            .unwrap();
        }
        db.write(|db| { db.execute_batch(&format!("INSERT INTO image_resolutions (instance_id, spec_revision, image_ref, digest, image_id, platform, first_operation_id) VALUES ('{ID}', 1, 'example:1', 'example@sha256:{}', 'sha256:{}', 'linux/amd64', 'operation');", "c".repeat(64), "d".repeat(64)))?; Ok(()) }).unwrap();
        let executable = root.path().join("docker");
        fs::write(&executable, format!(r#"#!/bin/sh
shift 2
if [ "$1" = info ]; then printf '{{"ID":"engine"}}\n'; exit 0; fi
printf '%s\n' "$*" >> calls
if [ "$1" = image ]; then printf '{{"Env":null,"Cmd":null}}\n'; exit 0; fi
if [ "$1" = ps ]; then exit 0; fi
if [ "$1" = container ] && [ "$2" = ls ]; then printf '{CONTAINER}\n'; exit 0; fi
if [ "$1" = container ] && [ "$2" = inspect ]; then /bin/cat current.json; exit 0; fi
if [ "$1" = container ] && [ "$2" = start ]; then if [ -f start-fail ]; then exit 1; fi; /bin/cp ready.json current.json; exit 0; fi
if [ "$1" = compose ]; then
  case "$*" in
    *' config --quiet') exit 0 ;;
    *' create --force-recreate --no-build --pull never main') /bin/cp target.json current.json; exit 0 ;;
  esac
fi
exit 1
"#)).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let probe = DockerProbe {
            executable,
            directory: root.path().into(),
            config_directory: root.path().into(),
        };
        let fixture = Self {
            root,
            db,
            probe,
            receipt,
        };
        for revision in 1..=if kind == OperationKind::EditPort {
            2
        } else {
            1
        } {
            fixture.artifact(revision).await;
        }
        fixture
            .db
            .set_status("operation", OperationStatus::OutcomeUnknown, "recreate")
            .unwrap();
        fixture
    }

    async fn artifact(&self, revision: u64) {
        let mut saved = self.db.confirmed_create(&self.receipt).unwrap();
        if revision == 1 {
            saved.ports = self.db.read(|db| Ok(vec![db.query_row("SELECT slot, host_ip, host_port, container_port FROM port_bindings WHERE spec_revision = 1", [], |row| Ok(composenest_application::state_store::PortAllocation { slot: row.get(0)?, host_ip: row.get(1)?, host_port: row.get(2)?, container_port: row.get(3)? }))?])).unwrap();
        }
        saved.spec_revision = revision;
        let image = self.db.image_resolution(ID, 1).unwrap().unwrap();
        let docker = self.probe.bind(saved.target.clone()).unwrap();
        let binds = BindStorage::new(self.root.path());
        let projection = CreateProjection::new(&self.db, &docker, &binds, &saved.project_name, ID);
        let model = projection.model(&saved, &image).unwrap();
        let artifacts = ArtifactStore::new(self.root.path(), &self.db);
        artifacts
            .publish(
                "operation",
                ArtifactInput {
                    id: format!("{ID}-r{revision}"),
                    instance_id: ID.into(),
                    spec_revision: revision,
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
        let expected = projection
            .expected(&model, &image, CONTAINER, &saved)
            .await
            .unwrap();
        fs::write(
            self.root.path().join(format!("r{revision}.json")),
            inspection(&expected, false),
        )
        .unwrap();
        fs::write(
            self.root.path().join("ready.json"),
            inspection(&expected, true),
        )
        .unwrap();
    }

    fn current(&self, revision: u64) {
        fs::copy(
            self.root.path().join(format!("r{revision}.json")),
            self.root.path().join("current.json"),
        )
        .unwrap();
    }

    async fn prepare_confirmed_runtime(
        &self,
        ports: Vec<composenest_application::state_store::PortAllocation>,
    ) {
        let mut saved = self.db.confirmed_create(&self.receipt).unwrap();
        saved.spec_revision += 1;
        saved.ports = ports;
        let image = self.db.image_resolution(ID, 1).unwrap().unwrap();
        let docker = self.probe.bind(saved.target.clone()).unwrap();
        let binds = BindStorage::new(self.root.path());
        let projection = CreateProjection::new(&self.db, &docker, &binds, &saved.project_name, ID);
        let model = projection.model(&saved, &image).unwrap();
        let expected = projection
            .expected(&model, &image, CONTAINER, &saved)
            .await
            .unwrap();
        fs::write(
            self.root.path().join("target.json"),
            inspection(&expected, false),
        )
        .unwrap();
        fs::write(
            self.root.path().join("ready.json"),
            inspection(&expected, true),
        )
        .unwrap();
    }
    async fn run(&self, action: PortRecoveryAction) -> PortRecoveryResult {
        self.try_run(true, action).await.unwrap()
    }

    async fn try_run(
        &self,
        exited: bool,
        action: PortRecoveryAction,
    ) -> Result<PortRecoveryResult, composenest_adapters::port_edit_stages::PortEditError> {
        recover_port_change(
            &self.db,
            &self.probe,
            self.root.path(),
            &OperationRunner::new(),
            &self.receipt,
            &PriorCli(exited),
            action,
        )
        .await
    }

    fn active_reservations(&self) -> i64 {
        self.db
            .read(|db| {
                Ok(db.query_row(
                    "SELECT COUNT(*) FROM port_reservations WHERE status IN ('held', 'committed')",
                    [],
                    |row| row.get(0),
                )?)
            })
            .unwrap()
    }
}

fn inspection(expected: &ExpectedContainer, ready: bool) -> Vec<u8> {
    let health = expected.healthcheck.as_ref().unwrap();
    serde_json::to_vec(&json!({
        "Id": CONTAINER, "Image": expected.image_id,
        "Config": { "Labels": { "com.docker.compose.project": expected.project, "com.docker.compose.service": "main", "io.composenest.scope": "scope", "io.composenest.instance": ID, "io.composenest.spec-revision": expected.spec_revision.to_string() }, "Env": expected.environment, "Cmd": expected.command, "Healthcheck": { "Test": health.test, "Interval": health.interval, "Timeout": health.timeout, "StartPeriod": health.start_period, "StartInterval": health.start_interval, "Retries": health.retries } },
        "HostConfig": { "PortBindings": expected.ports.iter().map(|(key, hosts)| (key.clone(), hosts.iter().map(|host| json!({"HostIp": host.host_ip, "HostPort": host.host_port})).collect::<Vec<_>>())).collect::<BTreeMap<_, _>>() },
        "Mounts": expected.mounts.iter().map(|mount| json!({ "Type": mount.kind, "Source": mount.source, "Destination": mount.destination, "RW": mount.read_write })).collect::<Vec<_>>(),
        "NetworkSettings": { "Networks": expected.networks.iter().map(|name| (name, json!({}))).collect::<BTreeMap<_, _>>() },
        "State": { "Status": if ready { "running" } else { "created" }, "Health": { "Status": if ready { "healthy" } else { "starting" } } }
    })).unwrap()
}

#[tokio::test]
async fn interrupted_application_completes_without_another_create_or_start() {
    let fixture = Fixture::new(OperationKind::EditPort).await;
    fixture.current(2);
    fixture
        .db
        .set_status("operation", OperationStatus::Executing, "recreate")
        .unwrap();
    fixture
        .db
        .record_step(&composenest_application::operation_journal::StepIntent {
            operation_id: "operation".into(),
            sequence: 1,
            attempt: 1,
            command_kind: composenest_application::operation_journal::StepCommand::ComposeCreate,
            resource_id: ID.into(),
            expected_result:
                composenest_application::operation_journal::ExpectedResult::ContainerCreated,
        })
        .unwrap();
    fixture
        .db
        .set_status("operation", OperationStatus::OutcomeUnknown, "recreate")
        .unwrap();
    let held = fixture
        .try_run(false, PortRecoveryAction::Retry)
        .await
        .unwrap();
    assert!(matches!(held, PortRecoveryResult::Held));
    assert!(matches!(
        fixture.run(PortRecoveryAction::Reconcile).await,
        PortRecoveryResult::Completed(_)
    ));
    let calls = fs::read_to_string(fixture.root.path().join("calls")).unwrap();
    assert!(!calls.contains(" create ") && !calls.contains("container start"));
}

#[tokio::test]
async fn ambiguous_ownership_artifact_and_storage_never_release_reservations() {
    for failure in ["owner", "artifact", "storage", "engine", "old-container"] {
        let fixture = Fixture::new(OperationKind::EditPort).await;
        fixture.current(2);
        match failure {
            "owner" => {
                let path = fixture.root.path().join("current.json");
                let mut data: serde_json::Value =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                data["Config"]["Labels"]["io.composenest.scope"] = json!("foreign");
                fs::write(path, serde_json::to_vec(&data).unwrap()).unwrap();
            }
            "artifact" => fs::write(
                fixture
                    .root
                    .path()
                    .join(format!("instances/{ID}/artifacts/{ID}-r2/compose.yaml")),
                "modified",
            )
            .unwrap(),
            "storage" => {
                fixture
                    .db
                    .write(|db| {
                        db.execute("UPDATE storage_allocations SET presence = 'missing'", [])?;
                        Ok(())
                    })
                    .unwrap();
            }
            "engine" => {
                let script = fs::read_to_string(&fixture.probe.executable)
                    .unwrap()
                    .replace("\"ID\":\"engine\"", "\"ID\":\"foreign\"");
                fs::write(&fixture.probe.executable, script).unwrap();
            }
            "old-container" => {
                fixture
                    .db
                    .write(|db| {
                        db.execute(
                            "INSERT INTO runtime_observations (instance_id, container_id, runtime_state, freshness) VALUES (?1, ?2, 'stopped', 'fresh') ON CONFLICT(instance_id) DO UPDATE SET container_id = excluded.container_id",
                            rusqlite::params![ID, "c".repeat(64)],
                        )?;
                        Ok(())
                    })
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let result = fixture.try_run(true, PortRecoveryAction::Retry).await;
        assert!(!matches!(result, Ok(PortRecoveryResult::Completed(_))));
        assert_eq!(fixture.active_reservations(), 2);
        let calls = fs::read_to_string(fixture.root.path().join("calls")).unwrap();
        assert!(!calls.contains(" create ") && !calls.contains("container start"));
    }
}

#[tokio::test]
async fn newly_applied_candidate_conflict_requires_confirmation_again() {
    let fixture = Fixture::new(OperationKind::Create).await;
    fixture.current(1);
    let original = fixture.db.confirmed_create(&fixture.receipt).unwrap();
    let occupied = TcpListener::bind(("127.0.0.1", original.ports[0].host_port)).unwrap();
    let PortRecoveryResult::Proposal(PortPlan::Complete(first)) = fixture
        .run(PortRecoveryAction::Propose(PortCursor::default()))
        .await
    else {
        panic!("expected proposal");
    };
    let mut ports = original.ports;
    ports[0].host_port = first["db"];
    fixture.prepare_confirmed_runtime(ports.clone()).await;
    fs::write(fixture.root.path().join("start-fail"), "").unwrap();
    assert!(
        fixture
            .try_run(
                true,
                PortRecoveryAction::Confirm {
                    candidate_revision: 1,
                    ports
                }
            )
            .await
            .is_err()
    );
    let competitor = TcpListener::bind(("127.0.0.1", first["db"])).unwrap();
    let PortRecoveryResult::Proposal(PortPlan::Complete(next)) = fixture
        .run(PortRecoveryAction::Propose(PortCursor::default()))
        .await
    else {
        panic!("expected renewed proposal");
    };
    assert!(next["db"] > first["db"]);
    assert_eq!(
        fixture.db.pending_ports("operation").unwrap()[0].host_port,
        first["db"]
    );
    assert_eq!(fixture.active_reservations(), 2);
    fs::remove_file(fixture.root.path().join("start-fail")).unwrap();
    let mut ports = fixture.db.confirmed_create(&fixture.receipt).unwrap().ports;
    ports[0].host_port = next["db"];
    fixture.prepare_confirmed_runtime(ports.clone()).await;
    assert!(matches!(
        fixture
            .run(PortRecoveryAction::Confirm {
                candidate_revision: 2,
                ports
            })
            .await,
        PortRecoveryResult::Completed(_)
    ));
    assert_eq!(fixture.active_reservations(), 1);
    drop((occupied, competitor));
}

#[tokio::test]
async fn retry_and_restore_apply_the_selected_artifact_without_starting() {
    for restore in [false, true] {
        let fixture = Fixture::new(OperationKind::EditPort).await;
        fixture.current(if restore { 2 } else { 1 });
        let target = if restore { 1 } else { 2 };
        fs::copy(
            fixture.root.path().join(format!("r{target}.json")),
            fixture.root.path().join("target.json"),
        )
        .unwrap();
        assert!(matches!(
            fixture
                .run(if restore {
                    PortRecoveryAction::Restore
                } else {
                    PortRecoveryAction::Retry
                })
                .await,
            PortRecoveryResult::Completed(_)
        ));
        let applied: u64 = fixture
            .db
            .read(|db| {
                Ok(
                    db.query_row("SELECT applied_spec_revision FROM instances", [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(applied, target);
        if restore {
            let mut receipt = fixture.receipt.clone();
            receipt.request_id = "next-edit".into();
            receipt.operation_id = "next-operation".into();
            receipt.confirmed_revision = 2;
            let ports = fixture.db.read(|db| Ok(vec![db.query_row("SELECT slot, host_ip, host_port, container_port FROM port_bindings WHERE spec_revision = 2", [], |row| Ok(composenest_application::state_store::PortAllocation { slot: row.get(0)?, host_ip: row.get(1)?, host_port: row.get(2)?, container_port: row.get(3)? }))?])).unwrap();
            fixture
                .db
                .begin_port_edit(&PortEditRequest {
                    receipt: receipt.clone(),
                    expected_instance_revision: 2,
                    old_spec_revision: 1,
                    ports,
                })
                .unwrap();
            assert_eq!(
                fixture.db.confirmed_create(&receipt).unwrap().spec_revision,
                3
            );
        }
        let calls = fs::read_to_string(fixture.root.path().join("calls")).unwrap();
        assert!(!calls.contains("container start"));
    }
}

#[tokio::test]
async fn creation_conflict_proposes_without_changes_and_rechecks_confirmation() {
    for kind in [OperationKind::Create, OperationKind::Clone] {
        let fixture = Fixture::new(kind).await;
        fixture.current(1);
        let old = fixture.db.confirmed_create(&fixture.receipt).unwrap();
        let occupied = TcpListener::bind(("127.0.0.1", old.ports[0].host_port)).unwrap();
        let PortRecoveryResult::Proposal(PortPlan::Complete(proposal)) = fixture
            .run(PortRecoveryAction::Propose(PortCursor::default()))
            .await
        else {
            panic!("expected complete proposal");
        };
        assert!(proposal["db"] > old.ports[0].host_port);
        let competing = TcpListener::bind(("127.0.0.1", proposal["db"])).unwrap();
        let mut ports = old.ports;
        ports[0].host_port = proposal["db"];
        assert!(
            fixture
                .try_run(
                    true,
                    PortRecoveryAction::Confirm {
                        candidate_revision: 1,
                        ports
                    }
                )
                .await
                .is_err()
        );
        let PortRecoveryResult::Proposal(PortPlan::Complete(next)) = fixture
            .run(PortRecoveryAction::Propose(PortCursor::default()))
            .await
        else {
            panic!("expected renewed proposal");
        };
        assert!(next["db"] > proposal["db"]);
        assert!(fixture.db.pending_ports("operation").is_err());
        let calls = fs::read_to_string(fixture.root.path().join("calls")).unwrap();
        assert!(!calls.contains(" create ") && !calls.contains("container start"));
        let mut ports = fixture.db.confirmed_create(&fixture.receipt).unwrap().ports;
        ports[0].host_port = next["db"];
        fixture.prepare_confirmed_runtime(ports.clone()).await;
        assert!(matches!(
            fixture
                .run(PortRecoveryAction::Confirm {
                    candidate_revision: 1,
                    ports
                })
                .await,
            PortRecoveryResult::Completed(_)
        ));
        let result: (String, String, String, String) = fixture.db.read(|db| Ok(db.query_row("SELECT o.phase, s.inputs_json, a.resource_identity, a.initialization FROM operations o JOIN instance_specs s ON s.instance_id = o.instance_id AND s.revision = o.new_spec_revision JOIN storage_allocations a ON a.instance_id = o.instance_id", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?)).unwrap();
        assert_eq!(result.0, "ready");
        assert_eq!(
            result.1,
            fixture
                .db
                .read(|db| Ok(db.query_row(
                    "SELECT inputs_json FROM instance_specs WHERE revision = 1",
                    [],
                    |row| row.get::<_, String>(0)
                )?))
                .unwrap()
        );
        assert_eq!(result.2, format!("data/{ID}/data"));
        assert_eq!(result.3, "ready_observed");
        drop((occupied, competing));
    }
}
