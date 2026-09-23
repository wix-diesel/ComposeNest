CREATE TABLE management_scopes (
    id TEXT PRIMARY KEY,
    owner_id TEXT NOT NULL,
    root_identity TEXT NOT NULL UNIQUE,
    default_storage_method TEXT NOT NULL DEFAULT 'bind' CHECK (default_storage_method IN ('bind', 'volume'))
);

CREATE TABLE runtime_targets (
    id TEXT PRIMARY KEY,
    scope_id TEXT NOT NULL REFERENCES management_scopes(id),
    endpoint TEXT NOT NULL,
    engine_id TEXT NOT NULL,
    platform TEXT NOT NULL,
    UNIQUE(scope_id, endpoint),
    UNIQUE(id, scope_id)
);

CREATE TABLE template_revisions (
    id TEXT PRIMARY KEY,
    template_id TEXT NOT NULL,
    template_version TEXT NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    normalization TEXT NOT NULL,
    semantic_hash TEXT NOT NULL,
    canonical_json TEXT NOT NULL CHECK (json_valid(canonical_json)),
    origin TEXT NOT NULL,
    UNIQUE(template_id, template_version)
);

CREATE TABLE template_revision_files (
    revision_id TEXT NOT NULL REFERENCES template_revisions(id),
    relative_path TEXT NOT NULL,
    contents BLOB NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY(revision_id, relative_path)
);

CREATE TABLE instances (
    id TEXT PRIMARY KEY,
    scope_id TEXT NOT NULL REFERENCES management_scopes(id),
    target_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    lifecycle TEXT NOT NULL DEFAULT 'managed' CHECK (lifecycle IN ('managed', 'retiring', 'retired')),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    project_name TEXT NOT NULL UNIQUE,
    clone_source_id TEXT REFERENCES instances(id) ON DELETE RESTRICT,
    FOREIGN KEY(target_id, scope_id) REFERENCES runtime_targets(id, scope_id)
);
CREATE UNIQUE INDEX unique_active_name ON instances(scope_id, normalized_name)
    WHERE lifecycle != 'retired';

CREATE TABLE template_snapshots (
    id TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL UNIQUE REFERENCES instances(id) ON DELETE RESTRICT,
    source_revision_id TEXT REFERENCES template_revisions(id) ON DELETE SET NULL,
    template_id TEXT NOT NULL,
    template_version TEXT NOT NULL,
    selected_version TEXT NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    normalization TEXT NOT NULL,
    semantic_hash TEXT NOT NULL,
    canonical_json TEXT NOT NULL CHECK (json_valid(canonical_json))
);

CREATE TABLE template_snapshot_files (
    snapshot_id TEXT NOT NULL REFERENCES template_snapshots(id) ON DELETE RESTRICT,
    relative_path TEXT NOT NULL,
    contents BLOB NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY(snapshot_id, relative_path)
);

CREATE TABLE instance_specs (
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE RESTRICT,
    revision INTEGER NOT NULL CHECK (revision > 0),
    selected_version TEXT NOT NULL,
    storage_method TEXT NOT NULL CHECK (storage_method IN ('bind', 'volume')),
    inputs_json TEXT NOT NULL CHECK (json_valid(inputs_json)),
    PRIMARY KEY(instance_id, revision)
);

CREATE TABLE port_bindings (
    instance_id TEXT NOT NULL,
    spec_revision INTEGER NOT NULL,
    slot TEXT NOT NULL,
    host_ip TEXT NOT NULL,
    host_port INTEGER NOT NULL CHECK (host_port BETWEEN 1 AND 65535),
    container_port INTEGER NOT NULL CHECK (container_port BETWEEN 1 AND 65535),
    PRIMARY KEY(instance_id, spec_revision, slot),
    FOREIGN KEY(instance_id, spec_revision) REFERENCES instance_specs(instance_id, revision)
);

CREATE TABLE port_reservations (
    id TEXT PRIMARY KEY,
    scope_id TEXT NOT NULL REFERENCES management_scopes(id),
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE RESTRICT,
    host_ip TEXT NOT NULL,
    protocol TEXT NOT NULL CHECK (protocol = 'tcp'),
    host_port INTEGER NOT NULL CHECK (host_port BETWEEN 1 AND 65535),
    status TEXT NOT NULL CHECK (status IN ('held', 'committed', 'released'))
);
CREATE UNIQUE INDEX unique_active_port ON port_reservations(scope_id, host_ip, protocol, host_port)
    WHERE status != 'released';

CREATE TABLE storage_allocations (
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE RESTRICT,
    slot TEXT NOT NULL,
    method TEXT NOT NULL CHECK (method IN ('bind', 'volume')),
    resource_identity TEXT NOT NULL UNIQUE,
    ownership_evidence TEXT NOT NULL,
    ownership TEXT NOT NULL CHECK (ownership IN ('assigned', 'retained')),
    presence TEXT NOT NULL CHECK (presence IN ('not_materialized', 'present', 'missing', 'unverified')),
    initialization TEXT NOT NULL CHECK (initialization IN ('not_attempted', 'may_have_initialized', 'ready_observed')),
    PRIMARY KEY(instance_id, slot)
);
