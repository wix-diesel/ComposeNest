use std::fs;

use composenest_adapters::sqlite::DatabaseWorker;
use composenest_application::{
    create_state::CreateStateStore,
    lifecycle_operation::LifecycleState,
    operation_journal::{OperationKind, RequestReceipt},
    state_store::{StorageAllocation, StoreConflict},
};
use composenest_domain::instance::RuntimeStatus;
use tempfile::TempDir;

fn fixture(kind: &str) -> (TempDir, DatabaseWorker, RequestReceipt) {
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
    let database = DatabaseWorker::start(root.path()).unwrap();
    let kind = kind.to_owned();
    database.write(move |db| {
        db.execute_batch("INSERT INTO management_scopes (id, owner_id, root_identity) VALUES ('scope', 'owner', 'root');
            INSERT INTO runtime_targets (id, scope_id, endpoint, engine_id, platform) VALUES ('target', 'scope', 'local', 'engine', 'linux/amd64');
            INSERT INTO instances (id, scope_id, target_id, display_name, normalized_name, project_name)
                VALUES ('instance', 'scope', 'target', 'Test', 'Test', 'cn-test');
            INSERT INTO template_snapshots (id, instance_id, template_id, template_version, selected_version,
                schema_version, normalization, semantic_hash, canonical_json)
                VALUES ('snapshot', 'instance', 'template', '1', '1', 1, 'template-normalization-v1', 'hash', '{}');
            INSERT INTO template_snapshot_files (snapshot_id, relative_path, contents, sha256)
                VALUES ('snapshot', 'template.yaml', x'74657374', 'hash');
            INSERT INTO instance_specs (instance_id, revision, selected_version, storage_method, inputs_json)
                VALUES ('instance', 1, '1', 'bind', '{\"password\":\"secret\"}');
            INSERT INTO port_bindings (instance_id, spec_revision, slot, host_ip, host_port, container_port)
                VALUES ('instance', 1, 'tcp', '127.0.0.1', 15432, 5432);
            INSERT INTO port_reservations (id, scope_id, instance_id, host_ip, protocol, host_port, status)
                VALUES ('reservation', 'scope', 'instance', '127.0.0.1', 'tcp', 15432, 'committed');
            INSERT INTO storage_allocations (instance_id, slot, method, resource_identity, ownership_evidence,
                ownership, presence, initialization)
                VALUES ('instance', 'data', 'bind', 'data/instance/data', 'proof', 'assigned', 'present', 'not_attempted');")?;
        db.execute("INSERT INTO operations (id, instance_id, kind, status, phase, expected_instance_revision,
                new_spec_revision) VALUES ('operation', 'instance', ?1, 'Accepted', 'accepted', 1, 1)",
            [&kind])?;
        db.execute_batch("INSERT INTO request_receipts (scope_id, request_id, plan_id, confirmed_revision,
                request_hash, instance_id, operation_id)
                VALUES ('scope', 'request', 'plan', 1, 'hash', 'instance', 'operation');")?;
        Ok(())
    }).unwrap();
    let receipt = RequestReceipt {
        scope_id: "scope".into(),
        request_id: "request".into(),
        plan_id: Some("plan".into()),
        confirmed_revision: 1,
        request_hash: "hash".into(),
        instance_id: "instance".into(),
        operation_id: "operation".into(),
    };
    (root, database, receipt)
}

