use std::fs;

use composenest_adapters::sqlite::DatabaseWorker;
use composenest_application::{
    operation_journal::RequestReceipt,
    port_edit::{PortEditRequest, PortEditStore},
    state_store::{PortAllocation, StoreConflict},
};

fn fixture() -> (tempfile::TempDir, DatabaseWorker, PortEditRequest) {
    let root = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    for dir in ["state", "locks"] {
        fs::create_dir(root.path().join(dir)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.path().join(dir), fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let db = DatabaseWorker::start(root.path()).unwrap();
    db.write(|db| {
        db.execute_batch("INSERT INTO management_scopes (id, owner_id, root_identity) VALUES ('scope', 'owner', 'root');
            INSERT INTO runtime_targets (id, scope_id, endpoint, engine_id, platform) VALUES ('target', 'scope', 'local', 'engine', 'linux/amd64');
            INSERT INTO instances (id, scope_id, target_id, display_name, normalized_name, project_name, applied_spec_revision)
                VALUES ('instance', 'scope', 'target', 'Test', 'Test', 'cn-instance', 1);
            INSERT INTO instance_specs (instance_id, revision, selected_version, storage_method, inputs_json)
                VALUES ('instance', 1, '1', 'bind', '{}');
            INSERT INTO port_bindings (instance_id, spec_revision, slot, host_ip, host_port, container_port)
                VALUES ('instance', 1, 'db', '127.0.0.1', 15432, 5432);
            INSERT INTO port_reservations (id, scope_id, instance_id, host_ip, protocol, host_port, status)
                VALUES ('old', 'scope', 'instance', '127.0.0.1', 'tcp', 15432, 'committed');
            INSERT INTO runtime_observations (instance_id, container_id, runtime_state, freshness)
                VALUES ('instance', NULL, 'absent', 'fresh');")?;
        Ok(())
    }).unwrap();
    let request = PortEditRequest {
        receipt: RequestReceipt {
            scope_id: "scope".into(),
            request_id: "edit-1".into(),
            plan_id: None,
            confirmed_revision: 1,
            request_hash: "a".repeat(64),
            instance_id: "instance".into(),
            operation_id: "edit".into(),
        },
        expected_instance_revision: 1,
        old_spec_revision: 1,
        ports: vec![PortAllocation {
            slot: "db".into(),
            host_ip: "127.0.0.1".into(),
            host_port: 15433,
            container_port: 5432,
        }],
    };
    (root, db, request)
}

fn status(db: &DatabaseWorker, port: u16) -> Option<String> {
    db.read(|db| {
        Ok(db
            .query_row(
                "SELECT status FROM port_reservations WHERE host_port = ?1",
                [port],
                |row| row.get(0),
            )
            .ok())
    })
    .unwrap()
}

#[test]
fn reserves_only_changed_port_and_switches_after_verified_artifact() {
    let (_root, db, request) = fixture();
    assert_eq!(db.begin_port_edit(&request), Ok(request.receipt.clone()));
    assert_eq!(db.begin_port_edit(&request), Ok(request.receipt.clone()));
    assert_eq!(status(&db, 15432).as_deref(), Some("committed"));
    assert_eq!(status(&db, 15433).as_deref(), Some("held"));
    assert_eq!(db.pending_ports("edit").unwrap()[0].host_port, 15433);
    let id = "a".repeat(64);
    assert_eq!(
        db.complete_port_edit("edit", &id),
        Err(StoreConflict::InvalidInput)
    );
    assert_eq!(status(&db, 15432).as_deref(), Some("committed"));
    assert_eq!(status(&db, 15433).as_deref(), Some("held"));
    db.write(|db| {
        db.execute_batch("INSERT INTO artifacts (id, instance_id, spec_revision, generator_version, manifest_hash, placement, publication_operation_id)
            VALUES ('instance-r2', 'instance', 2, 'v1', 'hash', 'published', 'edit');
            UPDATE operations SET status = 'Executing' WHERE id = 'edit';")?;
        Ok(())
    }).unwrap();
    db.complete_port_edit("edit", &id).unwrap();
    assert_eq!(status(&db, 15432).as_deref(), Some("released"));
    assert_eq!(status(&db, 15433).as_deref(), Some("committed"));
    let applied: i64 = db
        .read(|db| {
            Ok(db.query_row(
                "SELECT applied_spec_revision FROM instances WHERE id = 'instance'",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(applied, 2);
}

#[test]
fn rejects_duplicate_port_and_does_not_leave_pending_records() {
    let (_root, db, mut request) = fixture();
    request.ports[0].host_port = 15432;
    assert_eq!(
        db.begin_port_edit(&request),
        Err(StoreConflict::InvalidInput)
    );
    assert_eq!(status(&db, 15432).as_deref(), Some("committed"));
    assert_eq!(status(&db, 15433), None);
    let count: i64 = db
        .read(|db| Ok(db.query_row("SELECT COUNT(*) FROM pending_changes", [], |row| row.get(0))?))
        .unwrap();
    assert_eq!(count, 0);
}
