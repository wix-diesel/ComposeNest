//! Docker readiness checks and commands bound to a persisted Engine identity.

use std::{ffi::OsString, path::PathBuf, time::Duration};

use composenest_application::state_store::RuntimeTarget;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::docker_cli::{CliError, CliOutcome, CommandKind, DockerCli, is_local_endpoint};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Result of an individual Docker prerequisite check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// The prerequisite is available and supported.
    Ready,
    /// The CLI or Compose component is absent.
    Missing,
    /// The component exists but cannot currently be contacted.
    Unavailable,
    /// The detected endpoint or Engine is outside the v1 support range.
    Unsupported,
    /// The Engine differs from the one registered for this scope.
    Changed,
    /// Access to the component was explicitly denied by the operating system.
    PermissionDenied,
}

/// Independently reported Docker prerequisites; no raw CLI output is exposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerDiagnosis {
    /// Docker CLI installation and version check.
    pub cli: Check,
    /// Compose plugin installation and version check.
    pub compose: Check,
    /// Whether the configured context resolves to a local endpoint.
    pub endpoint: Check,
    /// Engine connection and stable ID check.
    pub engine: Check,
    /// Whether the Engine runs Linux containers.
    pub linux_containers: Check,
    /// Whether the Engine architecture matches this application's host architecture.
    pub platform: Check,
    /// Resolved local endpoint, if accepted.
    pub resolved_endpoint: Option<String>,
    /// Engine ID, if successfully observed.
    pub engine_id: Option<String>,
    /// Observed Engine OS and architecture, if available.
    pub observed_platform: Option<String>,
    /// Parsed CLI version, never raw command output.
    pub cli_version: Option<String>,
    /// Parsed Compose version, never raw command output.
    pub compose_version: Option<String>,
    /// Parsed Engine version from the observed server.
    pub engine_version: Option<String>,
    /// Current context name; informational only and never identity evidence.
    pub context_name: Option<String>,
}

impl Default for DockerDiagnosis {
    fn default() -> Self {
        Self {
            cli: Check::Unavailable,
            compose: Check::Unavailable,
            endpoint: Check::Unavailable,
            engine: Check::Unavailable,
            linux_containers: Check::Unavailable,
            platform: Check::Unavailable,
            resolved_endpoint: None,
            engine_id: None,
            observed_platform: None,
            cli_version: None,
            compose_version: None,
            engine_version: None,
            context_name: None,
        }
    }
}

impl DockerDiagnosis {
    /// Returns whether every prerequisite has been verified.
    pub fn is_ready(&self) -> bool {
        [
            self.cli,
            self.compose,
            self.endpoint,
            self.engine,
            self.linux_containers,
            self.platform,
        ]
        .iter()
        .all(|check| *check == Check::Ready)
    }

    /// Creates a target only from a complete, supported diagnosis.
    pub fn target(&self, id: String, scope_id: String) -> Option<RuntimeTarget> {
        if !self.is_ready() {
            return None;
        }
        Some(RuntimeTarget {
            id,
            scope_id,
            endpoint: self.resolved_endpoint.clone()?,
            engine_id: self.engine_id.clone()?,
            platform: self.observed_platform.clone()?,
        })
    }
}

/// Input paths used for diagnosis; all process calls use an isolated environment.
pub struct DockerProbe {
    /// Absolute Docker executable path.
    pub executable: PathBuf,
    /// Absolute working directory with no untrusted Compose files.
    pub directory: PathBuf,
    /// Absolute Docker configuration directory containing the user's context metadata.
    pub config_directory: PathBuf,
}

