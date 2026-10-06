//! Read-only desktop diagnosis using the same fixed target as runtime operations.

use crate::create_commands::CreateBackend;
use composenest_adapters::{
    SystemClock,
    docker_target::{Check, DockerDiagnosis, DockerProbe},
    sqlite::DatabaseError,
};
use composenest_application::{
    Clock, RequestContext, ResponseEnvelope,
    runtime_diagnostics::{DiagnosticCheck, RuntimeDiagnosis},
    state_store::StateStore,
};
use std::sync::Arc;
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
    let mut root = match state.database.check_root_access() {
        Ok(()) => Check::Ready,
        Err(DatabaseError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Check::PermissionDenied
        }
        Err(DatabaseError::UnsafePath(_)) => Check::Unsupported,
        Err(_) => Check::Unavailable,
    };
    let (registered, store_available) = match state.database.runtime_target(&state.scope) {
        Ok(target) => (target, true),
        Err(_) => {
            if root == Check::Ready {
                root = Check::Unavailable;
            }
            (None, false)
        }
    };
    let probe = composenest_adapters::docker_executable::discover(&state.home).map(|executable| {
        DockerProbe {
            executable,
            directory: state.database.management_root().to_path_buf(),
            config_directory: state.home.join(".docker"),
        }
    });
    let report = match (&probe, store_available) {
        (Some(probe), true) => probe.diagnose(registered.as_ref()).await,
        (None, true) => DockerDiagnosis {
            cli: Check::Missing,
            compose: Check::Missing,
            ..DockerDiagnosis::default()
        },
        (_, false) => DockerDiagnosis::default(),
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
    Ok(ResponseEnvelope::success(
        request.request_id,
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
            management_root: state
                .database
                .management_root()
                .to_string_lossy()
                .into_owned(),
        },
    ))
}
