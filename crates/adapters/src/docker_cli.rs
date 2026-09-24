//! Bounded, shell-free execution of the Docker CLI.

use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::Mutex,
    time::{sleep, timeout},
};

const OUTPUT_LIMIT: usize = 64 * 1024;
const REAP_TIMEOUT: Duration = Duration::from_secs(5);

/// A command category used for process tracking and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    /// A command that only observes Docker state.
    Read,
    /// A command that can change Docker state.
    Change,
}

/// A bounded copy of one CLI output stream.
#[derive(Debug)]
pub struct CapturedOutput {
    /// The first bytes read, limited to 64 KiB. These bytes may contain secrets.
    pub bytes: Vec<u8>,
    /// Whether bytes beyond the capture limit were discarded.
    pub truncated: bool,
}

/// A completed CLI attempt. A timeout never reports success.
#[derive(Debug)]
pub struct CliOutcome {
    /// Operating-system process identifier of the direct CLI child.
    pub pid: u32,
    /// Time at which the CLI child was started.
    pub started_at: SystemTime,
    /// Category supplied by the caller.
    pub kind: CommandKind,
    /// Exit status, available only when the attempt completed before its deadline.
    pub status: Option<ExitStatus>,
    /// Standard output, which must be redacted before logging or display.
    pub stdout: CapturedOutput,
    /// Standard error, which must be redacted before logging or display.
    pub stderr: CapturedOutput,
    /// True when the deadline expired; Docker may still have applied the operation.
    pub outcome_unknown: bool,
}

/// A failure to start, supervise, or confirm the end of a CLI attempt.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// A configured path or deadline is unsuitable for process execution.
    #[error("invalid CLI configuration: {0}")]
    InvalidConfiguration(&'static str),
    /// Process creation or pipe handling failed.
    #[error("CLI process I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The CLI may have changed Docker state before process supervision failed.
    #[error("CLI outcome is unknown: {0}")]
    OutcomeUnknown(io::Error),
    /// A previous CLI may still be alive; further changes are prohibited.
    #[error("previous CLI termination could not be confirmed")]
    TerminationUnconfirmed,
}

/// Runs one Docker executable against a fixed local endpoint.
///
/// Arguments supplied to `run` must contain only non-secret flags, identifiers,
/// and paths. Secrets belong in protected Compose artifacts, never in arguments.
/// The capture fields are sensitive until the caller redacts them.
#[derive(Clone)]
pub struct DockerCli {
    executable: PathBuf,
    directory: PathBuf,
    config_directory: PathBuf,
    endpoint: OsString,
    gate: Arc<Mutex<()>>,
    blocked: Arc<AtomicBool>,
}

