//! Ordered Start, Stop, and Restart operations over confirmed runtime state.

use composenest_domain::instance::{OperationStatus, RuntimeStatus};

use crate::{
    operation_journal::{
        ExpectedResult, OperationJournal, OperationKind, RequestReceipt, StepCommand, StepIntent,
        StepOutcome,
    },
    operation_runner::{OperationReservation, OperationRunner, RunnerError},
    state_store::StoreConflict,
};

/// Saved identity and revision that a lifecycle operation must preserve.
#[derive(Debug, Clone)]
pub struct LifecycleSnapshot {
    /// Full ID of the previously confirmed container.
    pub container_id: String,
    /// Committed spec revision; lifecycle operations do not increment it.
    pub spec_revision: u64,
}

/// Current Docker evidence, kept separate from persisted runtime status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleObservation {
    /// Current status, including an inconclusive observation.
    pub status: RuntimeStatus,
    /// Whether the full container ID and independent ownership labels agree.
    pub owned: bool,
    /// Whether all committed configuration fields agree.
    pub configuration_matches: bool,
}

/// A denied action or an external result that requires reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEffectError {
    /// A prerequisite or observed postcondition failed conclusively.
    Rejected,
    /// Docker may have applied an effect whose result cannot be established.
    OutcomeUnknown,
}

/// Access to confirmed state without changing the spec or allocations.
pub trait LifecycleState: Send + Sync {
    /// Loads a receipt, accepted operation, and previously recorded container.
    fn snapshot(
        &self,
        receipt: &RequestReceipt,
        kind: OperationKind,
    ) -> Result<LifecycleSnapshot, StoreConflict>;

    /// Atomically saves the final observation and completes the operation.
    fn complete(
        &self,
        operation_id: &str,
        container_id: &str,
        status: RuntimeStatus,
    ) -> Result<(), StoreConflict>;
}

/// Fixed adapters for inspecting and changing one confirmed instance.
pub trait LifecycleStages {
    /// Inspects the full saved ID and verifies its ownership and configuration.
    fn observe(&self, container_id: &str) -> impl Future<Output = LifecycleObservation>;
    /// Verifies every saved data allocation without creating a missing resource.
    fn verify_storage(&self) -> impl Future<Output = Result<(), LifecycleEffectError>>;
    /// Verifies the committed Compose artifact without replacing edited files.
    fn verify_artifact(&self) -> Result<(), LifecycleEffectError>;
    /// Recreates a stopped container from the same artifact and verifies its mounts.
    fn recreate(&self) -> impl Future<Output = Result<String, LifecycleEffectError>>;
    /// Starts only the supplied fully verified Docker ID.
    fn start(&self, container_id: &str) -> impl Future<Output = Result<(), LifecycleEffectError>>;
    /// Stops only the supplied owned Docker ID, without reading Compose files.
    fn stop(&self, container_id: &str) -> impl Future<Output = Result<(), LifecycleEffectError>>;
    /// Restarts only the supplied fully verified Docker ID.
    fn restart(&self, container_id: &str)
    -> impl Future<Output = Result<(), LifecycleEffectError>>;
    /// Polls until Ready while repeatedly checking ownership and configuration.
    fn wait_ready(
        &self,
        container_id: &str,
    ) -> impl Future<Output = Result<(), LifecycleEffectError>>;
}

/// A lifecycle request could not safely reach its observed postcondition.
#[derive(Debug, PartialEq, Eq)]
pub enum LifecycleOperationError {
    /// The local change gate rejected concurrent execution.
    Runner(RunnerError),
    /// Persisted data or a journal update failed.
    Store(StoreConflict),
    /// The recorded container is absent; the user should choose Start.
    StartSuggested,
    /// A prerequisite or postcondition failed conclusively.
    Rejected,
    /// An external effect requires reconciliation.
    OutcomeUnknown,
}

/// Coordinates one accepted lifecycle request and its durable effect steps.
pub struct LifecycleOperation<'a, S, J, A> {
    /// Confirmed state and atomic completion boundary.
    pub state: &'a S,
    /// Durable effect journal.
    pub journal: &'a J,
    /// Per-instance and global operation gate.
    pub runner: &'a OperationRunner,
    /// Fixed external adapters for the accepted instance.
    pub stages: &'a A,
}