impl DockerProbe {
    /// Diagnoses the user's current context without changing it.
    ///
    /// An existing target always takes precedence over current context for Engine checks.
    pub async fn diagnose(&self, registered: Option<&RuntimeTarget>) -> DockerDiagnosis {
        let mut report = DockerDiagnosis::default();
        if !self.executable.is_file() {
            report.cli = Check::Missing;
            report.compose = Check::Missing;
            return report;
        }
        let discovery = match self.cli(default_local_endpoint()) {
            Ok(cli) => cli,
            Err(_) => return report,
        };
        (report.cli, report.cli_version) = check_version(
            &discovery,
            &["--version".into()],
            "Docker version",
            [29, 8, 1],
        )
        .await;
        if matches!(
            report.cli,
            Check::Missing | Check::Unavailable | Check::PermissionDenied
        ) {
            return report;
        }
        (report.compose, report.compose_version) = check_version(
            &discovery,
            &["compose".into(), "version".into()],
            "Docker Compose version",
            [5, 5, 1],
        )
        .await;
        report.context_name = discovery
            .current_context_name()
            .await
            .ok()
            .and_then(|outcome| {
                (outcome.status.is_some_and(|status| status.success()) && !outcome.stdout.truncated)
                    .then(|| String::from_utf8(outcome.stdout.bytes).ok())
                    .flatten()
                    .map(|name| name.trim().to_owned())
                    .filter(|name| {
                        !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)
                    })
            });
        let endpoint = match registered {
            Some(target) => target.endpoint.clone(),
            None => match read_context_endpoint(&discovery).await {
                Some(endpoint) => endpoint,
                None => return report,
            },
        };
        report.endpoint = if is_local_endpoint(endpoint.as_ref()) {
            Check::Ready
        } else {
            Check::Unsupported
        };
        if report.endpoint != Check::Ready {
            return report;
        }
        report.resolved_endpoint = Some(endpoint.clone());
        let Ok(cli) = self.cli(OsString::from(endpoint)) else {
            return report;
        };
        let info = match engine_info_checked(&cli).await {
            Ok(info) => info,
            Err(check) => {
                report.engine = check;
                return report;
            }
        };
        let Some(id) = info
            .get("ID")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return report;
        };
        report.engine_id = Some(id.to_owned());
        report.engine_version = info
            .get("ServerVersion")
            .and_then(Value::as_str)
            .and_then(|version| parse_version(version.as_bytes(), ""))
            .map(version_text);
        report.engine = if registered.is_some_and(|target| target.engine_id != id) {
            Check::Changed
        } else if info
            .get("ServerVersion")
            .and_then(Value::as_str)
            .and_then(|version| parse_version(version.as_bytes(), ""))
            .is_some_and(|version| version >= [29, 8, 1])
        {
            Check::Ready
        } else {
            Check::Unsupported
        };
        let os = info.get("OSType").and_then(Value::as_str).unwrap_or("");
        let arch = info
            .get("Architecture")
            .and_then(Value::as_str)
            .unwrap_or("");
        report.observed_platform = Some(format!("{os}/{}", canonical_architecture(arch)));
        report.linux_containers = if os == "linux" {
            Check::Ready
        } else {
            Check::Unsupported
        };
        report.platform = if supported_architecture(arch) {
            Check::Ready
        } else {
            Check::Unsupported
        };
        report
    }

    fn cli(&self, endpoint: OsString) -> Result<DockerCli, CliError> {
        DockerCli::new(
            self.executable.clone(),
            self.directory.clone(),
            self.config_directory.clone(),
            endpoint,
        )
    }

    /// Opens an operation boundary against the endpoint saved in the ledger.
    pub fn bind(&self, target: RuntimeTarget) -> Result<BoundDocker, CliError> {
        let cli = self.cli(OsString::from(&target.endpoint))?;
        Ok(BoundDocker {
            cli,
            engine_id: target.engine_id,
            gate: Mutex::new(()),
        })
    }
}

/// Runs changes only after checking the registered Engine ID at the fixed endpoint.
pub struct BoundDocker {
    cli: DockerCli,
    engine_id: String,
    gate: Mutex<()>,
}

/// A rejected change caused by an unavailable or different Engine.
#[derive(Debug, thiserror::Error)]
pub enum TargetError {
    /// The Engine identity could not be confirmed.
    #[error("Docker Engine identity could not be confirmed")]
    Unavailable,
    /// The endpoint now resolves to a different Engine.
    #[error("Docker Engine identity changed")]
    Changed,
    /// The Docker CLI failed to execute safely.
    #[error(transparent)]
    Cli(#[from] CliError),
}

impl BoundDocker {
    /// Reads from the registered Engine after confirming its identity.
    pub(crate) async fn read(&self, args: &[OsString]) -> Result<CliOutcome, TargetError> {
        self.read_checked(args, false).await
    }

    /// Reads a projected inspect response with a larger bounded stdout budget.
    pub(crate) async fn read_inspection(
        &self,
        args: &[OsString],
    ) -> Result<CliOutcome, TargetError> {
        self.read_checked(args, true).await
    }

    async fn read_checked(
        &self,
        args: &[OsString],
        projected_inspect: bool,
    ) -> Result<CliOutcome, TargetError> {
        let info = engine_info(&self.cli)
            .await
            .ok_or(TargetError::Unavailable)?;
        if info.get("ID").and_then(Value::as_str) != Some(self.engine_id.as_str()) {
            return Err(TargetError::Changed);
        }
        if projected_inspect {
            Ok(self.cli.inspect_projected(args, PROBE_TIMEOUT).await?)
        } else {
            Ok(self.cli.run(CommandKind::Read, args, PROBE_TIMEOUT).await?)
        }
    }

