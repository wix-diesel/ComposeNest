//! SQLite operation journal and durable request receipts.

use composenest_application::operation_journal::{
    CloneSourceGuard, ExpectedResult, OperationIntent, OperationJournal, OperationKind,
    OperationStatus, PlanCommitStore, RequestReceipt, StepCommand, StepIntent, StepOutcome,
    StepRecord,
};
use composenest_application::operation_recovery::{
    RecoverableOperation, RecoveryJournal, VerifiedAbsence,
};
use composenest_application::state_store::{InstanceRecord, RuntimeTarget, StoreConflict};
use rusqlite::Error;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
    sea_query::Expr,
};

use crate::entities::{
    instance as instance_entity, instance_spec, operation as operation_entity, operation_step,
    request_receipt, runtime_target,
};
use crate::sqlite::{DatabaseError, DatabaseWorker};
use crate::state_store::{insert_instance, map_error};

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

fn operation_kind(value: &str) -> Option<OperationKind> {
    Some(match value {
        "create" => OperationKind::Create,
        "clone" => OperationKind::Clone,
        "start" => OperationKind::Start,
        "stop" => OperationKind::Stop,
        "restart" => OperationKind::Restart,
        "rename" => OperationKind::Rename,
        "edit_port" => OperationKind::EditPort,
        "delete" => OperationKind::Delete,
        "recover" => OperationKind::Recover,
        _ => return None,
    })
}

