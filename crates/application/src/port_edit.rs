//! Durable boundaries for changing published ports while an instance is stopped.

use crate::{
    operation_journal::RequestReceipt,
    state_store::{PortAllocation, StoreConflict},
};

/// Complete candidate bindings for a stopped instance, including unchanged slots.
#[derive(Debug, Clone)]
pub struct PortEditRequest {
    /// Idempotent request identity and operation identifier.
    pub receipt: RequestReceipt,
    /// Instance revision observed before external checks.
    pub expected_instance_revision: u64,
    /// Current committed spec revision.
    pub old_spec_revision: u64,
    /// Complete candidate bindings keyed by stable slot.
    pub ports: Vec<PortAllocation>,
}

/// Persistence of a proposed edit and its observed successful application.
pub trait PortEditStore: Send + Sync {
    /// Records candidate spec, changed reservations, pending change, and operation atomically.
    /// The caller must have just observed Stopped or Absent on the fixed Docker target.
    fn begin_port_edit(&self, request: &PortEditRequest) -> Result<RequestReceipt, StoreConflict>;

    /// Returns the candidate revision's complete bindings for an accepted edit.
    fn pending_ports(&self, operation_id: &str) -> Result<Vec<PortAllocation>, StoreConflict>;

    /// Switches the spec and reservations after full stopped-container verification.
    fn complete_port_edit(
        &self,
        operation_id: &str,
        container_id: &str,
    ) -> Result<(), StoreConflict>;
}

/// Confirmed replacement of the current candidate within the original operation.
#[derive(Debug, Clone)]
pub struct PortRecoveryRequest {
    /// Original accepted request; identity, secrets and storage are never regenerated.
    pub receipt: RequestReceipt,
    /// Candidate revision shown to the user, preventing stale confirmations.
    pub expected_candidate_revision: u64,
    /// Complete bindings explicitly confirmed by the user.
    pub ports: Vec<PortAllocation>,
}

/// Durable port recovery decisions; callers must freshly verify external evidence.
pub trait PortRecoveryStore: PortEditStore {
    /// Records a confirmed candidate without releasing any earlier reservation.
    /// The caller must have verified the prior CLI exited and the instance is stopped or absent.
    fn confirm_port_recovery(&self, request: &PortRecoveryRequest) -> Result<u64, StoreConflict>;

    /// Returns original and candidate revisions for an unresolved pending change.
    fn port_change_revisions(&self, operation_id: &str) -> Result<(u64, u64), StoreConflict>;

    /// Finishes an edit's explicit restoration after verifying the old artifact and stopped runtime.
    fn complete_port_restore(
        &self,
        operation_id: &str,
        container_id: &str,
    ) -> Result<(), StoreConflict>;
}
