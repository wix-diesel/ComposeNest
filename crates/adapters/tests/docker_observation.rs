#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt};

use composenest_adapters::docker_observation::{ExpectedContainer, Ownership};
use composenest_adapters::docker_target::DockerProbe;
use composenest_domain::instance::{ObservationFailure, RuntimeStatus};

#[tokio::test]
async fn absence_requires_successful_listing_on_registered_engine() {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("docker");
    fs::write(
        &executable,
        r#"#!/bin/sh
if [ "$1" != '--host' ] || [ "$2" != 'unix:///tmp/composenest-test.sock' ]; then exit 1; fi
shift 2
if [ "$1" = info ]; then
  if [ -f engine-down ]; then exit 1; fi
  printf '{"ID":"engine-a"}\n'
elif [ "$1" = container ] && [ "$2" = inspect ]; then
  if [ -f inspect.json ]; then
    case "$4" in *'{{json .}}'*) exit 1 ;; esac
    /bin/cat inspect.json
  else
    exit 1
  fi
elif [ "$1" = container ] && [ "$2" = ls ]; then
  if [ -f listing-down ]; then exit 1; fi
  if [ -f still-present ]; then printf '%064d\n' 0; fi
else
  exit 1
fi
"#,
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let probe = DockerProbe {
        executable,
        directory: root.path().into(),
        config_directory: root.path().into(),
    };
    let target = composenest_application::state_store::RuntimeTarget {
        id: "target".into(),
        scope_id: "scope".into(),
        endpoint: "unix:///tmp/composenest-test.sock".into(),
        engine_id: "engine-a".into(),
        platform: "linux/amd64".into(),
    };
    let bound = probe.bind(target).unwrap();
    let expected = ExpectedContainer {
        container_id: "a".repeat(64),
        project: "project".into(),
        scope: "scope".into(),
        instance: "instance".into(),
        image_id: "image".into(),
        mounts: vec![],
        ports: Default::default(),
        networks: vec![],
        command: vec![],
        environment: vec![],
        healthcheck: None,
        spec_revision: 1,
    };
    let absent = bound.observe(&expected).await;
    assert_eq!(absent.status, RuntimeStatus::Absent);
    assert_eq!(absent.ownership, Ownership::Unknown);
    assert!(!absent.can_change(absent.observed_at_unix_seconds, 10));

    fs::write(root.path().join("listing-down"), "").unwrap();
    assert_eq!(
        bound.observe(&expected).await.status,
        RuntimeStatus::Unknown(ObservationFailure::Unreachable)
    );
    fs::remove_file(root.path().join("listing-down")).unwrap();
    fs::write(root.path().join("still-present"), "").unwrap();
    assert_eq!(
        bound.observe(&expected).await.status,
        RuntimeStatus::Unknown(ObservationFailure::Unreachable)
    );
    fs::remove_file(root.path().join("still-present")).unwrap();
    fs::write(root.path().join("engine-down"), "").unwrap();
    assert_eq!(
        bound.observe(&expected).await.status,
        RuntimeStatus::Unknown(ObservationFailure::Unreachable)
    );
}

#[tokio::test]
async fn projected_inspect_accepts_large_environment_without_health_logs() {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("docker");
    fs::write(&executable, r#"#!/bin/sh
shift 2
if [ "$1" = info ]; then
  printf '{"ID":"engine-a"}\n'
elif [ "$1" = container ] && [ "$2" = inspect ]; then
  case "$4" in *'{{json .}}'*) exit 1 ;; esac
  /bin/cat inspect.json
else
  exit 1
fi
"#).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let probe = DockerProbe { executable, directory: root.path().into(), config_directory: root.path().into() };
    let target = composenest_application::state_store::RuntimeTarget {
        id: "target".into(), scope_id: "scope".into(), endpoint: "unix:///tmp/composenest-test.sock".into(), engine_id: "engine-a".into(), platform: "linux/amd64".into(),
    };
    let expected = ExpectedContainer {
        container_id: "a".repeat(64), project: "project".into(), scope: "scope".into(), instance: "instance".into(), image_id: "image".into(), mounts: vec![], ports: Default::default(), networks: vec![], command: vec![], environment: vec![format!("BIG={}", "X".repeat(96 * 1024))], healthcheck: None, spec_revision: 1,
    };
    let json = serde_json::json!({
        "Id": expected.container_id, "Image": "image",
        "Config": {"Labels": {"com.docker.compose.project":"project", "com.docker.compose.service":"main", "io.composenest.scope":"scope", "io.composenest.instance":"instance", "io.composenest.spec-revision":"1"}, "Cmd":null, "Env":expected.environment, "Healthcheck":null},
        "Mounts":[], "HostConfig":{"PortBindings":null}, "NetworkSettings":{"Networks":{}}, "State":{"Status":"running", "Health":{"Status":null}}
    });
    fs::write(root.path().join("inspect.json"), serde_json::to_vec(&json).unwrap()).unwrap();
    let observation = probe.bind(target).unwrap().observe(&expected).await;
    assert_eq!(observation.ownership, Ownership::Verified);
    assert_eq!(observation.configuration_matches, Some(true));
    assert_eq!(observation.status, RuntimeStatus::Preparing);
}
