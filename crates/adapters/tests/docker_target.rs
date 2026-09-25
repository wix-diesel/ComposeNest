#![cfg(unix)]

use std::{ffi::OsString, fs, os::unix::fs::PermissionsExt, time::Duration};

use composenest_adapters::docker_target::{Check, DockerProbe, TargetError};
use tempfile::TempDir;

fn fixture() -> (TempDir, DockerProbe) {
    let root = tempfile::tempdir().expect("temporary directory");
    let executable = root.path().join("docker mock.sh");
    let script = r#"#!/bin/sh
if [ "$1" = '--host' ]; then host="$2"; shift 2; else host=''; fi
case "$1 $2" in
  '--version ')
    printf 'Docker version 29.8.1, build mock\n' ;;
  'compose version')
    printf 'Docker Compose version v5.5.1\n' ;;
  'context inspect')
    if [ -n "$host" ]; then exit 1; fi
    printf '"unix:///tmp/docker-local.sock"\n' ;;
  'info --format')
    if [ "$host" != 'unix:///tmp/docker-local.sock' ]; then exit 1; fi
    if [ -f engine-down ]; then exit 1; fi
    id=$(/bin/cat engine-id)
    printf '{"ID":"%s","OSType":"linux","Architecture":"arm64"}\n' "$id" ;;
  'compose up')
    printf '%s\n' "$host" > changed-host ;;
  *) exit 1 ;;
esac
"#;
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => panic!("unsupported test architecture: {other}"),
    };
    fs::write(
        &executable,
        script.replace(
            "\"Architecture\":\"arm64\"",
            &format!("\"Architecture\":\"{architecture}\""),
        ),
    )
    .expect("script");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).expect("permissions");
    fs::write(root.path().join("engine-id"), "engine-a").expect("engine ID");
    let probe = DockerProbe {
        executable,
        directory: root.path().to_path_buf(),
        config_directory: root.path().to_path_buf(),
    };
    (root, probe)
}

#[tokio::test]
async fn engine_architecture_is_normalized_for_template_platforms() {
    let (_root, probe) = fixture();
    let script = fs::read_to_string(&probe.executable).unwrap();
    for (reported, expected) in [("x86_64", "linux/amd64"), ("aarch64", "linux/arm64")] {
        fs::write(
            &probe.executable,
            script.replace(
                &format!(r#""Architecture":"{}""#, match std::env::consts::ARCH {
                    "x86_64" => "amd64",
                    "aarch64" => "arm64",
                    _ => unreachable!(),
                }),
                &format!(r#""Architecture":"{reported}""#),
            ),
        )
        .unwrap();
        let diagnosis = probe.diagnose(None).await;
        assert_eq!(diagnosis.observed_platform.as_deref(), Some(expected));
        if reported == std::env::consts::ARCH {
            assert_eq!(diagnosis.platform, Check::Ready);
            assert_eq!(
                diagnosis.target("target".into(), "scope".into()).unwrap().platform,
                expected
            );
        }
    }
}

#[tokio::test]
async fn diagnosis_distinguishes_missing_stopped_and_changed_engine() {
    let (root, probe) = fixture();
    let diagnosis = probe.diagnose(None).await;
    assert!(diagnosis.is_ready());
    assert_eq!(
        diagnosis.resolved_endpoint.as_deref(),
        Some("unix:///tmp/docker-local.sock")
    );
    let target = diagnosis
        .target("target".into(), "scope".into())
        .expect("target");

    fs::write(root.path().join("engine-down"), "").expect("stop marker");
    let stopped = probe.diagnose(Some(&target)).await;
    assert_eq!(stopped.engine, Check::Unavailable);
    assert_eq!(stopped.cli, Check::Ready);
    fs::remove_file(root.path().join("engine-down")).expect("remove marker");

    fs::write(root.path().join("engine-id"), "engine-b").expect("change Engine");
    let changed = probe.diagnose(Some(&target)).await;
    assert_eq!(changed.engine, Check::Changed);
    assert!(!changed.is_ready());

    fs::remove_file(&probe.executable).expect("remove CLI");
    let missing = probe.diagnose(Some(&target)).await;
    assert_eq!(missing.cli, Check::Missing);
    assert_eq!(missing.compose, Check::Missing);
}

#[tokio::test]
async fn fixed_target_rejects_change_after_engine_replacement() {
    let (root, probe) = fixture();
    let target = probe
        .diagnose(None)
        .await
        .target("target".into(), "scope".into())
        .unwrap();
    let bound = probe.bind(target).expect("bind");
    fs::write(root.path().join("engine-id"), "engine-b").expect("change Engine");
    let result = bound
        .change(
            &[OsString::from("compose"), OsString::from("up")],
            Duration::from_secs(2),
        )
        .await;
    assert!(matches!(result, Err(TargetError::Changed)));
    assert!(!root.path().join("changed-host").exists());
}

#[tokio::test]
async fn remote_context_is_rejected_before_engine_access() {
    let (root, probe) = fixture();
    fs::write(
        &probe.executable,
        fs::read_to_string(&probe.executable).unwrap().replace(
            "unix:///tmp/docker-local.sock\"\\n'",
            "ssh://remote.example\"\\n'",
        ),
    )
    .expect("remote context");
    let result = probe.diagnose(None).await;
    assert_eq!(result.endpoint, Check::Unsupported);
    assert_eq!(result.engine, Check::Unavailable);
    assert!(!root.path().join("changed-host").exists());
}

#[tokio::test]
async fn old_cli_version_is_reported_as_unsupported() {
    let (_root, probe) = fixture();
    let script = fs::read_to_string(&probe.executable).unwrap();
    fs::write(
        &probe.executable,
        script.replace("Docker version 29.8.1", "Docker version 28.0.0"),
    )
    .expect("old CLI");
    let report = probe.diagnose(None).await;
    assert_eq!(report.cli, Check::Unsupported);
    assert_eq!(report.compose, Check::Ready);
    assert_eq!(report.engine, Check::Ready);
    assert!(!report.is_ready());
}

#[tokio::test]
async fn registered_target_ignores_later_context_switch() {
    let (_root, probe) = fixture();
    let target = probe
        .diagnose(None)
        .await
        .target("target".into(), "scope".into())
        .unwrap();
    let script = fs::read_to_string(&probe.executable).unwrap();
    fs::write(
        &probe.executable,
        script.replace(
            "unix:///tmp/docker-local.sock\"\\n'",
            "ssh://remote.example\"\\n'",
        ),
    )
    .expect("switch context");
    let diagnosis = probe.diagnose(Some(&target)).await;
    assert!(diagnosis.is_ready());
    assert_eq!(
        diagnosis.resolved_endpoint.as_deref(),
        Some(target.endpoint.as_str())
    );
}
