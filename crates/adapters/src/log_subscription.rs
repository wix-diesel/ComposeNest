//! Owned instance resolution and bounded, expiring log sessions outside OperationRunner.
use crate::{
    create_projection::CreateProjection,
    docker_cli::logs::LogSubscription,
    docker_observation::Ownership,
    docker_target::DockerProbe,
    query_service::read_instance,
    sqlite::{DatabaseError, DatabaseWorker},
    state_store::map_error,
    storage::BindStorage,
};
use composenest_application::{
    create_state::ConfirmedCreate,
    image_resolution::ImageResolutionStore,
    log_subscription::{LogsView, SubscribeLogsRequest},
    state_store::{PortAllocation, StateStore, StoreConflict, TemplateFile},
};
use composenest_domain::instance::RuntimeStatus;
use rusqlite::{OptionalExtension, params};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::{Mutex as AsyncMutex, watch};

const LEASE: Duration = Duration::from_secs(30);
const PENDING_LEASE: Duration = Duration::from_secs(90);
const CAPACITY: usize = 2;
struct Entry {
    owner: String,
    instance: String,
    revision: u64,
    container: Option<String>,
    touched: Instant,
    stream: Option<Arc<AsyncMutex<LogSubscription>>>,
    closing: Arc<AtomicBool>,
    started: watch::Receiver<bool>,
}
struct StartCompletion(watch::Sender<bool>);
impl Drop for StartCompletion {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}
#[derive(Default)]
struct Sessions {
    entries: HashMap<String, Entry>,
    shutting_down: bool,
}
/// Owns at most two reads, retaining starts and failed cleanup until termination is confirmed.
#[derive(Default)]
pub struct LogSessions(Mutex<Sessions>);
impl LogSessions {
    fn lock(&self) -> std::sync::MutexGuard<'_, Sessions> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
    /// Expires lost sessions; pending starts and unconfirmed cleanup remain tracked.
    pub fn expire(&self) {
        self.lock().entries.retain(|_, e| {
            if e.touched.elapsed()
                >= if e.stream.is_some() {
                    LEASE
                } else {
                    PENDING_LEASE
                }
            {
                e.closing.store(true, Ordering::Release);
            }
            if !e.closing.load(Ordering::Acquire) {
                return true;
            }
            if let Some(stream) = &e.stream {
                if let Ok(stream) = stream.try_lock() {
                    stream.cancel();
                    return !stream.termination_confirmed();
                }
                true
            } else {
                !*e.started.borrow()
            }
        });
    }
    /// Cancels a window's reads, retaining pending starts for shutdown confirmation.
    pub fn close_owner(&self, owner: Option<&str>) {
        for e in self.lock().entries.values() {
            if owner.is_some_and(|o| e.owner != o) {
                continue;
            }
            e.closing.store(true, Ordering::Release);
            if let Some(stream) = &e.stream
                && let Ok(stream) = stream.try_lock()
            {
                stream.cancel();
            }
        }
    }
    /// Rejects new starts and waits up to 90 seconds per pending validation, then reaps its CLI.
    pub async fn shutdown(&self) -> Result<(), StoreConflict> {
        let entries = {
            let mut sessions = self.lock();
            sessions.shutting_down = true;
            for e in sessions.entries.values() {
                e.closing.store(true, Ordering::Release);
            }
            sessions
                .entries
                .iter()
                .map(|(id, e)| (id.clone(), e.owner.clone()))
                .collect::<Vec<_>>()
        };
        self.close_owner(None);
        let mut failed = false;
        for (id, owner) in entries {
            failed |= self.unsubscribe(&owner, &id).await.is_err();
        }
        if failed {
            Err(StoreConflict::Backend)
        } else {
            Ok(())
        }
    }
    /// Keeps a failed release addressable; retrying it cannot report an absent-entry success.
    pub async fn unsubscribe(&self, owner: &str, id: &str) -> Result<(), StoreConflict> {
        let (closing, mut started) = {
            let sessions = self.lock();
            let Some(entry) = sessions.entries.get(id) else {
                return Ok(());
            };
            if entry.owner != owner {
                return Err(StoreConflict::Missing);
            }
            entry.closing.store(true, Ordering::Release);
            (Arc::clone(&entry.closing), entry.started.clone())
        };
        tokio::time::timeout(PENDING_LEASE, async {
            while !*started.borrow_and_update() {
                if started.changed().await.is_err() {
                    break;
                }
            }
        })
        .await
        .map_err(|_| StoreConflict::Backend)?;
        let stream = self
            .lock()
            .entries
            .get(id)
            .filter(|e| Arc::ptr_eq(&e.closing, &closing))
            .and_then(|e| e.stream.clone());
        if let Some(stream) = stream {
            stream
                .lock()
                .await
                .close()
                .await
                .map_err(|_| StoreConflict::Backend)?;
        }
        let mut sessions = self.lock();
        if sessions
            .entries
            .get(id)
            .is_some_and(|e| Arc::ptr_eq(&e.closing, &closing))
        {
            sessions.entries.remove(id);
        }
        Ok(())
    }
    /// Starts an immutable container; cancelled validation cannot create an untracked CLI.
    pub async fn subscribe(
        &self,
        database: &DatabaseWorker,
        probe: &DockerProbe,
        scope: &str,
        owner: &str,
        request: &SubscribeLogsRequest,
    ) -> Result<LogsView, StoreConflict> {
        self.expire();
        let id = &request.context.request_id;
        let closing = Arc::new(AtomicBool::new(false));
        let (done, started) = watch::channel(false);
        let _completion = StartCompletion(done);
        {
            let mut sessions = self.lock();
            if sessions.shutting_down
                || sessions.entries.len() >= CAPACITY
                || sessions.entries.contains_key(id)
                || sessions
                    .entries
                    .values()
                    .any(|e| e.closing.load(Ordering::Acquire))
            {
                return Err(StoreConflict::UnresolvedOperation);
            }
            sessions.entries.insert(
                id.clone(),
                Entry {
                    owner: owner.into(),
                    instance: request.instance_id.clone(),
                    revision: request.expected_spec_revision,
                    container: None,
                    touched: Instant::now(),
                    stream: None,
                    closing: Arc::clone(&closing),
                    started,
                },
            );
        }
        let result = start(database, probe, scope, request, &closing).await;
        let mut sessions = self.lock();
        let (container, stream) = match result {
            Ok(result) => result,
            Err(error) => {
                // Cancelled entries stay reserved until their completion signal is observed.
                if !closing.load(Ordering::Acquire) {
                    sessions.entries.remove(id);
                }
                return Err(error);
            }
        };
        // Closing entries remain in the map until their pending validation completes.
        let entry = sessions.entries.get_mut(id).ok_or(StoreConflict::Missing)?;
        entry.container = Some(container);
        entry.stream = Some(Arc::new(AsyncMutex::new(stream)));
        entry.touched = Instant::now();
        if closing.load(Ordering::Acquire) {
            if let Some(stream) = &entry.stream
                && let Ok(stream) = stream.try_lock()
            {
                stream.cancel();
            }
            return Err(StoreConflict::Missing);
        }
        snapshot(id, entry)
    }
    /// Rechecks saved identity before every pull and refreshes only a valid session's lease.
    pub fn get(
        &self,
        database: &DatabaseWorker,
        scope: &str,
        owner: &str,
        id: &str,
    ) -> Result<LogsView, StoreConflict> {
        self.expire();
        let mut sessions = self.lock();
        let entry = sessions
            .entries
            .get_mut(id)
            .filter(|e| e.owner == owner && !e.closing.load(Ordering::Acquire))
            .ok_or(StoreConflict::Missing)?;
        let identity = current(database, scope, &entry.instance, entry.revision);
        if identity.as_ref().is_err()
            || identity.as_ref().ok().map(|(_, c)| c) != entry.container.as_ref()
        {
            entry.closing.store(true, Ordering::Release);
            if let Some(stream) = &entry.stream
                && let Ok(stream) = stream.try_lock()
            {
                stream.cancel();
            }
            return Err(StoreConflict::StaleRevision);
        }
        entry.touched = Instant::now();
        snapshot(id, entry)
    }
}
fn snapshot(id: &str, entry: &Entry) -> Result<LogsView, StoreConflict> {
    let stream = entry
        .stream
        .as_ref()
        .ok_or(StoreConflict::UnresolvedOperation)?;
    let stream = stream
        .try_lock()
        .map_err(|_| StoreConflict::UnresolvedOperation)?;
    Ok(stream.snapshot(|b| LogsView {
        subscription_id: id.into(),
        instance_id: entry.instance.clone(),
        spec_revision: entry.revision,
        lines: b.lines(),
        dropped_lines: b.dropped_lines,
        truncated_lines: b.truncated_lines,
        finished: b.finished,
        failed: b.failed,
    }))
}
fn current(
    database: &DatabaseWorker,
    scope: &str,
    instance: &str,
    revision: u64,
) -> Result<(u64, String), StoreConflict> {
    database.read(|db| {
        let result = db.query_row("SELECT i.revision, r.container_id FROM instances i JOIN runtime_observations r ON r.instance_id=i.id WHERE i.id=?1 AND i.scope_id=?2 AND i.lifecycle='managed' AND i.applied_spec_revision=?3",
            params![instance, scope, revision], |r| Ok((r.get(0)?, r.get::<_, String>(1)?))).optional()?.ok_or(DatabaseError::Missing)?;
        if read_instance(db, scope, instance)?.spec_revision != revision { return Err(DatabaseError::InvalidInput); }
        Ok(result)
    }).map_err(map_error)
}
async fn start(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    scope: &str,
    request: &SubscribeLogsRequest,
    closing: &AtomicBool,
) -> Result<(String, LogSubscription), StoreConflict> {
    let instance = &request.instance_id;
    let (generation, container) =
        current(database, scope, instance, request.expected_spec_revision)?;
    let (confirmed, secrets) = confirmed_log_inputs(
        database,
        scope,
        instance,
        request.expected_spec_revision,
        generation,
    )?;
    let target = &confirmed.target;
    let docker = probe
        .bind(target.clone())
        .map_err(|_| StoreConflict::Backend)?;
    let binds = BindStorage::new(database.management_root());
    let projection =
        CreateProjection::new(database, &docker, &binds, &confirmed.project_name, instance);
    let image = database
        .image_resolution(instance, confirmed.spec_revision)?
        .ok_or(StoreConflict::Missing)?;
    let model = projection
        .model(&confirmed, &image)
        .map_err(|_| StoreConflict::InvalidInput)?;
    let expected = projection
        .expected(&model, &image, &container, &confirmed)
        .await
        .map_err(|_| StoreConflict::Backend)?;
    let actual = docker.observe(&expected).await;
    if actual.ownership != Ownership::Verified
        || actual.configuration_matches != Some(true)
        || matches!(
            actual.status,
            RuntimeStatus::Unknown(_) | RuntimeStatus::Absent
        )
    {
        return Err(StoreConflict::InvalidInput);
    }
    if current(database, scope, instance, request.expected_spec_revision)?
        != (generation, container.clone())
    {
        return Err(StoreConflict::StaleRevision);
    }
    let cli = crate::docker_cli::DockerCli::new(
        probe.executable.clone(),
        probe.directory.clone(),
        probe.config_directory.clone(),
        target.endpoint.clone().into(),
    )
    .map_err(|_| StoreConflict::Backend)?;
    if closing.load(Ordering::Acquire) {
        return Err(StoreConflict::Missing);
    }
    let stream = cli
        .follow_logs(&container, &secrets)
        .map_err(|_| StoreConflict::InvalidInput)?;
    Ok((container, stream))
}
fn confirmed_log_inputs(
    database: &DatabaseWorker,
    scope: &str,
    instance: &str,
    revision: u64,
    generation: u64,
) -> Result<(ConfirmedCreate, Vec<String>), StoreConflict> {
    // Read only the applied committed spec; unresolved changes do not affect log eligibility.
    let confirmed = database.read(|db| {
        let view = read_instance(db, scope, instance)?;
        if view.spec_revision != revision || view.revision != generation {
            return Err(DatabaseError::InvalidInput);
        }
        let target_id: String = db.query_row("SELECT target_id FROM instances WHERE id=?1", [instance], |r| r.get(0))?;
        let inputs_json: String = db.query_row(
            "SELECT inputs_json FROM instance_specs WHERE instance_id=?1 AND revision=?2",
            params![instance, view.spec_revision], |r| r.get(0))?;
        let inputs: serde_json::Value = serde_json::from_str(&inputs_json).map_err(|_| DatabaseError::InvalidInput)?;
        let secrets = view.inputs.iter().filter(|i| i.secret).map(|i| match &inputs[&i.slot] {
            serde_json::Value::Null => Ok(String::new()),
            serde_json::Value::String(value) => Ok(value.clone()),
            _ => Err(DatabaseError::InvalidInput),
        }).collect::<Result<Vec<_>, _>>()?;
        let mut q = db.prepare("SELECT relative_path, contents FROM template_snapshot_files WHERE snapshot_id=(SELECT id FROM template_snapshots WHERE instance_id=?1) ORDER BY relative_path")?;
        let files = q.query_map([instance], |r| Ok(TemplateFile { relative_path: r.get(0)?, contents: r.get(1)? }))?.collect::<Result<Vec<_>, _>>()?;
        Ok((view, target_id, inputs_json, files, secrets))
    }).map_err(map_error)?;
    let (view, target_id, inputs_json, snapshot_files, secrets) = confirmed;
    let ports = view
        .ports
        .into_iter()
        .map(|p| PortAllocation {
            slot: p.slot,
            host_ip: p.host_ip,
            host_port: p.host_port,
            container_port: p.container_port,
        })
        .collect();
    let target = database
        .runtime_target(scope)?
        .filter(|t| t.id == target_id)
        .ok_or(StoreConflict::Missing)?;
    let storage = view
        .storage
        .iter()
        .map(|s| {
            database
                .storage_allocation(instance, &s.slot)?
                .ok_or(StoreConflict::Missing)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let confirmed = ConfirmedCreate {
        instance_id: instance.into(),
        scope_id: scope.into(),
        project_name: view.project_name,
        target: target.clone(),
        spec_revision: revision,
        selected_version: view.selected_version,
        snapshot_files,
        inputs_json,
        ports,
        storage,
    };
    Ok((confirmed, secrets))
}

#[cfg(test)]
#[path = "log_subscription_tests.rs"]
mod tests;
