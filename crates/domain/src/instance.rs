//! Independent axes of instance identity, configuration and observed state.

use crate::identity::{DisplayName, InstanceId};

/// A confirmed instance's identity and current display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    id: InstanceId,
    name: DisplayName,
    target_id: String,
    revision: u64,
    lifecycle: Lifecycle,
}

impl Instance {
    /// Creates an instance after the application has allocated its ID and target.
    #[must_use]
    pub fn new(id: InstanceId, name: DisplayName, target_id: String) -> Self {
        Self {
            id,
            name,
            target_id,
            revision: 1,
            lifecycle: Lifecycle::Managed,
        }
    }

    /// Returns the immutable ID.
    #[must_use]
    pub const fn id(&self) -> InstanceId {
        self.id
    }

    /// Returns the immutable runtime target identifier.
    #[must_use]
    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    /// Returns the stable Compose project name.
    #[must_use]
    pub fn project_name(&self) -> String {
        self.id.compose_project_name()
    }

    /// Returns the current canonical display name.
    #[must_use]
    pub fn name(&self) -> &DisplayName {
        &self.name
    }

    /// Returns the instance update revision, separate from the spec revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the management lifecycle, independent of runtime observation.
    #[must_use]
    pub const fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
    }

    /// Renames a managed instance without changing its runtime configuration.
    pub fn rename(
        &mut self,
        name: DisplayName,
        expected_revision: u64,
    ) -> Result<(), InstanceError> {
        self.check_revision(expected_revision)?;
        if self.lifecycle != Lifecycle::Managed {
            return Err(InstanceError::InvalidLifecycle);
        }
        if self.name != name {
            self.revision = self
                .revision
                .checked_add(1)
                .ok_or(InstanceError::RevisionExhausted)?;
            self.name = name;
        }
        Ok(())
    }

    /// Starts deletion while retaining the instance identity and allocations.
    pub fn begin_retirement(&mut self, expected_revision: u64) -> Result<(), InstanceError> {
        self.check_revision(expected_revision)?;
        if self.lifecycle != Lifecycle::Managed {
            return Err(InstanceError::InvalidLifecycle);
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(InstanceError::RevisionExhausted)?;
        self.lifecycle = Lifecycle::Retiring;
        Ok(())
    }

    /// Marks removal complete after the application has verified runtime absence.
    pub fn finish_retirement(&mut self, absence_verified: bool) -> Result<(), InstanceError> {
        if self.lifecycle != Lifecycle::Retiring {
            return Err(InstanceError::InvalidLifecycle);
        }
        if !absence_verified {
            return Err(InstanceError::UnverifiedAbsence);
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(InstanceError::RevisionExhausted)?;
        self.lifecycle = Lifecycle::Retired;
        Ok(())
    }

    fn check_revision(&self, expected: u64) -> Result<(), InstanceError> {
        if self.revision == expected {
            Ok(())
        } else {
            Err(InstanceError::StaleRevision)
        }
    }
}

/// Failure to apply an instance update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceError {
    StaleRevision,
    InvalidLifecycle,
    UnverifiedAbsence,
    RevisionExhausted,
}

/// The application's management relationship with an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Managed,
    Retiring,
    Retired,
}

/// The chosen data storage method, fixed for a confirmed instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageMethod {
    BindMount,
    NamedVolume,
}

/// One version of the confirmed runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecRevision {
    /// Monotonically increasing version number.
    pub number: u64,
    /// Chosen method shared by all writable slots.
    pub storage_method: StorageMethod,
}

/// A proposed change kept distinct from the confirmed spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingChange {
    /// Confirmed version that remains authoritative until application succeeds.
    pub previous: SpecRevision,
    /// Candidate version to apply after verification.
    pub candidate: SpecRevision,
}

impl PendingChange {
    /// Validates that the candidate is the next version and keeps the storage method.
    pub fn new(previous: SpecRevision, candidate: SpecRevision) -> Result<Self, SpecError> {
        if previous.number.checked_add(1) != Some(candidate.number) {
            return Err(SpecError::InvalidRevision);
        }
        if previous.storage_method != candidate.storage_method {
            return Err(SpecError::StorageMethodChanged);
        }
        Ok(Self {
            previous,
            candidate,
        })
    }
}

