CREATE TABLE external_recoveries (
    operation_id TEXT PRIMARY KEY REFERENCES operations(id) ON DELETE RESTRICT,
    source_artifact_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE RESTRICT,
    replacement_artifact_id TEXT NOT NULL UNIQUE,
    confirmation_hash TEXT NOT NULL CHECK (length(confirmation_hash) = 64),
    original_container_id TEXT
);
CREATE TABLE artifact_selections (
    instance_id TEXT NOT NULL,
    spec_revision INTEGER NOT NULL,
    artifact_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE RESTRICT,
    PRIMARY KEY(instance_id, spec_revision),
    FOREIGN KEY(instance_id, spec_revision) REFERENCES instance_specs(instance_id, revision)
);