#[derive(Clone, Copy)]
enum Stage {
    Recreate,
    Start,
    Stop,
    Restart,
}

impl Stage {
    fn journal(self) -> (StepCommand, ExpectedResult, &'static str) {
        match self {
            Self::Recreate => (
                StepCommand::ComposeCreate,
                ExpectedResult::ContainerCreated,
                "recreate",
            ),
            Self::Start => (
                StepCommand::ComposeStart,
                ExpectedResult::ContainerRunning,
                "start",
            ),
            Self::Stop => (
                StepCommand::ComposeStop,
                ExpectedResult::ContainerStopped,
                "stop",
            ),
            Self::Restart => (
                StepCommand::ComposeRestart,
                ExpectedResult::ContainerRunning,
                "restart",
            ),
        }
    }
}

impl<S: LifecycleState, J: OperationJournal, A: LifecycleStages> LifecycleOperation<'_, S, J, A> {
    /// Runs Start, Stop, or Restart and returns the observed final container ID.
    pub async fn run(
        &self,
        receipt: &RequestReceipt,
        kind: OperationKind,
    ) -> Result<String, LifecycleOperationError> {
        if !matches!(
            kind,
            OperationKind::Start | OperationKind::Stop | OperationKind::Restart
        ) {
            return Err(LifecycleOperationError::Store(StoreConflict::InvalidInput));
        }
        self.runner
            .run_exclusive(&receipt.instance_id, || self.run_locked(receipt, kind))
            .await
            .map_err(LifecycleOperationError::Runner)?
    }

    /// Executes accepted work while holding capacity reserved before durable acceptance.
    pub async fn run_reserved(
        &self,
        receipt: &RequestReceipt,
        kind: OperationKind,
        reservation: OperationReservation,
    ) -> Result<String, LifecycleOperationError> {
        if !matches!(
            kind,
            OperationKind::Start | OperationKind::Stop | OperationKind::Restart
        ) {
            return Err(LifecycleOperationError::Store(StoreConflict::InvalidInput));
        }
        self.runner
            .run_reserved(reservation, &receipt.instance_id, || {
                self.run_locked(receipt, kind)
            })
            .await
            .map_err(LifecycleOperationError::Runner)?
    }

    async fn run_locked(
        &self,
        receipt: &RequestReceipt,
        kind: OperationKind,
    ) -> Result<String, LifecycleOperationError> {
        let saved = self
            .state
            .snapshot(receipt, kind)
            .map_err(LifecycleOperationError::Store)?;
        self.journal
            .set_status(&receipt.operation_id, OperationStatus::Executing, "inspect")
            .map_err(LifecycleOperationError::Store)?;
        let current = self.stages.observe(&saved.container_id).await;
        let mut container_id = saved.container_id;
        let result = match kind {
            OperationKind::Start => {
                self.stages
                    .verify_storage()
                    .await
                    .map_err(|error| self.fail(receipt, "storage", error))?;
                self.stages
                    .verify_artifact()
                    .map_err(|error| self.fail(receipt, "artifact", error))?;
                let mut sequence = 1;
                if current.status == RuntimeStatus::Absent {
                    container_id = self
                        .step(
                            receipt,
                            sequence,
                            Stage::Recreate,
                            &receipt.instance_id,
                            self.stages.recreate(),
                        )
                        .await?;
                    sequence += 1;
                } else {
                    require_matching(&current)
                        .map_err(|error| self.fail(receipt, "inspect", error))?;
                    if current.status != RuntimeStatus::Stopped {
                        return Err(self.fail(receipt, "inspect", LifecycleEffectError::Rejected));
                    }
                }
                self.step(
                    receipt,
                    sequence,
                    Stage::Start,
                    &container_id,
                    self.stages.start(&container_id),
                )
                .await?;
                self.stages
                    .wait_ready(&container_id)
                    .await
                    .map_err(|error| self.fail(receipt, "ready", error))?;
                RuntimeStatus::Ready
            }
            OperationKind::Stop => {
                if current.status == RuntimeStatus::Absent {
                    RuntimeStatus::Absent
                } else {
                    if !current.owned && !matches!(current.status, RuntimeStatus::Unknown(_)) {
                        return Err(self.fail(receipt, "inspect", LifecycleEffectError::Rejected));
                    }
                    if matches!(current.status, RuntimeStatus::Unknown(_)) {
                        return Err(self.fail(
                            receipt,
                            "inspect",
                            LifecycleEffectError::OutcomeUnknown,
                        ));
                    }
                    self.step(
                        receipt,
                        1,
                        Stage::Stop,
                        &container_id,
                        self.stages.stop(&container_id),
                    )
                    .await?;
                    let after = self.stages.observe(&container_id).await;
                    if !matches!(after.status, RuntimeStatus::Stopped | RuntimeStatus::Absent)
                        || (after.status != RuntimeStatus::Absent && !after.owned)
                    {
                        return Err(self.fail(
                            receipt,
                            "observe",
                            LifecycleEffectError::OutcomeUnknown,
                        ));
                    }
                    after.status
                }
            }
            OperationKind::Restart => {
                if current.status == RuntimeStatus::Absent {
                    return Err(self.suggest_start(receipt));
                }
                require_matching(&current).map_err(|error| self.fail(receipt, "inspect", error))?;
                self.stages
                    .verify_artifact()
                    .map_err(|error| self.fail(receipt, "artifact", error))?;
                self.step(
                    receipt,
                    1,
                    Stage::Restart,
                    &container_id,
                    self.stages.restart(&container_id),
                )
                .await?;
                self.stages
                    .wait_ready(&container_id)
                    .await
                    .map_err(|error| self.fail(receipt, "ready", error))?;
                RuntimeStatus::Ready
            }
            _ => unreachable!(),
        };
        self.state
            .complete(&receipt.operation_id, &container_id, result)
            .map_err(LifecycleOperationError::Store)?;
        Ok(container_id)
    }

    async fn step<T>(
        &self,
        receipt: &RequestReceipt,
        sequence: u64,
        stage: Stage,
        resource_id: &str,
        work: impl Future<Output = Result<T, LifecycleEffectError>>,
    ) -> Result<T, LifecycleOperationError> {
        let (command_kind, expected_result, phase) = stage.journal();
        self.journal
            .record_step(&StepIntent {
                operation_id: receipt.operation_id.clone(),
                sequence,
                attempt: 1,
                command_kind,
                resource_id: resource_id.into(),
                expected_result,
            })
            .map_err(LifecycleOperationError::Store)?;
        self.journal
            .set_status(&receipt.operation_id, OperationStatus::Executing, phase)
            .map_err(LifecycleOperationError::Store)?;
        let result = work.await;
        let outcome = match result.as_ref() {
            Ok(_) => StepOutcome::Succeeded,
            Err(LifecycleEffectError::Rejected) => StepOutcome::Failed,
            Err(LifecycleEffectError::OutcomeUnknown) => StepOutcome::Unknown,
        };
        self.journal
            .finish_step(&receipt.operation_id, sequence, outcome)
            .map_err(LifecycleOperationError::Store)?;
        result.map_err(|error| self.fail(receipt, phase, error))
    }

    fn suggest_start(&self, receipt: &RequestReceipt) -> LifecycleOperationError {
        self.journal
            .set_status(
                &receipt.operation_id,
                OperationStatus::Failed,
                "start_required",
            )
            .map_or_else(LifecycleOperationError::Store, |_| {
                LifecycleOperationError::StartSuggested
            })
    }

    fn fail(
        &self,
        receipt: &RequestReceipt,
        phase: &str,
        error: LifecycleEffectError,
    ) -> LifecycleOperationError {
        let (status, result) = match error {
            LifecycleEffectError::Rejected => {
                (OperationStatus::Failed, LifecycleOperationError::Rejected)
            }
            LifecycleEffectError::OutcomeUnknown => (
                OperationStatus::OutcomeUnknown,
                LifecycleOperationError::OutcomeUnknown,
            ),
        };
        self.journal
            .set_status(&receipt.operation_id, status, phase)
            .map_or_else(LifecycleOperationError::Store, |_| result)
    }
}

