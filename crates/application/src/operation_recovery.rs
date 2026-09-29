//! Conservative reconciliation of interrupted operations.

use crate::{
    operation_journal::{
        OperationJournal, OperationKind, OperationStatus, StepCommand, StepRecord,
    },
    state_store::StoreConflict,
};

/// Persisted identity and progress of an unresolved operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverableOperation {
    /// Stable operation identifier shared by every attempt.
    pub id: String,
    /// Instance whose reservations remain held.
    pub instance_id: String,
    /// Original operation kind.
    pub kind: OperationKind,
    /// Last recorded operation status.
    pub status: OperationStatus,
    /// Last recorded phase.
    pub phase: String,
    /// Current attempt number.
    pub attempt: u64,
    /// All effect intents, including earlier attempts.
    pub steps: Vec<StepRecord>,
}

/// Read and transition unresolved operations without changing their confirmed values.
pub trait RecoveryJournal: OperationJournal {
    /// Marks interrupted executions uncertain and returns every unresolved operation.
    fn recover_on_startup(&self) -> Result<Vec<RecoverableOperation>, StoreConflict>;
    /// Returns the persisted state of one operation.
    fn recoverable(&self, operation_id: &str) -> Result<RecoverableOperation, StoreConflict>;
    /// Records a fresh observation of an unresolved step without changing an earlier failure.
    fn reconcile_step(
        &self,
        operation_id: &str,
        sequence: u64,
        outcome: crate::operation_journal::StepOutcome,
    ) -> Result<(), StoreConflict>;
    /// Abandons only the exact attempt whose absence was independently verified.
    fn abandon_verified(&self, evidence: &VerifiedAbsence) -> Result<(), StoreConflict>;
}

/// Proof that a read-only probe established absence for one unchanged attempt.
pub struct VerifiedAbsence {
    operation_id: String,
    attempt: u64,
}

impl VerifiedAbsence {
    /// Returns the operation to which this proof is bound.
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// Returns the attempt to which this proof is bound.
    pub fn attempt(&self) -> u64 {
        self.attempt
    }
}

/// Evidence collected against the original, fixed operation target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryEvidence {
    /// The previous change CLI is known to have exited.
    pub previous_cli_exited: bool,
    /// The current endpoint and Engine ID match the confirmed target.
    pub target_matches: bool,
    /// The artifact state and published bytes match the journal and recorded hashes.
    pub artifact_matches: bool,
    /// Every storage allocation matches the ledger, with no Missing or unknown owner.
    pub storage_verified: bool,
    /// Docker ownership and configuration, or absence on the fixed target, were verified.
    pub docker_verified: bool,
    /// Current runtime observation, separate from the historical operation status.
    pub runtime: CurrentRuntime,
}

/// Collects fresh, read-only evidence from the recorded target and resources.
/// Implementations must not infer ownership from a resource name alone.
pub trait RecoveryProbe {
    /// Checks CLI liveness, target identity, artifact, storage and Docker state.
    fn inspect(
        &self,
        operation: &RecoverableOperation,
    ) -> impl std::future::Future<Output = RecoveryEvidence> + Send;
}

/// Current runtime state used only for a recovery decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentRuntime {
    /// The verified resource is Ready.
    Ready,
    /// The verified resource exists but is not Ready.
    Present,
    /// The verified resource is stopped.
    Stopped,
    /// Absence was established on the confirmed target.
    Absent,
    /// The state or ownership could not be established.
    Unknown,
}

/// Safe result of read-only reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryDecision {
    /// Existing results can be committed without another external change.
    Complete,
    /// A new attempt may be started explicitly after rechecking evidence.
    RetryAllowed,
    /// Keep the operation and every reservation unresolved.
    Hold,
}

/// Historical status and current observation are kept as separate display values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    /// The persisted operation, including its earlier failure and fixed identity.
    pub operation: RecoverableOperation,
    /// Current observed runtime, which may be Ready after a failure.
    pub current_runtime: CurrentRuntime,
    /// Action permitted by the complete read-only evidence set.
    pub decision: RecoveryDecision,
}

/// Classifies evidence without executing Docker commands or releasing reservations.
/// An earlier Failed status is retained in the operation snapshot for display.
#[must_use]
pub fn classify_recovery(
    operation: &RecoverableOperation,
    evidence: RecoveryEvidence,
) -> RecoveryDecision {
    if !evidence.previous_cli_exited
        || !evidence.target_matches
        || !evidence.artifact_matches
        || !evidence.storage_verified
    {
        return RecoveryDecision::Hold;
    }
    let ready_operation = matches!(
        operation.kind,
        OperationKind::Create
            | OperationKind::Clone
            | OperationKind::Start
            | OperationKind::Restart
    ) && operation.steps.iter().any(|step| {
        matches!(
            step.command_kind,
            StepCommand::ComposeStart | StepCommand::Observe
        )
    });
    if ready_operation && evidence.docker_verified && evidence.runtime == CurrentRuntime::Ready {
        return RecoveryDecision::Complete;
    }
    // A missing resource is deliberately not treated as proof that the prior
    // command did nothing. It may have initialized storage before disappearing.
    if evidence.docker_verified
        && evidence.runtime == CurrentRuntime::Stopped
        && operation.kind == OperationKind::Start
        && operation.status == OperationStatus::Failed
        && !operation.steps.is_empty()
        && operation.steps.iter().all(|step| step.outcome.is_some())
    {
        return RecoveryDecision::RetryAllowed;
    }
    RecoveryDecision::Hold
}

