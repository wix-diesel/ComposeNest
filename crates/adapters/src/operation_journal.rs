//! SQLite operation journal and durable request receipts.

use composenest_application::operation_journal::{
    ExpectedResult, OperationIntent, OperationJournal, OperationKind, OperationStatus,
    RequestReceipt, StepCommand, StepIntent, StepOutcome, StepRecord,
};
use composenest_application::state_store::StoreConflict;
use rusqlite::{Connection, Error, OptionalExtension, TransactionBehavior, params};

use crate::sqlite::{DatabaseError, DatabaseWorker};
use crate::state_store::map_error;

fn kind_name(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::Create => "create",
        OperationKind::Clone => "clone",
        OperationKind::Start => "start",
        OperationKind::Stop => "stop",
        OperationKind::Restart => "restart",
        OperationKind::Rename => "rename",
        OperationKind::EditPort => "edit_port",
        OperationKind::Delete => "delete",
        OperationKind::Recover => "recover",
    }
}

fn status_name(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::Accepted => "Accepted",
        OperationStatus::Executing => "Executing",
        OperationStatus::Failed => "Failed",
        OperationStatus::AwaitingDecision => "AwaitingDecision",
        OperationStatus::OutcomeUnknown => "OutcomeUnknown",
        OperationStatus::Succeeded => "Succeeded",
        OperationStatus::Abandoned => "Abandoned",
    }
}

fn step_command(value: &str) -> Option<StepCommand> {
    Some(match value {
        "generate_artifact" => StepCommand::GenerateArtifact,
        "resolve_image" => StepCommand::ResolveImage,
        "create_volume" => StepCommand::CreateVolume,
        "compose_create" => StepCommand::ComposeCreate,
        "compose_start" => StepCommand::ComposeStart,
        "compose_stop" => StepCommand::ComposeStop,
        "remove_container" => StepCommand::RemoveContainer,
        "observe" => StepCommand::Observe,
        _ => return None,
    })
}

fn expected_result(value: &str) -> Option<ExpectedResult> {
    Some(match value {
        "artifact_ready" => ExpectedResult::ArtifactReady,
        "image_resolved" => ExpectedResult::ImageResolved,
        "volume_created" => ExpectedResult::VolumeCreated,
        "container_created" => ExpectedResult::ContainerCreated,
        "container_running" => ExpectedResult::ContainerRunning,
        "container_stopped" => ExpectedResult::ContainerStopped,
        "container_absent" => ExpectedResult::ContainerAbsent,
        "state_observed" => ExpectedResult::StateObserved,
        _ => return None,
    })
}

fn step_outcome(value: Option<String>) -> Option<Option<StepOutcome>> {
    match value.as_deref() {
        None => Some(None),
        Some("succeeded") => Some(Some(StepOutcome::Succeeded)),
        Some("failed") => Some(Some(StepOutcome::Failed)),
        Some("unknown") => Some(Some(StepOutcome::Unknown)),
        _ => None,
    }
}

fn receipt_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RequestReceipt> {
    Ok(RequestReceipt {
        scope_id: row.get(0)?,
        request_id: row.get(1)?,
        plan_id: row.get(2)?,
        confirmed_revision: row.get::<_, i64>(3)? as u64,
        request_hash: row.get(4)?,
        instance_id: row.get(5)?,
        operation_id: row.get(6)?,
    })
}

fn find_receipt(
    db: &Connection,
    scope_id: &str,
    request_id: &str,
    plan_id: Option<&str>,
) -> rusqlite::Result<Option<RequestReceipt>> {
    db.query_row(
        "SELECT scope_id, request_id, plan_id, confirmed_revision, request_hash, instance_id, operation_id
         FROM request_receipts WHERE scope_id = ?1 AND (request_id = ?2 OR (plan_id IS NOT NULL AND plan_id = ?3))
         ORDER BY request_id = ?2 DESC LIMIT 1",
        params![scope_id, request_id, plan_id],
        receipt_row,
    )
    .optional()
}

fn checked_revision(revision: u64) -> Result<i64, StoreConflict> {
    i64::try_from(revision)
        .ok()
        .filter(|value| *value > 0)
        .ok_or(StoreConflict::InvalidInput)
}

