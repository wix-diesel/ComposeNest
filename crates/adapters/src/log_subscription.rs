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
    state_store::{StateStore, StoreConflict, TemplateFile},
};
use composenest_domain::instance::RuntimeStatus;
use rusqlite::{OptionalExtension, params};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

const LEASE: Duration = Duration::from_secs(30);
const PENDING_LEASE: Duration = Duration::from_secs(90);
const CAPACITY: usize = 2;
struct Entry {
    owner: String,
    instance: String,
    revision: u64,
    container: Option<String>,
    touched: Instant,
    stream: Option<LogSubscription>,
    closing: bool,
}
/// Owns at most two subscriptions; dropping entries cancels their read-only CLIs.
#[derive(Default)]
pub struct LogSessions(Mutex<HashMap<String, Entry>>);
impl LogSessions {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
    /// Expires lost or abandoned frontend sessions without depending on IPC cleanup.
    pub fn expire(&self) {
        self.lock().retain(|_, e| {
            if e.touched.elapsed()
                >= if e.stream.is_some() {
                    LEASE
                } else {
                    PENDING_LEASE
                }
            {
                e.closing = true;
            }
            if !e.closing {
                return true;
            }
            if let Some(stream) = &e.stream {
                stream.cancel();
                !stream.snapshot(|b| b.finished)
            } else {
                false
            }
        });
    }
    /// Releases all reads owned by a destroyed window, or all reads on application shutdown.
    pub fn close_owner(&self, owner: Option<&str>) {
        self.lock().retain(|_, e| {
            if owner.is_some_and(|o| e.owner != o) {
                return true;
            }
            e.closing = true;
            if let Some(stream) = &e.stream {
                stream.cancel();
                true
            } else {
                false
            }
        });
    }
    /// Waits for active and already-cancelled log supervisors before the runtime exits.
    pub async fn shutdown(&self) -> Result<(), StoreConflict> {
        let entries = std::mem::take(&mut *self.lock());
        let mut failed = false;
        for entry in entries.into_values() {
            if let Some(stream) = entry.stream {
                failed |= stream.close().await.is_err();
            }
        }
        if failed {
            Err(StoreConflict::Backend)
        } else {
            Ok(())
        }
    }
    /// Releases only the matching window's subscription, including a pending start.
    pub async fn unsubscribe(&self, owner: &str, id: &str) -> Result<(), StoreConflict> {
        let entry = {
            let mut entries = self.lock();
            if entries.get(id).is_some_and(|e| e.owner != owner) {
                return Err(StoreConflict::Missing);
            }
            entries.remove(id)
        };
        if let Some(stream) = entry.and_then(|e| e.stream) {
            stream.close().await.map_err(|_| StoreConflict::Backend)?;
        }
        Ok(())
    }
    /// Starts one validated immutable container, failing closed if the start was cancelled.
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
        let token = Instant::now();
        {
            let mut entries = self.lock();
            if entries.len() >= CAPACITY || entries.contains_key(id) {
                return Err(StoreConflict::UnresolvedOperation);
            }
            entries.insert(
                id.clone(),
                Entry {
                    owner: owner.into(),
                    instance: request.instance_id.clone(),
                    revision: request.expected_spec_revision,
                    container: None,
                    touched: token,
                    stream: None,
                    closing: false,
                },
            );
        }
        let result = start(database, probe, scope, request).await;
        let mut entries = self.lock();
        let (container, stream) = match result {
            Ok(result) => result,
            Err(error) => {
                if entries.get(id).is_some_and(|e| e.touched == token) {
                    entries.remove(id);
                }
                return Err(error);
            }
        };
        let entry = entries
            .get_mut(id)
            .filter(|e| e.owner == owner && e.touched == token && !e.closing)
            .ok_or(StoreConflict::Missing)?;
        entry.container = Some(container);
        entry.stream = Some(stream);
        entry.touched = Instant::now();
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
        let mut entries = self.lock();
        let entry = entries
            .get_mut(id)
            .filter(|e| e.owner == owner && !e.closing)
            .ok_or(StoreConflict::Missing)?;
        let identity = current(database, scope, &entry.instance, entry.revision);
        if identity.as_ref().is_err()
            || identity.as_ref().ok().map(|(_, c)| c) != entry.container.as_ref()
        {
            entry.closing = true;
            if let Some(stream) = &entry.stream {
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
) -> Result<(String, LogSubscription), StoreConflict> {
    let instance = &request.instance_id;
    let (generation, container) =
        current(database, scope, instance, request.expected_spec_revision)?;
    let saved = database
        .clone_source(scope, instance)?
        .ok_or(StoreConflict::UnresolvedOperation)?;
    if saved.spec_revision != request.expected_spec_revision || saved.revision != generation {
        return Err(StoreConflict::StaleRevision);
    }
    let target = database
        .runtime_target(scope)?
        .filter(|t| t.id == saved.target_id)
        .ok_or(StoreConflict::Missing)?;
    let (project, snapshot_files, secrets) = database.read(|db| {
        let view = read_instance(db, scope, instance)?;
        let inputs: serde_json::Value = serde_json::from_str(&saved.inputs_json).map_err(|_| DatabaseError::InvalidInput)?;
        let secrets = view.inputs.iter().filter(|i| i.secret).map(|i| match &inputs[&i.slot] {
            serde_json::Value::Null => Ok(String::new()),
            serde_json::Value::String(value) => Ok(value.clone()),
            _ => Err(DatabaseError::InvalidInput),
        }).collect::<Result<Vec<_>, _>>()?;
        let mut q = db.prepare("SELECT relative_path, contents FROM template_snapshot_files WHERE snapshot_id=(SELECT id FROM template_snapshots WHERE instance_id=?1) ORDER BY relative_path")?;
        let files = q.query_map([instance], |r| Ok(TemplateFile { relative_path: r.get(0)?, contents: r.get(1)? }))?.collect::<Result<Vec<_>, _>>()?;
        Ok((view.project_name, files, secrets))
    }).map_err(map_error)?;
    let storage = saved
        .storage
        .iter()
        .map(|s| {
            database
                .storage_allocation(instance, &s.slot)?
                .ok_or(StoreConflict::Missing)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let confirmed = ConfirmedCreate {
        instance_id: instance.clone(),
        scope_id: scope.into(),
        project_name: project,
        target: target.clone(),
        spec_revision: saved.spec_revision,
        selected_version: saved.selected_version,
        snapshot_files,
        inputs_json: saved.inputs_json,
        ports: saved.ports,
        storage,
    };
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
        target.endpoint.into(),
    )
    .map_err(|_| StoreConflict::Backend)?;
    let stream = cli
        .follow_logs(&container, &secrets)
        .map_err(|_| StoreConflict::InvalidInput)?;
    Ok((container, stream))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support as support;

    #[cfg(unix)]
    #[tokio::test]
    async fn only_verified_engine_owner_and_full_configuration_can_start_logs() {
        use composenest_application::{RequestContext, state_store::StorageMethod};
        use serde_json::json;
        use std::{fs, os::unix::fs::PermissionsExt};
        let (root, database, template) = support::store();
        database
            .create_target(
                "target",
                "scope",
                "unix:///tmp/logs.sock",
                "engine",
                "linux/amd64",
            )
            .unwrap();
        let mut record = support::source_instance(&template);
        record.id = "a".repeat(32);
        record.project_name = format!("cn-{}", record.id);
        record.storage_method = StorageMethod::Volume;
        record.storage[0].resource_identity = format!("{}-data", record.project_name);
        database.commit_instance(&record).unwrap();
        let container = "b".repeat(64);
        let image = format!("sha256:{}", "c".repeat(64));
        {
            let record = record.clone();
            let container = container.clone();
            let image = image.clone();
            database.write(move |db| {
            db.execute("UPDATE instances SET applied_spec_revision=1 WHERE id=?1", [&record.id])?;
            db.execute("UPDATE storage_allocations SET presence='present' WHERE instance_id=?1", [&record.id])?;
            db.execute("INSERT INTO operations(id, instance_id, kind, status, phase, expected_instance_revision, new_spec_revision, completed_at) VALUES('created', ?1, 'create', 'Succeeded', 'ready', 1, 1, CURRENT_TIMESTAMP)", [&record.id])?;
            db.execute("INSERT INTO runtime_observations(instance_id, container_id, runtime_state, freshness) VALUES(?1, ?2, 'stopped', 'fresh')", params![record.id, container])?;
            db.execute("INSERT INTO image_resolutions(instance_id, spec_revision, image_ref, digest, image_id, platform, first_operation_id) VALUES(?1, 1, 'example:1', ?2, ?3, 'linux/amd64', 'created')", params![record.id, format!("example@{image}"), image])?;
            Ok(())
        }).unwrap();
        }
        let actual = json!({ "Id": container, "Image": image,
            "Config": { "Labels": { "com.docker.compose.project": record.project_name, "com.docker.compose.service":"main", "io.composenest.scope":"scope", "io.composenest.instance":record.id, "io.composenest.spec-revision":"1" },
                "Env":["RETAINED=first", format!("PASSWORD={}", "p".repeat(32))], "Cmd":[],
                "Healthcheck":{"Test":["CMD","check"], "Interval":5_000_000_000_u64, "Timeout":3_000_000_000_u64, "StartPeriod":10_000_000_000_u64, "StartInterval":5_000_000_000_u64, "Retries":12}},
            "Mounts":[{"Type":"volume","Name":record.storage[0].resource_identity,"Source":"ignored","Destination":"/data","RW":true}],
            "HostConfig":{"PortBindings":{"5432/tcp":[{"HostIp":"127.0.0.1","HostPort":"5432"}]}},
            "NetworkSettings":{"Networks":{format!("{}_default",record.project_name):{}}}, "State":{"Status":"exited"} });
        fs::write(root.path().join("actual"), actual.to_string()).unwrap();
        fs::write(root.path().join("engine"), r#"{"ID":"engine"}"#).unwrap();
        let executable = root.path().join("docker");
        fs::write(
            &executable,
            format!(
                r#"#!/bin/sh
shift 2
if [ "$1" = info ]; then /bin/cat engine; exit; fi
if [ "$1" = image ]; then printf '{{"Env":[],"Cmd":[]}}'; exit; fi
if [ "$1" = container ]; then /bin/cat actual; exit; fi
if [ "$1" = logs ]; then echo $$ > log-pid; printf '{}\n'; exec /bin/sleep 60; fi
exit 9
"#,
                "p".repeat(32)
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let probe = DockerProbe {
            executable,
            directory: root.path().into(),
            config_directory: root.path().into(),
        };
        let sessions = LogSessions::default();
        let request = SubscribeLogsRequest {
            context: RequestContext {
                api_version: 1,
                request_id: "subscription".into(),
            },
            instance_id: record.id.clone(),
            expected_spec_revision: 1,
        };
        sessions
            .subscribe(&database, &probe, "scope", "main", &request)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if !sessions
                    .get(&database, "scope", "main", "subscription")
                    .unwrap()
                    .lines
                    .is_empty()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            sessions
                .get(&database, "scope", "main", "subscription")
                .unwrap()
                .lines,
            ["********"]
        );
        assert!(
            sessions
                .get(&database, "scope", "other", "subscription")
                .is_err()
        );
        let pid: i32 = fs::read_to_string(root.path().join("log-pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        sessions.unsubscribe("main", "subscription").await.unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        for mode in ["engine", "owner", "image"] {
            let mut changed = actual.clone();
            fs::write(
                root.path().join("engine"),
                if mode == "engine" {
                    r#"{"ID":"other"}"#
                } else {
                    r#"{"ID":"engine"}"#
                },
            )
            .unwrap();
            if mode == "owner" {
                changed["Config"]["Labels"]["io.composenest.instance"] = json!("foreign");
            }
            if mode == "image" {
                changed["Image"] = json!("different");
            }
            fs::write(root.path().join("actual"), changed.to_string()).unwrap();
            fs::remove_file(root.path().join("log-pid")).unwrap_or(());
            assert!(
                sessions
                    .subscribe(&database, &probe, "scope", "main", &request)
                    .await
                    .is_err(),
                "{mode}"
            );
            assert!(!root.path().join("log-pid").exists());
        }
    }

    #[test]
    fn target_requires_scope_applied_and_committed_revision_and_managed_identity() {
        let (_root, database, template) = support::store();
        support::setup_source(&database, &template);
        assert!(current(&database, "scope", "source", 1).is_err());
        database.write(|db| {
            db.execute("UPDATE instances SET applied_spec_revision=1 WHERE id='source'", [])?;
            db.execute("INSERT INTO runtime_observations(instance_id, container_id, runtime_state, freshness) VALUES('source', ?1, 'stopped', 'fresh')", ["a".repeat(64)])?;
            Ok(())
        }).unwrap();
        assert_eq!(
            current(&database, "scope", "source", 1).unwrap().1,
            "a".repeat(64)
        );
        assert!(current(&database, "other", "source", 1).is_err());
        assert!(current(&database, "scope", "other", 1).is_err());
        assert!(current(&database, "scope", "source", 2).is_err());
        database
            .write(|db| {
                db.execute(
                    "UPDATE instances SET lifecycle='retiring' WHERE id='source'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(current(&database, "scope", "source", 1).is_err());
    }
    fn pending(owner: &str, touched: Instant) -> Entry {
        Entry {
            owner: owner.into(),
            instance: "source".into(),
            revision: 1,
            container: None,
            touched,
            stream: None,
            closing: false,
        }
    }
    #[tokio::test]
    async fn window_ownership_pending_cancellation_expiry_and_shutdown_are_scoped() {
        let sessions = LogSessions::default();
        sessions
            .lock()
            .insert("one".into(), pending("main", Instant::now()));
        sessions
            .lock()
            .insert("two".into(), pending("other", Instant::now()));
        assert!(sessions.unsubscribe("other", "one").await.is_err());
        sessions.unsubscribe("main", "one").await.unwrap();
        assert!(!sessions.lock().contains_key("one"));
        sessions.unsubscribe("main", "one").await.unwrap();
        sessions.lock().insert(
            "expired".into(),
            pending("main", Instant::now() - PENDING_LEASE),
        );
        sessions.expire();
        assert!(!sessions.lock().contains_key("expired"));
        sessions.close_owner(Some("main"));
        assert!(sessions.lock().contains_key("two"));
        sessions.close_owner(None);
        assert!(sessions.lock().is_empty());
    }
}
