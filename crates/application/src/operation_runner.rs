//! Serialized, durable execution of one accepted operation step.

use std::{
    collections::HashSet,
    future::Future,
    sync::{Condvar, Mutex},
    time::{Duration, Instant},
};

use composenest_domain::instance::OperationStatus;

use crate::{
    OperationEvent, OperationEventKind,
    operation_journal::{OperationJournal, RequestReceipt, StepIntent, StepOutcome},
    state_store::StoreConflict,
};

const MAX_CONCURRENT_CHANGES: usize = 2;

/// Performs a fixed, validated external action and observes its actual result.
pub trait OperationStep {
    /// Rechecks prerequisites immediately before recording an effect intent.
    fn validate(&self) -> Result<(), String>;
    /// Performs the action after its intent is durable.
    fn execute(&self) -> Result<(), String>;
    /// Observes the external resource, including after an execution error.
    fn observe(&self) -> Result<StepOutcome, String>;
}

/// Receives non-sensitive progress only after the corresponding database commit.
pub trait ProgressSink {
    /// Sends a notification that can be recovered from persistent state if lost.
    fn send(&self, event: OperationEvent);
}

/// Failure to admit or persist an operation stage.
#[derive(Debug, PartialEq, Eq)]
pub enum RunnerError {
    /// The application is closing and accepts no new changes.
    ShuttingDown,
    /// The instance already has a local operation in progress.
    InstanceBusy,
    /// The configured number of simultaneous changes is in use.
    CapacityReached,
    /// Prerequisite validation failed before any external effect.
    Prerequisite(String),
    /// A durable journal update failed; the outcome needs reconciliation.
    Store(StoreConflict),
    /// The external result cannot be established safely.
    OutcomeUnknown,
    /// The observed state differs from the expected state.
    Failed,
}

impl From<StoreConflict> for RunnerError {
    fn from(value: StoreConflict) -> Self {
        Self::Store(value)
    }
}

#[derive(Default)]
struct Active {
    closing: bool,
    instances: HashSet<String>,
}

/// Limits changes to two instances while keeping reads outside the change gate.
pub struct OperationRunner {
    active: Mutex<Active>,
    idle: Condvar,
}

impl Default for OperationRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl OperationRunner {
    /// Creates the v1 runner with a maximum of two concurrent changes.
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: Mutex::new(Active::default()),
            idle: Condvar::new(),
        }
    }

    fn enter(&self, instance_id: &str) -> Result<ActiveGuard<'_>, RunnerError> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if active.closing {
            return Err(RunnerError::ShuttingDown);
        }
        if active.instances.contains(instance_id) {
            return Err(RunnerError::InstanceBusy);
        }
        if active.instances.len() == MAX_CONCURRENT_CHANGES {
            return Err(RunnerError::CapacityReached);
        }
        active.instances.insert(instance_id.to_owned());
        Ok(ActiveGuard {
            runner: self,
            instance_id: instance_id.to_owned(),
        })
    }

    /// Holds one instance's change gate across every stage of an async operation.
    pub async fn run_exclusive<T, E, F, Fut>(
        &self,
        instance_id: &str,
        work: F,
    ) -> Result<Result<T, E>, RunnerError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let _guard = self.enter(instance_id)?;
        Ok(work().await)
    }

    /// Executes one step of an already accepted operation using its durable journal.
    /// A screen disappearing does not cancel this call or change its committed intent.
    pub fn run_step<J: OperationJournal, S: OperationStep, P: ProgressSink>(
        &self,
        journal: &J,
        step: &StepIntent,
        receipt: &RequestReceipt,
        final_step: bool,
        action: &S,
        progress: &P,
    ) -> Result<(), RunnerError> {
        if receipt.operation_id != step.operation_id || receipt.instance_id.is_empty() {
            return Err(RunnerError::Store(StoreConflict::InvalidInput));
        }
        let _guard = self.enter(&receipt.instance_id)?;
        action.validate().map_err(RunnerError::Prerequisite)?;
        journal.record_step(step)?;
        journal.set_status(
            &step.operation_id,
            OperationStatus::Executing,
            step.command_kind.as_str(),
        )?;

        // The command may have taken effect even when it reports an error.
        let executed = action.execute().is_ok();
        let observed = action.observe();
        let outcome = match observed {
            Ok(StepOutcome::Succeeded) if executed => StepOutcome::Succeeded,
            Ok(StepOutcome::Failed) => StepOutcome::Failed,
            _ => StepOutcome::Unknown,
        };
        journal.finish_step(&step.operation_id, step.sequence, outcome)?;
        let status = match outcome {
            StepOutcome::Succeeded if final_step => OperationStatus::Succeeded,
            StepOutcome::Succeeded => OperationStatus::Executing,
            StepOutcome::Failed => OperationStatus::Failed,
            StepOutcome::Unknown => OperationStatus::OutcomeUnknown,
        };
        journal.set_status(&step.operation_id, status, step.command_kind.as_str())?;
        progress.send(OperationEvent {
            operation_id: step.operation_id.clone(),
            instance_id: Some(receipt.instance_id.clone()),
            revision: receipt.confirmed_revision,
            sequence: step.sequence,
            kind: match status {
                OperationStatus::Succeeded => OperationEventKind::Completed,
                OperationStatus::Failed | OperationStatus::OutcomeUnknown => {
                    OperationEventKind::Failed
                }
                _ => OperationEventKind::Progress,
            },
        });
        match outcome {
            StepOutcome::Succeeded => Ok(()),
            StepOutcome::Failed => Err(RunnerError::Failed),
            StepOutcome::Unknown => Err(RunnerError::OutcomeUnknown),
        }
    }

    /// Stops accepting changes and waits up to `timeout` for current stages to finish.
    /// Existing containers are not stopped by shutdown.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        active.closing = true;
        while !active.instances.is_empty() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            active = self
                .idle
                .wait_timeout(active, remaining)
                .unwrap_or_else(|poison| poison.into_inner())
                .0;
        }
        true
    }
}

