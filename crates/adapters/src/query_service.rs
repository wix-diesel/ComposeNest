//! SQLite projection of saved state into masked instance views.

use composenest_application::{
    query_service::{
        ConnectionView, InputView, InstanceDetailView, InstanceView, ObservationView,
        OperationView, PortView, QueryStore, StorageLocationView, StorageView,
    },
    state_store::StoreConflict,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::{
    sqlite::{DatabaseError, DatabaseWorker},
    state_store::map_error,
};

/// Reads detail metadata and masked settings in one scoped, read-only database snapshot.
pub fn view_instance_detail(
    database: &DatabaseWorker,
    scope: &str,
    id: &str,
) -> Result<InstanceDetailView, StoreConflict> {
    database.read(|db| {
        let clone_source_id = db.query_row(
            "SELECT clone_source_id FROM instances WHERE id=?1 AND scope_id=?2 AND lifecycle!='retired'",
            params![id, scope], |row| row.get::<_, Option<String>>(0),
        ).optional()?.ok_or(DatabaseError::Missing)?;
        let instance = read_instance(db, scope, id)?;
        let creation_started_at = db.query_row(
            "SELECT MIN(started_at) FROM operations WHERE instance_id=?1 AND kind IN ('create','clone')",
            [id], |row| row.get(0),
        )?;
        let mut query = db.prepare("SELECT slot, method, resource_identity FROM storage_allocations WHERE instance_id=?1 ORDER BY slot")?;
        let locations = query.query_map([id], |row| {
            let method: String = row.get(1)?;
            let identity: String = row.get(2)?;
            Ok(StorageLocationView {
                slot: row.get(0)?,
                location: if method == "bind" {
                    database.management_root().join(identity).to_string_lossy().into_owned()
                } else { identity },
            })
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok(InstanceDetailView {
            state: composenest_application::instance_actions::InstanceActionView::from(instance.clone()),
            instance, locations, creation_started_at, clone_source_id,
        })
    }).map_err(map_error)
}

impl QueryStore for DatabaseWorker {
    fn list_instances(&self, scope_id: &str) -> Result<Vec<InstanceView>, StoreConflict> {
        self.read(|db| {
            let mut query = db.prepare("SELECT id FROM instances WHERE scope_id=?1 AND lifecycle!='retired' ORDER BY display_name, id")?;
            let ids = query.query_map([scope_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids.iter().map(|id| read_instance(db, scope_id, id))
                .collect::<Result<Vec<_>, _>>()
        }).map_err(map_error)
    }

    fn get_instance(
        &self,
        scope_id: &str,
        id: &str,
    ) -> Result<Option<InstanceView>, StoreConflict> {
        self.read(|db| {
            let exists = db
                .query_row(
                    "SELECT 1 FROM instances WHERE scope_id=?1 AND id=?2 AND lifecycle!='retired'",
                    params![scope_id, id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            exists.then(|| read_instance(db, scope_id, id)).transpose()
        })
        .map_err(map_error)
    }
}

pub(crate) fn read_instance(
    db: &Connection,
    scope: &str,
    id: &str,
) -> Result<InstanceView, DatabaseError> {
    let (name, revision, lifecycle, project, applied): (String, i64, String, String, Option<i64>) = db.query_row(
        "SELECT display_name, revision, lifecycle, project_name, applied_spec_revision FROM instances WHERE id=?1 AND scope_id=?2",
        params![id, scope], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?;
    let (template_id, template_version, canonical): (String, String, String) = db.query_row(
        "SELECT template_id, template_version, canonical_json FROM template_snapshots WHERE instance_id=?1",
        [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    let (spec_revision, selected_version, storage_method, inputs_json): (i64, String, String, String) = db.query_row(
        "SELECT revision, selected_version, storage_method, inputs_json FROM instance_specs WHERE instance_id=?1 AND revision=COALESCE((SELECT MAX(new_spec_revision) FROM operations WHERE instance_id=?1 AND status='Succeeded'), 1)",
        [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
    let ports = {
        let mut query = db.prepare("SELECT slot, host_ip, host_port, container_port FROM port_bindings WHERE instance_id=?1 AND spec_revision=?2 ORDER BY slot")?;
        query
            .query_map(params![id, spec_revision], |row| {
                Ok(PortView {
                    slot: row.get(0)?,
                    host_ip: row.get(1)?,
                    host_port: row.get(2)?,
                    container_port: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let storage = {
        let mut query = db.prepare("SELECT slot, method, presence, initialization FROM storage_allocations WHERE instance_id=?1 ORDER BY slot")?;
        query
            .query_map([id], |row| {
                Ok(StorageView {
                    slot: row.get(0)?,
                    method: row.get(1)?,
                    presence: row.get(2)?,
                    initialization: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let observation = db.query_row(
        "SELECT runtime_state, health, observed_at, freshness FROM runtime_observations WHERE instance_id=?1",
        [id], |row| Ok(ObservationView { runtime_state: row.get(0)?, health: row.get(1)?, observed_at: row.get(2)?, freshness: row.get(3)? }),
    ).optional()?;
    let last_operation = db.query_row(
        "SELECT id, kind, status, started_at, phase FROM operations WHERE instance_id=?1 ORDER BY started_at DESC, rowid DESC LIMIT 1",
        [id], |row| Ok(OperationView { id: row.get(0)?, kind: row.get(1)?, status: row.get(2)?, started_at: row.get(3)?, phase: row.get(4)? }),
    ).optional()?;
    let snapshot: Value =
        serde_json::from_str(&canonical).map_err(|_| DatabaseError::InvalidInput)?;
    let saved_inputs: Value =
        serde_json::from_str(&inputs_json).map_err(|_| DatabaseError::InvalidInput)?;
    let definition = snapshot["versions"]
        .as_array()
        .and_then(|versions| {
            versions
                .iter()
                .find(|version| version["key"] == selected_version)
        })
        .map(|version| &version["definition"])
        .ok_or(DatabaseError::InvalidInput)?;
    let inputs = project_inputs(definition, &saved_inputs)?;
    let connections = project_connections(definition, &ports)?;
    let image = definition["image"].as_str().map(str::to_owned);
    let runtime_status = runtime_status(observation.as_ref());
    let needs_attention = lifecycle != "managed"
        || storage.iter().any(|item| item.presence == "missing")
        || matches!(runtime_status, "unknown" | "unhealthy")
        || last_operation
            .as_ref()
            .is_some_and(|item| !matches!(item.status.as_str(), "Succeeded" | "Abandoned"));
    Ok(InstanceView {
        id: id.into(),
        name,
        revision: positive(revision)?,
        lifecycle,
        project_name: project,
        template_id,
        template_version,
        selected_version,
        image,
        storage_method,
        spec_revision: positive(spec_revision)?,
        applied_spec_revision: applied.map(positive).transpose()?,
        ports,
        storage,
        inputs,
        connections,
        observation,
        runtime_status: runtime_status.into(),
        last_operation,
        needs_attention,
    })
}

fn positive(value: i64) -> Result<u64, DatabaseError> {
    u64::try_from(value).map_err(|_| DatabaseError::InvalidInput)
}

fn runtime_status(observation: Option<&ObservationView>) -> &'static str {
    let Some(item) = observation.filter(|item| item.freshness == "fresh") else {
        return "unknown";
    };
    match item.runtime_state.as_str() {
        "absent" => "absent",
        "stopped" => "stopped",
        "running" if item.health.as_deref() == Some("healthy") => "ready",
        "running" if item.health.as_deref() == Some("unhealthy") => "unhealthy",
        "running" | "created" => "preparing",
        _ => "unknown",
    }
}

fn entries(node: &Value) -> Option<&serde_json::Map<String, Value>> {
    node.get("values")
        .and_then(Value::as_object)
        .or_else(|| node.as_object())
}

fn project_inputs(definition: &Value, saved: &Value) -> Result<Vec<InputView>, DatabaseError> {
    let saved = saved.as_object().ok_or(DatabaseError::InvalidInput)?;
    if saved.is_empty() {
        return Ok(Vec::new());
    }
    let definitions = entries(&definition["inputs"]).ok_or(DatabaseError::InvalidInput)?;
    Ok(saved
        .iter()
        .map(|(slot, value)| {
            let input_type = definitions.get(slot).and_then(|item| item["type"].as_str());
            let secret = !matches!(
                input_type,
                Some("string" | "integer" | "boolean" | "select")
            );
            InputView {
                slot: slot.clone(),
                secret,
                value: (!secret).then(|| value.clone()),
            }
        })
        .collect())
}

fn project_connections(
    definition: &Value,
    ports: &[PortView],
) -> Result<Vec<ConnectionView>, DatabaseError> {
    let Some(connections) = definition.get("connections") else {
        return Ok(ports
            .iter()
            .map(|port| ConnectionView {
                slot: port.slot.clone(),
                label: port.slot.clone(),
                port: port.clone(),
                input_slots: Vec::new(),
            })
            .collect());
    };
    let connections = entries(connections).ok_or(DatabaseError::InvalidInput)?;
    connections
        .iter()
        .map(|(slot, item)| {
            let port_slot = item["port"].as_str().ok_or(DatabaseError::InvalidInput)?;
            let port = ports
                .iter()
                .find(|port| port.slot == port_slot)
                .ok_or(DatabaseError::InvalidInput)?;
            let input_slots = item["inputs"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            Ok(ConnectionView {
                slot: slot.clone(),
                label: item["label"].as_str().unwrap_or(slot).into(),
                port: port.clone(),
                input_slots,
            })
        })
        .collect()
}
