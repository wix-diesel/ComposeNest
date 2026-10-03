//! Ordered execution of a committed create or clone operation.

use std::future::Future;

use composenest_domain::instance::OperationStatus;

use crate::{
    OperationEvent, OperationEventKind,
    create_state::{ConfirmedCreate, CreateStateStore},
    operation_journal::{
        ExpectedResult, OperationJournal, RequestReceipt, StepCommand, StepIntent, StepOutcome,
    },
    operation_runner::{OperationReservation, OperationRunner, ProgressSink, RunnerError},
    state_store::StoreConflict,
};

/// An adapter failure classified by whether an external effect may have happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateEffectError {
    /// Observation established that the stage did not meet its expected result.
    Failed,
    /// A command or observation may have taken effect but cannot be established.
    OutcomeUnknown,
}

/// The durable create or clone operation could not advance safely.
#[derive(Debug, PartialEq, Eq)]
pub enum CreateOperationError {
    /// The local concurrency gate rejected the operation.
    Runner(RunnerError),
    /// A committed record or journal update failed.
    Store(StoreConflict),
    /// A stage failed with an observed outcome.
    Failed,
    /// The external outcome needs reconciliation before any retry.
    OutcomeUnknown,
}

impl From<StoreConflict> for CreateOperationError {
    fn from(value: StoreConflict) -> Self {
        Self::Store(value)
    }
}

/// Performs the fixed create stages against verified adapters.
/// Every method must use only the confirmed records passed by the coordinator.
pub trait CreateStages {
    /// Prepares and verifies all storage slots, journaling each materializing effect.
    /// Returns the next unused journal sequence number.
    fn prepare_storage<'a>(
        &'a self,
        confirmed: &'a ConfirmedCreate,
        operation_id: &'a str,
        first_sequence: u64,
    ) -> impl Future<Output = Result<u64, CreateEffectError>> + 'a;
    /// Resolves and durably pins the selected Image and platform.
    fn resolve_image<'a>(
        &'a self,
        confirmed: &'a ConfirmedCreate,
    ) -> impl Future<Output = Result<(), CreateEffectError>> + 'a;
    /// Generates and publishes the exact Compose artifact.
    fn publish_artifact<'a>(
        &'a self,
        confirmed: &'a ConfirmedCreate,
    ) -> impl Future<Output = Result<(), CreateEffectError>> + 'a;
    /// Checks the published artifact with Compose config --quiet.
    fn validate_config(&self) -> impl Future<Output = Result<(), CreateEffectError>>;
    /// Creates the main container while stopped, without building or pulling.
    fn create_stopped(&self) -> impl Future<Output = Result<(), CreateEffectError>>;
    /// Finds one container and verifies ownership and every expected setting.
    fn inspect_created(&self) -> impl Future<Output = Result<String, CreateEffectError>>;
    /// Starts only the verified container ID.
    fn start(&self, container_id: &str) -> impl Future<Output = Result<(), CreateEffectError>>;
    /// Observes ownership, configuration, running status and Health until Ready.
    fn wait_ready(&self, container_id: &str)
    -> impl Future<Output = Result<(), CreateEffectError>>;
}

#[derive(Clone, Copy)]
enum Stage {
    Image,
    Artifact,
    Config,
    Create,
    InspectCreated,
    Start,
    Ready,
}

impl Stage {
    fn journal(self) -> (StepCommand, ExpectedResult, &'static str) {
        match self {
            Self::Image => (
                StepCommand::ResolveImage,
                ExpectedResult::ImageResolved,
                "image",
            ),
            Self::Artifact => (
                StepCommand::GenerateArtifact,
                ExpectedResult::ArtifactReady,
                "artifact",
            ),
            Self::Config => (
                StepCommand::Observe,
                ExpectedResult::StateObserved,
                "config",
            ),
            Self::Create => (
                StepCommand::ComposeCreate,
                ExpectedResult::ContainerCreated,
                "create",
            ),
            Self::InspectCreated => (
                StepCommand::Observe,
                ExpectedResult::StateObserved,
                "inspect_created",
            ),
            Self::Start => (
                StepCommand::ComposeStart,
                ExpectedResult::ContainerRunning,
                "start",
            ),
            Self::Ready => (StepCommand::Observe, ExpectedResult::StateObserved, "ready"),
        }
    }
}

/// Runs accepted create and clone requests through the same durable sequence.
pub struct CreateOperation<'a, S, J, A, P> {
    /// Fixed persistence boundary for confirmed data and Ready completion.
    pub state: &'a S,
    /// Durable effect journal.
    pub journal: &'a J,
    /// Per-instance and global change gate.
    pub runner: &'a OperationRunner,
    /// Fixed external adapters for this operation.
    pub stages: &'a A,
    /// Non-sensitive progress events.
    pub progress: &'a P,
}

