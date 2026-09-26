//! Verified bind-storage allocation under the protected management root.

use std::path::{Path, PathBuf};

use composenest_application::{
    state_store::StorageAllocation,
    storage::{StorageError, StoragePort},
};
use composenest_domain::{
    identity::{InstanceId, SlotId},
    instance::StoragePresence,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const EVIDENCE_VERSION: u64 = 1;
const MAX_EVIDENCE_BYTES: u64 = 16 * 1024;

/// Creates and verifies instance storage inside a management root.
pub struct BindStorage {
    root: PathBuf,
}

impl BindStorage {
    /// Uses an existing management root. The root is checked without following its final link.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the management root used by this adapter.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl StoragePort for BindStorage {
    fn create_bind_set(
        &self,
        instance_id: InstanceId,
        slots: &[SlotId],
    ) -> Result<Vec<StorageAllocation>, StorageError> {
        if slots.is_empty() {
            return Ok(Vec::new());
        }
        if !self.root.is_absolute() {
            return Err(StorageError::Unverified);
        }
        platform::create(&self.root, instance_id, slots)
    }

    fn inspect_bind(
        &self,
        instance_id: InstanceId,
        allocation: &StorageAllocation,
    ) -> StoragePresence {
        if !self.root.is_absolute() {
            return StoragePresence::Unverified;
        }
        platform::inspect(&self.root, instance_id, allocation)
    }

    fn bind_path(
        &self,
        instance_id: InstanceId,
        allocation: &StorageAllocation,
    ) -> Result<PathBuf, StorageError> {
        match self.inspect_bind(instance_id, allocation) {
            StoragePresence::Present => Ok(self.root.join(&allocation.resource_identity)),
            StoragePresence::Missing => Err(StorageError::Missing),
            _ => Err(StorageError::Unverified),
        }
    }
}

fn instance_key(instance_id: InstanceId) -> String {
    format!("{:032x}", instance_id.as_u128())
}

fn allocation_path(instance_id: InstanceId, slot: &SlotId) -> String {
    format!("data/{}/{}", instance_key(instance_id), slot.as_str())
}

fn valid_allocation(instance_id: InstanceId, allocation: &StorageAllocation) -> Option<SlotId> {
    let slot = SlotId::parse(&allocation.slot).ok()?;
    (allocation.resource_identity == allocation_path(instance_id, &slot)
        && is_lower_hex_digest(&allocation.ownership_evidence))
    .then_some(slot)
}

fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn digest(contents: &[u8]) -> String {
    Sha256::digest(contents)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn proof_json(
    instance_id: InstanceId,
    slot: Option<&SlotId>,
    root_identity: [u64; 2],
    data_identity: [u64; 2],
    instance_identity: [u64; 2],
    slot_identity: Option<[u64; 2]>,
    nonce: &str,
) -> Vec<u8> {
    json!({
        "version": EVIDENCE_VERSION,
        "instanceId": instance_key(instance_id),
        "slot": slot.map(SlotId::as_str),
        "rootIdentity": root_identity,
        "dataIdentity": data_identity,
        "instanceIdentity": instance_identity,
        "slotIdentity": slot_identity,
        "nonce": nonce,
    })
    .to_string()
    .into_bytes()
}

fn proof_matches(
    contents: &[u8],
    instance_id: InstanceId,
    slot: Option<&SlotId>,
    evidence: Option<&str>,
) -> Option<Value> {
    if contents.len() as u64 > MAX_EVIDENCE_BYTES
        || evidence.is_some_and(|expected| digest(contents) != expected)
    {
        return None;
    }
    let proof: Value = serde_json::from_slice(contents).ok()?;
    (proof["version"].as_u64() == Some(EVIDENCE_VERSION)
        && proof["instanceId"].as_str() == Some(instance_key(instance_id).as_str())
        && proof["slot"].as_str() == slot.map(SlotId::as_str)
        && proof["nonce"]
            .as_str()
            .is_some_and(|nonce| nonce.len() == 64))
    .then_some(proof)
}

fn proof_identity(proof: &Value, name: &str) -> Option<[u64; 2]> {
    let values = proof.get(name)?.as_array()?;
    Some([values.first()?.as_u64()?, values.get(1)?.as_u64()?])
}

fn random_nonce() -> Result<String, StorageError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| StorageError::Backend)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(unix)]
#[path = "storage/unix.rs"]
mod platform;

