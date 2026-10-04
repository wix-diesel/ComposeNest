use std::fs;

use composenest_adapters::sqlite::DatabaseWorker;
use composenest_application::{query_service::QueryService, state_store::StoreConflict};
use tempfile::TempDir;

fn fixture() -> (TempDir, DatabaseWorker) {
    let root = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    for name in ["state", "locks"] {
        let path = root.path().join(name);
        fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let db = DatabaseWorker::start(root.path()).unwrap();
    db.write(|db| {
        db.execute_batch(r#"
            INSERT INTO management_scopes (id, owner_id, root_identity) VALUES ('scope', 'owner', 'root');
            INSERT INTO runtime_targets (id, scope_id, endpoint, engine_id, platform)
                VALUES ('target', 'scope', 'local', 'engine', 'linux/amd64');
            INSERT INTO instances (id, scope_id, target_id, display_name, normalized_name, project_name, applied_spec_revision)
                VALUES ('one', 'scope', 'target', 'Database', 'Database', 'cn-one', 1);
            INSERT INTO template_snapshots (id, instance_id, template_id, template_version, selected_version,
                schema_version, normalization, semantic_hash, canonical_json) VALUES
                ('snapshot', 'one', 'postgresql', '17', '17', 1, 'template-normalization-v1', 'hash',
                '{"versions":[{"key":"17","definition":{"image":"postgres:17","inputs":{"order":["username","password","mode"],"values":{"username":{"type":"string"},"password":{"type":"secret"},"mode":{"type":"select"}}},"connections":{"order":["database"],"values":{"database":{"label":"PostgreSQL","port":"database","inputs":["username","password"]}}}}}]}');
            INSERT INTO instance_specs (instance_id, revision, selected_version, storage_method, inputs_json)
                VALUES ('one', 1, '17', 'bind', '{"username":"app","password":"top-secret","mode":"safe"}');
            INSERT INTO port_bindings (instance_id, spec_revision, slot, host_ip, host_port, container_port)
                VALUES ('one', 1, 'database', '127.0.0.1', 15432, 5432);
            INSERT INTO storage_allocations (instance_id, slot, method, resource_identity, ownership_evidence,
                ownership, presence, initialization) VALUES
                ('one', 'data', 'bind', 'data/one/data', 'proof', 'assigned', 'present', 'ready_observed');
            INSERT INTO runtime_observations (instance_id, runtime_state, health, observed_at, freshness)
                VALUES ('one', 'running', 'healthy', '2026-09-01 00:00:00', 'stale');
            INSERT INTO operations (id, instance_id, kind, status, phase, expected_instance_revision,
                new_spec_revision, completed_at) VALUES
                ('create', 'one', 'create', 'Succeeded', 'done', 1, 1, CURRENT_TIMESTAMP);
            INSERT INTO instance_specs (instance_id, revision, selected_version, storage_method, inputs_json)
                VALUES ('one', 2, '17', 'bind', '{"password":"different-secret"}');
            INSERT INTO operations (id, instance_id, kind, status, phase, expected_instance_revision,
                new_spec_revision, completed_at) VALUES
                ('abandoned', 'one', 'edit_port', 'Abandoned', 'done', 1, 2, CURRENT_TIMESTAMP);
        "#)?;
        Ok(())
    }).unwrap();
    (root, db)
}

#[test]
fn saved_list_and_detail_use_committed_ports_and_mask_secrets() {
    let (_root, db) = fixture();
    let query = QueryService::new(&db);
    let list = query.list_instances("scope").unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(
        query.get_instance("scope", "one").unwrap(),
        Some(list[0].clone())
    );
    assert!(query.get_instance("other", "one").unwrap().is_none());
    let view = &list[0];
    assert_eq!(view.template_id, "postgresql");
    assert_eq!(view.image.as_deref(), Some("postgres:17"));
    assert_eq!(view.spec_revision, 1);
    assert_eq!(view.connections[0].port.host_port, 15432);
    assert_eq!(view.observation.as_ref().unwrap().freshness, "stale");
    assert_eq!(view.runtime_status, "unknown");
    assert!(view.needs_attention);
    assert_eq!(
        view.observation.as_ref().unwrap().observed_at,
        "2026-09-01 00:00:00"
    );
    assert!(
        view.inputs
            .iter()
            .find(|item| item.slot == "password")
            .unwrap()
            .value
            .is_none()
    );
    let mode = view.inputs.iter().find(|item| item.slot == "mode").unwrap();
    assert!(!mode.secret);
    assert_eq!(mode.value.as_ref().unwrap(), "safe");
    let json = serde_json::to_string(view).unwrap();
    assert!(!json.contains("top-secret"));
    assert!(!json.contains("different-secret"));
}

#[test]
fn rename_checks_revision_duplicates_and_unresolved_operations() {
    let (_root, db) = fixture();
    let query = QueryService::new(&db);
    assert_eq!(
        query.rename("scope", "one", 0, "Next"),
        Err(StoreConflict::StaleRevision)
    );
    assert_eq!(
        query.rename("other", "one", 1, "Next"),
        Err(StoreConflict::Missing)
    );
    db.write(|db| {
        db.execute_batch("INSERT INTO instances (id, scope_id, target_id, display_name, normalized_name, project_name)
            VALUES ('two', 'scope', 'target', 'Taken', 'Taken', 'cn-two');")?;
        Ok(())
    }).unwrap();
    assert_eq!(
        query.rename("scope", "one", 1, " Taken "),
        Err(StoreConflict::Duplicate)
    );
    db.write(|db| {
        db.execute_batch("INSERT INTO operations (id, instance_id, kind, status, phase, expected_instance_revision)
            VALUES ('pending', 'one', 'start', 'Failed', 'run', 1);")?;
        Ok(())
    }).unwrap();
    assert_eq!(
        query.rename("scope", "one", 1, "Next"),
        Err(StoreConflict::UnresolvedOperation)
    );
    db.write(|db| {
        db.execute_batch("UPDATE operations SET status='Abandoned', completed_at=CURRENT_TIMESTAMP WHERE id='pending';")?;
        Ok(())
    }).unwrap();
    assert_eq!(query.rename("scope", "one", 1, "  Next  "), Ok(2));
    assert_eq!(
        query.rename("scope", "one", 1, "Old"),
        Err(StoreConflict::StaleRevision)
    );
    let view = query.get_instance("scope", "one").unwrap().unwrap();
    assert_eq!(
        (
            view.name.as_str(),
            view.project_name.as_str(),
            view.spec_revision
        ),
        ("Next", "cn-one", 1)
    );
    assert_eq!(view.storage[0].slot, "data");
}

#[test]
fn failed_operation_and_missing_storage_remain_visible_with_ready_observation() {
    let (_root, db) = fixture();
    db.write(|db| {
        db.execute_batch("UPDATE runtime_observations SET freshness='fresh' WHERE instance_id='one';
            UPDATE storage_allocations SET presence='missing' WHERE instance_id='one';
            INSERT INTO operations (id, instance_id, kind, status, phase, expected_instance_revision)
                VALUES ('failed-start', 'one', 'start', 'Failed', 'observe', 1);")?;
        Ok(())
    }).unwrap();
    let view = QueryService::new(&db)
        .get_instance("scope", "one")
        .unwrap()
        .unwrap();
    assert_eq!(view.runtime_status, "ready");
    assert_eq!(view.storage[0].presence, "missing");
    assert_eq!(view.last_operation.unwrap().status, "Failed");
    assert!(view.needs_attention);
}

#[test]
fn lifecycle_acceptance_is_scoped_versioned_idempotent_and_never_optimistic() {
    use composenest_application::{
        RequestContext,
        instance_actions::{self, ChangeInstanceRequest},
    };
    let (_root, db) = fixture();
    db.write(|db| { db.execute_batch("UPDATE runtime_observations SET freshness='fresh', runtime_state='stopped', health=NULL;")?; Ok(()) }).unwrap();
    let accept = |scope: &str, request: &ChangeInstanceRequest| {
        instance_actions::accept(&db, scope, request, &mut composenest_adapters::SystemRandom)
    };
    let mut request = ChangeInstanceRequest {
        context: RequestContext {
            api_version: 1,
            request_id: "lifecycle-request".into(),
        },
        instance_id: "one".into(),
        expected_revision: 1,
        action: "start".into(),
    };
    assert!(matches!(
        accept("other", &request),
        Err(StoreConflict::Missing)
    ));
    request.expected_revision = 2;
    assert!(matches!(
        accept("scope", &request),
        Err(StoreConflict::StaleRevision)
    ));
    request.expected_revision = 1;
    let (receipt, _, fresh) = accept("scope", &request).unwrap();
    assert!(fresh);
    let state = instance_actions::view(&db, "scope", "one").unwrap();
    assert_eq!(state.runtime_status, "stopped");
    assert_eq!(state.operation_status.as_deref(), Some("Accepted"));
    assert!(state.actions.is_empty());
    assert!(
        !serde_json::to_string(&state)
            .unwrap()
            .contains("top-secret")
    );
    let (repeated, _, fresh) = accept("scope", &request).unwrap();
    assert_eq!(receipt, repeated);
    assert!(!fresh);
    request.action = "stop".into();
    assert!(matches!(
        accept("scope", &request),
        Err(StoreConflict::Duplicate)
    ));
    request.context.request_id = "another-request".into();
    assert!(matches!(
        accept("scope", &request),
        Err(StoreConflict::UnresolvedOperation)
    ));
    for status in ["Executing", "Failed", "OutcomeUnknown", "AwaitingDecision"] {
        db.write(move |db| {
            db.execute(
                "UPDATE operations SET status=?1 WHERE status NOT IN ('Succeeded','Abandoned')",
                [status],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(
            instance_actions::view(&db, "scope", "one")
                .unwrap()
                .actions
                .is_empty()
        );
        assert_eq!(
            QueryService::new(&db).rename("scope", "one", 1, "Renamed"),
            Err(StoreConflict::UnresolvedOperation)
        );
    }
}

#[test]
fn absent_restart_offers_start_and_running_rename_keeps_runtime_and_ports() {
    use composenest_application::instance_actions;
    let (_root, db) = fixture();
    db.write(|db| { db.execute_batch("UPDATE runtime_observations SET freshness='fresh', runtime_state='absent', health=NULL;")?; Ok(()) }).unwrap();
    let state = instance_actions::view(&db, "scope", "one").unwrap();
    assert!(state.actions.contains(&"start".into()));
    assert!(!state.actions.contains(&"restart".into()));
    db.write(|db| {
        db.execute_batch(
            "UPDATE runtime_observations SET runtime_state='running', health='healthy';",
        )?;
        Ok(())
    })
    .unwrap();
    assert!(
        instance_actions::view(&db, "scope", "one")
            .unwrap()
            .actions
            .contains(&"rename".into())
    );
    assert_eq!(
        QueryService::new(&db).rename("scope", "one", 1, "Running DB"),
        Ok(2)
    );
    let state = QueryService::new(&db)
        .get_instance("scope", "one")
        .unwrap()
        .unwrap();
    assert_eq!(state.runtime_status, "ready");
    assert_eq!(state.ports[0].host_port, 15432);
    assert_eq!(state.spec_revision, 1);
}
