//! Read-only desktop diagnosis using the same fixed target as runtime operations.

use crate::create_commands::CreateBackend;
use composenest_adapters::{
    SystemClock,
    docker_target::{Check, DockerDiagnosis, DockerProbe},
    sqlite::{DatabaseError, DatabaseWorker},
};
use composenest_application::{
    Clock, RequestContext, ResponseEnvelope,
    runtime_diagnostics::{DiagnosticCheck, RuntimeDiagnosis},
    state_store::StateStore,
};
use std::{io, path::PathBuf, sync::Arc};
use tauri::State;

fn status(check: Check) -> String {
    match check {
        Check::Ready => "ready",
        Check::Missing => "missing",
        Check::Unavailable => "unavailable",
        Check::Unsupported => "unsupported",
        Check::Changed => "changed",
        Check::PermissionDenied => "permission_denied",
    }
    .into()
}

/// Observes prerequisites without registering targets or changing Docker resources.
#[tauri::command]
pub async fn diagnose_runtime(
    request: RequestContext,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<RuntimeDiagnosis>, ()> {
    if let Err(error) = request.validate() {
        return Ok(ResponseEnvelope::failure(request.request_id, *error));
    }
    let report = observe_runtime(
        &state.database,
        &state.scope,
        state.home.join(".docker"),
        composenest_adapters::docker_executable::discover_checked(&state.home),
    )
    .await;
    Ok(ResponseEnvelope::success(request.request_id, report))
}

async fn observe_runtime(
    database: &DatabaseWorker,
    scope: &str,
    config_directory: PathBuf,
    discovery: io::Result<Option<PathBuf>>,
) -> RuntimeDiagnosis {
    let mut root = match database.check_root_access() {
        Ok(()) => Check::Ready,
        Err(DatabaseError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Check::PermissionDenied
        }
        Err(DatabaseError::UnsafePath(_)) => Check::Unsupported,
        Err(_) => Check::Unavailable,
    };
    let (registered, store_available) = match database.runtime_target(scope) {
        Ok(target) => (target, true),
        Err(_) => {
            if root == Check::Ready {
                root = Check::Unavailable;
            }
            (None, false)
        }
    };
    let report = match discovery {
        Ok(Some(executable)) => {
            DockerProbe {
                executable,
                directory: database.management_root().to_path_buf(),
                config_directory,
            }
            .diagnose(registered.as_ref())
            .await
        }
        Ok(None) => DockerDiagnosis {
            cli: Check::Missing,
            compose: Check::Missing,
            ..DockerDiagnosis::default()
        },
        Err(error) => DockerDiagnosis {
            cli: if error.kind() == io::ErrorKind::PermissionDenied {
                Check::PermissionDenied
            } else {
                Check::Unavailable
            },
            ..DockerDiagnosis::default()
        },
    };
    let checks = [
        ("cli", report.cli, report.cli_version),
        ("compose", report.compose, report.compose_version),
        ("engine", report.engine, report.engine_version),
        ("linux", report.linux_containers, None),
        ("platform", report.platform, None),
        ("endpoint", report.endpoint, None),
        ("root", root, None),
    ]
    .into_iter()
    .map(|(name, check, version)| DiagnosticCheck {
        name: name.into(),
        status: status(check),
        version,
    })
    .collect();
    let target_status = match (&registered, &report.engine_id) {
        (None, _) if store_available => "unregistered",
        (Some(target), Some(id)) if target.engine_id != *id => "changed",
        (Some(_), Some(_)) if report.engine == Check::Ready => "verified",
        _ => "unverified",
    };
    RuntimeDiagnosis {
        observed_at: SystemClock.unix_seconds(),
        checks,
        endpoint: report
            .resolved_endpoint
            .or_else(|| registered.as_ref().map(|target| target.endpoint.clone())),
        context_name: report.context_name,
        platform: report.observed_platform,
        engine_id: report.engine_id,
        registered_engine_id: registered.map(|target| target.engine_id),
        target_status: target_status.into(),
        management_root: database.management_root().to_string_lossy().into_owned(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn database() -> (tempfile::TempDir, DatabaseWorker) {
        let root = tempfile::tempdir().unwrap();
        for path in [
            root.path().to_path_buf(),
            root.path().join("state"),
            root.path().join("locks"),
        ] {
            fs::create_dir_all(&path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let database = DatabaseWorker::start(root.path()).unwrap();
        (root, database)
    }

    #[tokio::test]
    async fn discovery_permission_failure_is_not_reported_as_missing() {
        let (root, database) = database();
        let report = observe_runtime(
            &database,
            "scope",
            root.path().to_path_buf(),
            Err(io::ErrorKind::PermissionDenied.into()),
        )
        .await;
        assert_eq!(report.checks[0].status, "permission_denied");
        assert_eq!(report.checks[1].status, "unavailable");
        let missing =
            observe_runtime(&database, "scope", root.path().to_path_buf(), Ok(None)).await;
        assert_eq!(missing.checks[0].status, "missing");
    }

    #[tokio::test]
    async fn unreadable_target_store_preserves_independent_docker_observations() {
        let (root, database) = database();
        database
            .write(|db| {
                db.execute("DROP TABLE runtime_targets", [])?;
                Ok(())
            })
            .unwrap();
        let executable = root.path().join("docker");
        let architecture = if std::env::consts::ARCH == "aarch64" {
            "arm64"
        } else {
            "amd64"
        };
        fs::write(&executable, format!(r#"#!/bin/sh
if [ "$1" = '--host' ]; then shift 2; fi
case "$1 $2" in
  '--version ') printf 'Docker version 29.8.1\n' ;;
  'compose version') printf 'Docker Compose version v5.5.1\n' ;;
  'context show') printf 'observed-context\n' ;;
  'context inspect') printf '"unix:///tmp/observed.sock"\n' ;;
  'info --format') printf '{{"ID":"observed-engine","ServerVersion":"29.8.1","OSType":"linux","Architecture":"{architecture}"}}\n' ;;
  *) exit 1 ;;
esac
"#)).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let report = observe_runtime(
            &database,
            "scope",
            root.path().to_path_buf(),
            Ok(Some(executable)),
        )
        .await;
        assert!(
            report
                .checks
                .iter()
                .filter(|check| check.name != "root")
                .all(|check| check.status == "ready")
        );
        assert_eq!(report.checks.last().unwrap().status, "unavailable");
        assert_eq!(report.target_status, "unverified");
        assert_eq!(report.engine_id.as_deref(), Some("observed-engine"));
        assert!(report.registered_engine_id.is_none());
    }
}
