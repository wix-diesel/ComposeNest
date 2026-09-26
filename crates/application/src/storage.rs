//! Storage operations that must distinguish allocation from verification.

use std::path::PathBuf;

use composenest_domain::{
    identity::{InstanceId, SlotId},
    instance::StoragePresence,
};

use crate::state_store::StorageAllocation;

/// A bind-storage operation failed without making an existing directory trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageError {
    /// Existing files or missing proof prevent establishing ownership.
    Unverified,
    /// A previously allocated directory or its proof is missing.
    Missing,
    /// The persisted allocation does not describe this instance and slot.
    InvalidAllocation,
    /// The operating system rejected the requested operation.
    Backend,
}

/// Creates and verifies bind storage without importing an existing directory.
pub trait StoragePort {
    /// Exclusively creates every slot directory for a new instance.
    ///
    /// The operation never adopts existing paths. If any path already exists or
    /// the operation is interrupted, callers must treat the result as unverified.
    fn create_bind_set(
        &self,
        instance_id: InstanceId,
        slots: &[SlotId],
    ) -> Result<Vec<StorageAllocation>, StorageError>;

    /// Reports whether a recorded allocation still has matching ownership proof.
    fn inspect_bind(
        &self,
        instance_id: InstanceId,
        allocation: &StorageAllocation,
    ) -> StoragePresence;

    /// Returns a mount source only after rechecking the stored proof and path identity.
    fn bind_path(
        &self,
        instance_id: InstanceId,
        allocation: &StorageAllocation,
    ) -> Result<PathBuf, StorageError>;
}
