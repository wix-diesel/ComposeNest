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