impl OperationJournal for DatabaseWorker {
    fn accept(
        &self,
        intent: &OperationIntent,
        receipt: &RequestReceipt,
    ) -> Result<RequestReceipt, StoreConflict> {
        if intent.id.is_empty()
            || intent.instance_id.is_empty()
            || intent.phase.is_empty()
            || receipt.scope_id.is_empty()
            || receipt.request_id.is_empty()
            || receipt.plan_id.as_deref() == Some("")
            || receipt.request_hash.len() != 64
            || !receipt
                .request_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || receipt.instance_id != intent.instance_id
            || receipt.operation_id != intent.id
        {
            return Err(StoreConflict::InvalidInput);
        }
        let expected = checked_revision(intent.expected_revision)?;
        let confirmed = checked_revision(receipt.confirmed_revision)?;
        let old = intent.old_spec_revision.map(checked_revision).transpose()?;
        let new = intent.new_spec_revision.map(checked_revision).transpose()?;
        let (intent, receipt) = (intent.clone(), receipt.clone());
        self.write(move |db| {
            let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = find_receipt(
                &transaction,
                &receipt.scope_id,
                &receipt.request_id,
                receipt.plan_id.as_deref(),
            )? {
                let same_plan = receipt.plan_id.is_some()
                    && receipt.plan_id == existing.plan_id
                    && receipt.confirmed_revision == existing.confirmed_revision
                    && receipt.request_hash == existing.request_hash;
                return Ok(if existing == receipt || same_plan {
                    Ok(existing)
                } else {
                    Err(StoreConflict::Duplicate)
                });
            }
            let current = transaction
                .query_row(
                    "SELECT revision FROM instances WHERE id = ?1 AND scope_id = ?2 AND lifecycle = 'managed'",
                    params![intent.instance_id, receipt.scope_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            match current {
                None => return Ok(Err(StoreConflict::Missing)),
                Some(revision) if revision != expected => {
                    return Ok(Err(StoreConflict::StaleRevision));
                }
                _ => {}
            }
            transaction.execute(
                "INSERT INTO operations (id, instance_id, kind, phase, expected_instance_revision, old_spec_revision, new_spec_revision)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![intent.id, intent.instance_id, kind_name(intent.kind), intent.phase, expected, old, new],
            )?;
            transaction.execute(
                "INSERT INTO request_receipts (scope_id, request_id, plan_id, confirmed_revision, request_hash, instance_id, operation_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![receipt.scope_id, receipt.request_id, receipt.plan_id, confirmed,
                    receipt.request_hash, receipt.instance_id, receipt.operation_id],
            )?;
            transaction.commit()?;
            Ok(Ok(receipt))
        })
        .map_err(map_error)?
    }

    fn receipt(
        &self,
        scope_id: &str,
        request_id: &str,
    ) -> Result<Option<RequestReceipt>, StoreConflict> {
        self.read(|db| Ok(find_receipt(db, scope_id, request_id, None)?))
            .map_err(map_error)
    }

    fn plan_receipt(
        &self,
        scope_id: &str,
        plan_id: &str,
    ) -> Result<Option<RequestReceipt>, StoreConflict> {
        self.read(|db| {
            Ok(db
                .query_row(
                    "SELECT scope_id, request_id, plan_id, confirmed_revision, request_hash, instance_id, operation_id
                     FROM request_receipts WHERE scope_id = ?1 AND plan_id = ?2",
                    params![scope_id, plan_id],
                    receipt_row,
                )
                .optional()?)
        })
        .map_err(map_error)
    }

    fn record_step(&self, step: &StepIntent) -> Result<(), StoreConflict> {
        if step.operation_id.is_empty() || step.resource_id.is_empty() {
            return Err(StoreConflict::InvalidInput);
        }
        let sequence = checked_revision(step.sequence)?;
        let attempt = checked_revision(step.attempt)?;
        let step = step.clone();
        self.write(move |db| {
            let count = db.execute(
                "INSERT INTO operation_steps (operation_id, sequence, attempt, command_kind, resource_id, expected_result)
                 SELECT id, ?2, ?3, ?4, ?5, ?6 FROM operations
                 WHERE id = ?1 AND attempt = ?3 AND status NOT IN ('Succeeded', 'Abandoned')",
                params![step.operation_id, sequence, attempt, step.command_kind.as_str(),
                    step.resource_id, step.expected_result.as_str()],
            )?;
            Ok(count)
        })
        .map_err(map_error)
        .and_then(|count| if count == 1 { Ok(()) } else { Err(StoreConflict::Missing) })
    }

    fn steps_for_resource(
        &self,
        operation_id: &str,
        resource_id: &str,
    ) -> Result<Vec<StepRecord>, StoreConflict> {
        let (operation_id, resource_id) = (operation_id.to_owned(), resource_id.to_owned());
        self.read(move |db| {
            let mut query = db.prepare(
                "SELECT o.instance_id, r.scope_id, s.sequence, s.attempt, s.command_kind,
                        s.resource_id, s.expected_result, s.outcome
                 FROM operation_steps s
                 JOIN operations o ON o.id = s.operation_id
                 JOIN request_receipts r ON r.operation_id = o.id
                 WHERE s.operation_id = ?1 AND s.resource_id = ?2
                 ORDER BY s.sequence",
            )?;
            let rows = query.query_map(params![operation_id, resource_id], |row| {
                let instance_id = row.get::<_, String>(0)?;
                let scope_id = row.get::<_, String>(1)?;
                let sequence = row.get::<_, i64>(2)?;
                let attempt = row.get::<_, i64>(3)?;
                let command = row.get::<_, String>(4)?;
                let resource_id = row.get::<_, String>(5)?;
                let expected = row.get::<_, String>(6)?;
                let outcome = row.get::<_, Option<String>>(7)?;
                Ok((
                    instance_id,
                    scope_id,
                    sequence,
                    attempt,
                    command,
                    resource_id,
                    expected,
                    outcome,
                ))
            })?;
            let mut records = Vec::new();
            for row in rows {
                let (
                    instance_id,
                    scope_id,
                    sequence,
                    attempt,
                    command,
                    resource_id,
                    expected,
                    outcome,
                ) = row?;
                let Some(command_kind) = step_command(&command) else {
                    return Err(DatabaseError::Sqlite(Error::InvalidQuery));
                };
                let Some(expected_result) = expected_result(&expected) else {
                    return Err(DatabaseError::Sqlite(Error::InvalidQuery));
                };
                let Some(outcome) = step_outcome(outcome) else {
                    return Err(DatabaseError::Sqlite(Error::InvalidQuery));
                };
                records.push(StepRecord {
                    instance_id,
                    scope_id,
                    sequence: u64::try_from(sequence)
                        .map_err(|_| DatabaseError::Sqlite(Error::InvalidQuery))?,
                    attempt: u64::try_from(attempt)
                        .map_err(|_| DatabaseError::Sqlite(Error::InvalidQuery))?,
                    command_kind,
                    resource_id,
                    expected_result,
                    outcome,
                });
            }
            Ok(records)
        })
        .map_err(map_error)
    }

    fn finish_step(
        &self,
        operation_id: &str,
        sequence: u64,
        outcome: StepOutcome,
    ) -> Result<(), StoreConflict> {
        let sequence = checked_revision(sequence)?;
        let operation_id = operation_id.to_owned();
        self.write(move |db| {
            let count = db.execute(
                "UPDATE operation_steps SET outcome = ?3, observed_at = CURRENT_TIMESTAMP
                 WHERE operation_id = ?1 AND sequence = ?2 AND outcome IS NULL",
                params![operation_id, sequence, outcome.as_str()],
            )?;
            Ok(count)
        })
        .map_err(map_error)
        .and_then(|count| {
            if count == 1 {
                Ok(())
            } else {
                Err(StoreConflict::Missing)
            }
        })
    }

    fn retry(&self, operation_id: &str, phase: &str) -> Result<u64, StoreConflict> {
        if phase.is_empty() {
            return Err(StoreConflict::InvalidInput);
        }
        let (operation_id, phase) = (operation_id.to_owned(), phase.to_owned());
        self.write(move |db| {
            let mut statement = db.prepare(
                "UPDATE operations SET attempt = attempt + 1, status = 'Executing', phase = ?2
                 WHERE id = ?1 AND status IN ('Failed', 'AwaitingDecision', 'OutcomeUnknown')
                 RETURNING attempt",
            )?;
            Ok(statement
                .query_row(params![operation_id, phase], |row| row.get::<_, i64>(0))
                .optional()?)
        })
        .map_err(map_error)?
        .map(|attempt| attempt as u64)
        .ok_or(StoreConflict::InvalidLifecycle)
    }

    fn set_status(
        &self,
        operation_id: &str,
        status: OperationStatus,
        phase: &str,
    ) -> Result<(), StoreConflict> {
        if phase.is_empty() {
            return Err(StoreConflict::InvalidInput);
        }
        let (operation_id, phase) = (operation_id.to_owned(), phase.to_owned());
        self.write(move |db| {
            let count = db.execute(
                "UPDATE operations SET status = ?2, phase = ?3,
                    completed_at = CASE WHEN ?4 THEN CURRENT_TIMESTAMP ELSE NULL END
                 WHERE id = ?1 AND status NOT IN ('Succeeded', 'Abandoned')
                   AND (?2 != 'Succeeded' OR NOT EXISTS (
                       SELECT 1 FROM operation_steps WHERE operation_id = ?1 AND outcome IS NULL))",
                params![
                    operation_id,
                    status_name(status),
                    phase,
                    !status.is_unresolved()
                ],
            )?;
            Ok(count)
        })
        .map_err(map_error)
        .and_then(|count| {
            if count == 1 {
                Ok(())
            } else {
                Err(StoreConflict::InvalidLifecycle)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use composenest_application::operation_journal::{ExpectedResult, StepCommand};
    use std::fs;
    use tempfile::TempDir;

    fn store() -> (TempDir, DatabaseWorker) {
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        for directory in ["state", "locks"] {
            fs::create_dir(root.path().join(directory)).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(
                    root.path().join(directory),
                    fs::Permissions::from_mode(0o700),
                )
                .unwrap();
            }
        }
        let worker = DatabaseWorker::start(root.path()).unwrap();
        worker.write(|db| {
            db.execute_batch("INSERT INTO management_scopes (id, owner_id, root_identity) VALUES ('scope', 'owner', 'root');
                INSERT INTO runtime_targets (id, scope_id, endpoint, engine_id, platform) VALUES ('target', 'scope', 'local', 'engine', 'linux');
                INSERT INTO instances (id, scope_id, target_id, display_name, normalized_name, project_name) VALUES ('instance', 'scope', 'target', 'Name', 'Name', 'cn-instance');
                INSERT INTO instance_specs VALUES ('instance', 1, '8', 'bind', '{}');")?;
            Ok(())
        }).unwrap();
        (root, worker)
    }

    fn intent(id: &str) -> OperationIntent {
        OperationIntent {
            id: id.into(),
            instance_id: "instance".into(),
            kind: OperationKind::Start,
            phase: "prepare".into(),
            expected_revision: 1,
            old_spec_revision: Some(1),
            new_spec_revision: Some(1),
        }
    }

    fn receipt(id: &str) -> RequestReceipt {
        RequestReceipt {
            scope_id: "scope".into(),
            request_id: id.into(),
            plan_id: Some(id.into()),
            confirmed_revision: 1,
            request_hash: "a".repeat(64),
            instance_id: "instance".into(),
            operation_id: id.into(),
        }
    }

    #[test]
    fn unresolved_statuses_keep_instance_exclusive() {
        let (_root, worker) = store();
        worker.accept(&intent("first"), &receipt("first")).unwrap();
        for status in [
            OperationStatus::Failed,
            OperationStatus::AwaitingDecision,
            OperationStatus::OutcomeUnknown,
        ] {
            worker.set_status("first", status, "observe").unwrap();
            assert_eq!(
                worker.accept(&intent("second"), &receipt("second")),
                Err(StoreConflict::Duplicate)
            );
            assert_eq!(worker.receipt("scope", "second").unwrap(), None);
        }
        assert_eq!(worker.retry("first", "retry"), Ok(2));
        assert_eq!(
            worker.accept(&intent("second"), &receipt("second")),
            Err(StoreConflict::Duplicate)
        );
        worker
            .set_status("first", OperationStatus::Abandoned, "done")
            .unwrap();
        worker
            .accept(&intent("second"), &receipt("second"))
            .unwrap();
    }

    #[test]
    fn receipt_and_step_survive_restart_without_raw_arguments() {
        let (root, worker) = store();
        let accepted = worker.accept(&intent("first"), &receipt("first")).unwrap();
        let step = StepIntent {
            operation_id: "first".into(),
            sequence: 1,
            attempt: 1,
            command_kind: StepCommand::ComposeStart,
            resource_id: "instance".into(),
            expected_result: ExpectedResult::ContainerRunning,
        };
        worker.record_step(&step).unwrap();
        assert_eq!(
            worker.set_status("first", OperationStatus::Succeeded, "done"),
            Err(StoreConflict::InvalidLifecycle)
        );
        worker
            .finish_step("first", 1, StepOutcome::Succeeded)
            .unwrap();
        worker
            .set_status("first", OperationStatus::Succeeded, "done")
            .unwrap();
        drop(worker);
        let reopened = DatabaseWorker::start(root.path()).unwrap();
        assert_eq!(
            reopened.receipt("scope", "first"),
            Ok(Some(accepted.clone()))
        );
        assert_eq!(
            reopened.plan_receipt("scope", "first"),
            Ok(Some(accepted.clone()))
        );
        assert_eq!(reopened.accept(&intent("first"), &accepted), Ok(accepted));
        assert_eq!(
            reopened.finish_step("first", 1, StepOutcome::Succeeded),
            Err(StoreConflict::Missing)
        );
        let stored: (String, String) = reopened.read(|db| Ok(db.query_row(
            "SELECT command_kind, expected_result FROM operation_steps WHERE operation_id = 'first'",
            [], |row| Ok((row.get(0)?, row.get(1)?)))?)).unwrap();
        assert_eq!(stored, ("compose_start".into(), "container_running".into()));
    }

    #[test]
    fn changed_request_or_plan_cannot_claim_prior_result() {
        let (_root, worker) = store();
        worker.accept(&intent("first"), &receipt("first")).unwrap();
        let mut changed = receipt("first");
        changed.request_hash = "b".repeat(64);
        assert_eq!(
            worker.accept(&intent("first"), &changed),
            Err(StoreConflict::Duplicate)
        );
        let mut same_plan = receipt("second");
        same_plan.plan_id = Some("first".into());
        assert_eq!(
            worker.accept(&intent("second"), &same_plan),
            Ok(receipt("first"))
        );
        same_plan.confirmed_revision = 2;
        assert_eq!(
            worker.accept(&intent("second"), &same_plan),
            Err(StoreConflict::Duplicate)
        );
    }

    #[test]
    fn missing_spec_is_not_reported_as_duplicate() {
        let (_root, worker) = store();
        let mut operation = intent("missing-spec");
        operation.old_spec_revision = Some(99);
        assert_eq!(
            worker.accept(&operation, &receipt("missing-spec")),
            Err(StoreConflict::Missing)
        );
        assert_eq!(worker.receipt("scope", "missing-spec"), Ok(None));
        assert_eq!(
            worker.accept(&intent("valid"), &receipt("valid")),
            Ok(receipt("valid"))
        );
        let invalid_status = worker
            .write(|db| {
                db.execute(
                    "UPDATE operations SET status = 'invalid' WHERE id = 'valid'",
                    [],
                )?;
                Ok(())
            })
            .unwrap_err();
        assert_eq!(map_error(invalid_status), StoreConflict::InvalidInput);
    }

    #[test]
    fn journal_uses_domain_kind_and_status_vocabulary() {
        let (_root, worker) = store();
        let mut operation = intent("rename");
        operation.kind = OperationKind::Rename;
        worker.accept(&operation, &receipt("rename")).unwrap();
        let persisted: (String, String) = worker
            .read(|db| {
                Ok(db.query_row(
                    "SELECT kind, status FROM operations WHERE id = 'rename'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?)
            })
            .unwrap();
        assert_eq!(persisted, ("rename".into(), "Accepted".into()));
        worker
            .set_status("rename", OperationStatus::Executing, "run")
            .unwrap();
        worker
            .set_status("rename", OperationStatus::Failed, "observe")
            .unwrap();
        worker
            .set_status("rename", OperationStatus::Abandoned, "done")
            .unwrap();
        let mut recovery = intent("recover");
        recovery.kind = OperationKind::Recover;
        worker.accept(&recovery, &receipt("recover")).unwrap();
        worker
            .set_status("recover", OperationStatus::Executing, "run")
            .unwrap();
        let persisted: (String, String) = worker
            .read(|db| {
                Ok(db.query_row(
                    "SELECT kind, status FROM operations WHERE id = 'recover'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?)
            })
            .unwrap();
        assert_eq!(persisted, ("recover".into(), "Executing".into()));
    }
}