impl PlanCommitStore for DatabaseWorker {
    fn commit_plan(
        &self,
        instance: &InstanceRecord,
        intent: &OperationIntent,
        receipt: &RequestReceipt,
        target_guard: &RuntimeTarget,
        clone_guard: Option<&CloneSourceGuard>,
    ) -> Result<RequestReceipt, StoreConflict> {
        let valid = !instance.id.is_empty()
            && !instance.scope_id.is_empty()
            && !intent.id.is_empty()
            && intent.instance_id == instance.id
            && ((instance.clone_source_id.is_none() && intent.kind == OperationKind::Create)
                || (instance.clone_source_id.is_some() && intent.kind == OperationKind::Clone))
            && intent.expected_revision == 1
            && intent.old_spec_revision.is_none()
            && intent.new_spec_revision == Some(1)
            && receipt.scope_id == instance.scope_id
            && receipt.instance_id == instance.id
            && receipt.operation_id == intent.id
            && receipt.plan_id.as_deref().is_some_and(|id| !id.is_empty())
            && !receipt.request_id.is_empty()
            && receipt.request_hash.len() == 64
            && receipt
                .request_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            && target_guard.id == instance.target_id
            && target_guard.scope_id == instance.scope_id
            && clone_guard.map(|guard| guard.instance_id.as_str())
                == instance.clone_source_id.as_deref();
        if !valid {
            return Err(StoreConflict::InvalidInput);
        }
        let confirmed = checked_revision(receipt.confirmed_revision)?;
        let guard_revision = clone_guard
            .map(|guard| checked_revision(guard.revision))
            .transpose()?;
        let guard_spec_revision = clone_guard
            .map(|guard| checked_revision(guard.spec_revision))
            .transpose()?;
        let (instance, intent, receipt, target_guard, guard) = (
            instance.clone(),
            intent.clone(),
            receipt.clone(),
            target_guard.clone(),
            clone_guard.cloned(),
        );
        self.orm_write(move |db| {
            let transaction = db.begin()?;
            if let Some(existing) = find_receipt_orm(
                &transaction,
                &receipt.scope_id,
                &receipt.request_id,
                receipt.plan_id.as_deref(),
            )? {
                let same_plan = existing.plan_id == receipt.plan_id
                    && existing.confirmed_revision == receipt.confirmed_revision
                    && existing.request_hash == receipt.request_hash;
                return Ok(if same_plan {
                    Ok(existing)
                } else {
                    Err(StoreConflict::Duplicate)
                });
            }
            let target = runtime_target::Entity::find_by_id(&target_guard.id).one(&transaction)?;
            let target_matches = target.is_some_and(|target| {
                target.scope_id == target_guard.scope_id
                    && target.endpoint == target_guard.endpoint
                    && target.engine_id == target_guard.engine_id
                    && target.platform == target_guard.platform
            });
            if !target_matches {
                return Ok(Err(StoreConflict::StaleRevision));
            }
            if let (Some(guard), Some(revision), Some(spec_revision)) =
                (&guard, guard_revision, guard_spec_revision)
            {
                let current =
                    instance_entity::Entity::find_by_id(&guard.instance_id).one(&transaction)?;
                let matches = current.is_some_and(|current| {
                    current.scope_id == instance.scope_id
                        && current.target_id == instance.target_id
                        && current.lifecycle == "managed"
                        && current.revision == revision
                });
                let latest = operation_entity::Entity::find()
                    .filter(operation_entity::Column::InstanceId.eq(&guard.instance_id))
                    .filter(operation_entity::Column::Status.eq("Succeeded"))
                    .all(&transaction)?
                    .into_iter()
                    .filter_map(|operation| operation.new_spec_revision)
                    .max()
                    .unwrap_or(1);
                if !matches
                    || latest != spec_revision
                    || instance_spec::Entity::find_by_id((guard.instance_id.clone(), latest))
                        .one(&transaction)?
                        .is_none()
                {
                    return Ok(Err(StoreConflict::StaleRevision));
                }
                let unresolved = operation_entity::Entity::find()
                    .filter(operation_entity::Column::InstanceId.eq(&guard.instance_id))
                    .filter(operation_entity::Column::Status.is_not_in(["Succeeded", "Abandoned"]))
                    .one(&transaction)?
                    .is_some();
                if unresolved {
                    return Ok(Err(StoreConflict::InvalidLifecycle));
                }
            }
            insert_instance(&transaction, &instance)?;
            operation_entity::Entity::insert(operation_entity::ActiveModel {
                id: Set(intent.id),
                instance_id: Set(intent.instance_id),
                kind: Set(kind_name(intent.kind).into()),
                phase: Set(intent.phase),
                expected_instance_revision: Set(1),
                new_spec_revision: Set(Some(1)),
                ..Default::default()
            })
            .exec(&transaction)?;
            let result = receipt.clone();
            request_receipt::Entity::insert(request_receipt::ActiveModel {
                scope_id: Set(receipt.scope_id),
                request_id: Set(receipt.request_id),
                plan_id: Set(receipt.plan_id),
                confirmed_revision: Set(confirmed),
                request_hash: Set(receipt.request_hash),
                instance_id: Set(receipt.instance_id),
                operation_id: Set(receipt.operation_id),
                ..Default::default()
            })
            .exec(&transaction)?;
            transaction.commit()?;
            Ok(Ok(result))
        })
        .map_err(|error| match error {
            DatabaseError::Sqlite(Error::QueryReturnedNoRows) => StoreConflict::Missing,
            other => map_error(other),
        })?
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

fn operation_status(value: &str) -> Option<OperationStatus> {
    Some(match value {
        "Accepted" => OperationStatus::Accepted,
        "Executing" => OperationStatus::Executing,
        "Failed" => OperationStatus::Failed,
        "AwaitingDecision" => OperationStatus::AwaitingDecision,
        "OutcomeUnknown" => OperationStatus::OutcomeUnknown,
        "Succeeded" => OperationStatus::Succeeded,
        "Abandoned" => OperationStatus::Abandoned,
        _ => return None,
    })
}

fn step_command(value: &str) -> Option<StepCommand> {
    Some(match value {
        "generate_artifact" => StepCommand::GenerateArtifact,
        "resolve_image" => StepCommand::ResolveImage,
        "create_bind" => StepCommand::CreateBind,
        "create_volume" => StepCommand::CreateVolume,
        "compose_create" => StepCommand::ComposeCreate,
        "compose_start" => StepCommand::ComposeStart,
        "compose_stop" => StepCommand::ComposeStop,
        "compose_restart" => StepCommand::ComposeRestart,
        "remove_container" => StepCommand::RemoveContainer,
        "observe" => StepCommand::Observe,
        _ => return None,
    })
}

fn expected_result(value: &str) -> Option<ExpectedResult> {
    Some(match value {
        "artifact_ready" => ExpectedResult::ArtifactReady,
        "image_resolved" => ExpectedResult::ImageResolved,
        "bind_created" => ExpectedResult::BindCreated,
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

fn receipt_from_model(model: request_receipt::Model) -> Result<RequestReceipt, DatabaseError> {
    Ok(RequestReceipt {
        scope_id: model.scope_id,
        request_id: model.request_id,
        plan_id: model.plan_id,
        confirmed_revision: u64::try_from(model.confirmed_revision)
            .map_err(|_| DatabaseError::InvalidInput)?,
        request_hash: model.request_hash,
        instance_id: model.instance_id,
        operation_id: model.operation_id,
    })
}

fn find_receipt_orm(
    db: &sea_orm::DatabaseTransaction,
    scope_id: &str,
    request_id: &str,
    plan_id: Option<&str>,
) -> Result<Option<RequestReceipt>, DatabaseError> {
    let receipt = request_receipt::Entity::find_by_id((scope_id.to_owned(), request_id.to_owned()))
        .one(db)?;
    let receipt = if receipt.is_some() || plan_id.is_none() {
        receipt
    } else {
        request_receipt::Entity::find()
            .filter(request_receipt::Column::ScopeId.eq(scope_id))
            .filter(request_receipt::Column::PlanId.eq(plan_id.unwrap_or_default()))
            .one(db)?
    };
    receipt.map(receipt_from_model).transpose()
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
        self.orm_write(move |db| {
            let transaction = db.begin()?;
            if let Some(existing) = find_receipt_orm(
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
            let current =
                instance_entity::Entity::find_by_id(&intent.instance_id).one(&transaction)?;
            let Some(current) = current.filter(|instance| {
                instance.scope_id == receipt.scope_id && instance.lifecycle == "managed"
            }) else {
                return Ok(Err(StoreConflict::Missing));
            };
            if current.revision != expected {
                return Ok(Err(StoreConflict::StaleRevision));
            }
            operation_entity::Entity::insert(operation_entity::ActiveModel {
                id: Set(intent.id),
                instance_id: Set(intent.instance_id),
                kind: Set(kind_name(intent.kind).into()),
                phase: Set(intent.phase),
                expected_instance_revision: Set(expected),
                old_spec_revision: Set(old),
                new_spec_revision: Set(new),
                ..Default::default()
            })
            .exec(&transaction)?;
            let result = receipt.clone();
            request_receipt::Entity::insert(request_receipt::ActiveModel {
                scope_id: Set(receipt.scope_id),
                request_id: Set(receipt.request_id),
                plan_id: Set(receipt.plan_id),
                confirmed_revision: Set(confirmed),
                request_hash: Set(receipt.request_hash),
                instance_id: Set(receipt.instance_id),
                operation_id: Set(receipt.operation_id),
                ..Default::default()
            })
            .exec(&transaction)?;
            transaction.commit()?;
            Ok(Ok(result))
        })
        .map_err(map_error)?
    }

    fn receipt(
        &self,
        scope_id: &str,
        request_id: &str,
    ) -> Result<Option<RequestReceipt>, StoreConflict> {
        self.orm_read(|db| find_receipt_orm(db, scope_id, request_id, None))
            .map_err(map_error)
    }

    fn plan_receipt(
        &self,
        scope_id: &str,
        plan_id: &str,
    ) -> Result<Option<RequestReceipt>, StoreConflict> {
        self.orm_read(|db| {
            request_receipt::Entity::find()
                .filter(request_receipt::Column::ScopeId.eq(scope_id))
                .filter(request_receipt::Column::PlanId.eq(plan_id))
                .one(db)?
                .map(receipt_from_model)
                .transpose()
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
        self.orm_write(move |db| {
            let transaction = db.begin()?;
            let parent =
                operation_entity::Entity::find_by_id(&step.operation_id).one(&transaction)?;
            let Some(parent) = parent.filter(|parent| {
                parent.attempt == attempt
                    && matches!(parent.status.as_str(), "Accepted" | "Executing")
            }) else {
                return Err(DatabaseError::Missing);
            };
            let previous = operation_step::Entity::find()
                .filter(operation_step::Column::OperationId.eq(&step.operation_id))
                .order_by_desc(operation_step::Column::Sequence)
                .one(&transaction)?;
            let next = previous
                .as_ref()
                .map_or(1, |last| last.sequence.saturating_add(1));
            if sequence != next || previous.is_some_and(|last| last.outcome.is_none()) {
                return Err(DatabaseError::InvalidInput);
            }
            operation_step::Entity::insert(operation_step::ActiveModel {
                operation_id: Set(parent.id),
                sequence: Set(sequence),
                attempt: Set(attempt),
                command_kind: Set(step.command_kind.as_str().into()),
                resource_id: Set(step.resource_id),
                expected_result: Set(step.expected_result.as_str().into()),
                ..Default::default()
            })
            .exec(&transaction)?;
            transaction.commit()?;
            Ok(())
        })
        .map_err(map_error)
    }

    fn steps_for_resource(
        &self,
        operation_id: &str,
        resource_id: &str,
    ) -> Result<Vec<StepRecord>, StoreConflict> {
        self.orm_read(|db| {
            let steps = operation_step::Entity::find()
                .filter(operation_step::Column::OperationId.eq(operation_id))
                .filter(operation_step::Column::ResourceId.eq(resource_id))
                .order_by_asc(operation_step::Column::Sequence)
                .all(db)?;
            if steps.is_empty() {
                return Ok(Vec::new());
            }
            let operation = operation_entity::Entity::find_by_id(operation_id).one(db)?;
            let receipt = request_receipt::Entity::find()
                .filter(request_receipt::Column::OperationId.eq(operation_id))
                .one(db)?;
            let (Some(operation), Some(receipt)) = (operation, receipt) else {
                return Ok(Vec::new());
            };
            steps
                .into_iter()
                .map(|step| {
                    Ok(StepRecord {
                        instance_id: operation.instance_id.clone(),
                        scope_id: receipt.scope_id.clone(),
                        sequence: u64::try_from(step.sequence)
                            .map_err(|_| DatabaseError::InvalidInput)?,
                        attempt: u64::try_from(step.attempt)
                            .map_err(|_| DatabaseError::InvalidInput)?,
                        command_kind: step_command(&step.command_kind)
                            .ok_or(DatabaseError::InvalidInput)?,
                        resource_id: step.resource_id,
                        expected_result: expected_result(&step.expected_result)
                            .ok_or(DatabaseError::InvalidInput)?,
                        outcome: step_outcome(step.outcome).ok_or(DatabaseError::InvalidInput)?,
                    })
                })
                .collect()
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
        self.orm_write(move |db| {
            Ok(operation_step::Entity::update_many()
                .col_expr(
                    operation_step::Column::Outcome,
                    Expr::value(outcome.as_str()),
                )
                .col_expr(
                    operation_step::Column::ObservedAt,
                    Expr::cust("CURRENT_TIMESTAMP"),
                )
                .filter(operation_step::Column::OperationId.eq(operation_id))
                .filter(operation_step::Column::Sequence.eq(sequence))
                .filter(operation_step::Column::Outcome.is_null())
                .exec(db)?
                .rows_affected)
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
        self.orm_write(move |db| {
            let transaction = db.begin()?;
            let operation =
                operation_entity::Entity::find_by_id(&operation_id).one(&transaction)?;
            let Some(operation) = operation.filter(|operation| {
                matches!(
                    operation.status.as_str(),
                    "Failed" | "AwaitingDecision" | "OutcomeUnknown"
                )
            }) else {
                return Ok(None);
            };
            if operation_step::Entity::find()
                .filter(operation_step::Column::OperationId.eq(&operation_id))
                .filter(operation_step::Column::Outcome.is_null())
                .one(&transaction)?
                .is_some()
            {
                return Ok(None);
            }
            let attempt = operation
                .attempt
                .checked_add(1)
                .ok_or(DatabaseError::InvalidInput)?;
            operation_entity::ActiveModel {
                id: Set(operation.id),
                attempt: Set(attempt),
                status: Set("Executing".into()),
                phase: Set(phase),
                ..Default::default()
            }
            .update(&transaction)?;
            transaction.commit()?;
            Ok(Some(attempt))
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
        if status == OperationStatus::Abandoned {
            return Err(StoreConflict::InvalidLifecycle);
        }
        let (operation_id, phase) = (operation_id.to_owned(), phase.to_owned());
        self.orm_write(move |db| {
            let transaction = db.begin()?;
            let current = operation_entity::Entity::find_by_id(&operation_id).one(&transaction)?;
            let Some(current) = current
                .filter(|current| !matches!(current.status.as_str(), "Succeeded" | "Abandoned"))
            else {
                return Ok(false);
            };
            if status == OperationStatus::Succeeded
                && operation_step::Entity::find()
                    .filter(operation_step::Column::OperationId.eq(&operation_id))
                    .filter(operation_step::Column::Outcome.is_null())
                    .one(&transaction)?
                    .is_some()
            {
                return Ok(false);
            }
            let completed_at = if status.is_unresolved() {
                Expr::value(Option::<String>::None)
            } else {
                Expr::cust("CURRENT_TIMESTAMP")
            };
            operation_entity::Entity::update_many()
                .col_expr(
                    operation_entity::Column::Status,
                    Expr::value(status_name(status)),
                )
                .col_expr(operation_entity::Column::Phase, Expr::value(phase))
                .col_expr(operation_entity::Column::CompletedAt, completed_at)
                .filter(operation_entity::Column::Id.eq(current.id))
                .exec(&transaction)?;
            transaction.commit()?;
            Ok(true)
        })
        .map_err(map_error)
        .and_then(|changed| {
            if changed {
                Ok(())
            } else {
                Err(StoreConflict::InvalidLifecycle)
            }
        })
    }
}

fn recovery_snapshot(
    db: &sea_orm::DatabaseTransaction,
    operation: operation_entity::Model,
) -> Result<RecoverableOperation, DatabaseError> {
    let receipt = request_receipt::Entity::find()
        .filter(request_receipt::Column::OperationId.eq(&operation.id))
        .one(db)?
        .ok_or(DatabaseError::Missing)?;
    let steps = operation_step::Entity::find()
        .filter(operation_step::Column::OperationId.eq(&operation.id))
        .order_by_asc(operation_step::Column::Sequence)
        .all(db)?
        .into_iter()
        .map(|step| {
            Ok(StepRecord {
                instance_id: operation.instance_id.clone(),
                scope_id: receipt.scope_id.clone(),
                sequence: u64::try_from(step.sequence).map_err(|_| DatabaseError::InvalidInput)?,
                attempt: u64::try_from(step.attempt).map_err(|_| DatabaseError::InvalidInput)?,
                command_kind: step_command(&step.command_kind)
                    .ok_or(DatabaseError::InvalidInput)?,
                resource_id: step.resource_id,
                expected_result: expected_result(&step.expected_result)
                    .ok_or(DatabaseError::InvalidInput)?,
                outcome: step_outcome(step.outcome).ok_or(DatabaseError::InvalidInput)?,
            })
        })
        .collect::<Result<Vec<_>, DatabaseError>>()?;
    Ok(RecoverableOperation {
        id: operation.id,
        instance_id: operation.instance_id,
        kind: operation_kind(&operation.kind).ok_or(DatabaseError::InvalidInput)?,
        status: operation_status(&operation.status).ok_or(DatabaseError::InvalidInput)?,
        phase: operation.phase,
        attempt: u64::try_from(operation.attempt).map_err(|_| DatabaseError::InvalidInput)?,
        steps,
    })
}

impl RecoveryJournal for DatabaseWorker {
    fn recover_on_startup(&self) -> Result<Vec<RecoverableOperation>, StoreConflict> {
        self.orm_write(|db| {
            let transaction = db.begin()?;
            operation_entity::Entity::update_many()
                .col_expr(
                    operation_entity::Column::Status,
                    Expr::value("OutcomeUnknown"),
                )
                .filter(operation_entity::Column::Status.eq("Executing"))
                .exec(&transaction)?;
            let unresolved = operation_entity::Entity::find()
                .filter(operation_entity::Column::Status.is_not_in(["Succeeded", "Abandoned"]))
                .order_by_asc(operation_entity::Column::StartedAt)
                .all(&transaction)?;
            let snapshots = unresolved
                .into_iter()
                .map(|operation| recovery_snapshot(&transaction, operation))
                .collect::<Result<Vec<_>, _>>()?;
            transaction.commit()?;
            Ok(snapshots)
        })
        .map_err(map_error)
    }

    fn recoverable(&self, operation_id: &str) -> Result<RecoverableOperation, StoreConflict> {
        self.orm_read(|db| {
            let operation = operation_entity::Entity::find_by_id(operation_id)
                .one(db)?
                .filter(|operation| !matches!(operation.status.as_str(), "Succeeded" | "Abandoned"))
                .ok_or(DatabaseError::Missing)?;
            recovery_snapshot(db, operation)
        })
        .map_err(map_error)
    }

    fn reconcile_step(
        &self,
        operation_id: &str,
        sequence: u64,
        outcome: StepOutcome,
    ) -> Result<(), StoreConflict> {
        let sequence = checked_revision(sequence)?;
        let operation_id = operation_id.to_owned();
        self.orm_write(move |db| {
            let transaction = db.begin()?;
            let operation = operation_entity::Entity::find_by_id(&operation_id)
                .one(&transaction)?
                .ok_or(DatabaseError::Missing)?;
            if !matches!(
                operation.status.as_str(),
                "OutcomeUnknown" | "Failed" | "AwaitingDecision"
            ) {
                return Err(DatabaseError::InvalidInput);
            }
            let step = operation_step::Entity::find_by_id((operation_id.clone(), sequence))
                .one(&transaction)?
                .ok_or(DatabaseError::Missing)?;
            if !matches!(step.outcome.as_deref(), None | Some("unknown")) {
                return Err(DatabaseError::InvalidInput);
            }
            operation_step::Entity::update_many()
                .col_expr(
                    operation_step::Column::Outcome,
                    Expr::value(outcome.as_str()),
                )
                .col_expr(
                    operation_step::Column::ObservedAt,
                    Expr::cust("CURRENT_TIMESTAMP"),
                )
                .filter(operation_step::Column::OperationId.eq(&operation_id))
                .filter(operation_step::Column::Sequence.eq(sequence))
                .exec(&transaction)?;
            transaction.commit()?;
            Ok(())
        })
        .map_err(map_error)
    }

    fn abandon_verified(&self, evidence: &VerifiedAbsence) -> Result<(), StoreConflict> {
        let attempt = checked_revision(evidence.attempt())?;
        let operation_id = evidence.operation_id().to_owned();
        self.orm_write(move |db| {
            let transaction = db.begin()?;
            let operation = operation_entity::Entity::find_by_id(&operation_id)
                .one(&transaction)?
                .ok_or(DatabaseError::Missing)?;
            if operation.attempt != attempt
                || !matches!(
                    operation.status.as_str(),
                    "Failed" | "AwaitingDecision" | "OutcomeUnknown"
                )
                || operation_step::Entity::find()
                    .filter(operation_step::Column::OperationId.eq(&operation_id))
                    .filter(operation_step::Column::Outcome.is_null())
                    .one(&transaction)?
                    .is_some()
                || operation_step::Entity::find()
                    .filter(operation_step::Column::OperationId.eq(&operation_id))
                    .filter(operation_step::Column::Outcome.eq("unknown"))
                    .one(&transaction)?
                    .is_some()
            {
                return Err(DatabaseError::InvalidInput);
            }
            operation_entity::Entity::update_many()
                .col_expr(operation_entity::Column::Status, Expr::value("Abandoned"))
                .col_expr(operation_entity::Column::Phase, Expr::value("abandoned"))
                .col_expr(
                    operation_entity::Column::CompletedAt,
                    Expr::cust("CURRENT_TIMESTAMP"),
                )
                .filter(operation_entity::Column::Id.eq(&operation_id))
                .exec(&transaction)?;
            transaction.commit()?;
            Ok(())
        })
        .map_err(map_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use composenest_application::operation_journal::{ExpectedResult, StepCommand};
    use composenest_application::operation_recovery::{
        CurrentRuntime, RecoveryDecision, RecoveryEvidence, RecoveryProbe, abandon_operation,
        resolve_operation,
    };
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

    struct VerifiedRuntime(CurrentRuntime);

    impl RecoveryProbe for VerifiedRuntime {
        fn inspect(
            &self,
            _: &RecoverableOperation,
        ) -> impl std::future::Future<Output = RecoveryEvidence> + Send {
            std::future::ready(RecoveryEvidence {
                previous_cli_exited: true,
                target_matches: true,
                artifact_matches: true,
                storage_verified: true,
                docker_verified: true,
                runtime: self.0,
            })
        }
    }

    #[tokio::test]
    async fn startup_marks_executing_unknown_and_preserves_identity_and_reservations() {
        let (root, worker) = store();
        worker.accept(&intent("first"), &receipt("first")).unwrap();
        worker
            .record_step(&StepIntent {
                operation_id: "first".into(),
                sequence: 1,
                attempt: 1,
                command_kind: StepCommand::ComposeStart,
                resource_id: "instance".into(),
                expected_result: ExpectedResult::ContainerRunning,
            })
            .unwrap();
        worker
            .set_status("first", OperationStatus::Executing, "start")
            .unwrap();
        drop(worker);

        let worker = DatabaseWorker::start(root.path()).unwrap();
        let pending = worker.recover_on_startup().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].status, OperationStatus::OutcomeUnknown);
        assert_eq!(pending[0].attempt, 1);
        assert_eq!(pending[0].steps[0].outcome, None);
        assert_eq!(
            worker.retry("first", "start"),
            Err(StoreConflict::InvalidLifecycle)
        );
        assert_eq!(
            worker.accept(&intent("second"), &receipt("second")),
            Err(StoreConflict::Duplicate)
        );
        worker
            .reconcile_step("first", 1, StepOutcome::Failed)
            .unwrap();
        worker
            .set_status("first", OperationStatus::Failed, "start")
            .unwrap();
        assert_eq!(
            composenest_application::operation_recovery::retry_operation(
                &worker,
                "first",
                &VerifiedRuntime(CurrentRuntime::Stopped)
            )
            .await,
            Ok(2)
        );
        assert_eq!(
            worker
                .receipt("scope", "first")
                .unwrap()
                .unwrap()
                .operation_id,
            "first"
        );
    }

    #[tokio::test]
    async fn failed_status_remains_visible_when_current_runtime_is_ready() {
        let (_root, worker) = store();
        worker.accept(&intent("first"), &receipt("first")).unwrap();
        worker
            .record_step(&StepIntent {
                operation_id: "first".into(),
                sequence: 1,
                attempt: 1,
                command_kind: StepCommand::ComposeStart,
                resource_id: "instance".into(),
                expected_result: ExpectedResult::ContainerRunning,
            })
            .unwrap();
        worker.finish_step("first", 1, StepOutcome::Failed).unwrap();
        worker
            .set_status("first", OperationStatus::Failed, "start")
            .unwrap();
        let report = resolve_operation(&worker, "first", &VerifiedRuntime(CurrentRuntime::Ready))
            .await
            .unwrap();
        assert_eq!(report.operation.status, OperationStatus::Failed);
        assert_eq!(report.current_runtime, CurrentRuntime::Ready);
        assert_eq!(report.decision, RecoveryDecision::Complete);
        assert_eq!(
            worker.recoverable("first").unwrap().status,
            OperationStatus::Failed
        );
    }

    #[tokio::test]
    async fn unresolved_statuses_keep_instance_exclusive() {
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
            .set_status("first", OperationStatus::Failed, "observe")
            .unwrap();
        assert_eq!(
            worker.set_status("first", OperationStatus::Abandoned, "done"),
            Err(StoreConflict::InvalidLifecycle)
        );
        abandon_operation(&worker, "first", &VerifiedRuntime(CurrentRuntime::Absent))
            .await
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
    fn next_step_requires_prior_observation_and_monotonic_sequence() {
        let (_root, worker) = store();
        worker.accept(&intent("first"), &receipt("first")).unwrap();
        let first = StepIntent {
            operation_id: "first".into(),
            sequence: 1,
            attempt: 1,
            command_kind: StepCommand::Observe,
            resource_id: "instance".into(),
            expected_result: ExpectedResult::StateObserved,
        };
        let mut next = first.clone();
        next.sequence = 2;
        assert_eq!(worker.record_step(&next), Err(StoreConflict::InvalidInput));
        worker.record_step(&first).unwrap();
        assert_eq!(worker.record_step(&next), Err(StoreConflict::InvalidInput));
        worker
            .finish_step("first", 1, StepOutcome::Succeeded)
            .unwrap();
        worker.record_step(&next).unwrap();
        worker
            .set_status("first", OperationStatus::Failed, "observe")
            .unwrap();
        let mut third = next;
        third.sequence = 3;
        assert_eq!(worker.record_step(&third), Err(StoreConflict::Missing));
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

    #[tokio::test]
    async fn journal_uses_domain_kind_and_status_vocabulary() {
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
        abandon_operation(&worker, "rename", &VerifiedRuntime(CurrentRuntime::Absent))
            .await
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