#[cfg(windows)]
#[path = "storage/windows.rs"]
mod platform;

#[cfg(test)]
mod tests {
    use std::fs;

    use composenest_domain::{identity::SlotId, instance::StoragePresence};
    use tempfile::tempdir;

    use super::{BindStorage, StorageError, StoragePort};
    use composenest_application::state_store::StorageAllocation;

    fn slots() -> Vec<SlotId> {
        vec![
            SlotId::parse("data").unwrap(),
            SlotId::parse("logs").unwrap(),
        ]
    }

    #[test]
    fn creates_each_slot_once_and_keeps_proof_outside_data() {
        let root = tempdir().unwrap();
        let storage = BindStorage::new(root.path());
        let instance = composenest_domain::identity::InstanceId::from_u128(42);
        let allocations = storage.create_bind_set(instance, &slots()).unwrap();

        assert_eq!(allocations.len(), 2);
        for allocation in &allocations {
            assert_eq!(
                storage.inspect_bind(instance, allocation),
                StoragePresence::Present
            );
            assert!(storage.bind_path(instance, allocation).unwrap().is_dir());
            assert!(
                root.path()
                    .join("ownership/0000000000000000000000000000002a")
                    .join(format!("{}.json", allocation.slot))
                    .is_file()
            );
        }
        let second = composenest_domain::identity::InstanceId::from_u128(43);
        assert_eq!(storage.create_bind_set(second, &slots()).unwrap().len(), 2);
        assert!(matches!(
            storage.create_bind_set(instance, &slots()),
            Err(StorageError::Unverified)
        ));
    }

    #[test]
    fn empty_storage_set_does_not_create_a_data_root() {
        let root = tempdir().unwrap();
        let storage = BindStorage::new(root.path());
        let instance = composenest_domain::identity::InstanceId::from_u128(41);

        assert!(storage.create_bind_set(instance, &[]).unwrap().is_empty());
        assert!(!root.path().join("data").exists());
        assert!(!root.path().join("ownership").exists());
    }

    #[test]
    fn rejects_a_relative_management_root() {
        let storage = BindStorage::new("relative-management-root");
        let instance = composenest_domain::identity::InstanceId::from_u128(40);

        assert!(matches!(
            storage.create_bind_set(instance, &slots()),
            Err(StorageError::Unverified)
        ));
    }

    #[test]
    fn never_adopts_a_preexisting_slot_directory() {
        let root = tempdir().unwrap();
        let instance = composenest_domain::identity::InstanceId::from_u128(7);
        let existing = root
            .path()
            .join("data/00000000000000000000000000000007/data");
        fs::create_dir_all(&existing).unwrap();

        assert!(matches!(
            BindStorage::new(root.path()).create_bind_set(instance, &slots()),
            Err(StorageError::Unverified)
        ));
        assert!(existing.is_dir());
        assert!(
            !root
                .path()
                .join("ownership/00000000000000000000000000000007/data.json")
                .exists()
        );
    }

    #[test]
    fn missing_previous_storage_is_not_recreated() {
        let root = tempdir().unwrap();
        let storage = BindStorage::new(root.path());
        let instance = composenest_domain::identity::InstanceId::from_u128(9);
        let allocation = storage
            .create_bind_set(instance, &slots())
            .unwrap()
            .remove(0);
        let path = root.path().join(&allocation.resource_identity);
        fs::remove_dir(&path).unwrap();

        assert_eq!(
            storage.inspect_bind(instance, &allocation),
            StoragePresence::Missing
        );
        assert_eq!(
            storage.bind_path(instance, &allocation),
            Err(StorageError::Missing)
        );
        assert!(matches!(
            storage.create_bind_set(instance, &slots()),
            Err(StorageError::Unverified)
        ));
        assert!(!path.exists());
    }

