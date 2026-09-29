//! Confirmed inputs shared by creation and lifecycle operations.

use crate::{
    operation_journal::RequestReceipt,
    state_store::{
        PortAllocation, RuntimeTarget, StorageAllocation, StorageLedgerEntry, StoreConflict,
        TemplateFile,
    },
};

/// Immutable records needed to execute an accepted create or clone operation.
#[derive(Debug, Clone)]
pub struct ConfirmedCreate {
    /// Instance identity and the management scope that owns it.
    pub instance_id: String,
    pub scope_id: String,
    /// Dedicated Compose project.
    pub project_name: String,
    /// Fixed Docker target checked at confirmation.
    pub target: RuntimeTarget,
    /// Current spec revision selected by the operation.
    pub spec_revision: u64,
    /// Selected version in the private template snapshot.
    pub selected_version: String,
    /// Original snapshot files, including every version definition.
    pub snapshot_files: Vec<TemplateFile>,
    /// Confirmed inputs, including secrets; never emit this value in progress events.
    pub inputs_json: String,
    /// Committed TCP reservations and bindings.
    pub ports: Vec<PortAllocation>,
    /// Independently owned writable storage slots.
    pub storage: Vec<StorageLedgerEntry>,
}

/// Persists creation milestones without releasing confirmed allocations on failure.
pub trait CreateStateStore: Send + Sync {
    /// Reads confirmed records for an accepted create, clone, or lifecycle request.
    fn confirmed_create(&self, receipt: &RequestReceipt) -> Result<ConfirmedCreate, StoreConflict>;

    /// Records proof from an exclusively created bind directory before it can be mounted.
    fn record_bind_materialization(
        &self,
        operation_id: &str,
        allocation: &StorageAllocation,
    ) -> Result<(), StoreConflict>;

    /// Marks every present storage slot as possibly initialized before start is sent.
    fn mark_may_have_initialized(&self, operation_id: &str) -> Result<(), StoreConflict>;

    /// Atomically records a verified Ready observation and the applied spec revision.
    /// Call only after the runtime adapter has verified ownership and full configuration.
    fn complete_ready(&self, operation_id: &str, container_id: &str) -> Result<(), StoreConflict>;
}
