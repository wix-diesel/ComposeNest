//! Saved-state views of retired instances and their independently retained data.

use crate::{
    delete_operation::StorageCheck,
    query_service::{InstanceView, StorageView},
    state_store::StoreConflict,
};
use serde::Serialize;

/// A saved allocation's location and last ownership check.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetainedLocation {
    /// Saved slot, storage method, presence, and initialization.
    pub storage: StorageView,
    /// Absolute bind path or Docker volume name; no arbitrary-path opening is exposed.
    pub location: String,
    /// Retained allocation relationship, distinct from physical ownership verification.
    pub ownership: String,
    /// True only when the latest observation verified the actual resource.
    pub ownership_verified: bool,
    /// Last check time in SQLite UTC format, or None before any retained check.
    pub observed_at: Option<String>,
}

/// An artifact reference without reading its potentially confidential contents.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetainedArtifact {
    /// Stable saved artifact ID.
    pub id: String,
    /// Configuration revision from which the artifact was generated.
    pub spec_revision: u64,
    /// Staged, published, or retained placement.
    pub placement: String,
    /// Published directory, or None when publication never completed.
    pub directory: Option<String>,
}

/// Original masked settings and locations after runtime deletion.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetainedInstance {
    /// Original name, service identity, version, and masked saved settings.
    pub instance: InstanceView,
    /// Service display name from the private template snapshot.
    pub service_name: String,
    /// Delete completion time, not the time deletion was requested.
    pub deleted_at: String,
    /// Per-slot physical locations and actual presence states.
    pub locations: Vec<RetainedLocation>,
    /// Protected database containing the original spec and snapshot.
    pub settings_database: String,
    /// Snapshot record key in that database; spec keys are in `instance`.
    pub snapshot_id: String,
    /// Every saved artifact reference, including interrupted publication.
    pub artifacts: Vec<RetainedArtifact>,
}

/// Read-only retained views and atomic application of fresh allocation observations.
pub trait RetainedStore {
    /// Lists retired instances in one consistent database view without contacting Docker.
    fn list_retained_storage(&self, scope_id: &str)
    -> Result<Vec<RetainedInstance>, StoreConflict>;
    /// Records all slot checks only for the expected retired instance and scope.
    fn update_retained_checks(
        &self,
        scope_id: &str,
        instance_id: &str,
        expected_revision: u64,
        checks: &[StorageCheck],
    ) -> Result<(), StoreConflict>;
}