    #[test]
    fn retained_proof_prevents_reusing_an_instance_directory() {
        let root = tempdir().unwrap();
        let storage = BindStorage::new(root.path());
        let instance = composenest_domain::identity::InstanceId::from_u128(14);
        storage.create_bind_set(instance, &slots()).unwrap();
        fs::remove_dir_all(root.path().join("data/0000000000000000000000000000000e")).unwrap();

        assert!(matches!(
            storage.create_bind_set(instance, &slots()),
            Err(StorageError::Unverified)
        ));
        assert!(
            !root
                .path()
                .join("data/0000000000000000000000000000000e")
                .exists()
        );
    }

    #[test]
    fn modified_or_missing_proof_is_unverified() {
        let root = tempdir().unwrap();
        let storage = BindStorage::new(root.path());
        let instance = composenest_domain::identity::InstanceId::from_u128(10);
        let allocation = storage
            .create_bind_set(instance, &slots())
            .unwrap()
            .remove(0);
        let proof = root
            .path()
            .join("ownership/0000000000000000000000000000000a/data.json");
        fs::write(&proof, b"{}").unwrap();

        assert_eq!(
            storage.inspect_bind(instance, &allocation),
            StoragePresence::Unverified
        );
        fs::remove_file(proof).unwrap();
        assert_eq!(
            storage.inspect_bind(instance, &allocation),
            StoragePresence::Unverified
        );
    }

    #[test]
    fn rejects_noncanonical_paths_without_prefix_comparisons() {
        let root = tempdir().unwrap();
        let storage = BindStorage::new(root.path());
        let instance = composenest_domain::identity::InstanceId::from_u128(11);
        let allocation = storage
            .create_bind_set(instance, &slots())
            .unwrap()
            .remove(0);
        let malformed = StorageAllocation {
            slot: allocation.slot.clone(),
            resource_identity: format!("{}-other", allocation.resource_identity),
            ownership_evidence: allocation.ownership_evidence.clone(),
        };

        assert_eq!(
            storage.inspect_bind(instance, &malformed),
            StoragePresence::Unverified
        );
        assert_eq!(
            storage.bind_path(instance, &malformed),
            Err(StorageError::Unverified)
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_replaced_slot_symlink() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let storage = BindStorage::new(root.path());
        let instance = composenest_domain::identity::InstanceId::from_u128(12);
        let allocation = storage
            .create_bind_set(instance, &slots())
            .unwrap()
            .remove(0);
        let path = root.path().join(&allocation.resource_identity);
        fs::remove_dir(&path).unwrap();
        symlink(outside.path(), &path).unwrap();

        assert_eq!(
            storage.inspect_bind(instance, &allocation),
            StoragePresence::Unverified
        );
        assert_eq!(
            storage.bind_path(instance, &allocation),
            Err(StorageError::Unverified)
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_data_root_without_writing_through_it() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        symlink(outside.path(), root.path().join("data")).unwrap();
        let instance = composenest_domain::identity::InstanceId::from_u128(13);

        assert!(matches!(
            BindStorage::new(root.path()).create_bind_set(instance, &slots()),
            Err(StorageError::Unverified)
        ));
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn an_unowned_existing_region_blocks_other_allocations() {
        let root = tempdir().unwrap();
        fs::create_dir_all(
            root.path()
                .join("data/0000000000000000000000000000000e/data"),
        )
        .unwrap();
        let next_instance = composenest_domain::identity::InstanceId::from_u128(15);

        assert!(matches!(
            BindStorage::new(root.path()).create_bind_set(next_instance, &slots()),
            Err(StorageError::Unverified)
        ));
        assert!(
            !root
                .path()
                .join("data/0000000000000000000000000000000f")
                .exists()
        );
    }
}
