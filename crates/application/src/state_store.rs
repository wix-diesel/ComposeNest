//! Typed persistence boundary for confirmed state and resource ledgers.

use composenest_domain::instance::{Initialization, StorageOwnership, StoragePresence};
use serde::{Deserialize, Serialize};

/// The persisted default for newly created instances.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageMethod {
    Bind,
    Volume,
}

/// A local Docker Engine identity persisted independently of Engine availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTarget {
    /// Stable target identifier.
    pub id: String,
    /// Management scope that owns this target.
    pub scope_id: String,
    /// Explicit local Docker endpoint.
    pub endpoint: String,
    /// Docker Engine ID observed during registration.
    pub engine_id: String,
    /// Observed operating system and architecture.
    pub platform: String,
}

/// One original document in a complete template package.
#[derive(Debug, Clone)]
pub struct TemplateFile {
    /// Package-relative path, starting with `template.yaml` or `versions/`.
    pub relative_path: String,
    /// Exact original bytes.
    pub contents: Vec<u8>,
}

/// A validated template revision and all its version definitions.
#[derive(Debug, Clone)]
pub struct TemplateRevision {
    /// Stable revision identifier.
    pub id: String,
    /// Template identifier.
    pub template_id: String,
    /// Package revision.
    pub version: String,
    /// Canonicalization rule identifier.
    pub normalization: String,
    /// Hash of the complete canonical meaning, distinct from original file hashes.
    pub semantic_hash: String,
    /// Canonical JSON containing every version's complete definition.
    pub canonical_json: String,
    /// Registration source recorded by the application, not the package author.
    pub origin: String,
    /// Manifest and every listed version document.
    pub files: Vec<TemplateFile>,
}

/// One persisted catalog revision, independent of current package files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateCatalogItem {
    /// Immutable revision identifier.
    pub id: String,
    /// Stable template identifier.
    pub template_id: String,
    /// Author-defined revision string.
    pub version: String,
    /// Application-assigned source retained from first registration.
    pub origin: String,
    /// Hash of the complete normalized definition.
    pub semantic_hash: String,
    /// Canonical definition, including every complete service version.
    pub canonical_json: String,
}

/// Data committed together with a private template snapshot.
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    /// Immutable instance ID.
    pub id: String,
    /// Management scope.
    pub scope_id: String,
    /// Fixed runtime target.
    pub target_id: String,
    /// Validated and normalized display name.
    pub name: String,
    /// Stable Compose project name.
    pub project_name: String,
    /// Optional clone source retained for history.
    pub clone_source_id: Option<String>,
    /// Registered Template revision for creation; clones copy the source Snapshot instead.
    pub template_revision_id: String,
    /// Version selected from the copied package.
    pub selected_version: String,
    /// Chosen data storage method.
    pub storage_method: StorageMethod,
    /// Confirmed input values, including secrets, as JSON.
    pub inputs_json: String,
    /// Port allocations to reserve at confirmation.
    pub ports: Vec<PortAllocation>,
    /// Independent writable storage allocations.
    pub storage: Vec<StorageAllocation>,
}

/// A consistent view of a managed clone source and its committed configuration.
#[derive(Debug, Clone)]
pub struct CloneSource {
    /// Source instance ID.
    pub id: String,
    /// Management scope containing the source.
    pub scope_id: String,
    /// Runtime target to reuse for the clone.
    pub target_id: String,
    /// Current instance revision guarded again at commit.
    pub revision: u64,
    /// Current committed spec revision, excluding abandoned pending changes.
    pub spec_revision: u64,
    /// Source display name.
    pub name: String,
    /// Complete immutable package definition from the private Snapshot.
    pub snapshot_json: String,
    /// Selected service Version from the current CommittedSpec.
    pub selected_version: String,
    /// Committed storage method.
    pub storage_method: StorageMethod,
    /// Committed inputs, including secrets, as JSON.
    pub inputs_json: String,
    /// Committed port bindings.
    pub ports: Vec<PortAllocation>,
    /// Source writable allocations that the clone must not reuse.
    pub storage: Vec<StorageAllocation>,
}

/// One TCP port binding and reservation.
#[derive(Debug, Clone)]
pub struct PortAllocation {
    /// Stable template slot.
    pub slot: String,
    /// Host IP in canonical form.
    pub host_ip: String,
    /// Host TCP port.
    pub host_port: u16,
    /// Container TCP port.
    pub container_port: u16,
}

