CREATE TABLE operation_failures (
    operation_id TEXT PRIMARY KEY REFERENCES operations(id),
    status TEXT NOT NULL,
    phase TEXT NOT NULL,
    attempt INTEGER NOT NULL
);
INSERT INTO operation_failures SELECT id, status, phase, attempt FROM operations
    WHERE status IN ('Failed', 'OutcomeUnknown', 'AwaitingDecision');
CREATE TRIGGER remember_operation_failure AFTER UPDATE OF status ON operations
WHEN NEW.status IN ('Failed', 'OutcomeUnknown', 'AwaitingDecision')
BEGIN
    INSERT INTO operation_failures VALUES (NEW.id, NEW.status, NEW.phase, NEW.attempt)
        ON CONFLICT(operation_id) DO UPDATE SET status=excluded.status, phase=excluded.phase, attempt=excluded.attempt
        WHERE operation_failures.status != 'Failed' OR excluded.status = 'Failed';
END;