/// Invalid change to a confirmed instance spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecError {
    InvalidRevision,
    StorageMethodChanged,
}

/// Ownership of a storage allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageOwnership {
    Assigned,
    Retained,
}

/// Existence evidence for a storage allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoragePresence {
    NotMaterialized,
    Present,
    Missing,
    Unverified,
}

/// Initialization can advance but is not reset by failed operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Initialization {
    NotAttempted,
    MayHaveInitialized,
    ReadyObserved,
}

/// A fresh observation of the runtime; it does not change the confirmed spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeObservation {
    /// Time supplied by the observing adapter, in Unix seconds.
    pub observed_at_unix_seconds: u64,
    /// Current observed status, including failure to observe.
    pub status: RuntimeStatus,
}

/// Current observed state. Running without verified readiness is Preparing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStatus {
    Unknown(ObservationFailure),
    Absent,
    Stopped,
    Preparing,
    Ready,
    Unhealthy,
}

/// Why current runtime state could not be established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationFailure {
    NotYetObserved,
    Unreachable,
    PermissionDenied,
    Inconclusive,
}

/// Whether generated files and observed runtime match the confirmed spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchStatus {
    Unknown,
    Matching,
    Different,
    Missing,
}

/// Separate evidence of generated and applied runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationStatus {
    /// Version from which the current artifact was generated, if known.
    pub generated_spec_revision: Option<u64>,
    /// Last version confirmed as applied to Docker, if known.
    pub applied_spec_revision: Option<u64>,
    /// Current artifact comparison against its recorded content.
    pub artifact_match: MatchStatus,
    /// Current observed runtime comparison against the confirmed spec.
    pub runtime_match: MatchStatus,
}

/// Kind of requested instance change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    Create,
    Clone,
    Start,
    Stop,
    Restart,
    Rename,
    EditPort,
    Delete,
}

/// Progress of one change intent, independent of current runtime state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStatus {
    Accepted,
    Executing,
    Succeeded,
    AwaitingDecision,
    Failed,
    OutcomeUnknown,
    Abandoned,
}

impl OperationStatus {
    /// Returns whether this operation still prevents another change intent.
    #[must_use]
    pub const fn is_unresolved(self) -> bool {
        !matches!(self, Self::Succeeded | Self::Abandoned)
    }
}

/// One durable operation intent; retries retain its ID and advance its attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    /// Externally allocated operation identifier.
    id: u128,
    /// Instance this operation may change.
    instance_id: InstanceId,
    /// Requested operation kind.
    kind: OperationKind,
    /// Current progress, separate from runtime observation.
    status: OperationStatus,
    /// Number of external execution attempts.
    attempt: u32,
}

impl Operation {
    /// Creates a recorded intent before any external execution.
    #[must_use]
    pub const fn new(id: u128, instance_id: InstanceId, kind: OperationKind) -> Self {
        Self {
            id,
            instance_id,
            kind,
            status: OperationStatus::Accepted,
            attempt: 0,
        }
    }

    /// Returns the stable operation identifier.
    #[must_use]
    pub const fn id(&self) -> u128 {
        self.id
    }

    /// Returns the instance affected by this intent.
    #[must_use]
    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    /// Returns the requested change kind.
    #[must_use]
    pub const fn kind(&self) -> OperationKind {
        self.kind
    }

    /// Returns current operation progress.
    #[must_use]
    pub const fn status(&self) -> OperationStatus {
        self.status
    }

    /// Returns the number of external execution attempts.
    #[must_use]
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Records a permitted state change, preserving the operation ID.
    pub fn transition(&mut self, next: OperationStatus) -> Result<(), OperationError> {
        use OperationStatus::{
            Abandoned, Accepted, AwaitingDecision, Executing, Failed, OutcomeUnknown, Succeeded,
        };
        let allowed = matches!(
            (self.status, next),
            (Accepted, Executing)
                | (
                    Executing,
                    Succeeded | AwaitingDecision | Failed | OutcomeUnknown
                )
                | (OutcomeUnknown, Succeeded | Failed | AwaitingDecision)
                | (Failed | AwaitingDecision, Executing | Abandoned)
        );
        if !allowed {
            return Err(OperationError::InvalidTransition);
        }
        if next == Executing {
            self.attempt = self
                .attempt
                .checked_add(1)
                .ok_or(OperationError::AttemptExhausted)?;
        }
        self.status = next;
        Ok(())
    }
}

