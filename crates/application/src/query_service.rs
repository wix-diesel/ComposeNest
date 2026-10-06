//! Read models for instance cards, details, and connection information.

use composenest_domain::identity::DisplayName;
use serde::{Deserialize, Serialize};

use crate::RequestContext;

use crate::state_store::{StateStore, StoreConflict};

/// A confirmed TCP port, independent of current Docker availability.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PortView {
    /// Template port slot.
    pub slot: String,
    /// Confirmed host address.
    pub host_ip: String,
    /// Confirmed host port.
    pub host_port: u16,
    /// Container port.
    pub container_port: u16,
}

/// Last persisted storage state without its private resource identity.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StorageView {
    /// Template storage slot.
    pub slot: String,
    /// Bind or volume.
    pub method: String,
    /// Last verified presence.
    pub presence: String,
    /// Initialization state.
    pub initialization: String,
}

/// One saved input with secret values omitted.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InputView {
    /// Template input slot.
    pub slot: String,
    /// Display label from the saved template, falling back to the slot if absent.
    pub label: String,
    /// Whether the input is confidential.
    pub secret: bool,
    /// Saved non-secret value, or `None` for secrets.
    pub value: Option<serde_json::Value>,
}

/// A connection card resolved against the committed port bindings.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionView {
    /// Template connection slot.
    pub slot: String,
    /// Display label from the private snapshot.
    pub label: String,
    /// Confirmed address and port.
    pub port: PortView,
    /// Input slots relevant to this connection; values are supplied separately.
    pub input_slots: Vec<String>,
}

/// The last saved Docker observation, even when it is no longer fresh.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ObservationView {
    /// Container state observed by Core.
    pub runtime_state: String,
    /// Health observed by Core, if available.
    pub health: Option<String>,
    /// Time of the observation in SQLite UTC format.
    pub observed_at: String,
    /// Whether this observation was marked fresh when persisted.
    pub freshness: String,
}

/// The most recent operation, including unresolved failures.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationView {
    /// Stable operation ID.
    pub id: String,
    /// Operation kind.
    pub kind: String,
    /// Persisted outcome or progress state.
    pub status: String,
    /// Fixed progress phase without external arguments.
    pub phase: String,
    /// Start time in SQLite UTC format.
    pub started_at: String,
}

/// Reads one durable operation; the trusted scope is supplied by the backend.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationRequest {
    /// Correlation and API version metadata.
    pub context: RequestContext,
    /// Stable identity from the acceptance receipt.
    pub operation_id: String,
}

/// A scoped operation and masked target read in one saved database snapshot.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationProgressView {
    /// Historical progress of the requested operation, not the latest operation.
    pub operation: OperationView,
    /// Current committed target settings and saved runtime observation.
    pub instance: InstanceView,
    /// Highest durable step sequence; events are only invalidation hints.
    pub sequence: u64,
    /// Actual completion time, absent until recorded by Core.
    pub completed_at: Option<String>,
}

/// A single masked read model shared by list cards and instance details.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstanceView {
    /// Stable internal identifier.
    pub id: String,
    /// Validated display name.
    pub name: String,
    /// Current optimistic-lock revision.
    pub revision: u64,
    /// Managed, retiring, or retired lifecycle; active queries exclude retired records.
    pub lifecycle: String,
    /// Stable Compose project name.
    pub project_name: String,
    /// Template identity from the private snapshot.
    pub template_id: String,
    /// Package revision from the private snapshot.
    pub template_version: String,
    /// Selected service version from the committed spec.
    pub selected_version: String,
    /// Service image declared by that version.
    pub image: Option<String>,
    /// Bind or volume.
    pub storage_method: String,
    /// Committed configuration revision.
    pub spec_revision: u64,
    /// Revision last applied to the runtime, if known.
    pub applied_spec_revision: Option<u64>,
    /// Confirmed port bindings.
    pub ports: Vec<PortView>,
    /// Persisted storage states.
    pub storage: Vec<StorageView>,
    /// Saved inputs with secret values removed.
    pub inputs: Vec<InputView>,
    /// Connection cards built from confirmed ports.
    pub connections: Vec<ConnectionView>,
    /// Last observation; reading this model does not contact Docker.
    pub observation: Option<ObservationView>,
    /// Display state derived from the observation and its freshness.
    pub runtime_status: String,
    /// Most recent operation.
    pub last_operation: Option<OperationView>,
    /// Whether the saved axes call for user attention.
    pub needs_attention: bool,
}

