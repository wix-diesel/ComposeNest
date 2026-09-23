//! Typed persistence boundary for confirmed state and resource ledgers.

/// The persisted default for newly created instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageMethod {
    Bind,
    Volume,
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
    /// Template revision to copy into the instance snapshot.
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
#[derive(Debug, Clone)]
pub struct StorageAllocation {
    /// Stable template slot.
    pub slot: String,
    /// Canonical path or Docker volume identity, validated by the caller.
    pub resource_identity: String,
    /// Ownership proof outside the writable data resource.
    pub ownership_evidence: String,
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
pub trait StateStore {
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

    /// Registers a complete package or leaves no new revision or files.
    fn register_template(&self, revision: &TemplateRevision) -> Result<(), StoreConflict>;

    /// Lists immutable revisions even when their source packages have changed or disappeared.
    fn list_templates(&self) -> Result<Vec<TemplateCatalogItem>, StoreConflict>;

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
}
