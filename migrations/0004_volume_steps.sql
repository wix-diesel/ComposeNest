ALTER TABLE operation_steps RENAME TO operation_steps_v3;

CREATE TABLE operation_steps (
    operation_id TEXT NOT NULL REFERENCES operations(id) ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    attempt INTEGER NOT NULL CHECK (attempt > 0),
    command_kind TEXT NOT NULL CHECK (command_kind IN ('generate_artifact', 'resolve_image', 'create_volume', 'compose_create', 'compose_start', 'compose_stop', 'remove_container', 'observe')),
    resource_id TEXT NOT NULL,
    expected_result TEXT NOT NULL CHECK (expected_result IN ('artifact_ready', 'image_resolved', 'volume_created', 'container_created', 'container_running', 'container_stopped', 'container_absent', 'state_observed')),
    outcome TEXT CHECK (outcome IN ('succeeded', 'failed', 'unknown')),
    recorded_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    observed_at TEXT,
    PRIMARY KEY(operation_id, sequence),
    CHECK ((outcome IS NULL) = (observed_at IS NULL))
);

INSERT INTO operation_steps (
    operation_id, sequence, attempt, command_kind, resource_id,
    expected_result, outcome, recorded_at, observed_at
)
SELECT operation_id, sequence, attempt, command_kind, resource_id,
       expected_result, outcome, recorded_at, observed_at
FROM operation_steps_v3;

DROP TABLE operation_steps_v3;