fn require_matching(observation: &LifecycleObservation) -> Result<(), LifecycleEffectError> {
    if observation.owned
        && observation.configuration_matches
        && !matches!(
            observation.status,
            RuntimeStatus::Unknown(_) | RuntimeStatus::Absent
        )
    {
        Ok(())
    } else {
        Err(LifecycleEffectError::Rejected)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::operation_journal::{OperationIntent, StepRecord};

    struct Fake {
        observation: LifecycleObservation,
        storage: Result<(), LifecycleEffectError>,
        artifact: Result<(), LifecycleEffectError>,
        events: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(status: RuntimeStatus) -> Self {
            Self {
                observation: LifecycleObservation {
                    status,
                    owned: true,
                    configuration_matches: true,
                },
                storage: Ok(()),
                artifact: Ok(()),
                events: Mutex::new(Vec::new()),
            }
        }
        fn record(&self, value: &str) {
            self.events.lock().unwrap().push(value.into());
        }
        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }

    impl LifecycleState for Fake {
        fn snapshot(
            &self,
            _: &RequestReceipt,
            _: OperationKind,
        ) -> Result<LifecycleSnapshot, StoreConflict> {
            Ok(LifecycleSnapshot {
                container_id: "a".repeat(64),
                spec_revision: 7,
            })
        }
        fn complete(&self, _: &str, _: &str, status: RuntimeStatus) -> Result<(), StoreConflict> {
            self.record(&format!("complete:{status:?}"));
            Ok(())
        }
    }

    impl OperationJournal for Fake {
        fn accept(
            &self,
            _: &OperationIntent,
            _: &RequestReceipt,
        ) -> Result<RequestReceipt, StoreConflict> {
            unreachable!()
        }
        fn receipt(&self, _: &str, _: &str) -> Result<Option<RequestReceipt>, StoreConflict> {
            unreachable!()
        }
        fn plan_receipt(&self, _: &str, _: &str) -> Result<Option<RequestReceipt>, StoreConflict> {
            unreachable!()
        }
        fn record_step(&self, step: &StepIntent) -> Result<(), StoreConflict> {
            self.record(&format!(
                "step:{}:{}",
                step.sequence,
                step.command_kind.as_str()
            ));
            Ok(())
        }
        fn steps_for_resource(&self, _: &str, _: &str) -> Result<Vec<StepRecord>, StoreConflict> {
            unreachable!()
        }
        fn finish_step(&self, _: &str, _: u64, outcome: StepOutcome) -> Result<(), StoreConflict> {
            self.record(&format!("outcome:{outcome:?}"));
            Ok(())
        }
        fn retry(&self, _: &str, _: &str) -> Result<u64, StoreConflict> {
            unreachable!()
        }
        fn set_status(
            &self,
            _: &str,
            status: OperationStatus,
            phase: &str,
        ) -> Result<(), StoreConflict> {
            self.record(&format!("status:{status:?}:{phase}"));
            Ok(())
        }
    }

    impl LifecycleStages for Fake {
        async fn observe(&self, _: &str) -> LifecycleObservation {
            self.record("observe");
            self.observation.clone()
        }
        async fn verify_storage(&self) -> Result<(), LifecycleEffectError> {
            self.record("storage");
            self.storage
        }
        fn verify_artifact(&self) -> Result<(), LifecycleEffectError> {
            self.record("artifact");
            self.artifact
        }
        async fn recreate(&self) -> Result<String, LifecycleEffectError> {
            self.record("recreate");
            Ok("b".repeat(64))
        }
        async fn start(&self, _: &str) -> Result<(), LifecycleEffectError> {
            self.record("start");
            Ok(())
        }
        async fn stop(&self, _: &str) -> Result<(), LifecycleEffectError> {
            self.record("stop");
            Ok(())
        }
        async fn restart(&self, _: &str) -> Result<(), LifecycleEffectError> {
            self.record("restart");
            Ok(())
        }
        async fn wait_ready(&self, _: &str) -> Result<(), LifecycleEffectError> {
            self.record("ready");
            Ok(())
        }
    }

    fn receipt() -> RequestReceipt {
        RequestReceipt {
            scope_id: "scope".into(),
            request_id: "request".into(),
            plan_id: None,
            confirmed_revision: 1,
            request_hash: "hash".into(),
            instance_id: "instance".into(),
            operation_id: "operation".into(),
        }
    }

    async fn run(fake: &Fake, kind: OperationKind) -> Result<String, LifecycleOperationError> {
        LifecycleOperation {
            state: fake,
            journal: fake,
            runner: &OperationRunner::new(),
            stages: fake,
        }
        .run(&receipt(), kind)
        .await
    }

    #[tokio::test]
    async fn reserved_lifecycle_can_finish_during_shutdown_and_releases_capacity() {
        let fake = Fake::new(RuntimeStatus::Stopped);
        let runner = std::sync::Arc::new(OperationRunner::new());
        let reservation = runner.reserve().unwrap();
        assert!(!runner.shutdown(std::time::Duration::ZERO));
        let operation = LifecycleOperation {
            state: &fake,
            journal: &fake,
            runner: &runner,
            stages: &fake,
        };
        assert_eq!(
            operation
                .run_reserved(&receipt(), OperationKind::Start, reservation)
                .await,
            Ok("a".repeat(64))
        );
        assert!(runner.shutdown(std::time::Duration::ZERO));
    }

    #[tokio::test]
    async fn missing_storage_never_recreates_or_starts() {
        let mut fake = Fake::new(RuntimeStatus::Absent);
        fake.storage = Err(LifecycleEffectError::Rejected);
        assert_eq!(
            run(&fake, OperationKind::Start).await,
            Err(LifecycleOperationError::Rejected)
        );
        assert_eq!(
            fake.events().last().map(String::as_str),
            Some("status:Failed:storage")
        );
        assert!(
            !fake
                .events()
                .iter()
                .any(|event| event == "recreate" || event == "start")
        );
    }

    #[tokio::test]
    async fn absent_start_recreates_then_starts_the_new_id() {
        let fake = Fake::new(RuntimeStatus::Absent);
        assert_eq!(run(&fake, OperationKind::Start).await, Ok("b".repeat(64)));
        let events = fake.events();
        assert!(
            events
                .iter()
                .position(|event| event == "step:1:compose_create")
                < events.iter().position(|event| event == "recreate")
        );
        assert!(
            events.iter().position(|event| event == "recreate")
                < events
                    .iter()
                    .position(|event| event == "step:2:compose_start")
        );
        assert_eq!(events.last().map(String::as_str), Some("complete:Ready"));
    }

    #[tokio::test]
    async fn stopped_start_uses_the_recorded_id_without_recreation() {
        let fake = Fake::new(RuntimeStatus::Stopped);
        assert_eq!(run(&fake, OperationKind::Start).await, Ok("a".repeat(64)));
        let events = fake.events();
        assert!(events.iter().any(|event| event == "step:1:compose_start"));
        assert!(!events.iter().any(|event| event == "recreate"));
        assert_eq!(events.last().map(String::as_str), Some("complete:Ready"));
    }

    #[tokio::test]
    async fn matching_restart_waits_for_ready_before_completion() {
        let fake = Fake::new(RuntimeStatus::Ready);
        assert_eq!(run(&fake, OperationKind::Restart).await, Ok("a".repeat(64)));
        let events = fake.events();
        let position = |name: &str| events.iter().position(|event| event == name).unwrap();
        assert!(position("artifact") < position("step:1:compose_restart"));
        assert!(position("step:1:compose_restart") < position("restart"));
        assert!(position("restart") < position("ready"));
        assert!(position("ready") < position("complete:Ready"));
    }

    #[tokio::test]
    async fn owned_stop_ignores_configuration_and_artifact_drift() {
        let mut fake = Fake::new(RuntimeStatus::Stopped);
        fake.observation.configuration_matches = false;
        fake.artifact = Err(LifecycleEffectError::Rejected);
        assert_eq!(run(&fake, OperationKind::Stop).await, Ok("a".repeat(64)));
        assert!(fake.events().iter().any(|event| event == "stop"));
        assert!(!fake.events().iter().any(|event| event == "artifact"));
    }

    #[tokio::test]
    async fn foreign_container_is_never_stopped() {
        let mut fake = Fake::new(RuntimeStatus::Ready);
        fake.observation.owned = false;
        assert_eq!(
            run(&fake, OperationKind::Stop).await,
            Err(LifecycleOperationError::Rejected)
        );
        assert!(!fake.events().iter().any(|event| event == "stop"));
    }

    #[tokio::test]
    async fn absent_restart_suggests_start_without_sending_restart() {
        let fake = Fake::new(RuntimeStatus::Absent);
        assert_eq!(
            run(&fake, OperationKind::Restart).await,
            Err(LifecycleOperationError::StartSuggested)
        );
        assert!(!fake.events().iter().any(|event| event == "restart"));
    }
}
