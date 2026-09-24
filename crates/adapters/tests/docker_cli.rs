#![cfg(unix)]

use std::{ffi::OsString, fs, os::unix::fs::PermissionsExt, time::Duration};

use composenest_adapters::docker_cli::{CliError, CommandKind, DockerCli};
use tempfile::TempDir;

fn fixture(script: &str) -> (TempDir, DockerCli) {
    let root = tempfile::tempdir().expect("temporary directory");
    let executable = root.path().join("docker mock 日本語.sh");
    let staged = root.path().join("staged-script");
    fs::write(&staged, format!("#!/bin/sh\nshift 2\n{script}\n")).expect("write fixture");
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o700)).expect("permissions");
    fs::rename(&staged, &executable).expect("publish fixture");
    let cli = DockerCli::new(
        executable,
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        OsString::from("unix:///var/run/docker.sock"),
    )
    .expect("CLI configuration");
    (root, cli)
}

#[tokio::test]
async fn arguments_are_not_expanded_and_environment_is_fixed() {
    let (_root, cli) = fixture(
        "printf '%s\\n' \"$1\" \"$2\" \"$DOCKER_HOST\" \"$DOCKER_CONFIG\" \"$COMPOSE_DISABLE_ENV_FILE\" \"$PWD\"",
    );
    let args = [
        OsString::from("空白 and $HOME"),
        OsString::from("a;*'\\\"b"),
    ];
    let result = cli
        .run(CommandKind::Read, &args, Duration::from_secs(3))
        .await
        .expect("run");
    assert!(result.status.expect("exit status").success());
    assert!(!result.outcome_unknown);
    let output = String::from_utf8(result.stdout.bytes).expect("UTF-8");
    assert!(output.starts_with("空白 and $HOME\na;*'\\\"b\n\n"));
    assert!(output.contains("\n1\n"));
    assert!(output.ends_with("\n"));
}

#[tokio::test]
async fn large_output_is_drained_and_bounded() {
    let (_root, cli) = fixture(
        "i=0; while [ $i -lt 10000 ]; do printf 'abcdefghijklmnop' ; printf 'stderr chunk' >&2; i=$((i+1)); done",
    );
    let result = cli
        .run(CommandKind::Read, &[], Duration::from_secs(10))
        .await
        .expect("run");
    assert!(result.status.expect("exit status").success());
    assert_eq!(result.stdout.bytes.len(), 64 * 1024);
    assert_eq!(result.stderr.bytes.len(), 64 * 1024);
    assert!(result.stdout.truncated && result.stderr.truncated);
}

#[tokio::test]
async fn timeout_is_unknown_and_reaped_before_next_change() {
    let (_root, cli) = fixture("if [ \"$1\" = slow ]; then /bin/sleep 2; else printf done; fi");
    let result = cli
        .run(
            CommandKind::Change,
            &["slow".into()],
            Duration::from_millis(50),
        )
        .await
        .expect("timeout");
    assert!(result.outcome_unknown);
    assert!(result.status.is_none());
    let next = cli
        .run(
            CommandKind::Change,
            &["fast".into()],
            Duration::from_secs(3),
        )
        .await
        .expect("next run");
    assert_eq!(next.stdout.bytes, b"done");
}

#[tokio::test]
async fn cancelled_caller_does_not_release_the_change_gate() {
    let (root, cli) = fixture(
        "if [ \"$1\" = slow ]; then printf started > marker; /bin/sleep 1; else printf done; fi",
    );
    let cli = std::sync::Arc::new(cli);
    let first_cli = cli.clone();
    let first = tokio::spawn(async move {
        first_cli
            .run(
                CommandKind::Change,
                &["slow".into()],
                Duration::from_secs(3),
            )
            .await
    });
    for _ in 0..100 {
        if root.path().join("marker").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(root.path().join("marker").exists());
    first.abort();
    let started = std::time::Instant::now();
    let next = cli
        .run(
            CommandKind::Change,
            &["fast".into()],
            Duration::from_secs(3),
        )
        .await
        .expect("next run");
    assert!(started.elapsed() >= Duration::from_millis(500));
    assert_eq!(next.stdout.bytes, b"done");
}

#[tokio::test]
async fn caller_cannot_override_the_fixed_target() {
    let (_root, cli) = fixture("printf unexpected");
    let result = cli
        .run(
            CommandKind::Read,
            &["--host=tcp://example:2375".into()],
            Duration::from_secs(1),
        )
        .await;
    assert!(matches!(result, Err(CliError::InvalidConfiguration(_))));
}
