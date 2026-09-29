//! Atomic state transitions for a stopped instance's proposed port bindings.

use std::collections::{BTreeMap, BTreeSet};

use composenest_application::{
    operation_journal::RequestReceipt,
    port_edit::{PortEditRequest, PortEditStore},
    state_store::{PortAllocation, StoreConflict},
};
use rusqlite::{OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

use crate::{
    sqlite::{DatabaseError, DatabaseWorker},
    state_store::map_error,
};

fn bindings(
    tx: &Transaction<'_>,
    instance: &str,
    revision: i64,
) -> Result<Vec<PortAllocation>, DatabaseError> {
    let mut statement = tx.prepare("SELECT slot, host_ip, host_port, container_port FROM port_bindings WHERE instance_id = ?1 AND spec_revision = ?2 ORDER BY slot")?;
    let rows = statement.query_map(params![instance, revision], |row| {
        Ok(PortAllocation {
            slot: row.get(0)?,
            host_ip: row.get(1)?,
            host_port: row.get(2)?,
            container_port: row.get(3)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn checked_ports(
    old: &[PortAllocation],
    proposed: &[PortAllocation],
) -> Result<BTreeMap<String, PortAllocation>, StoreConflict> {
    let old_slots: BTreeSet<_> = old.iter().map(|port| port.slot.as_str()).collect();
    let mut next = BTreeMap::new();
    let mut ports = BTreeSet::new();
    for port in proposed {
        let previous = old
            .iter()
            .find(|old| old.slot == port.slot)
            .ok_or(StoreConflict::InvalidInput)?;
        if port.host_ip != previous.host_ip
            || port.container_port != previous.container_port
            || port.host_port == 0
            || !ports.insert((port.host_ip.clone(), port.host_port))
            || next.insert(port.slot.clone(), port.clone()).is_some()
        {
            return Err(StoreConflict::InvalidInput);
        }
    }
    if next.len() != old_slots.len()
        || !old_slots.iter().all(|slot| next.contains_key(*slot))
        || old
            .iter()
            .all(|port| next[&port.slot].host_port == port.host_port)
    {
        return Err(StoreConflict::InvalidInput);
    }
    Ok(next)
}

fn hash_ports(ports: &BTreeMap<String, PortAllocation>) -> String {
    let mut hash = Sha256::new();
    for port in ports.values() {
        hash.update(port.slot.as_bytes());
        hash.update([0]);
        hash.update(port.host_ip.as_bytes());
        hash.update([0]);
        hash.update(port.host_port.to_be_bytes());
        hash.update(port.container_port.to_be_bytes());
    }
    format!("{:x}", hash.finalize())
}

impl PortEditStore for DatabaseWorker {
    fn begin_port_edit(&self, request: &PortEditRequest) -> Result<RequestReceipt, StoreConflict> {
        let receipt = &request.receipt;
        if request.expected_instance_revision == 0
            || request.old_spec_revision == 0
            || receipt.instance_id.is_empty()
            || receipt.operation_id.is_empty()
            || receipt.request_id.is_empty()
            || receipt.scope_id.is_empty()
            || receipt.plan_id.is_some()
            || receipt.confirmed_revision != request.expected_instance_revision
            || receipt.request_hash.len() != 64
            || !receipt
                .request_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(StoreConflict::InvalidInput);
        }
        let request = request.clone();
        self.write(move |db| {
            let tx = db.transaction()?;
            let receipt = &request.receipt;
            let prior: Option<(String, String, String)> = tx.query_row(
                "SELECT instance_id, operation_id, request_hash FROM request_receipts WHERE scope_id = ?1 AND request_id = ?2",
                params![receipt.scope_id, receipt.request_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional()?;
            if let Some(prior) = prior {
                return Ok(if prior == (receipt.instance_id.clone(), receipt.operation_id.clone(), receipt.request_hash.clone()) {
                    Ok(receipt.clone())
                } else { Err(StoreConflict::Duplicate) });
            }
            let current: Option<(String, i64, Option<i64>, String, String)> = tx.query_row(
                "SELECT scope_id, revision, applied_spec_revision, lifecycle, target_id FROM instances WHERE id = ?1",
                [&receipt.instance_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            ).optional()?;
            let Some((scope, revision, applied, lifecycle, _target)) = current else { return Ok(Err(StoreConflict::Missing)); };
            if scope != receipt.scope_id || lifecycle != "managed" { return Ok(Err(StoreConflict::InvalidLifecycle)); }
            if revision != request.expected_instance_revision as i64 { return Ok(Err(StoreConflict::StaleRevision)); }
            if applied != Some(request.old_spec_revision as i64) { return Ok(Err(StoreConflict::InvalidInput)); }
            let busy: i64 = tx.query_row("SELECT COUNT(*) FROM operations WHERE instance_id = ?1 AND status NOT IN ('Succeeded', 'Abandoned')", [&receipt.instance_id], |row| row.get(0))?;
            if busy != 0 { return Ok(Err(StoreConflict::UnresolvedOperation)); }
            let old = bindings(&tx, &receipt.instance_id, request.old_spec_revision as i64)?;
            let next = match checked_ports(&old, &request.ports) { Ok(next) => next, Err(error) => return Ok(Err(error)) };
            let new_revision = request.old_spec_revision.checked_add(1).ok_or(DatabaseError::InvalidInput)?;
            let old_spec: (String, String, String) = tx.query_row(
                "SELECT selected_version, storage_method, inputs_json FROM instance_specs WHERE instance_id = ?1 AND revision = ?2",
                params![receipt.instance_id, request.old_spec_revision],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            tx.execute("INSERT INTO instance_specs (instance_id, revision, selected_version, storage_method, inputs_json) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![receipt.instance_id, new_revision, old_spec.0, old_spec.1, old_spec.2])?;
            tx.execute("INSERT INTO operations (id, instance_id, kind, phase, expected_instance_revision, old_spec_revision, new_spec_revision) VALUES (?1, ?2, 'edit_port', 'accepted', ?3, ?4, ?5)",
                params![receipt.operation_id, receipt.instance_id, revision, request.old_spec_revision, new_revision])?;
            tx.execute("INSERT INTO pending_changes (operation_id, instance_id, old_spec_revision, new_spec_revision, confirmed_diff_hash) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![receipt.operation_id, receipt.instance_id, request.old_spec_revision, new_revision, hash_ports(&next)])?;
            for port in next.values() {
                tx.execute("INSERT INTO port_bindings (instance_id, spec_revision, slot, host_ip, host_port, container_port) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![receipt.instance_id, new_revision, port.slot, port.host_ip, port.host_port, port.container_port])?;
                if old.iter().any(|previous| previous.slot == port.slot && previous.host_port == port.host_port) { continue; }
                let reservation_id = format!("{}:{}:r{}", receipt.instance_id, port.slot, new_revision);
                tx.execute("INSERT INTO port_reservations (id, scope_id, instance_id, host_ip, protocol, host_port, status) VALUES (?1, ?2, ?3, ?4, 'tcp', ?5, 'held')",
                    params![reservation_id, receipt.scope_id, receipt.instance_id, port.host_ip, port.host_port])?;
                tx.execute("INSERT INTO pending_change_reservations (operation_id, reservation_id) VALUES (?1, ?2)", params![receipt.operation_id, reservation_id])?;
            }
            tx.execute("INSERT INTO image_resolutions (instance_id, spec_revision, image_ref, digest, image_id, platform, first_operation_id) SELECT instance_id, ?1, image_ref, digest, image_id, platform, first_operation_id FROM image_resolutions WHERE instance_id = ?2 AND spec_revision = ?3",
                params![new_revision, receipt.instance_id, request.old_spec_revision])?;
            tx.execute("INSERT INTO request_receipts (scope_id, request_id, confirmed_revision, request_hash, instance_id, operation_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![receipt.scope_id, receipt.request_id, revision, receipt.request_hash, receipt.instance_id, receipt.operation_id])?;
            tx.commit()?;
            Ok(Ok(receipt.clone()))
        }).map_err(map_error)?
    }

    fn pending_ports(&self, operation_id: &str) -> Result<Vec<PortAllocation>, StoreConflict> {
        self.read(|db| {
            let (instance, revision): (String, i64) = db.query_row(
                "SELECT instance_id, new_spec_revision FROM pending_changes WHERE operation_id = ?1", [operation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let mut statement = db.prepare("SELECT slot, host_ip, host_port, container_port FROM port_bindings WHERE instance_id = ?1 AND spec_revision = ?2 ORDER BY slot")?;
            let rows = statement.query_map(params![instance, revision], |row| Ok(PortAllocation {
                slot: row.get(0)?, host_ip: row.get(1)?, host_port: row.get(2)?, container_port: row.get(3)?,
            }))?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
        }).map_err(map_error)
    }

    fn complete_port_edit(
        &self,
        operation_id: &str,
        container_id: &str,
    ) -> Result<(), StoreConflict> {
        if container_id.len() != 64 || !container_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(StoreConflict::InvalidInput);
        }
        let operation_id = operation_id.to_owned();
        let container_id = container_id.to_owned();
        self.write(move |db| {
            let tx = db.transaction()?;
            let pending: (String, i64, i64, String, String, i64, Option<i64>) = tx.query_row(
                "SELECT p.instance_id, p.old_spec_revision, p.new_spec_revision, o.status, i.lifecycle, i.revision, i.applied_spec_revision FROM pending_changes p JOIN operations o ON o.id = p.operation_id JOIN instances i ON i.id = p.instance_id WHERE p.operation_id = ?1 AND o.kind = 'edit_port'",
                [&operation_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
            )?;
            let (instance, old, new, status, lifecycle, revision, applied) = pending;
            if status != "Executing" || lifecycle != "managed" || applied != Some(old) { return Err(DatabaseError::InvalidInput); }
            let artifact_id = format!("{instance}-r{new}");
            let artifact: Option<String> = tx.query_row("SELECT placement FROM artifacts WHERE id = ?1 AND instance_id = ?2 AND spec_revision = ?3", params![artifact_id, instance, new], |row| row.get(0)).optional()?;
            if artifact.as_deref() != Some("published") { return Err(DatabaseError::InvalidInput); }
            let old_ports = bindings(&tx, &instance, old)?;
            let new_ports = bindings(&tx, &instance, new)?;
            let next = checked_ports(&old_ports, &new_ports).map_err(|_| DatabaseError::InvalidInput)?;
            let expected_hash: String = tx.query_row("SELECT confirmed_diff_hash FROM pending_changes WHERE operation_id = ?1", [&operation_id], |row| row.get(0))?;
            if hash_ports(&next) != expected_hash { return Err(DatabaseError::InvalidInput); }
            for port in &old_ports {
                if next[&port.slot].host_port != port.host_port {
                    tx.execute("UPDATE port_reservations SET status = 'released' WHERE instance_id = ?1 AND host_ip = ?2 AND host_port = ?3 AND status = 'committed'", params![instance, port.host_ip, port.host_port])?;
                }
            }
            tx.execute("UPDATE port_reservations SET status = 'committed' WHERE id IN (SELECT reservation_id FROM pending_change_reservations WHERE operation_id = ?1) AND status = 'held'", [&operation_id])?;
            tx.execute("UPDATE instances SET revision = ?2, applied_spec_revision = ?3 WHERE id = ?1 AND revision = ?4", params![instance, revision + 1, new, revision])?;
            tx.execute("UPDATE runtime_observations SET operation_id = ?2, container_id = ?3, runtime_state = 'stopped', health = NULL, freshness = 'fresh', observed_at = CURRENT_TIMESTAMP WHERE instance_id = ?1", params![instance, operation_id, container_id])?;
            tx.execute("UPDATE operations SET status = 'Succeeded', phase = 'stopped', completed_at = CURRENT_TIMESTAMP WHERE id = ?1", [&operation_id])?;
            tx.commit()?;
            Ok(())
        }).map_err(map_error)
    }
}
