//! Persistence boundary for operation intent, progress, and idempotent requests.

use crate::state_store::StoreConflict;
pub use composenest_domain::instance::{OperationKind, OperationStatus};

/// An operation that has been durably accepted for an instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationIntent {
    /// Stable identifier reused across attempts.
    pub id: String,
    /// Instance whose external resources may change.
    pub instance_id: String,
    /// Validated operation kind, such as `start` or `edit_port`.
    pub kind: OperationKind,
    /// Initial stage name, without command arguments.
    pub phase: String,
    /// Revision checked when the operation is accepted.
    pub expected_revision: u64,
    /// Existing configuration revision, if applicable.
    pub old_spec_revision: Option<u64>,
    /// Intended configuration revision, if applicable.
    pub new_spec_revision: Option<u64>,
}

/// Identity and result of a confirmed request or plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestReceipt {
    /// Management scope containing the request.
    pub scope_id: String,
    /// Request identifier scoped to `scope_id`.
    pub request_id: String,
    /// Optional plan identifier, also unique within the scope.
    pub plan_id: Option<String>,
    /// Confirmed plan or instance revision.
    pub confirmed_revision: u64,
    /// Hash of canonical, non-secret request identity data.
    pub request_hash: String,
    /// Instance returned to repeated callers.
    pub instance_id: String,
    /// Operation returned to repeated callers.
    pub operation_id: String,
}

/// One durable external effect intent, without command arguments or inspect data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepIntent {
    /// Parent operation.
    pub operation_id: String,
    /// Monotonic step number within the operation.
    pub sequence: u64,
    /// Attempt number for this operation.
    pub attempt: u64,
    /// Fixed command category.
    pub command_kind: StepCommand,
    /// Stable resource identifier.
    pub resource_id: String,
    /// Expected state, without command arguments.
    pub expected_result: ExpectedResult,
}

/// Allowed effect categories; arbitrary command lines cannot enter the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepCommand {
    GenerateArtifact,
    ResolveImage,
    ComposeCreate,
    ComposeStart,
    ComposeStop,
    RemoveContainer,
    Observe,
}

impl StepCommand {
    /// Returns the stable database representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GenerateArtifact => "generate_artifact",
            Self::ResolveImage => "resolve_image",
            Self::ComposeCreate => "compose_create",
            Self::ComposeStart => "compose_start",
            Self::ComposeStop => "compose_stop",
            Self::RemoveContainer => "remove_container",
            Self::Observe => "observe",
        }
    }
}

/// Expected observable outcome of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedResult {
    ArtifactReady,
    ImageResolved,
    ContainerCreated,
    ContainerRunning,
    ContainerStopped,
    ContainerAbsent,
    StateObserved,
}

impl ExpectedResult {
    /// Returns the stable database representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ArtifactReady => "artifact_ready",
            Self::ImageResolved => "image_resolved",
            Self::ContainerCreated => "container_created",
            Self::ContainerRunning => "container_running",
            Self::ContainerStopped => "container_stopped",
            Self::ContainerAbsent => "container_absent",
            Self::StateObserved => "state_observed",
        }
    }
}

/// Observed result of a previously recorded step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    Succeeded,
    Failed,
    Unknown,
}

impl StepOutcome {
    /// Returns the stable database representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}

/// Stores operation progress before and after external effects.
pub trait OperationJournal {
    /// Atomically accepts an operation and its receipt, or returns the matching prior receipt.
    fn accept(
        &self,
        intent: &OperationIntent,
        receipt: &RequestReceipt,
    ) -> Result<RequestReceipt, StoreConflict>;

    /// Finds a persisted request after process restart.
    fn receipt(
        &self,
        scope_id: &str,
        request_id: &str,
    ) -> Result<Option<RequestReceipt>, StoreConflict>;

    /// Finds a confirmed plan after process restart.
    fn plan_receipt(
        &self,
        scope_id: &str,
        plan_id: &str,
    ) -> Result<Option<RequestReceipt>, StoreConflict>;

    /// Records intent before the external action is sent.
    fn record_step(&self, step: &StepIntent) -> Result<(), StoreConflict>;

    /// Records the observed result of one step once.
    fn finish_step(
        &self,
        operation_id: &str,
        sequence: u64,
        outcome: StepOutcome,
    ) -> Result<(), StoreConflict>;

    /// Starts another attempt on the same operation after an unresolved outcome.
    fn retry(&self, operation_id: &str, phase: &str) -> Result<u64, StoreConflict>;

    /// Advances an operation or resolves it after observation.
    fn set_status(
        &self,
        operation_id: &str,
        status: OperationStatus,
        phase: &str,
    ) -> Result<(), StoreConflict>;
}