    /// Verifies the Engine ID immediately before a potentially changing command.
    pub async fn change(
        &self,
        args: &[OsString],
        deadline: Duration,
    ) -> Result<CliOutcome, TargetError> {
        let _guard = self.gate.lock().await;
        let info = engine_info(&self.cli)
            .await
            .ok_or(TargetError::Unavailable)?;
        let observed = info
            .get("ID")
            .and_then(Value::as_str)
            .ok_or(TargetError::Unavailable)?;
        if observed != self.engine_id {
            return Err(TargetError::Changed);
        }
        Ok(self.cli.run(CommandKind::Change, args, deadline).await?)
    }
}

async fn check_version(
    cli: &DockerCli,
    args: &[OsString],
    prefix: &str,
    minimum: [u32; 3],
) -> (Check, Option<String>) {
    match cli.run(CommandKind::Read, args, PROBE_TIMEOUT).await {
        Ok(outcome)
            if outcome.status.is_some_and(|status| status.success())
                && !outcome.stdout.truncated =>
        {
            let version = parse_version(&outcome.stdout.bytes, prefix);
            (
                if version.is_some_and(|version| version >= minimum) {
                    Check::Ready
                } else {
                    Check::Unsupported
                },
                version.map(version_text),
            )
        }
        Ok(outcome)
            if String::from_utf8_lossy(&outcome.stderr.bytes).contains("not a docker command") =>
        {
            (Check::Missing, None)
        }
        Ok(outcome) if permission_denied(&outcome.stderr.bytes) => (Check::PermissionDenied, None),
        Err(CliError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            (Check::PermissionDenied, None)
        }
        _ => (Check::Unavailable, None),
    }
}

fn version_text(version: [u32; 3]) -> String {
    format!("{}.{}.{}", version[0], version[1], version[2])
}

fn permission_denied(output: &[u8]) -> bool {
    let text = String::from_utf8_lossy(output).to_ascii_lowercase();
    ["permission denied", "access is denied", "access denied"]
        .iter()
        .any(|reason| text.contains(reason))
}

fn parse_version(output: &[u8], prefix: &str) -> Option<[u32; 3]> {
    let text = std::str::from_utf8(output).ok()?.trim();
    let version = text.strip_prefix(prefix)?.trim().trim_start_matches('v');
    let version = version.split([',', ' ', '-', '+']).next()?;
    let mut parts = version.split('.').map(str::parse::<u32>);
    let result = [
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    ];
    parts.next().is_none().then_some(result)
}

async fn read_context_endpoint(cli: &DockerCli) -> Option<String> {
    let outcome = cli.inspect_current_context().await.ok()?;
    if !outcome.status?.success() || outcome.stdout.truncated {
        return None;
    }
    serde_json::from_slice::<String>(&outcome.stdout.bytes).ok()
}

async fn engine_info(cli: &DockerCli) -> Option<Value> {
    engine_info_checked(cli).await.ok()
}

async fn engine_info_checked(cli: &DockerCli) -> Result<Value, Check> {
    let args = ["info".into(), "--format".into(), "{{json .}}".into()];
    let outcome = cli
        .run(CommandKind::Read, &args, PROBE_TIMEOUT)
        .await
        .map_err(|error| match error {
            CliError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                Check::PermissionDenied
            }
            _ => Check::Unavailable,
        })?;
    if !outcome.status.is_some_and(|status| status.success()) || outcome.stdout.truncated {
        return Err(if permission_denied(&outcome.stderr.bytes) {
            Check::PermissionDenied
        } else {
            Check::Unavailable
        });
    }
    serde_json::from_slice(&outcome.stdout.bytes).map_err(|_| Check::Unavailable)
}

fn canonical_architecture(architecture: &str) -> &str {
    match architecture {
        "x86_64" | "amd64" => "amd64",
        "aarch64" | "arm64" => "arm64",
        other => other,
    }
}

fn supported_architecture(architecture: &str) -> bool {
    match std::env::consts::ARCH {
        "x86_64" => matches!(architecture, "x86_64" | "amd64"),
        "aarch64" => matches!(architecture, "aarch64" | "arm64"),
        _ => false,
    }
}

fn default_local_endpoint() -> OsString {
    #[cfg(windows)]
    {
        OsString::from("npipe:////./pipe/docker_engine")
    }
    #[cfg(not(windows))]
    {
        OsString::from("unix:///var/run/docker.sock")
    }
}