/// Invalid operation progress change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationError {
    InvalidTransition,
    AttemptExhausted,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance() -> Instance {
        Instance::new(
            InstanceId::from_u128(7),
            DisplayName::parse("First").unwrap(),
            "target-a".into(),
        )
    }

    #[test]
    fn rename_changes_only_name_and_instance_revision() {
        let mut value = instance();
        let id = value.id();
        let target = value.target_id().to_owned();
        let project = value.project_name();
        assert_eq!(
            value.rename(DisplayName::parse("Second").unwrap(), 0),
            Err(InstanceError::StaleRevision)
        );
        value
            .rename(DisplayName::parse("Second").unwrap(), 1)
            .unwrap();
        assert_eq!(
            (value.id(), value.target_id(), value.project_name()),
            (id, target.as_str(), project)
        );
        assert_eq!(value.revision(), 2);
    }

    #[test]
    fn retirement_requires_confirmed_absence() {
        let mut value = instance();
        value.begin_retirement(1).unwrap();
        assert_eq!(
            value.finish_retirement(false),
            Err(InstanceError::UnverifiedAbsence)
        );
        assert_eq!(value.lifecycle(), Lifecycle::Retiring);
        value.finish_retirement(true).unwrap();
        assert_eq!(value.lifecycle(), Lifecycle::Retired);
        assert_eq!(
            value.rename(DisplayName::parse("New").unwrap(), value.revision()),
            Err(InstanceError::InvalidLifecycle)
        );
    }

    #[test]
    fn pending_change_preserves_confirmed_storage_method() {
        let old = SpecRevision {
            number: 2,
            storage_method: StorageMethod::BindMount,
        };
        let next = SpecRevision {
            number: 3,
            storage_method: StorageMethod::BindMount,
        };
        assert!(PendingChange::new(old.clone(), next.clone()).is_ok());
        assert_eq!(
            PendingChange::new(
                old.clone(),
                SpecRevision {
                    number: 4,
                    ..next.clone()
                }
            ),
            Err(SpecError::InvalidRevision)
        );
        assert_eq!(
            PendingChange::new(
                old,
                SpecRevision {
                    storage_method: StorageMethod::NamedVolume,
                    ..next
                }
            ),
            Err(SpecError::StorageMethodChanged)
        );
    }

    #[test]
    fn runtime_and_operation_results_are_independent() {
        let observation = RuntimeObservation {
            observed_at_unix_seconds: 5,
            status: RuntimeStatus::Ready,
        };
        let unknown = RuntimeStatus::Unknown(ObservationFailure::Unreachable);
        assert_ne!(observation.status, unknown);
        assert_ne!(unknown, RuntimeStatus::Absent);
        assert_ne!(RuntimeStatus::Preparing, RuntimeStatus::Ready);
        let mut operation = Operation::new(11, InstanceId::from_u128(7), OperationKind::Start);
        operation.transition(OperationStatus::Executing).unwrap();
        operation.transition(OperationStatus::Failed).unwrap();
        assert_eq!(operation.status(), OperationStatus::Failed);
        assert_eq!(observation.status, RuntimeStatus::Ready);
        assert!(operation.status().is_unresolved());
        operation.transition(OperationStatus::Executing).unwrap();
        assert_eq!((operation.id(), operation.attempt()), (11, 2));
        assert_eq!(
            operation.transition(OperationStatus::Accepted),
            Err(OperationError::InvalidTransition)
        );
    }

    #[test]
    fn missing_storage_is_not_unmaterialized_storage() {
        assert_ne!(StoragePresence::Missing, StoragePresence::NotMaterialized);
        assert_ne!(
            StoragePresence::Unverified,
            StoragePresence::NotMaterialized
        );
        assert_ne!(StorageOwnership::Retained, StorageOwnership::Assigned);
        assert!(Initialization::MayHaveInitialized > Initialization::NotAttempted);
    }
}
