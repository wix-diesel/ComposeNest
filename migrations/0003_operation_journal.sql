CREATE TABLE operations (
    id TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN ('create', 'clone', 'start', 'stop', 'restart', 'edit_port', 'delete', 'recover')),
    status TEXT NOT NULL DEFAULT 'Pending' CHECK (status IN ('Pending', 'Running', 'Failed', 'AwaitingDecision', 'OutcomeUnknown', 'Succeeded', 'Abandoned')),
    phase TEXT NOT NULL,
    attempt INTEGER NOT NULL DEFAULT 1 CHECK (attempt > 0),
    expected_instance_revision INTEGER NOT NULL CHECK (expected_instance_revision > 0),
    old_spec_revision INTEGER,
    new_spec_revision INTEGER,
    started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at TEXT,
    FOREIGN KEY(instance_id, old_spec_revision) REFERENCES instance_specs(instance_id, revision),
    FOREIGN KEY(instance_id, new_spec_revision) REFERENCES instance_specs(instance_id, revision),
    CHECK ((status IN ('Succeeded', 'Abandoned')) = (completed_at IS NOT NULL))
);
CREATE UNIQUE INDEX one_unresolved_operation_per_instance ON operations(instance_id)
    WHERE status NOT IN ('Succeeded', 'Abandoned');

CREATE TABLE operation_steps (
    operation_id TEXT NOT NULL REFERENCES operations(id) ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    attempt INTEGER NOT NULL CHECK (attempt > 0),
    command_kind TEXT NOT NULL CHECK (command_kind IN ('generate_artifact', 'resolve_image', 'compose_create', 'compose_start', 'compose_stop', 'remove_container', 'observe')),
    resource_id TEXT NOT NULL,
    expected_result TEXT NOT NULL CHECK (expected_result IN ('artifact_ready', 'image_resolved', 'container_created', 'container_running', 'container_stopped', 'container_absent', 'state_observed')),
    outcome TEXT CHECK (outcome IN ('succeeded', 'failed', 'unknown')),
    recorded_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    observed_at TEXT,
    PRIMARY KEY(operation_id, sequence),
    CHECK ((outcome IS NULL) = (observed_at IS NULL))
);

CREATE TABLE pending_changes (
    operation_id TEXT PRIMARY KEY REFERENCES operations(id) ON DELETE RESTRICT,
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE RESTRICT,
    old_spec_revision INTEGER NOT NULL,
    new_spec_revision INTEGER NOT NULL,
    confirmed_diff_hash TEXT NOT NULL,
    FOREIGN KEY(instance_id, old_spec_revision) REFERENCES instance_specs(instance_id, revision),
    FOREIGN KEY(instance_id, new_spec_revision) REFERENCES instance_specs(instance_id, revision)
);
CREATE TABLE pending_change_reservations (
    operation_id TEXT NOT NULL REFERENCES pending_changes(operation_id) ON DELETE RESTRICT,
    reservation_id TEXT NOT NULL REFERENCES port_reservations(id) ON DELETE RESTRICT,
    PRIMARY KEY(operation_id, reservation_id)
);

CREATE TABLE request_receipts (
    scope_id TEXT NOT NULL REFERENCES management_scopes(id),
    request_id TEXT NOT NULL,
    plan_id TEXT,
    confirmed_revision INTEGER NOT NULL CHECK (confirmed_revision > 0),
    request_hash TEXT NOT NULL,
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE RESTRICT,
    operation_id TEXT NOT NULL UNIQUE REFERENCES operations(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(scope_id, request_id)
);
CREATE UNIQUE INDEX unique_confirmed_plan ON request_receipts(scope_id, plan_id)
    WHERE plan_id IS NOT NULL;

CREATE TABLE artifacts (
    id TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL,
    spec_revision INTEGER NOT NULL,
    generator_version TEXT NOT NULL,
    manifest_hash TEXT NOT NULL,
    placement TEXT NOT NULL CHECK (placement IN ('staged', 'published', 'retained')),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(instance_id, spec_revision) REFERENCES instance_specs(instance_id, revision)
);
CREATE TABLE artifact_files (
    artifact_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE RESTRICT,
    relative_path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY(artifact_id, relative_path)
);

CREATE TABLE image_resolutions (
    instance_id TEXT NOT NULL REFERENCES instances(id) ON DELETE RESTRICT,
    spec_revision INTEGER NOT NULL,
    image_ref TEXT NOT NULL,
    digest TEXT NOT NULL,
    image_id TEXT NOT NULL,
    platform TEXT NOT NULL,
    first_operation_id TEXT NOT NULL REFERENCES operations(id) ON DELETE RESTRICT,
    PRIMARY KEY(instance_id, spec_revision, image_ref),
    FOREIGN KEY(instance_id, spec_revision) REFERENCES instance_specs(instance_id, revision)
);

CREATE TABLE runtime_observations (
    instance_id TEXT PRIMARY KEY REFERENCES instances(id) ON DELETE RESTRICT,
    operation_id TEXT REFERENCES operations(id) ON DELETE RESTRICT,
    container_id TEXT,
    runtime_state TEXT NOT NULL CHECK (runtime_state IN ('absent', 'created', 'running', 'stopped', 'unknown')),
    health TEXT CHECK (health IN ('starting', 'healthy', 'unhealthy', 'none', 'unknown')),
    observed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    freshness TEXT NOT NULL CHECK (freshness IN ('fresh', 'stale'))
);
