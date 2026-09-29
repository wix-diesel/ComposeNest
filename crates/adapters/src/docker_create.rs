//! Fixed Docker commands for the create and clone operation path.

use std::{
    ffi::OsString,
    time::{Duration, Instant},
};

use composenest_domain::instance::RuntimeStatus;
use tokio::time::sleep;

use crate::{
    artifact_store::ArtifactStore,
    docker_cli::CliOutcome,
    docker_observation::{ExpectedContainer, Ownership},
    docker_target::BoundDocker,
};

const CREATE_DEADLINE: Duration = Duration::from_secs(120);
const START_DEADLINE: Duration = Duration::from_secs(30);
const READY_DEADLINE: Duration = Duration::from_secs(180);
const READY_POLL: Duration = Duration::from_secs(2);

/// Safe failure categories without raw command output or secret values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateDockerError {
    /// The artifact or requested identity is invalid or changed.
    InvalidInput,
    /// Docker rejected the command or observation.
    Unavailable,
    /// A changing command may have taken effect despite its error.
    OutcomeUnknown,
    /// A container already occupies the project before creation.
    ExistingContainer,
    /// The observed container identity or complete configuration differs.
    Mismatch,
    /// The container became unhealthy or did not reach Ready within the deadline.
    NotReady,
}

/// Executes only the approved create and start commands against a fixed Engine.
pub struct DockerCreate<'a> {
    docker: &'a BoundDocker,
    artifacts: &'a ArtifactStore<'a>,
    artifact_id: &'a str,
    project: &'a str,
    instance_id: &'a str,
}

impl<'a> DockerCreate<'a> {
    /// Binds the Docker target and immutable artifact to one confirmed instance.
    pub fn new(
        docker: &'a BoundDocker,
        artifacts: &'a ArtifactStore<'a>,
        artifact_id: &'a str,
        project: &'a str,
        instance_id: &'a str,
    ) -> Result<Self, CreateDockerError> {
        if instance_id.len() != 32
            || !instance_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || project != format!("cn-{instance_id}")
            || !valid_project(project)
        {
            return Err(CreateDockerError::InvalidInput);
        }
        Ok(Self {
            docker,
            artifacts,
            artifact_id,
            project,
            instance_id,
        })
    }

    /// Validates the published Compose document before any container creation.
    pub async fn validate_config(&self) -> Result<(), CreateDockerError> {
        let mut args = self.compose_args()?;
        args.extend(["config".into(), "--quiet".into()]);
        let result = self
            .docker
            .read(&args)
            .await
            .map_err(|_| CreateDockerError::Unavailable)?;
        check_read(&result)
    }

    /// Creates the stopped main container without building or pulling an image.
    /// The caller must journal intent before invoking this method.
    pub async fn create_stopped(&self) -> Result<(), CreateDockerError> {
        self.validate_config().await?;
        if self.find_container().await?.is_some() {
            return Err(CreateDockerError::ExistingContainer);
        }
        let mut args = self.compose_args()?;
        args.extend([
            "create".into(),
            "--no-build".into(),
            "--pull".into(),
            "never".into(),
            "main".into(),
        ]);
        let result = self
            .docker
            .change(&args, CREATE_DEADLINE)
            .await
            .map_err(|_| CreateDockerError::OutcomeUnknown)?;
        check_change(&result)
    }

    /// Locates exactly one full Docker ID after creation, without adopting a name.
    pub async fn created_container_id(&self) -> Result<String, CreateDockerError> {
        self.find_container()
            .await?
            .ok_or(CreateDockerError::OutcomeUnknown)
    }

    /// Verifies ownership, image, every mount and public setting while stopped.
    pub async fn verify_stopped(
        &self,
        expected: &ExpectedContainer,
    ) -> Result<(), CreateDockerError> {
        self.check_expected(expected)?;
        let observation = self.docker.observe(expected).await;
        if observation.ownership != Ownership::Verified
            || observation.configuration_matches != Some(true)
        {
            return Err(CreateDockerError::Mismatch);
        }
        if observation.status != RuntimeStatus::Stopped {
            return Err(CreateDockerError::OutcomeUnknown);
        }
        Ok(())
    }

