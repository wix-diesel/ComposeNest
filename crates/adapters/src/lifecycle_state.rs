//! SQLite boundaries for lifecycle operations over an existing instance.

use composenest_application::{
    create_state::CreateStateStore,
    lifecycle_operation::{LifecycleSnapshot, LifecycleState},
    operation_journal::{OperationKind, RequestReceipt},
    state_store::StoreConflict,
};
use composenest_domain::instance::RuntimeStatus;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, TransactionTrait, sea_query::Expr};

use crate::{
    entities::{instance, operation, runtime_observation},
    sqlite::{DatabaseError, DatabaseWorker},
    state_store::map_error,
};

fn kind_name(kind: OperationKind) -> Option<&'static str> {
    match kind {
        OperationKind::Start => Some("start"),
        OperationKind::Stop => Some("stop"),
        OperationKind::Restart => Some("restart"),
        _ => None,
    }
}

impl LifecycleState for DatabaseWorker {
    fn snapshot(
        &self,
        receipt: &RequestReceipt,
        kind: OperationKind,
    ) -> Result<LifecycleSnapshot, StoreConflict> {
        let confirmed = self.confirmed_create(receipt)?;
        let expected_kind = kind_name(kind).ok_or(StoreConflict::InvalidInput)?;
        let receipt = receipt.clone();
        self.orm_read(move |db| {
            let op = operation::Entity::find_by_id(&receipt.operation_id)
                .one(db)?
                .ok_or(DatabaseError::Missing)?;
            let observation = runtime_observation::Entity::find_by_id(&receipt.instance_id)
                .one(db)?
                .ok_or(DatabaseError::Missing)?;
            if op.kind != expected_kind
                || op.instance_id != receipt.instance_id
                || op.old_spec_revision
                    != Some(
                        i64::try_from(confirmed.spec_revision)
                            .map_err(|_| DatabaseError::InvalidInput)?,
                    )
                || observation.container_id.as_deref().is_none_or(|id| {
                    id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            {
                return Err(DatabaseError::InvalidInput);
            }
            Ok(LifecycleSnapshot {
                container_id: observation.container_id.ok_or(DatabaseError::Missing)?,
                spec_revision: confirmed.spec_revision,
            })
        })
        .map_err(map_error)
    }

    fn complete(
        &self,
        operation_id: &str,
        container_id: &str,
        status: RuntimeStatus,
    ) -> Result<(), StoreConflict> {
        if container_id.len() != 64 || !container_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(StoreConflict::InvalidInput);
        }
        let (runtime_state, health) = match status {
            RuntimeStatus::Ready => ("running", Some("healthy")),
            RuntimeStatus::Stopped => ("stopped", None),
            RuntimeStatus::Absent => ("absent", None),
            _ => return Err(StoreConflict::InvalidInput),
        };
        let operation_id = operation_id.to_owned();
        let container_id = container_id.to_owned();
        self.orm_write(move |db| {
            let tx = db.begin()?;
            let op = operation::Entity::find_by_id(&operation_id)
                .one(&tx)?
                .ok_or(DatabaseError::Missing)?;
            let owned = instance::Entity::find_by_id(&op.instance_id)
                .one(&tx)?
                .ok_or(DatabaseError::Missing)?;
            if op.status != "Executing"
                || owned.lifecycle != "managed"
                || !matches!(op.kind.as_str(), "start" | "stop" | "restart")
                || op.old_spec_revision != owned.applied_spec_revision
            {
                return Err(DatabaseError::InvalidInput);
            }
            let changed = runtime_observation::Entity::update_many()
                .col_expr(
                    runtime_observation::Column::OperationId,
                    Expr::value(Some(operation_id.clone())),
                )
                .col_expr(
                    runtime_observation::Column::ContainerId,
                    Expr::value(Some(container_id)),
                )
                .col_expr(
                    runtime_observation::Column::RuntimeState,
                    Expr::value(runtime_state),
                )
                .col_expr(runtime_observation::Column::Health, Expr::value(health))
                .col_expr(runtime_observation::Column::Freshness, Expr::value("fresh"))
                .col_expr(
                    runtime_observation::Column::ObservedAt,
                    Expr::cust("CURRENT_TIMESTAMP"),
                )
                .filter(runtime_observation::Column::InstanceId.eq(&op.instance_id))
                .exec(&tx)?;
            if changed.rows_affected != 1 {
                return Err(DatabaseError::Missing);
            }
            operation::Entity::update_many()
                .col_expr(operation::Column::Status, Expr::value("Succeeded"))
                .col_expr(operation::Column::Phase, Expr::value(runtime_state))
                .col_expr(
                    operation::Column::CompletedAt,
                    Expr::cust("CURRENT_TIMESTAMP"),
                )
                .filter(operation::Column::Id.eq(operation_id))
                .exec(&tx)?;
            tx.commit()?;
            Ok(())
        })
        .map_err(map_error)
    }
}
