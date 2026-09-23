//! SQLite operation journal and durable request receipts.

use composenest_application::operation_journal::{
    OperationIntent, OperationJournal, OperationStatus, RequestReceipt, StepIntent, StepOutcome,
};
use composenest_application::state_store::StoreConflict;
use rusqlite::{Connection, Error, OptionalExtension, TransactionBehavior, params};

use crate::sqlite::{DatabaseError, DatabaseWorker};

fn conflict(error: DatabaseError) -> StoreConflict {
    match error {
        DatabaseError::Sqlite(Error::SqliteFailure(code, _))
            if code.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            StoreConflict::Duplicate
        }
        _ => StoreConflict::Backend,
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
                params![intent.id, intent.instance_id, intent.kind.as_str(), intent.phase, expected, old, new],
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
        .map_err(conflict)?
    }

    fn receipt(
        &self,
        scope_id: &str,
        request_id: &str,
    ) -> Result<Option<RequestReceipt>, StoreConflict> {
        self.read(|db| Ok(find_receipt(db, scope_id, request_id, None)?))
            .map_err(conflict)
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
        .map_err(conflict)
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
        .map_err(conflict)
        .and_then(|count| if count == 1 { Ok(()) } else { Err(StoreConflict::Missing) })
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
        .map_err(conflict)
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
                "UPDATE operations SET attempt = attempt + 1, status = 'Running', phase = ?2
                 WHERE id = ?1 AND status IN ('Failed', 'AwaitingDecision', 'OutcomeUnknown')
                 RETURNING attempt",
            )?;
            Ok(statement
                .query_row(params![operation_id, phase], |row| row.get::<_, i64>(0))
                .optional()?)
        })
        .map_err(conflict)?
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
                params![operation_id, status.as_str(), phase, status.is_resolved()],
            )?;
            Ok(count)
        })
        .map_err(conflict)
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
    use composenest_application::operation_journal::{ExpectedResult, OperationKind, StepCommand};
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
}
