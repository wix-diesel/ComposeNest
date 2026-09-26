//! Named-volume allocation contracts and deterministic Docker identities.

use std::{collections::HashSet, future::Future};

use composenest_domain::identity::{InstanceId, SlotId};

use crate::{
    operation_journal::OperationJournal,
    state_store::{StateStore, StorageAllocation, StorageLedgerEntry},
};

/// Why a named-volume operation could not establish safe ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedVolumeError {
    /// The recorded volume was previously present but is now missing.
    Missing,
    /// The Docker state or ownership labels could not be verified.
    Unverified,
    /// The persisted allocation does not match this instance and slot.
    InvalidAllocation,
    /// The operation or slot input is invalid.
    InvalidInput,
    /// Persistence or Docker failed unexpectedly.
    Backend,
}

/// Creates the deterministic name for one isolated writable slot.
#[must_use]
pub fn named_volume_name(instance_id: InstanceId, slot: &SlotId) -> String {
    format!("cn-{:032x}-{}", instance_id.as_u128(), slot.as_str())
}

/// Builds allocations before the instance and operation are committed to the ledger.
pub fn named_volume_allocations(
    instance_id: InstanceId,
    operation_id: &str,
    slots: &[SlotId],
) -> Result<Vec<StorageAllocation>, NamedVolumeError> {
    if !valid_label_value(operation_id) {
        return Err(NamedVolumeError::InvalidInput);
    }
    let mut seen = HashSet::new();
    let mut allocations = Vec::with_capacity(slots.len());
    for slot in slots {
        if !seen.insert(slot.as_str()) {
            return Err(NamedVolumeError::InvalidInput);
        }
        allocations.push(StorageAllocation {
            slot: slot.as_str().to_owned(),
            resource_identity: named_volume_name(instance_id, slot),
            ownership_evidence: operation_id.to_owned(),
        });
    }
    Ok(allocations)
}

/// Performs an idempotent allocation against one fixed Docker Engine.
pub trait NamedVolumePort: Send + Sync {
    /// Creates a volume or resumes the same journaled allocation after verifying all labels.
    fn ensure_named_volume<'a>(
        &'a self,
        state: &'a dyn StateStore,
        journal: &'a dyn OperationJournal,
        instance_id: InstanceId,
        slot: &'a SlotId,
        attempt: u64,
        sequence: u64,
    ) -> impl Future<Output = Result<StorageLedgerEntry, NamedVolumeError>> + Send + 'a;

    /// Refreshes existence and ownership in the ledger without creating a missing volume.
    fn inspect_named_volume<'a>(
        &'a self,
        state: &'a dyn StateStore,
        journal: &'a dyn OperationJournal,
        instance_id: InstanceId,
        slot: &'a SlotId,
    ) -> impl Future<Output = Result<StorageLedgerEntry, NamedVolumeError>> + Send + 'a;
}

/// Accepts a bounded identifier that can be stored as a Docker label value.
#[must_use]
pub fn valid_label_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_stable_names_and_persisted_operation_evidence() {
        let instance_id = InstanceId::from_u128(42);
        let slots = [
            SlotId::parse("data").unwrap(),
            SlotId::parse("logs-1").unwrap(),
        ];
        let allocations = named_volume_allocations(instance_id, "op-42", &slots).unwrap();

        assert_eq!(
            allocations[0].resource_identity,
            "cn-0000000000000000000000000000002a-data"
        );
        assert_eq!(allocations[0].ownership_evidence, "op-42");
        assert_eq!(allocations[1].slot, "logs-1");
        assert_ne!(
            allocations[0].resource_identity,
            named_volume_name(InstanceId::from_u128(43), &slots[0])
        );
    }

    #[test]
    fn rejects_duplicate_slots_and_unsafe_operation_ids() {
        let instance_id = InstanceId::from_u128(1);
        let duplicate = SlotId::parse("data").unwrap();
        assert_eq!(
            named_volume_allocations(instance_id, "op-1", &[duplicate.clone(), duplicate]),
            Err(NamedVolumeError::InvalidInput)
        );
        assert_eq!(
            named_volume_allocations(instance_id, "../op", &[]),
            Err(NamedVolumeError::InvalidInput)
        );
        assert!(
            named_volume_allocations(instance_id, "op-1", &[])
                .unwrap()
                .is_empty()
        );
    }
}