    /// Starts only the fully verified, recorded container ID.
    /// The caller must persist MayHaveInitialized and journal intent first.
    pub async fn start_verified(
        &self,
        expected: &ExpectedContainer,
    ) -> Result<(), CreateDockerError> {
        self.verify_stopped(expected).await?;
        let args = [
            "container".into(),
            "start".into(),
            expected.container_id.clone().into(),
        ];
        let result = self
            .docker
            .change(&args, START_DEADLINE)
            .await
            .map_err(|_| CreateDockerError::OutcomeUnknown)?;
        check_change(&result)
    }

    /// Waits for healthy Ready while repeatedly checking ownership and full configuration.
    pub async fn wait_ready(&self, expected: &ExpectedContainer) -> Result<(), CreateDockerError> {
        self.check_expected(expected)?;
        let deadline = Instant::now() + READY_DEADLINE;
        loop {
            let observation = self.docker.observe(expected).await;
            if observation.ownership != Ownership::Verified
                || observation.configuration_matches != Some(true)
            {
                return Err(CreateDockerError::Mismatch);
            }
            match observation.status {
                RuntimeStatus::Ready => return Ok(()),
                RuntimeStatus::Unhealthy | RuntimeStatus::Stopped | RuntimeStatus::Absent => {
                    return Err(CreateDockerError::NotReady);
                }
                RuntimeStatus::Unknown(_) => return Err(CreateDockerError::OutcomeUnknown),
                RuntimeStatus::Preparing => {}
            }
            if Instant::now() >= deadline {
                return Err(CreateDockerError::NotReady);
            }
            sleep(READY_POLL.min(deadline.saturating_duration_since(Instant::now()))).await;
        }
    }

    fn check_expected(&self, expected: &ExpectedContainer) -> Result<(), CreateDockerError> {
        if expected.project != self.project
            || expected.instance != self.instance_id
            || !valid_container_id(&expected.container_id)
        {
            return Err(CreateDockerError::InvalidInput);
        }
        Ok(())
    }

    fn compose_args(&self) -> Result<Vec<OsString>, CreateDockerError> {
        let path = self
            .artifacts
            .verified_compose_path(self.artifact_id)
            .map_err(|_| CreateDockerError::InvalidInput)?;
        Ok(vec![
            "compose".into(),
            "--project-name".into(),
            self.project.into(),
            "--file".into(),
            path.into_os_string(),
        ])
    }

    async fn find_container(&self) -> Result<Option<String>, CreateDockerError> {
        let args: [OsString; 11] = [
            "container".into(),
            "ls".into(),
            "--all".into(),
            "--no-trunc".into(),
            "--quiet".into(),
            "--filter".into(),
            format!("label=com.docker.compose.project={}", self.project).into(),
            "--filter".into(),
            format!("label=io.composenest.instance={}", self.instance_id).into(),
            "--filter".into(),
            "label=com.docker.compose.service=main".into(),
        ];
        let result = self
            .docker
            .read(&args)
            .await
            .map_err(|_| CreateDockerError::Unavailable)?;
        check_read(&result)?;
        let output = std::str::from_utf8(&result.stdout.bytes)
            .map_err(|_| CreateDockerError::OutcomeUnknown)?;
        let mut ids = output.lines();
        let first = ids.next().map(str::trim);
        if ids.next().is_some() {
            return Err(CreateDockerError::ExistingContainer);
        }
        match first {
            Some(id) if valid_container_id(id) => Ok(Some(id.to_owned())),
            None => Ok(None),
            _ => Err(CreateDockerError::OutcomeUnknown),
        }
    }
}

fn valid_project(value: &str) -> bool {
    value.starts_with("cn-")
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_container_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn check_read(result: &CliOutcome) -> Result<(), CreateDockerError> {
    if result.outcome_unknown || result.stdout.truncated || result.stderr.truncated {
        return Err(CreateDockerError::OutcomeUnknown);
    }
    if result.status.is_some_and(|status| status.success()) {
        Ok(())
    } else {
        Err(CreateDockerError::Unavailable)
    }
}

fn check_change(result: &CliOutcome) -> Result<(), CreateDockerError> {
    if result.outcome_unknown {
        return Err(CreateDockerError::OutcomeUnknown);
    }
    if result.status.is_some_and(|status| status.success()) {
        Ok(())
    } else {
        Err(CreateDockerError::OutcomeUnknown)
    }
}
