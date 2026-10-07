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
if [ "$1" = container ]; then
  if [ -f block-inspect ]; then
    echo $$ > inspect-pid
    while [ ! -f release-inspect ]; do /bin/sleep 0.01; done
  fi
  /bin/cat actual; exit
fi
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
    // A pending spec and unresolved change do not prevent reading the applied spec.
    {
        let id = record.id.clone();
        database.write(move |db| {
                db.execute("INSERT INTO instance_specs(instance_id, revision, selected_version, storage_method, inputs_json) VALUES(?1, 2, '1', 'volume', '{}')", [&id])?;
                db.execute("INSERT INTO operations(id, instance_id, kind, status, phase, expected_instance_revision, old_spec_revision, new_spec_revision) VALUES('changing', ?1, 'edit_port', 'Executing', 'waiting', 1, 1, 2)", [&id])?;
                Ok(())
            }).unwrap();
    }
    for status in [
        "Accepted",
        "Executing",
        "Failed",
        "AwaitingDecision",
        "OutcomeUnknown",
    ] {
        let status = status.to_owned();
        database
            .write(move |db| {
                db.execute(
                    "UPDATE operations SET status=?1 WHERE id='changing'",
                    [status],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(
            database
                .clone_source("scope", &record.id)
                .unwrap()
                .is_none()
        );
        let view = sessions
            .subscribe(&database, &probe, "scope", "main", &request)
            .await
            .unwrap();
        assert_eq!(view.spec_revision, 1);
        sessions.unsubscribe("main", "subscription").await.unwrap();
    }
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
    // Shutdown remains pending while ownership inspection is in flight.
    fs::write(root.path().join("engine"), r#"{"ID":"engine"}"#).unwrap();
    fs::write(root.path().join("actual"), actual.to_string()).unwrap();
    fs::write(root.path().join("block-inspect"), "").unwrap();
    let subscribing = sessions.subscribe(&database, &probe, "scope", "main", &request);
    let shutdown = async {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !root.path().join("inspect-pid").exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        sessions.close_owner(None);
        sessions.expire();
        assert!(sessions.lock().entries.contains_key("subscription"));
        let stopping = sessions.shutdown();
        tokio::pin!(stopping);
        tokio::select! {
            result = &mut stopping => panic!("shutdown skipped validation: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(30)) => (),
        }
        fs::write(root.path().join("release-inspect"), "").unwrap();
        stopping.await.unwrap();
    };
    let (started, ()) = tokio::join!(subscribing, shutdown);
    assert_eq!(started.err().unwrap(), StoreConflict::Missing);
    assert!(
        !root.path().join("log-pid").exists(),
        "cancelled validation must not spawn logs"
    );
    let inspect_pid: i32 = fs::read_to_string(root.path().join("inspect-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(inspect_pid, 0) }, -1);
    assert!(sessions.lock().entries.is_empty());
    assert!(
        sessions
            .subscribe(&database, &probe, "scope", "main", &request)
            .await
            .is_err()
    );
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
        closing: Arc::new(AtomicBool::new(false)),
        started: watch::channel(true).1,
    }
}
#[cfg(unix)]
#[tokio::test]
async fn shutdown_reaps_a_cli_registered_after_cancellation_started() {
    use std::{fs, os::unix::fs::PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("docker");
    let marker = root.path().join("pid");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nexec /bin/sleep 60\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let cli = crate::docker_cli::DockerCli::new(
        executable,
        root.path().into(),
        root.path().into(),
        "unix:///tmp/log-shutdown.sock".into(),
    )
    .unwrap();
    let sessions = LogSessions::default();
    let (done, started) = watch::channel(false);
    let completion = StartCompletion(done);
    let mut entry = pending("main", Instant::now());
    entry.started = started;
    sessions.lock().entries.insert("late".into(), entry);
    let stopping = sessions.shutdown();
    tokio::pin!(stopping);
    tokio::select! {
        result = &mut stopping => panic!("shutdown skipped pending start: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(10)) => (),
    }
    let stream = cli.follow_logs(&"a".repeat(64), &[]).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while fs::read_to_string(&marker)
            .ok()
            .is_none_or(|s| s.trim().is_empty())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let pid: i32 = fs::read_to_string(&marker).unwrap().trim().parse().unwrap();
    sessions.lock().entries.get_mut("late").unwrap().stream =
        Some(Arc::new(AsyncMutex::new(stream)));
    drop(completion);
    stopping.await.unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert!(sessions.lock().entries.is_empty());
}
#[tokio::test]
async fn failed_cleanup_stays_addressable_blocks_new_reads_and_fails_shutdown() {
    let sessions = LogSessions::default();
    let mut entry = pending("main", Instant::now());
    entry.stream = Some(Arc::new(AsyncMutex::new(
        LogSubscription::unconfirmed_for_test(),
    )));
    sessions.lock().entries.insert("failed".into(), entry);
    assert_eq!(
        sessions.unsubscribe("other", "failed").await,
        Err(StoreConflict::Missing)
    );
    assert_eq!(
        sessions.unsubscribe("main", "failed").await,
        Err(StoreConflict::Backend)
    );
    sessions.expire();
    assert!(sessions.lock().entries.contains_key("failed"));
    assert_eq!(
        sessions.unsubscribe("main", "failed").await,
        Err(StoreConflict::Backend)
    );
    let (_root, database, _) = support::store();
    let probe = DockerProbe {
        executable: std::path::PathBuf::new(),
        directory: std::path::PathBuf::new(),
        config_directory: std::path::PathBuf::new(),
    };
    let request = SubscribeLogsRequest {
        context: composenest_application::RequestContext {
            api_version: 1,
            request_id: "new".into(),
        },
        instance_id: "source".into(),
        expected_spec_revision: 1,
    };
    assert_eq!(
        sessions
            .subscribe(&database, &probe, "scope", "main", &request)
            .await
            .err()
            .unwrap(),
        StoreConflict::UnresolvedOperation
    );
    assert_eq!(sessions.shutdown().await, Err(StoreConflict::Backend));
    assert!(sessions.lock().entries.contains_key("failed"));
}
#[tokio::test]
async fn window_ownership_pending_cancellation_expiry_and_shutdown_are_scoped() {
    let sessions = LogSessions::default();
    sessions
        .lock()
        .entries
        .insert("one".into(), pending("main", Instant::now()));
    sessions
        .lock()
        .entries
        .insert("two".into(), pending("other", Instant::now()));
    assert!(sessions.unsubscribe("other", "one").await.is_err());
    sessions.unsubscribe("main", "one").await.unwrap();
    assert!(!sessions.lock().entries.contains_key("one"));
    sessions.unsubscribe("main", "one").await.unwrap();
    sessions.lock().entries.insert(
        "expired".into(),
        pending("main", Instant::now() - PENDING_LEASE),
    );
    sessions.expire();
    assert!(!sessions.lock().entries.contains_key("expired"));
    sessions.close_owner(Some("main"));
    assert!(sessions.lock().entries.contains_key("two"));
    sessions.close_owner(None);
    sessions.shutdown().await.unwrap();
    assert!(sessions.lock().entries.is_empty());
}