impl<S: CreateStateStore, J: OperationJournal, A: CreateStages, P: ProgressSink>
    CreateOperation<'_, S, J, A, P>
{
    /// Executes an accepted operation; failures preserve its confirmed allocation records.
    pub async fn run(&self, receipt: &RequestReceipt) -> Result<String, CreateOperationError> {
        self.runner
            .run_exclusive(&receipt.instance_id, || self.run_locked(receipt))
            .await
            .map_err(CreateOperationError::Runner)?
    }

    /// Executes accepted work using capacity reserved before its durable acceptance.
    pub async fn run_reserved(
        &self,
        receipt: &RequestReceipt,
        reservation: OperationReservation,
    ) -> Result<String, CreateOperationError> {
        self.runner
            .run_reserved(reservation, &receipt.instance_id, || {
                self.run_locked(receipt)
            })
            .await
            .map_err(CreateOperationError::Runner)?
    }

    async fn run_locked(&self, receipt: &RequestReceipt) -> Result<String, CreateOperationError> {
        let confirmed = self.state.confirmed_create(receipt)?;
        if confirmed.instance_id != receipt.instance_id || confirmed.scope_id != receipt.scope_id {
            return Err(CreateOperationError::Store(StoreConflict::InvalidInput));
        }
        self.journal
            .set_status(&receipt.operation_id, OperationStatus::Executing, "storage")?;
        let mut sequence = self
            .stages
            .prepare_storage(&confirmed, &receipt.operation_id, 1)
            .await
            .map_err(|error| self.fail(receipt, "storage", 1, error))?;
        self.stage(
            receipt,
            sequence,
            Stage::Image,
            &confirmed.instance_id,
            self.stages.resolve_image(&confirmed),
        )
        .await?;
        sequence += 1;
        self.stage(
            receipt,
            sequence,
            Stage::Artifact,
            &confirmed.instance_id,
            self.stages.publish_artifact(&confirmed),
        )
        .await?;
        sequence += 1;
        self.stage(
            receipt,
            sequence,
            Stage::Config,
            &confirmed.instance_id,
            self.stages.validate_config(),
        )
        .await?;
        sequence += 1;
        self.stage(
            receipt,
            sequence,
            Stage::Create,
            &confirmed.instance_id,
            self.stages.create_stopped(),
        )
        .await?;
        sequence += 1;
        let container_id = self
            .stage(
                receipt,
                sequence,
                Stage::InspectCreated,
                &confirmed.instance_id,
                self.stages.inspect_created(),
            )
            .await?;
        self.state
            .mark_may_have_initialized(&receipt.operation_id)?;
        sequence += 1;
        self.stage(
            receipt,
            sequence,
            Stage::Start,
            &container_id,
            self.stages.start(&container_id),
        )
        .await?;
        sequence += 1;
        self.stage(
            receipt,
            sequence,
            Stage::Ready,
            &container_id,
            self.stages.wait_ready(&container_id),
        )
        .await?;
        self.state
            .complete_ready(&receipt.operation_id, &container_id)?;
        self.progress.send(OperationEvent {
            operation_id: receipt.operation_id.clone(),
            instance_id: Some(receipt.instance_id.clone()),
            revision: receipt.confirmed_revision,
            sequence,
            kind: OperationEventKind::Completed,
        });
        Ok(container_id)
    }

    async fn stage<T, F>(
        &self,
        receipt: &RequestReceipt,
        sequence: u64,
        stage: Stage,
        resource_id: &str,
        work: F,
    ) -> Result<T, CreateOperationError>
    where
        F: Future<Output = Result<T, CreateEffectError>>,
    {
        let (command, expected, phase) = stage.journal();
        self.journal.record_step(&StepIntent {
            operation_id: receipt.operation_id.clone(),
            sequence,
            attempt: 1,
            command_kind: command,
            resource_id: resource_id.into(),
            expected_result: expected,
        })?;
        self.journal
            .set_status(&receipt.operation_id, OperationStatus::Executing, phase)?;
        let result = work.await;
        let outcome = match &result {
            Ok(_) => StepOutcome::Succeeded,
            Err(CreateEffectError::Failed) => StepOutcome::Failed,
            Err(CreateEffectError::OutcomeUnknown) => StepOutcome::Unknown,
        };
        self.journal
            .finish_step(&receipt.operation_id, sequence, outcome)?;
        match result {
            Ok(value) => {
                self.progress.send(OperationEvent {
                    operation_id: receipt.operation_id.clone(),
                    instance_id: Some(receipt.instance_id.clone()),
                    revision: receipt.confirmed_revision,
                    sequence,
                    kind: OperationEventKind::Progress,
                });
                Ok(value)
            }
            Err(error) => Err(self.fail(receipt, phase, sequence, error)),
        }
    }

    fn fail(
        &self,
        receipt: &RequestReceipt,
        phase: &str,
        sequence: u64,
        error: CreateEffectError,
    ) -> CreateOperationError {
        let (status, result) = match error {
            CreateEffectError::Failed => (OperationStatus::Failed, CreateOperationError::Failed),
            CreateEffectError::OutcomeUnknown => (
                OperationStatus::OutcomeUnknown,
                CreateOperationError::OutcomeUnknown,
            ),
        };
        if let Err(store) = self
            .journal
            .set_status(&receipt.operation_id, status, phase)
        {
            return CreateOperationError::Store(store);
        }
        self.progress.send(OperationEvent {
            operation_id: receipt.operation_id.clone(),
            instance_id: Some(receipt.instance_id.clone()),
            revision: receipt.confirmed_revision,
            sequence,
            kind: OperationEventKind::Failed,
        });
        result
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::{
        operation_journal::{OperationIntent, StepRecord},
        state_store::{RuntimeTarget, TemplateFile},
    };

    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<String>>>);
    impl Log {
        fn push(&self, event: &str) {
            self.0.lock().unwrap().push(event.into());
        }
        fn entries(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    struct FakeState(Log);
    impl CreateStateStore for FakeState {
        fn confirmed_create(
            &self,
            receipt: &RequestReceipt,
        ) -> Result<ConfirmedCreate, StoreConflict> {
            self.0.push("load");
            Ok(ConfirmedCreate {
                instance_id: receipt.instance_id.clone(),
                scope_id: receipt.scope_id.clone(),
                project_name: "cn-instance".into(),
                target: RuntimeTarget {
                    id: "target".into(),
                    scope_id: receipt.scope_id.clone(),
                    endpoint: "local".into(),
                    engine_id: "engine".into(),
                    platform: "linux/amd64".into(),
                },
                spec_revision: 1,
                selected_version: "1".into(),
                snapshot_files: vec![TemplateFile {
                    relative_path: "template.yaml".into(),
                    contents: vec![],
                }],
                inputs_json: "{\"secret\":\"kept\"}".into(),
                ports: vec![],
                storage: vec![],
            })
        }
        fn record_bind_materialization(
            &self,
            _: &str,
            _: &crate::state_store::StorageAllocation,
        ) -> Result<(), StoreConflict> {
            unreachable!()
        }
        fn mark_may_have_initialized(&self, _: &str) -> Result<(), StoreConflict> {
            self.0.push("may_have_initialized");
            Ok(())
        }
        fn complete_ready(&self, _: &str, _: &str) -> Result<(), StoreConflict> {
            self.0.push("complete_ready");
            Ok(())
        }
    }

    struct FakeJournal(Log);
    impl OperationJournal for FakeJournal {
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
            self.0
                .push(&format!("intent:{}", step.command_kind.as_str()));
            Ok(())
        }
        fn steps_for_resource(&self, _: &str, _: &str) -> Result<Vec<StepRecord>, StoreConflict> {
            unreachable!()
        }
        fn finish_step(&self, _: &str, _: u64, outcome: StepOutcome) -> Result<(), StoreConflict> {
            self.0.push(&format!("outcome:{outcome:?}"));
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
            self.0.push(&format!("status:{status:?}:{phase}"));
            Ok(())
        }
    }

    struct FakeStages {
        log: Log,
        fail: Option<(&'static str, CreateEffectError)>,
    }
    impl FakeStages {
        fn action(&self, name: &'static str) -> Result<(), CreateEffectError> {
            self.log.push(name);
            self.fail
                .filter(|(stage, _)| *stage == name)
                .map_or(Ok(()), |(_, error)| Err(error))
        }
    }
    impl CreateStages for FakeStages {
        async fn prepare_storage(
            &self,
            _: &ConfirmedCreate,
            _: &str,
            sequence: u64,
        ) -> Result<u64, CreateEffectError> {
            self.action("storage")?;
            Ok(sequence)
        }
        async fn resolve_image(&self, _: &ConfirmedCreate) -> Result<(), CreateEffectError> {
            self.action("image")
        }
        async fn publish_artifact(&self, _: &ConfirmedCreate) -> Result<(), CreateEffectError> {
            self.action("artifact")
        }
        async fn validate_config(&self) -> Result<(), CreateEffectError> {
            self.action("config")
        }
        async fn create_stopped(&self) -> Result<(), CreateEffectError> {
            self.action("create")
        }
        async fn inspect_created(&self) -> Result<String, CreateEffectError> {
            self.action("inspect_created")?;
            Ok("a".repeat(64))
        }
        async fn start(&self, _: &str) -> Result<(), CreateEffectError> {
            self.action("start")
        }
        async fn wait_ready(&self, _: &str) -> Result<(), CreateEffectError> {
            self.action("ready")
        }
    }
    struct FakeProgress(Log);
    impl ProgressSink for FakeProgress {
        fn send(&self, event: OperationEvent) {
            self.0.push(&format!("event:{:?}", event.kind));
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

    #[tokio::test]
    async fn creates_in_order_and_marks_initialization_before_start() {
        let log = Log::default();
        let state = FakeState(log.clone());
        let journal = FakeJournal(log.clone());
        let stages = FakeStages {
            log: log.clone(),
            fail: None,
        };
        let progress = FakeProgress(log.clone());
        let runner = OperationRunner::new();
        let result = CreateOperation {
            state: &state,
            journal: &journal,
            runner: &runner,
            stages: &stages,
            progress: &progress,
        }
        .run(&receipt())
        .await;
        assert_eq!(result, Ok("a".repeat(64)));
        let entries = log.entries();
        let position = |name: &str| entries.iter().position(|entry| entry == name).unwrap();
        for pair in [
            ("storage", "image"),
            ("image", "artifact"),
            ("artifact", "config"),
            ("config", "create"),
            ("create", "inspect_created"),
            ("inspect_created", "may_have_initialized"),
            ("may_have_initialized", "start"),
            ("start", "ready"),
            ("ready", "complete_ready"),
        ] {
            assert!(position(pair.0) < position(pair.1), "{pair:?}");
        }
        assert!(position("intent:compose_create") < position("create"));
        assert!(position("intent:compose_start") < position("start"));
        assert_eq!(entries.last().map(String::as_str), Some("event:Completed"));
    }

    #[tokio::test]
    async fn reserved_operation_executes_without_reentering_a_full_capacity_gate() {
        let log = Log::default();
        let state = FakeState(log.clone());
        let journal = FakeJournal(log.clone());
        let stages = FakeStages {
            log: log.clone(),
            fail: None,
        };
        let progress = FakeProgress(log.clone());
        let runner = std::sync::Arc::new(OperationRunner::new());
        let reservation = runner.reserve().unwrap();
        let other = runner.reserve().unwrap();
        let result = CreateOperation {
            state: &state,
            journal: &journal,
            runner: &runner,
            stages: &stages,
            progress: &progress,
        }
        .run_reserved(&receipt(), reservation)
        .await;
        assert_eq!(result, Ok("a".repeat(64)));
        assert_eq!(
            log.entries().last().map(String::as_str),
            Some("event:Completed")
        );
        drop(other);
        assert!(runner.shutdown(std::time::Duration::ZERO));
    }

    #[tokio::test]
    async fn config_or_inspection_failure_prevents_start() {
        for fail in ["config", "inspect_created"] {
            let log = Log::default();
            let state = FakeState(log.clone());
            let journal = FakeJournal(log.clone());
            let stages = FakeStages {
                log: log.clone(),
                fail: Some((fail, CreateEffectError::Failed)),
            };
            let progress = FakeProgress(log.clone());
            let runner = OperationRunner::new();
            let result = CreateOperation {
                state: &state,
                journal: &journal,
                runner: &runner,
                stages: &stages,
                progress: &progress,
            }
            .run(&receipt())
            .await;
            assert_eq!(result, Err(CreateOperationError::Failed));
            let entries = log.entries();
            assert!(
                !entries
                    .iter()
                    .any(|event| event == "start" || event == "complete_ready")
            );
            assert!(
                entries
                    .iter()
                    .any(|event| event.starts_with("status:Failed:"))
            );
        }
    }

    #[tokio::test]
    async fn unknown_stage_outcome_is_journaled_and_blocks_start() {
        let log = Log::default();
        let state = FakeState(log.clone());
        let journal = FakeJournal(log.clone());
        let stages = FakeStages {
            log: log.clone(),
            fail: Some(("create", CreateEffectError::OutcomeUnknown)),
        };
        let progress = FakeProgress(log.clone());
        let runner = OperationRunner::new();
        let result = CreateOperation {
            state: &state,
            journal: &journal,
            runner: &runner,
            stages: &stages,
            progress: &progress,
        }
        .run(&receipt())
        .await;
        assert_eq!(result, Err(CreateOperationError::OutcomeUnknown));
        let entries = log.entries();
        assert!(entries.iter().any(|entry| entry == "outcome:Unknown"));
        assert!(
            entries
                .iter()
                .any(|entry| entry == "status:OutcomeUnknown:create")
        );
        assert!(!entries.iter().any(|entry| entry == "start"));
    }
}