struct ActiveGuard<'a> {
    runner: &'a OperationRunner,
    instance_id: String,
}

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        let mut active = self
            .runner
            .active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        active.instances.remove(&self.instance_id);
        self.runner.idle.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_journal::{OperationIntent, StepRecord};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[derive(Default)]
    struct FakeJournal {
        log: Mutex<Vec<&'static str>>,
        statuses: Mutex<Vec<OperationStatus>>,
    }

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
        fn record_step(&self, _: &StepIntent) -> Result<(), StoreConflict> {
            self.log.lock().unwrap().push("intent");
            Ok(())
        }
        fn steps_for_resource(&self, _: &str, _: &str) -> Result<Vec<StepRecord>, StoreConflict> {
            unreachable!()
        }
        fn finish_step(&self, _: &str, _: u64, _: StepOutcome) -> Result<(), StoreConflict> {
            self.log.lock().unwrap().push("outcome");
            Ok(())
        }
        fn retry(&self, _: &str, _: &str) -> Result<u64, StoreConflict> {
            unreachable!()
        }
        fn set_status(
            &self,
            _: &str,
            status: OperationStatus,
            _: &str,
        ) -> Result<(), StoreConflict> {
            self.log.lock().unwrap().push("status");
            self.statuses.lock().unwrap().push(status);
            Ok(())
        }
    }

    struct FakeStep<'a> {
        journal: &'a FakeJournal,
        observed: Result<StepOutcome, String>,
    }
    impl OperationStep for FakeStep<'_> {
        fn validate(&self) -> Result<(), String> {
            self.journal.log.lock().unwrap().push("validate");
            Ok(())
        }
        fn execute(&self) -> Result<(), String> {
            self.journal.log.lock().unwrap().push("execute");
            Ok(())
        }
        fn observe(&self) -> Result<StepOutcome, String> {
            self.journal.log.lock().unwrap().push("observe");
            self.observed.clone()
        }
    }
    struct FakeProgress<'a>(&'a FakeJournal);
    impl ProgressSink for FakeProgress<'_> {
        fn send(&self, _: OperationEvent) {
            self.0.log.lock().unwrap().push("notify");
        }
    }

    struct CapturingProgress<'a> {
        journal: &'a FakeJournal,
        events: Mutex<Vec<OperationEvent>>,
    }
    impl ProgressSink for CapturingProgress<'_> {
        fn send(&self, event: OperationEvent) {
            self.journal.log.lock().unwrap().push("notify");
            self.events.lock().unwrap().push(event);
        }
    }

    fn step() -> StepIntent {
        StepIntent {
            operation_id: "op".into(),
            sequence: 1,
            attempt: 1,
            command_kind: crate::operation_journal::StepCommand::Observe,
            resource_id: "resource".into(),
            expected_result: crate::operation_journal::ExpectedResult::StateObserved,
        }
    }

    fn receipt() -> RequestReceipt {
        RequestReceipt {
            scope_id: "scope".into(),
            request_id: "request".into(),
            plan_id: None,
            confirmed_revision: 1,
            request_hash: "a".repeat(64),
            instance_id: "instance".into(),
            operation_id: "op".into(),
        }
    }

    #[test]
    fn writes_intent_before_effect_and_notifies_after_status() {
        let journal = FakeJournal::default();
        let action = FakeStep {
            journal: &journal,
            observed: Ok(StepOutcome::Succeeded),
        };
        let result = OperationRunner::new().run_step(
            &journal,
            &step(),
            &receipt(),
            true,
            &action,
            &FakeProgress(&journal),
        );
        assert_eq!(result, Ok(()));
        assert_eq!(
            *journal.log.lock().unwrap(),
            [
                "validate", "intent", "status", "execute", "observe", "outcome", "status", "notify"
            ]
        );
    }

    #[test]
    fn uncertain_observation_is_durable_before_failure_notification() {
        let journal = FakeJournal::default();
        let action = FakeStep {
            journal: &journal,
            observed: Err("unreachable".into()),
        };
        let result = OperationRunner::new().run_step(
            &journal,
            &step(),
            &receipt(),
            true,
            &action,
            &FakeProgress(&journal),
        );
        assert_eq!(result, Err(RunnerError::OutcomeUnknown));
        assert_eq!(journal.log.lock().unwrap().last(), Some(&"notify"));
    }

    #[test]
    fn failed_observation_persists_failure_before_notifying() {
        let journal = FakeJournal::default();
        let action = FakeStep {
            journal: &journal,
            observed: Ok(StepOutcome::Failed),
        };
        let progress = CapturingProgress {
            journal: &journal,
            events: Mutex::new(Vec::new()),
        };
        let result = OperationRunner::new().run_step(
            &journal,
            &step(),
            &receipt(),
            true,
            &action,
            &progress,
        );
        assert_eq!(result, Err(RunnerError::Failed));
        assert_eq!(
            journal.statuses.lock().unwrap().last(),
            Some(&OperationStatus::Failed)
        );
        let events = progress.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[0].kind, OperationEventKind::Failed);
        assert_eq!(journal.log.lock().unwrap().last(), Some(&"notify"));
    }

    #[test]
    fn instance_and_capacity_gate_and_shutdown() {
        let runner = OperationRunner::new();
        let first = runner.enter("one").unwrap();
        assert!(matches!(
            runner.enter("one"),
            Err(RunnerError::InstanceBusy)
        ));
        let second = runner.enter("two").unwrap();
        assert!(matches!(
            runner.enter("three"),
            Err(RunnerError::CapacityReached)
        ));
        assert!(!runner.shutdown(Duration::ZERO));
        assert!(matches!(
            runner.enter("three"),
            Err(RunnerError::ShuttingDown)
        ));
        drop(first);
        drop(second);
        assert!(runner.shutdown(Duration::ZERO));
    }

    #[test]
    fn closing_a_screen_cannot_cancel_a_running_step() {
        let running = Arc::new(AtomicBool::new(false));
        let runner = Arc::new(OperationRunner::new());
        let completed = Arc::clone(&running);
        let worker = Arc::clone(&runner);
        let handle = std::thread::spawn(move || {
            let _guard = worker.enter("instance").unwrap();
            completed.store(true, Ordering::Release);
            std::thread::sleep(Duration::from_millis(30));
        });
        while !running.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        assert!(runner.shutdown(Duration::from_secs(1)));
        handle.join().unwrap();
    }
}
