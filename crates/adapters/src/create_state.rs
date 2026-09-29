//! SQLite reads of confirmed execution inputs and creation completion commits.

use composenest_application::{
    create_state::{ConfirmedCreate, CreateStateStore},
    operation_journal::RequestReceipt,
    state_store::{PortAllocation, RuntimeTarget, StorageAllocation, StoreConflict, TemplateFile},
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};

use crate::{
    entities::{
        instance, instance_spec, operation, operation_step, port_binding, port_reservation,
        request_receipt, runtime_observation, runtime_target, storage_allocation,
        template_snapshot, template_snapshot_file,
    },
    sqlite::{DatabaseError, DatabaseWorker},
    state_store::{map_error, storage_entry},
};

fn create_operation(kind: &str) -> bool {
    matches!(kind, "create" | "clone")
}

impl CreateStateStore for DatabaseWorker {
    fn confirmed_create(&self, receipt: &RequestReceipt) -> Result<ConfirmedCreate, StoreConflict> {
        let receipt = receipt.clone();
        self.orm_read(move |db| {
            let saved = request_receipt::Entity::find_by_id((
                receipt.scope_id.clone(),
                receipt.request_id.clone(),
            ))
            .one(db)?
            .ok_or(DatabaseError::Missing)?;
            if saved.operation_id != receipt.operation_id
                || saved.instance_id != receipt.instance_id
                || saved.request_hash != receipt.request_hash
                || saved.confirmed_revision
                    != i64::try_from(receipt.confirmed_revision)
                        .map_err(|_| DatabaseError::InvalidInput)?
            {
                return Err(DatabaseError::InvalidInput);
            }
            let op = operation::Entity::find_by_id(&receipt.operation_id)
                .one(db)?
                .ok_or(DatabaseError::Missing)?;
            let owned = instance::Entity::find_by_id(&receipt.instance_id)
                .one(db)?
                .ok_or(DatabaseError::Missing)?;
            let lifecycle = matches!(op.kind.as_str(), "start" | "stop" | "restart");
            let edit = op.kind == "edit_port";
            if !create_operation(&op.kind) && !lifecycle && !edit
                || op.instance_id != owned.id
                || owned.scope_id != receipt.scope_id
                || owned.lifecycle != "managed"
                || op.status != "Accepted"
                || (lifecycle
                    && (op.old_spec_revision != owned.applied_spec_revision
                        || op.new_spec_revision.is_some()))
                || (edit
                    && (op.old_spec_revision != owned.applied_spec_revision
                        || op.new_spec_revision
                            != op.old_spec_revision.and_then(|old| old.checked_add(1))))
                || (create_operation(&op.kind) && op.new_spec_revision != Some(1))
                || op.expected_instance_revision != owned.revision
            {
                return Err(DatabaseError::InvalidInput);
            }
            let revision = if lifecycle {
                op.old_spec_revision
            } else {
                op.new_spec_revision
            }
            .ok_or(DatabaseError::InvalidInput)?;
            let spec = instance_spec::Entity::find_by_id((owned.id.clone(), revision))
                .one(db)?
                .ok_or(DatabaseError::Missing)?;
            let snapshot = template_snapshot::Entity::find()
                .filter(template_snapshot::Column::InstanceId.eq(&owned.id))
                .one(db)?
                .ok_or(DatabaseError::Missing)?;
            if snapshot.selected_version != spec.selected_version {
                return Err(DatabaseError::InvalidInput);
            }
            let target = runtime_target::Entity::find_by_id(&owned.target_id)
                .one(db)?
                .ok_or(DatabaseError::Missing)?;
            if target.scope_id != owned.scope_id {
                return Err(DatabaseError::InvalidInput);
            }
            let files = template_snapshot_file::Entity::find()
                .filter(template_snapshot_file::Column::SnapshotId.eq(&snapshot.id))
                .order_by_asc(template_snapshot_file::Column::RelativePath)
                .all(db)?
                .into_iter()
                .map(|file| TemplateFile {
                    relative_path: file.relative_path,
                    contents: file.contents,
                })
                .collect::<Vec<_>>();
            if files.is_empty() {
                return Err(DatabaseError::Missing);
            }
            let ports = port_binding::Entity::find()
                .filter(port_binding::Column::InstanceId.eq(&owned.id))
                .filter(port_binding::Column::SpecRevision.eq(revision))
                .order_by_asc(port_binding::Column::Slot)
                .all(db)?
                .into_iter()
                .map(|port| {
                    Ok(PortAllocation {
                        slot: port.slot,
                        host_ip: port.host_ip,
                        host_port: u16::try_from(port.host_port)
                            .map_err(|_| DatabaseError::InvalidInput)?,
                        container_port: u16::try_from(port.container_port)
                            .map_err(|_| DatabaseError::InvalidInput)?,
                    })
                })
                .collect::<Result<Vec<_>, DatabaseError>>()?;
            let reservations = port_reservation::Entity::find()
                .filter(port_reservation::Column::InstanceId.eq(&owned.id))
                .filter(port_reservation::Column::Status.is_in(if edit {
                    vec!["committed", "held"]
                } else {
                    vec!["committed"]
                }))
                .all(db)?;
            if (!edit && ports.len() != reservations.len())
                || ports.iter().any(|port| {
                    !reservations.iter().any(|reserved| {
                        reserved.scope_id == owned.scope_id
                            && reserved.host_ip == port.host_ip
                            && reserved.host_port == i64::from(port.host_port)
                            && reserved.protocol == "tcp"
                    })
                })
            {
                return Err(DatabaseError::InvalidInput);
            }
            let storage = storage_allocation::Entity::find()
                .filter(storage_allocation::Column::InstanceId.eq(&owned.id))
                .order_by_asc(storage_allocation::Column::Slot)
                .all(db)?
                .into_iter()
                .map(|entry| storage_entry(entry, owned.scope_id.clone()))
                .collect::<Result<Vec<_>, _>>()?;
            if storage.iter().any(|entry| match entry.method {
                composenest_application::state_store::StorageMethod::Bind => {
                    spec.storage_method != "bind"
                }
                composenest_application::state_store::StorageMethod::Volume => {
                    spec.storage_method != "volume"
                }
            }) {
                return Err(DatabaseError::InvalidInput);
            }
            Ok(ConfirmedCreate {
                instance_id: owned.id,
                scope_id: owned.scope_id,
                project_name: owned.project_name,
                target: RuntimeTarget {
                    id: target.id,
                    scope_id: target.scope_id,
                    endpoint: target.endpoint,
                    engine_id: target.engine_id,
                    platform: target.platform,
                },
                spec_revision: u64::try_from(revision).map_err(|_| DatabaseError::InvalidInput)?,
                selected_version: spec.selected_version,
                snapshot_files: files,
                inputs_json: spec.inputs_json,
                ports,
                storage,
            })
        })
        .map_err(map_error)
    }