/// One independently owned writable data resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageAllocation {
    /// Stable template slot.
    pub slot: String,
    /// Canonical path or Docker volume identity, validated by the caller.
    pub resource_identity: String,
    /// Ownership proof outside writable data, such as a bind proof hash or volume operation ID.
    pub ownership_evidence: String,
}

/// One persisted storage allocation with its current observed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLedgerEntry {
    /// Immutable instance ID.
    pub instance_id: String,
    /// Management scope that owns the instance.
    pub scope_id: String,
    /// Stable template slot.
    pub slot: String,
    /// Persisted storage method.
    pub method: StorageMethod,
    /// Immutable resource identity and ownership evidence.
    pub allocation: StorageAllocation,
    /// Whether the data must be retained after retirement.
    pub ownership: StorageOwnership,
    /// Last verified existence state.
    pub presence: StoragePresence,
    /// Highest initialization state observed so far.
    pub initialization: Initialization,
}

/// Expected persistence conflicts and rejected input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreConflict {
    /// Another transaction already owns the resource or ID.
    Duplicate,
    /// The expected instance revision no longer matches.
    StaleRevision,
    /// A required record does not exist.
    Missing,
    /// The record cannot be changed in its current lifecycle.
    InvalidLifecycle,
    /// Input failed a persistence boundary check.
    InvalidInput,
    /// Persistence backend failed unexpectedly.
    Backend,
}

/// Atomic persistence operations needed by instance and template use cases.
pub trait StateStore: Send + Sync {
    /// Registers a management scope with bind storage as its initial default.
    fn create_scope(
        &self,
        id: &str,
        owner_id: &str,
        root_identity: &str,
    ) -> Result<(), StoreConflict>;

    /// Records the fixed local runtime target for a scope.
    fn create_target(
        &self,
        id: &str,
        scope_id: &str,
        endpoint: &str,
        engine_id: &str,
        platform: &str,
    ) -> Result<(), StoreConflict>;

    /// Reads a registered target without contacting Docker.
    fn runtime_target(&self, scope_id: &str) -> Result<Option<RuntimeTarget>, StoreConflict>;

    /// Registers a complete package or leaves no new revision or files.
    fn register_template(&self, revision: &TemplateRevision) -> Result<(), StoreConflict>;

    /// Lists immutable revisions even when their source packages have changed or disappeared.
    fn list_templates(&self) -> Result<Vec<TemplateCatalogItem>, StoreConflict>;

    /// Reads a managed source, its current spec and private Snapshot in one view.
    /// Returns no source while an operation that may change it is unresolved.
    fn clone_source(&self, scope_id: &str, id: &str) -> Result<Option<CloneSource>, StoreConflict>;

    /// Reads the source revision even while an operation is unresolved or after retirement.
    fn source_revision(&self, scope_id: &str, id: &str) -> Result<Option<u64>, StoreConflict>;

    /// Checks whether an instance ID is already used, including retained history.
    fn instance_id_exists(&self, id: &str) -> Result<bool, StoreConflict>;

    /// Commits an instance, private snapshot, spec, ports and storage atomically.
    fn commit_instance(&self, instance: &InstanceRecord) -> Result<(), StoreConflict>;

    /// Renames a managed instance only if the expected revision still matches.
    fn rename_instance(&self, id: &str, expected: u64, name: &str) -> Result<(), StoreConflict>;

    /// Marks a verified retired instance while preserving its history and storage.
    fn retire_instance(
        &self,
        id: &str,
        expected: u64,
        absence_verified: bool,
    ) -> Result<(), StoreConflict>;

    /// Returns the persisted default for a management scope.
    fn default_storage_method(&self, scope_id: &str) -> Result<StorageMethod, StoreConflict>;

    /// Sets the default used for future instance creation.
    fn set_default_storage_method(
        &self,
        scope_id: &str,
        method: StorageMethod,
    ) -> Result<(), StoreConflict>;

    /// Reads one storage allocation and its current ledger state.
    fn storage_allocation(
        &self,
        instance_id: &str,
        slot: &str,
    ) -> Result<Option<StorageLedgerEntry>, StoreConflict>;

    /// Advances the observed presence without allowing a missing resource to be recreated.
    fn set_storage_presence(
        &self,
        instance_id: &str,
        slot: &str,
        presence: StoragePresence,
    ) -> Result<(), StoreConflict>;

    /// Advances initialization state monotonically after runtime verification.
    fn advance_storage_initialization(
        &self,
        instance_id: &str,
        slot: &str,
        initialization: Initialization,
    ) -> Result<(), StoreConflict>;
}