#[test]
fn create_and_clone_load_the_same_confirmed_records() {
    for kind in ["create", "clone"] {
        let (_root, db, receipt) = fixture(kind);
        let state = db.confirmed_create(&receipt).unwrap();
        assert_eq!(state.target.platform, "linux/amd64");
        assert_eq!(state.selected_version, "1");
        assert_eq!(state.inputs_json, r#"{"password":"secret"}"#);
        assert_eq!(state.ports[0].host_port, 15432);
        assert_eq!(state.storage.len(), 1);
    }
}

#[test]
fn lifecycle_completion_preserves_spec_secret_storage_and_reservation() {
    let (_root, db, receipt) = fixture("start");
    let id = "a".repeat(64);
    db.write({
        let id = id.clone();
        move |db| {
            db.execute_batch("UPDATE instances SET applied_spec_revision=1 WHERE id='instance';
                UPDATE operations SET new_spec_revision=NULL, old_spec_revision=1 WHERE id='operation';")?;
            db.execute("INSERT INTO runtime_observations
                (instance_id, operation_id, container_id, runtime_state, freshness)
                VALUES ('instance', NULL, ?1, 'stopped', 'fresh')", [&id])?;
            Ok(())
        }
    }).unwrap();
    let saved = db.snapshot(&receipt, OperationKind::Start).unwrap();
    assert_eq!(saved.container_id, id);
    assert_eq!(saved.spec_revision, 1);
    db.write(|db| {
        db.execute(
            "UPDATE operations SET status='Executing' WHERE id='operation'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    db.complete(&receipt.operation_id, &id, RuntimeStatus::Ready)
        .unwrap();
    let values = db
        .read(|db| {
            Ok(db.query_row(
                "SELECT i.applied_spec_revision, s.inputs_json,
            a.resource_identity, r.status, o.status, v.runtime_state
            FROM instances i JOIN instance_specs s ON s.instance_id=i.id
            JOIN storage_allocations a ON a.instance_id=i.id
            JOIN port_reservations r ON r.instance_id=i.id
            JOIN operations o ON o.instance_id=i.id
            JOIN runtime_observations v ON v.instance_id=i.id
            WHERE i.id='instance'",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )?)
        })
        .unwrap();
    assert_eq!(
        values,
        (
            1,
            r#"{"password":"secret"}"#.into(),
            "data/instance/data".into(),
            "committed".into(),
            "Succeeded".into(),
            "running".into()
        )
    );
}

#[test]
fn confirmed_create_rejects_inconsistent_committed_records() {
    for sql in [
        "UPDATE operations SET status='Failed' WHERE id='operation'",
        "UPDATE operations SET expected_instance_revision=2 WHERE id='operation'",
        "UPDATE template_snapshots SET selected_version='2' WHERE id='snapshot'",
        "UPDATE port_reservations SET status='released' WHERE id='reservation'",
        "UPDATE instance_specs SET storage_method='volume' WHERE instance_id='instance'",
    ] {
        let (_root, db, receipt) = fixture("create");
        db.write(|db| {
            db.execute(sql, [])?;
            Ok(())
        })
        .unwrap();
        assert!(
            matches!(
                db.confirmed_create(&receipt),
                Err(StoreConflict::InvalidInput)
            ),
            "inconsistent record was accepted: {sql}; got {:?}",
            db.confirmed_create(&receipt).err()
        );
    }

    let (_root, db, mut receipt) = fixture("clone");
    receipt.request_hash = "different".into();
    assert!(matches!(
        db.confirmed_create(&receipt),
        Err(StoreConflict::InvalidInput)
    ));

    let (_root, db, receipt) = fixture("create");
    db.write(|db| {
        db.execute(
            "DELETE FROM template_snapshot_files WHERE snapshot_id='snapshot'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        db.confirmed_create(&receipt),
        Err(StoreConflict::Missing)
    ));
}

#[test]
fn ready_requires_observed_success_and_preserves_allocations_on_failure() {
    let (_root, db, receipt) = fixture("create");
    let id = "a".repeat(64);
    db.write(|db| {
        db.execute(
            "UPDATE operations SET status='Executing', phase='create' WHERE id='operation'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        db.complete_ready(&receipt.operation_id, &id),
        Err(StoreConflict::Missing)
    );
    assert_eq!(
        db.mark_may_have_initialized(&receipt.operation_id),
        Err(StoreConflict::Missing)
    );
    db.write(|db| {
        db.execute_batch("INSERT INTO operation_steps (operation_id, sequence, attempt, command_kind,
            expected_result, resource_id, outcome, observed_at)
            VALUES ('operation', 1, 1, 'compose_create', 'container_created', 'instance', 'succeeded', CURRENT_TIMESTAMP);
            INSERT INTO operation_steps (operation_id, sequence, attempt, command_kind,
            expected_result, resource_id, outcome, observed_at)
            VALUES ('operation', 2, 1, 'observe', 'state_observed', 'instance', 'succeeded', CURRENT_TIMESTAMP);")?;
        Ok(())
    }).unwrap();
    db.mark_may_have_initialized(&receipt.operation_id).unwrap();
    let observed_id = id.clone();
    db.write(move |db| {
        db.execute_batch("INSERT INTO operation_steps (operation_id, sequence, attempt, command_kind,
            expected_result, resource_id, outcome, observed_at)
            VALUES ('operation', 3, 1, 'compose_start', 'container_running', 'instance', 'succeeded', CURRENT_TIMESTAMP);")?;
        db.execute("INSERT INTO operation_steps (operation_id, sequence, attempt, command_kind,
            expected_result, resource_id, outcome, observed_at)
            VALUES ('operation', 4, 1, 'observe', 'state_observed', ?1, 'failed', CURRENT_TIMESTAMP)",
            [&observed_id])?;
        Ok(())
    }).unwrap();
    assert_eq!(
        db.complete_ready(&receipt.operation_id, &id),
        Err(StoreConflict::InvalidInput)
    );
    db.write(|db| {
        db.execute("UPDATE operation_steps SET outcome = 'succeeded' WHERE operation_id = 'operation' AND sequence = 4", [])?;
        Ok(())
    }).unwrap();
    db.complete_ready(&receipt.operation_id, &id).unwrap();
    db.read(|db| {
        let (applied, status, initialization, reservation): (i64, String, String, String) = db
            .query_row(
                "SELECT i.applied_spec_revision, o.status, s.initialization, p.status
             FROM instances i JOIN operations o ON o.instance_id=i.id
             JOIN storage_allocations s ON s.instance_id=i.id
             JOIN port_reservations p ON p.instance_id=i.id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        assert_eq!(
            (
                applied,
                status.as_str(),
                initialization.as_str(),
                reservation.as_str()
            ),
            (1, "Succeeded", "ready_observed", "committed")
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn bind_proof_is_recorded_only_for_the_allocated_path() {
    let (_root, db, _receipt) = fixture("clone");
    db.write(|db| {
        db.execute(
            "UPDATE operations SET status='Executing', phase='storage' WHERE id='operation'",
            [],
        )?;
        db.execute(
            "UPDATE storage_allocations SET presence='not_materialized' WHERE instance_id='instance'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let proof = "a".repeat(64);
    let wrong = StorageAllocation {
        slot: "data".into(),
        resource_identity: "data/other/data".into(),
        ownership_evidence: proof.clone(),
    };
    assert_eq!(
        db.record_bind_materialization("operation", &wrong),
        Err(StoreConflict::InvalidInput)
    );
    let allocation = StorageAllocation {
        resource_identity: "data/instance/data".into(),
        ..wrong
    };
    db.record_bind_materialization("operation", &allocation)
        .unwrap();
    assert_eq!(
        db.record_bind_materialization("operation", &allocation),
        Err(StoreConflict::InvalidInput)
    );
    db.read(|db| {
        let (presence, evidence): (String, String) = db.query_row(
            "SELECT presence, ownership_evidence FROM storage_allocations WHERE instance_id='instance' AND slot='data'",
            [], |row| Ok((row.get(0)?, row.get(1)?)))?;
        assert_eq!((presence.as_str(), evidence.as_str()), ("present", proof.as_str()));
        Ok(())
    }).unwrap();
}
