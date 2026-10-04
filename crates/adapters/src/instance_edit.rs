//! Scoped saved edit projections and idempotent connection to the stopped-port runner.
use crate::{query_service::read_instance, sqlite::DatabaseWorker, state_store::map_error};
use composenest_application::{
    instance_actions::InstanceActionView,
    instance_edit::{EditInstancePortsRequest, EditPortView, EditSettingView, InstanceEditView},
    operation_journal::{OperationJournal, RequestReceipt},
    operation_runner::OperationRunner,
    port_edit::PortEditRequest,
    state_store::{PortAllocation, StoreConflict},
};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Reads settings and reservations in one database snapshot, without exposing secrets or paths.
pub fn view_instance_edit(
    database: &DatabaseWorker,
    scope: &str,
    id: &str,
) -> Result<InstanceEditView, StoreConflict> {
    database.read(|db| {
        db.query_row("SELECT 1 FROM instances WHERE id=?1 AND scope_id=?2 AND lifecycle!='retired'", params![id, scope], |_| Ok(()))
            .optional()?.ok_or(crate::sqlite::DatabaseError::Missing)?;
        let current = read_instance(db, scope, id)?;
        let edit: Option<(u64, u64)> = db.query_row(
            "SELECT old_spec_revision, new_spec_revision FROM operations WHERE instance_id=?1 AND kind='edit_port' ORDER BY started_at DESC, rowid DESC LIMIT 1",
            [id], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        let mut ports = vec![];
        for port in &current.ports {
            let binding = |revision| db.query_row(
                "SELECT host_port FROM port_bindings WHERE instance_id=?1 AND spec_revision=?2 AND slot=?3",
                params![id, revision, port.slot], |row| row.get::<_, u16>(0),
            );
            let reservation = |host_port| db.query_row(
                "SELECT status FROM port_reservations WHERE instance_id=?1 AND host_ip=?2 AND host_port=?3 AND protocol='tcp' ORDER BY rowid DESC LIMIT 1",
                params![id, port.host_ip, host_port], |row| row.get::<_, String>(0),
            ).optional().map(|status| status.unwrap_or_else(|| "unknown".into()));
            let old_port = edit.map_or(Ok(port.host_port), |(old, _)| binding(old))?;
            let candidate_port = edit.map(|(_, new)| binding(new)).transpose()?;
            ports.push(EditPortView {
                slot: port.slot.clone(), host_ip: port.host_ip.clone(), container_port: port.container_port,
                old_port, committed_port: port.host_port, candidate_port,
                old_reservation: reservation(old_port)?,
                candidate_reservation: candidate_port.map(reservation).transpose()?,
            });
        }
        let state = InstanceActionView::from(current.clone());
        let can_edit_ports = state.actions.iter().any(|action| action == "rename")
            && matches!(state.runtime_status.as_str(), "stopped" | "absent")
            && current.applied_spec_revision == Some(current.spec_revision)
            && !ports.is_empty();
        Ok(InstanceEditView {
            state, template_id: current.template_id, selected_version: current.selected_version,
            storage_method: current.storage_method, spec_revision: current.spec_revision,
            applied_spec_revision: current.applied_spec_revision, ports, can_edit_ports,
            inputs: current.inputs.into_iter().map(|input| EditSettingView {
                slot: input.slot, secret: input.secret, value: input.value,
            }).collect(),
        })
    }).map_err(map_error)
}

fn request_hash(request: &EditInstancePortsRequest) -> Result<String, StoreConflict> {
    let bytes = serde_json::to_vec(&(
        "edit_port",
        &request.instance_id,
        request.expected_revision,
        request.expected_spec_revision,
        &request.ports,
    ))
    .map_err(|_| StoreConflict::InvalidInput)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn prior(
    database: &DatabaseWorker,
    scope: &str,
    request: &EditInstancePortsRequest,
) -> Result<bool, StoreConflict> {
    let Some(receipt) = database.receipt(scope, &request.context.request_id)? else {
        return Ok(false);
    };
    if receipt.instance_id != request.instance_id
        || receipt.confirmed_revision != request.expected_revision
        || receipt.plan_id.is_some()
        || receipt.request_hash != request_hash(request)?
    {
        return Err(StoreConflict::Duplicate);
    }
    Ok(true)
}

fn candidate(
    view: &InstanceEditView,
    scope: &str,
    request: &EditInstancePortsRequest,
) -> Result<PortEditRequest, StoreConflict> {
    if view.state.revision != request.expected_revision
        || view.spec_revision != request.expected_spec_revision
    {
        return Err(StoreConflict::StaleRevision);
    }
    if !view.can_edit_ports {
        return Err(StoreConflict::UnresolvedOperation);
    }
    if request.ports.len() != view.ports.len() {
        return Err(StoreConflict::InvalidInput);
    }
    let mut ports = vec![];
    for port in &view.ports {
        let host_port = *request
            .ports
            .get(&port.slot)
            .ok_or(StoreConflict::InvalidInput)?;
        if host_port < 1024
            || ports
                .iter()
                .any(|other: &PortAllocation| other.host_port == host_port)
        {
            return Err(StoreConflict::InvalidInput);
        }
        ports.push(PortAllocation {
            slot: port.slot.clone(),
            host_ip: port.host_ip.clone(),
            host_port,
            container_port: port.container_port,
        });
    }
    if ports
        .iter()
        .zip(&view.ports)
        .all(|(new, old)| new.host_port == old.committed_port)
    {
        return Err(StoreConflict::InvalidInput);
    }
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| StoreConflict::Backend)?;
    Ok(PortEditRequest {
        receipt: RequestReceipt {
            scope_id: scope.into(),
            request_id: request.context.request_id.clone(),
            plan_id: None,
            confirmed_revision: request.expected_revision,
            request_hash: request_hash(request)?,
            instance_id: request.instance_id.clone(),
            operation_id: format!("{:032x}", u128::from_be_bytes(bytes)),
        },
        expected_instance_revision: request.expected_revision,
        old_spec_revision: request.expected_spec_revision,
        ports,
    })
}

/// Looks up prior acceptance before taking capacity, then rechecks under the instance gate.
/// Newly accepted work inspects Docker immediately and preserves both reservations on failure.
pub async fn apply_instance_ports(
    database: &DatabaseWorker,
    probe: Option<&crate::docker_target::DockerProbe>,
    management_root: &Path,
    runner: &OperationRunner,
    scope: &str,
    request: &EditInstancePortsRequest,
) -> Result<InstanceEditView, crate::port_edit_stages::PortEditError> {
    use crate::port_edit_stages::{PortEditError, run_locked};
    if prior(database, scope, request).map_err(PortEditError::Store)? {
        return view_instance_edit(database, scope, &request.instance_id)
            .map_err(PortEditError::Store);
    }
    runner.run_exclusive(&request.instance_id, || async {
        if !prior(database, scope, request).map_err(PortEditError::Store)? {
            let view = view_instance_edit(database, scope, &request.instance_id).map_err(PortEditError::Store)?;
            let candidate = candidate(&view, scope, request).map_err(PortEditError::Store)?;
            let probe = probe.ok_or(PortEditError::Rejected)?;
            if let Err(error) = run_locked(database, probe, management_root, &candidate).await {
                // Acceptance is durable even when an external stage failed. Return its actual state.
                if !prior(database, scope, request).map_err(PortEditError::Store)? { return Err(error); }
                database.write(move |db| {
                    db.execute("UPDATE operations SET status='OutcomeUnknown', phase='reconcile' WHERE id=?1 AND status IN ('Accepted','Executing')", [&candidate.receipt.operation_id])?;
                    Ok(())
                }).map_err(|_| PortEditError::OutcomeUnknown)?;
            }
        }
        view_instance_edit(database, scope, &request.instance_id).map_err(PortEditError::Store)
    }).await.map_err(PortEditError::Runner)?
}