/// Returns a fresh reconciliation report without changing the operation.
pub async fn resolve_operation<J: RecoveryJournal, P: RecoveryProbe>(
    journal: &J,
    operation_id: &str,
    probe: &P,
) -> Result<RecoveryReport, StoreConflict> {
    let operation = journal.recoverable(operation_id)?;
    let evidence = probe.inspect(&operation).await;
    Ok(RecoveryReport {
        decision: classify_recovery(&operation, evidence),
        current_runtime: evidence.runtime,
        operation,
    })
}

/// Begins another attempt only after a fresh read-only reconciliation.
pub async fn retry_operation<J: RecoveryJournal, P: RecoveryProbe>(
    journal: &J,
    operation_id: &str,
    probe: &P,
) -> Result<u64, StoreConflict> {
    let report = resolve_operation(journal, operation_id, probe).await?;
    if report.decision != RecoveryDecision::RetryAllowed {
        return Err(StoreConflict::InvalidLifecycle);
    }
    journal.retry(operation_id, &report.operation.phase)
}

/// Abandons an operation only after fresh target, resource and CLI checks prove absence.
pub async fn abandon_operation<J: RecoveryJournal, P: RecoveryProbe>(
    journal: &J,
    operation_id: &str,
    probe: &P,
) -> Result<(), StoreConflict> {
    let operation = journal.recoverable(operation_id)?;
    let evidence = probe.inspect(&operation).await;
    let safe = matches!(
        operation.status,
        OperationStatus::Failed
            | OperationStatus::AwaitingDecision
            | OperationStatus::OutcomeUnknown
    ) && evidence.previous_cli_exited
        && evidence.target_matches
        && evidence.artifact_matches
        && evidence.storage_verified
        && evidence.docker_verified
        && evidence.runtime == CurrentRuntime::Absent
        && operation.steps.iter().all(|step| {
            matches!(
                step.outcome,
                Some(
                    crate::operation_journal::StepOutcome::Succeeded
                        | crate::operation_journal::StepOutcome::Failed
                )
            )
        });
    if !safe {
        return Err(StoreConflict::InvalidLifecycle);
    }
    journal.abandon_verified(&VerifiedAbsence {
        operation_id: operation.id,
        attempt: operation.attempt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_journal::{ExpectedResult, StepOutcome};

    fn operation(status: OperationStatus) -> RecoverableOperation {
        RecoverableOperation {
            id: "op".into(),
            instance_id: "instance".into(),
            kind: OperationKind::Create,
            status,
            phase: "start".into(),
            attempt: 1,
            steps: vec![StepRecord {
                instance_id: "instance".into(),
                scope_id: "scope".into(),
                sequence: 1,
                attempt: 1,
                command_kind: StepCommand::ComposeStart,
                resource_id: "container".into(),
                expected_result: ExpectedResult::ContainerRunning,
                outcome: Some(StepOutcome::Failed),
            }],
        }
    }

    fn evidence() -> RecoveryEvidence {
        RecoveryEvidence {
            previous_cli_exited: true,
            target_matches: true,
            artifact_matches: true,
            storage_verified: true,
            docker_verified: true,
            runtime: CurrentRuntime::Ready,
        }
    }

    #[test]
    fn ready_can_be_completed_without_erasing_prior_failure() {
        assert_eq!(
            classify_recovery(&operation(OperationStatus::Failed), evidence()),
            RecoveryDecision::Complete
        );
    }

    #[test]
    fn uncertain_evidence_and_missing_resource_keep_reservations() {
        let op = operation(OperationStatus::OutcomeUnknown);
        for changed in [
            RecoveryEvidence {
                previous_cli_exited: false,
                ..evidence()
            },
            RecoveryEvidence {
                target_matches: false,
                ..evidence()
            },
            RecoveryEvidence {
                artifact_matches: false,
                ..evidence()
            },
            RecoveryEvidence {
                storage_verified: false,
                ..evidence()
            },
            RecoveryEvidence {
                docker_verified: false,
                ..evidence()
            },
            RecoveryEvidence {
                runtime: CurrentRuntime::Absent,
                ..evidence()
            },
        ] {
            assert_eq!(classify_recovery(&op, changed), RecoveryDecision::Hold);
        }
    }

    #[test]
    fn stopped_start_can_retry_but_unrelated_operation_cannot_complete() {
        let mut op = operation(OperationStatus::Failed);
        op.kind = OperationKind::Start;
        assert_eq!(
            classify_recovery(
                &op,
                RecoveryEvidence {
                    runtime: CurrentRuntime::Stopped,
                    ..evidence()
                }
            ),
            RecoveryDecision::RetryAllowed
        );
        op.kind = OperationKind::Delete;
        assert_eq!(classify_recovery(&op, evidence()), RecoveryDecision::Hold);
    }
}