/// Saved display data shared by instance cards and grids, without input values.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstanceListView {
    /// Stable instance identity used for navigation.
    pub id: String,
    /// Validated display name.
    pub name: String,
    /// Service identity from the private snapshot.
    pub template_id: String,
    /// Committed service version.
    pub selected_version: String,
    /// Confirmed endpoints, including stopped instances.
    pub ports: Vec<PortView>,
    /// Committed storage method.
    pub storage_method: String,
    /// Committed configuration revision.
    pub spec_revision: u64,
    /// Last applied configuration revision; this does not prove absence of drift.
    pub applied_spec_revision: Option<u64>,
    /// Saved runtime classification, including unknown observations.
    pub runtime_status: String,
    /// Core's attention flag across saved state axes.
    pub needs_attention: bool,
    /// Original observation time and freshness, never the list retrieval time.
    pub observation: Option<ObservationView>,
}

/// Recorded storage destination for display only; reading it does not verify presence.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageLocationView {
    /// Template storage slot.
    pub slot: String,
    /// Saved bind path or named volume identity.
    pub location: String,
}

/// Scoped detail snapshot with masked inputs and separate Core action availability.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceDetailView {
    /// Committed configuration, connections and saved observation.
    pub instance: InstanceView,
    /// Actions derived from the same database snapshot.
    pub state: crate::instance_actions::InstanceActionView,
    /// Recorded storage destinations; these are not executable frontend paths.
    pub locations: Vec<StorageLocationView>,
    /// Original create or clone acceptance time, if journaled.
    pub creation_started_at: Option<String>,
    /// Saved source identity for configuration-only clones.
    pub clone_source_id: Option<String>,
}

impl From<InstanceView> for InstanceListView {
    fn from(view: InstanceView) -> Self {
        Self {
            id: view.id,
            name: view.name,
            template_id: view.template_id,
            selected_version: view.selected_version,
            ports: view.ports,
            storage_method: view.storage_method,
            spec_revision: view.spec_revision,
            applied_spec_revision: view.applied_spec_revision,
            runtime_status: view.runtime_status,
            needs_attention: view.needs_attention,
            observation: view.observation,
        }
    }
}

/// Read-only persistence boundary for consistent masked instance views.
pub trait QueryStore {
    /// Lists active instances in one saved-state view.
    fn list_instances(&self, scope_id: &str) -> Result<Vec<InstanceView>, StoreConflict>;
    /// Reads an active instance in the requested scope.
    fn get_instance(&self, scope_id: &str, id: &str)
    -> Result<Option<InstanceView>, StoreConflict>;
}

/// Serves saved state without requiring a running Docker Engine.
pub struct QueryService<'a, S> {
    store: &'a S,
}

impl<'a, S: QueryStore + StateStore> QueryService<'a, S> {
    /// Uses the same store for reads and optimistic-lock updates.
    pub fn new(store: &'a S) -> Self {
        Self { store }
    }

    /// Returns active instances for cards and grids.
    pub fn list_instances(&self, scope_id: &str) -> Result<Vec<InstanceView>, StoreConflict> {
        self.store.list_instances(scope_id)
    }

    /// Returns an active instance and its masked connection details.
    pub fn get_instance(
        &self,
        scope_id: &str,
        id: &str,
    ) -> Result<Option<InstanceView>, StoreConflict> {
        self.store.get_instance(scope_id, id)
    }

    /// Changes only the display name if the expected revision is current.
    pub fn rename(
        &self,
        scope_id: &str,
        id: &str,
        expected: u64,
        name: &str,
    ) -> Result<u64, StoreConflict> {
        let name = DisplayName::parse(name).map_err(|_| StoreConflict::InvalidInput)?;
        if self.store.get_instance(scope_id, id)?.is_none() {
            return Err(StoreConflict::Missing);
        }
        self.store.rename_instance(id, expected, name.as_str())?;
        expected.checked_add(1).ok_or(StoreConflict::InvalidInput)
    }
}
