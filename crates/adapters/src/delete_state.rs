//! SQLite deletion guards and atomic retention of saved configuration and data.

use crate::{
    entities::{
        artifact, instance, operation, operation_step, port_reservation, runtime_observation,
        storage_allocation,
    },
    sqlite::{DatabaseError, DatabaseWorker},
    state_store::{map_error, presence_name},
};
use composenest_application::{
    delete_operation::{DeleteState, StorageCheck},
    operation_journal::{OperationJournal, RequestReceipt},
    state_store::StoreConflict,
};
use sea_orm::{
    ColumnTrait, EntityTrait, ExprTrait, QueryFilter, Set, TransactionTrait,
    sea_query::{Expr, OnConflict},
};

impl DeleteState for DatabaseWorker {
    fn delete_pending(&self, receipt: &RequestReceipt) -> Result<bool, StoreConflict> {
        if self
            .receipt(&receipt.scope_id, &receipt.request_id)?
            .as_ref()
            != Some(receipt)
        {
            return Err(StoreConflict::InvalidInput);
        }
        self.orm_read(|db| {
            let op = operation::Entity::find_by_id(&receipt.operation_id)
                .one(db)?
                .ok_or(DatabaseError::Missing)?;
            let owned = instance::Entity::find_by_id(&receipt.instance_id)
                .one(db)?
                .ok_or(DatabaseError::Missing)?;
            if op.kind != "delete"
                || op.instance_id != owned.id
                || owned.scope_id != receipt.scope_id
            {
                return Err(DatabaseError::InvalidInput);
            }
            if op.status == "Succeeded" && owned.lifecycle == "retired" {
                return Ok(false);
            }
            if !(op.status == "Accepted" || (op.status == "Executing" && op.attempt > 1))
                || owned.lifecycle != "retiring"
                || op.expected_instance_revision.checked_add(1) != Some(owned.revision)
                || operation_step::Entity::find()
                    .filter(operation_step::Column::OperationId.eq(&op.id))
                    .filter(
                        operation_step::Column::Outcome
                            .is_null()
                            .or(operation_step::Column::Outcome.eq("unknown")),
                    )
                    .one(db)?
                    .is_some()
            {
                return Err(DatabaseError::InvalidInput);
            }
            Ok(true)
        })
        .map_err(map_error)
    }

    fn complete_delete(
        &self,
        receipt: &RequestReceipt,
        storage: &[StorageCheck],
    ) -> Result<(), StoreConflict> {
        if self
            .receipt(&receipt.scope_id, &receipt.request_id)?
            .as_ref()
            != Some(receipt)
        {
            return Err(StoreConflict::InvalidInput);
        }
        let receipt = receipt.clone();
        let storage = storage.to_vec();
        self.orm_write(move |db| {
            let tx = db.begin()?;
            let op = operation::Entity::find_by_id(&receipt.operation_id)
                .one(&tx)?
                .ok_or(DatabaseError::Missing)?;
            let owned = instance::Entity::find_by_id(&receipt.instance_id)
                .one(&tx)?
                .ok_or(DatabaseError::Missing)?;
            if op.kind != "delete"
                || op.status != "Executing"
                || op.instance_id != owned.id
                || owned.scope_id != receipt.scope_id
                || owned.lifecycle != "retiring"
                || op.expected_instance_revision.checked_add(1) != Some(owned.revision)
                || operation_step::Entity::find()
                    .filter(operation_step::Column::OperationId.eq(&op.id))
                    .filter(operation_step::Column::Outcome.is_null())
                    .one(&tx)?
                    .is_some()
            {
                return Err(DatabaseError::InvalidInput);
            }
            let allocations = storage_allocation::Entity::find()
                .filter(storage_allocation::Column::InstanceId.eq(&owned.id))
                .all(&tx)?;
            if allocations.len() != storage.len()
                || allocations.iter().any(|allocation| {
                    storage
                        .iter()
                        .filter(|check| check.slot == allocation.slot)
                        .count()
                        != 1
                })
            {
                return Err(DatabaseError::InvalidInput);
            }
            for check in storage {
                storage_allocation::Entity::update_many()
                    .col_expr(
                        storage_allocation::Column::Ownership,
                        Expr::value("retained"),
                    )
                    .col_expr(
                        storage_allocation::Column::Presence,
                        Expr::value(presence_name(check.presence)),
                    )
                    .col_expr(
                        storage_allocation::Column::ObservedAt,
                        Expr::cust("CURRENT_TIMESTAMP"),
                    )
                    .filter(storage_allocation::Column::InstanceId.eq(&owned.id))
                    .filter(storage_allocation::Column::Slot.eq(check.slot))
                    .exec(&tx)?;
            }
            artifact::Entity::update_many()
                .col_expr(artifact::Column::Placement, Expr::value("retained"))
                .filter(artifact::Column::InstanceId.eq(&owned.id))
                .filter(artifact::Column::Placement.eq("published"))
                .exec(&tx)?;
            port_reservation::Entity::update_many()
                .col_expr(port_reservation::Column::Status, Expr::value("released"))
                .filter(port_reservation::Column::InstanceId.eq(&owned.id))
                .exec(&tx)?;
            instance::Entity::update_many()
                .col_expr(instance::Column::Lifecycle, Expr::value("retired"))
                .col_expr(
                    instance::Column::Revision,
                    Expr::value(
                        owned
                            .revision
                            .checked_add(1)
                            .ok_or(DatabaseError::InvalidInput)?,
                    ),
                )
                .filter(instance::Column::Id.eq(&owned.id))
                .exec(&tx)?;
            operation::Entity::update_many()
                .col_expr(operation::Column::Status, Expr::value("Succeeded"))
                .col_expr(operation::Column::Phase, Expr::value("retired"))
                .col_expr(
                    operation::Column::CompletedAt,
                    Expr::cust("CURRENT_TIMESTAMP"),
                )
                .filter(operation::Column::Id.eq(op.id))
                .exec(&tx)?;
            runtime_observation::Entity::insert(runtime_observation::ActiveModel {
                instance_id: Set(owned.id.clone()),
                runtime_state: Set("absent".into()),
                freshness: Set("fresh".into()),
                ..Default::default()
            })
            .on_conflict(
                OnConflict::column(runtime_observation::Column::InstanceId)
                    .do_nothing()
                    .to_owned(),
            )
            .exec_without_returning(&tx)?;
            runtime_observation::Entity::update_many()
                .col_expr(
                    runtime_observation::Column::RuntimeState,
                    Expr::value("absent"),
                )
                .col_expr(
                    runtime_observation::Column::Health,
                    Expr::value(Option::<String>::None),
                )
                .col_expr(runtime_observation::Column::Freshness, Expr::value("fresh"))
                .col_expr(
                    runtime_observation::Column::OperationId,
                    Expr::value(Some(receipt.operation_id)),
                )
                .col_expr(
                    runtime_observation::Column::ObservedAt,
                    Expr::cust("CURRENT_TIMESTAMP"),
                )
                .filter(runtime_observation::Column::InstanceId.eq(&owned.id))
                .exec(&tx)?;
            tx.commit()?;
            Ok(())
        })
        .map_err(map_error)
    }
}