    fn record_bind_materialization(
        &self,
        operation_id: &str,
        allocation: &StorageAllocation,
    ) -> Result<(), StoreConflict> {
        if allocation.ownership_evidence.len() != 64
            || !allocation
                .ownership_evidence
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(StoreConflict::InvalidInput);
        }
        let (operation_id, allocation) = (operation_id.to_owned(), allocation.clone());
        self.orm_write(move |db| {
            let tx = db.begin()?;
            let op = operation::Entity::find_by_id(&operation_id)
                .one(&tx)?
                .ok_or(DatabaseError::Missing)?;
            if !create_operation(&op.kind) || op.status != "Executing" {
                return Err(DatabaseError::InvalidInput);
            }
            let saved = storage_allocation::Entity::find_by_id((
                op.instance_id.clone(),
                allocation.slot.clone(),
            ))
            .one(&tx)?
            .ok_or(DatabaseError::Missing)?;
            if saved.method != "bind"
                || saved.presence != "not_materialized"
                || saved.resource_identity != allocation.resource_identity
            {
                return Err(DatabaseError::InvalidInput);
            }
            storage_allocation::ActiveModel {
                instance_id: Set(op.instance_id),
                slot: Set(allocation.slot),
                ownership_evidence: Set(allocation.ownership_evidence),
                presence: Set("present".into()),
                ..Default::default()
            }
            .update(&tx)?;
            tx.commit()?;
            Ok(())
        })
        .map_err(map_error)
    }

    fn mark_may_have_initialized(&self, operation_id: &str) -> Result<(), StoreConflict> {
        let operation_id = operation_id.to_owned();
        self.orm_write(move |db| {
            let tx = db.begin()?;
            let op = operation::Entity::find_by_id(&operation_id)
                .one(&tx)?
                .ok_or(DatabaseError::Missing)?;
            if !create_operation(&op.kind) || op.status != "Executing" {
                return Err(DatabaseError::InvalidInput);
            }
            let last = operation_step::Entity::find()
                .filter(operation_step::Column::OperationId.eq(&operation_id))
                .order_by_desc(operation_step::Column::Sequence)
                .one(&tx)?
                .ok_or(DatabaseError::Missing)?;
            if last.command_kind != "observe"
                || last.expected_result != "state_observed"
                || last.outcome.as_deref() != Some("succeeded")
                || last.attempt != op.attempt
            {
                return Err(DatabaseError::InvalidInput);
            }
            let created = operation_step::Entity::find()
                .filter(operation_step::Column::OperationId.eq(&operation_id))
                .filter(operation_step::Column::Attempt.eq(op.attempt))
                .filter(operation_step::Column::CommandKind.eq("compose_create"))
                .filter(operation_step::Column::Outcome.eq("succeeded"))
                .order_by_desc(operation_step::Column::Sequence)
                .one(&tx)?;
            if !created.is_some_and(|step| step.sequence < last.sequence) {
                return Err(DatabaseError::InvalidInput);
            }
            let entries = storage_allocation::Entity::find()
                .filter(storage_allocation::Column::InstanceId.eq(&op.instance_id))
                .all(&tx)?;
            if entries.iter().any(|entry| {
                entry.presence != "present"
                    || !matches!(
                        entry.initialization.as_str(),
                        "not_attempted" | "may_have_initialized"
                    )
            }) {
                return Err(DatabaseError::InvalidInput);
            }
            for entry in entries {
                storage_allocation::ActiveModel {
                    instance_id: Set(entry.instance_id),
                    slot: Set(entry.slot),
                    initialization: Set("may_have_initialized".into()),
                    ..Default::default()
                }
                .update(&tx)?;
            }
            tx.commit()?;
            Ok(())
        })
        .map_err(map_error)
    }

    fn complete_ready(&self, operation_id: &str, container_id: &str) -> Result<(), StoreConflict> {
        if container_id.len() != 64 || !container_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(StoreConflict::InvalidInput);
        }
        let (operation_id, container_id) = (operation_id.to_owned(), container_id.to_owned());
        self.orm_write(move |db| {
            let tx = db.begin()?;
            let op = operation::Entity::find_by_id(&operation_id)
                .one(&tx)?
                .ok_or(DatabaseError::Missing)?;
            if !create_operation(&op.kind)
                || op.status != "Executing"
                || op.new_spec_revision != Some(1)
            {
                return Err(DatabaseError::InvalidInput);
            }
            let last = operation_step::Entity::find()
                .filter(operation_step::Column::OperationId.eq(&operation_id))
                .order_by_desc(operation_step::Column::Sequence)
                .one(&tx)?
                .ok_or(DatabaseError::Missing)?;
            if last.command_kind != "observe"
                || last.expected_result != "state_observed"
                || last.outcome.as_deref() != Some("succeeded")
                || last.attempt != op.attempt
                || last.resource_id != container_id
            {
                return Err(DatabaseError::InvalidInput);
            }
            let observed_at = last.observed_at.ok_or(DatabaseError::InvalidInput)?;
            let started = operation_step::Entity::find()
                .filter(operation_step::Column::OperationId.eq(&operation_id))
                .filter(operation_step::Column::Attempt.eq(op.attempt))
                .filter(operation_step::Column::CommandKind.eq("compose_start"))
                .filter(operation_step::Column::Outcome.eq("succeeded"))
                .order_by_desc(operation_step::Column::Sequence)
                .one(&tx)?;
            if !started.is_some_and(|step| step.sequence < last.sequence) {
                return Err(DatabaseError::InvalidInput);
            }
            let entries = storage_allocation::Entity::find()
                .filter(storage_allocation::Column::InstanceId.eq(&op.instance_id))
                .all(&tx)?;
            if entries.iter().any(|entry| {
                entry.presence != "present" || entry.initialization != "may_have_initialized"
            }) {
                return Err(DatabaseError::InvalidInput);
            }
            for entry in entries {
                storage_allocation::ActiveModel {
                    instance_id: Set(entry.instance_id),
                    slot: Set(entry.slot),
                    initialization: Set("ready_observed".into()),
                    ..Default::default()
                }
                .update(&tx)?;
            }
            let observation = runtime_observation::ActiveModel {
                instance_id: Set(op.instance_id.clone()),
                operation_id: Set(Some(operation_id.clone())),
                container_id: Set(Some(container_id)),
                runtime_state: Set("running".into()),
                health: Set(Some("healthy".into())),
                freshness: Set("fresh".into()),
                observed_at: Set(observed_at.clone()),
            };
            if runtime_observation::Entity::find_by_id(&op.instance_id)
                .one(&tx)?
                .is_some()
            {
                observation.update(&tx)?;
            } else {
                observation.insert(&tx)?;
            }
            instance::ActiveModel {
                id: Set(op.instance_id.clone()),
                applied_spec_revision: Set(op.new_spec_revision),
                ..Default::default()
            }
            .update(&tx)?;
            operation::ActiveModel {
                id: Set(operation_id),
                status: Set("Succeeded".into()),
                phase: Set("ready".into()),
                completed_at: Set(Some(observed_at)),
                ..Default::default()
            }
            .update(&tx)?;
            tx.commit()?;
            Ok(())
        })
        .map_err(map_error)
    }
}