impl DockerCli {
    /// Configures an absolute executable, working directory, Docker config, and local endpoint.
    pub fn new(
        executable: PathBuf,
        directory: PathBuf,
        config_directory: PathBuf,
        endpoint: OsString,
    ) -> Result<Self, CliError> {
        if !executable.is_absolute() || !directory.is_absolute() || !config_directory.is_absolute()
        {
            return Err(CliError::InvalidConfiguration("paths must be absolute"));
        }
        #[cfg(windows)]
        if !executable
            .extension()
            .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("exe"))
        {
            return Err(CliError::InvalidConfiguration(
                "Windows CLI must be an .exe file",
            ));
        }
        if endpoint.is_empty() || !is_local_endpoint(&endpoint) {
            return Err(CliError::InvalidConfiguration("endpoint must be local"));
        }
        Ok(Self {
            executable,
            directory,
            config_directory,
            endpoint,
            gate: Arc::new(Mutex::new(())),
            blocked: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Runs a CLI attempt and drains both output streams within a fixed byte budget.
    ///
    /// The caller must reconcile Docker state after `outcome_unknown` before retrying
    /// a change. A failed termination confirmation blocks subsequent attempts.
    pub async fn run(
        &self,
        kind: CommandKind,
        args: &[OsString],
        deadline: Duration,
    ) -> Result<CliOutcome, CliError> {
        if deadline.is_zero() {
            return Err(CliError::InvalidConfiguration("deadline must be positive"));
        }
        validate_arguments(args)?;
        // Keep process supervision and the serialization gate alive if the caller is cancelled.
        let runner = self.clone();
        let args = args.to_vec();
        tokio::spawn(async move { runner.run_inner(kind, &args, deadline).await })
            .await
            .map_err(|_| {
                self.blocked.store(true, Ordering::Release);
                CliError::TerminationUnconfirmed
            })?
    }

    async fn run_inner(
        &self,
        kind: CommandKind,
        args: &[OsString],
        deadline: Duration,
    ) -> Result<CliOutcome, CliError> {
        let _guard = self.gate.lock().await;
        if self.blocked.load(Ordering::Acquire) {
            return Err(CliError::TerminationUnconfirmed);
        }
        let mut command = Command::new(&self.executable);
        command
            .arg("--host")
            .arg(&self.endpoint)
            .args(args)
            .current_dir(&self.directory)
            .env_clear()
            .env("DOCKER_CONFIG", &self.config_directory)
            .env("COMPOSE_DISABLE_ENV_FILE", "1")
            .env("PATH", executable_parent(&self.executable)?)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        configure_platform(&mut command);
        let started_at = SystemTime::now();
        let mut child = command.spawn()?;
        let pid = child
            .id()
            .ok_or(CliError::InvalidConfiguration("missing process ID"))?;
        let group = match ProcessGroup::attach(pid) {
            Ok(group) => group,
            Err(error) => {
                if child.kill().await.is_err() {
                    self.blocked.store(true, Ordering::Release);
                    return Err(CliError::TerminationUnconfirmed);
                }
                return Err(error);
            }
        };
        let stdout = tokio::spawn(drain(
            child
                .stdout
                .take()
                .ok_or(CliError::InvalidConfiguration("missing stdout"))?,
        ));
        let stderr = tokio::spawn(drain(
            child
                .stderr
                .take()
                .ok_or(CliError::InvalidConfiguration("missing stderr"))?,
        ));
        let waited = timeout(deadline, child.wait()).await;
        let (status, outcome_unknown) = match waited {
            Ok(Ok(status)) => (Some(status), false),
            Ok(Err(error)) => {
                if group.terminate().is_err() {
                    self.blocked.store(true, Ordering::Release);
                    return Err(CliError::TerminationUnconfirmed);
                }
                if !matches!(timeout(REAP_TIMEOUT, child.wait()).await, Ok(Ok(_))) {
                    self.blocked.store(true, Ordering::Release);
                    return Err(CliError::TerminationUnconfirmed);
                }
                self.blocked.store(true, Ordering::Release);
                return Err(CliError::OutcomeUnknown(error));
            }
            Err(_) => {
                if group.terminate().is_err() {
                    self.blocked.store(true, Ordering::Release);
                    return Err(CliError::TerminationUnconfirmed);
                }
                if !matches!(timeout(REAP_TIMEOUT, child.wait()).await, Ok(Ok(_))) {
                    self.blocked.store(true, Ordering::Release);
                    return Err(CliError::TerminationUnconfirmed);
                }
                (None, true)
            }
        };
        let streams = timeout(REAP_TIMEOUT, async { tokio::join!(stdout, stderr) }).await;
        let (stdout, stderr) = match streams {
            Ok((Ok(Ok(stdout)), Ok(Ok(stderr)))) => (stdout, stderr),
            _ => {
                self.blocked.store(true, Ordering::Release);
                return Err(CliError::TerminationUnconfirmed);
            }
        };
        if !matches!(
            timeout(REAP_TIMEOUT, async {
                loop {
                    match group.is_empty() {
                        Ok(true) => return true,
                        Ok(false) => sleep(Duration::from_millis(20)).await,
                        Err(_) => return false,
                    }
                }
            })
            .await,
            Ok(true)
        ) {
            self.blocked.store(true, Ordering::Release);
            return Err(CliError::TerminationUnconfirmed);
        }
        Ok(CliOutcome {
            pid,
            started_at,
            kind,
            status,
            stdout,
            stderr,
            outcome_unknown,
        })
    }
}

fn executable_parent(executable: &Path) -> Result<&Path, CliError> {
    executable
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or(CliError::InvalidConfiguration("executable has no parent"))
}

fn validate_arguments(args: &[OsString]) -> Result<(), CliError> {
    for arg in args {
        let value = arg.to_string_lossy();
        if [
            "--host",
            "--context",
            "--config",
            "--tlscacert",
            "--tlscert",
            "--tlskey",
            "--tls",
            "--tlsverify",
        ]
        .iter()
        .any(|flag| value == *flag || value.starts_with(&format!("{flag}=")))
            || value == "-H"
            || value.starts_with("-H=")
            || (value.starts_with("-H") && value.len() > 2)
        {
            return Err(CliError::InvalidConfiguration(
                "connection options are fixed by the adapter",
            ));
        }
    }
    Ok(())
}

fn is_local_endpoint(endpoint: &OsStr) -> bool {
    let value = endpoint.to_string_lossy();
    value.starts_with("unix:///") || value.starts_with("npipe:////./pipe/")
}

async fn drain(mut stream: impl AsyncRead + Unpin) -> io::Result<CapturedOutput> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut chunk = [0_u8; 8192];
    loop {
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        let retained = count.min(OUTPUT_LIMIT - bytes.len());
        bytes.extend_from_slice(&chunk[..retained]);
        truncated |= retained < count;
    }
    Ok(CapturedOutput { bytes, truncated })
}

#[cfg(unix)]
fn configure_platform(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.as_std_mut().process_group(0);
}

#[cfg(windows)]
fn configure_platform(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command
        .as_std_mut()
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
}

#[cfg(unix)]
struct ProcessGroup(u32);

#[cfg(unix)]
impl ProcessGroup {
    fn attach(pid: u32) -> Result<Self, CliError> {
        Ok(Self(pid))
    }

    fn terminate(&self) -> Result<(), CliError> {
        // The spawned child leads its own process group.
        let result = unsafe { libc::kill(-(self.0 as i32), libc::SIGKILL) };
        if result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(CliError::Io(io::Error::last_os_error()))
        }
    }

    fn is_empty(&self) -> Result<bool, CliError> {
        let result = unsafe { libc::kill(-(self.0 as i32), 0) };
        if result == 0 {
            Ok(false)
        } else {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                Ok(true)
            } else {
                Err(CliError::Io(error))
            }
        }
    }
}

#[cfg(windows)]
mod windows_job;
#[cfg(windows)]
use windows_job::ProcessGroup;
