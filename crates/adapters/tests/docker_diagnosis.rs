//! Opt-in, read-only diagnosis against the user's installed Docker runtime.

use composenest_adapters::{docker_executable, docker_target::DockerProbe};
use std::{env, path::PathBuf};

#[tokio::test]
#[ignore = "requires an installed Docker CLI, Compose plugin, and running local Linux Engine"]
async fn installed_docker_desktop_is_ready_in_isolated_environment() {
    let home = PathBuf::from(
        env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).expect("home directory"),
    );
    let executable = docker_executable::discover_checked(&home)
        .expect("Docker discovery")
        .expect("installed Docker CLI");
    let directory = tempfile::tempdir().unwrap();
    let probe = DockerProbe {
        executable,
        directory: directory.path().into(),
        config_directory: home.join(".docker"),
    };
    let report = probe.diagnose(None).await;
    assert!(report.is_ready(), "{report:?}");
    assert!(
        report
            .target("diagnosis-test".into(), "test-scope".into())
            .is_some()
    );
}
