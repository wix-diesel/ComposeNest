use std::{collections::BTreeMap, fs};

use composenest_adapters::{
    artifact_store::{ArtifactInput, ArtifactStore},
    external_recovery_state::ExternalRecoveryRequest,
    sqlite::DatabaseWorker,
};
use composenest_application::{
    operation_journal::{
        ExpectedResult, OperationJournal, OperationStatus, RequestReceipt, StepCommand, StepIntent,
        StepOutcome,
    },
    operation_recovery::RecoveryJournal,
};
use composenest_domain::instance::RuntimeStatus;

fn fixture() -> (tempfile::TempDir, DatabaseWorker, ExternalRecoveryRequest) {
    let root = tempfile::tempdir().unwrap();
    for name in ["state", "locks"] {
        fs::create_dir(root.path().join(name)).unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [
            root.path().to_path_buf(),
            root.path().join("state"),
            root.path().join("locks"),
        ] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let db = DatabaseWorker::start(root.path()).unwrap();
    db.write(|db| {
        db.execute_batch("INSERT INTO management_scopes (id, owner_id, root_identity) VALUES ('scope', 'owner', 'root');
            INSERT INTO runtime_targets (id, scope_id, endpoint, engine_id, platform) VALUES ('target', 'scope', 'local', 'engine', 'linux/amd64');
            INSERT INTO instances (id, scope_id, target_id, display_name, normalized_name, project_name, applied_spec_revision) VALUES ('instance', 'scope', 'target', 'Test', 'Test', 'cn-instance', 1);
            INSERT INTO instance_specs (instance_id, revision, selected_version, storage_method, inputs_json) VALUES ('instance', 1, '1', 'bind', '{\"password\":\"saved-secret\"}');
            INSERT INTO operations (id, instance_id, kind, phase, expected_instance_revision) VALUES ('create', 'instance', 'create', 'artifact', 1);
            INSERT INTO port_reservations (id, scope_id, instance_id, host_ip, protocol, host_port, status) VALUES ('port', 'scope', 'instance', '127.0.0.1', 'tcp', 15432, 'committed');")?;
        Ok(())
    }).unwrap();
    ArtifactStore::new(root.path(), &db)
        .publish(
            "create",
            ArtifactInput {
                id: "instance-r1".into(),
                instance_id: "instance".into(),
                spec_revision: 1,
                generator_version: "compose-v1".into(),
                files: BTreeMap::from([("compose.yaml".into(), b"saved-secret".to_vec())]),
            },
        )
        .unwrap();
    db.write(|db| {
        db.execute("UPDATE operations SET status = 'Succeeded', completed_at = CURRENT_TIMESTAMP WHERE id = 'create'", [])?;
        Ok(())
    }).unwrap();
    let mut request = ExternalRecoveryRequest {
        receipt: RequestReceipt {
            scope_id: "scope".into(),
            request_id: "recover-request".into(),
            plan_id: None,
            confirmed_revision: 1,
            request_hash: String::new(),
            instance_id: "instance".into(),
            operation_id: "recover".into(),
        },
        spec_revision: 1,
        source_artifact_id: "instance-r1".into(),
        confirmation_hash: "a".repeat(64),
    };
    request.receipt.request_hash = request.request_hash();
    (root, db, request)
}

#[test]
fn acceptance_is_idempotent_hash_bound_and_survives_restart() {
    let (root, db, request) = fixture();
    assert_eq!(
        db.begin_external_recovery(&request).unwrap(),
        request.receipt
    );
    let mut changed = request.clone();
    changed.confirmation_hash = "b".repeat(64);
    changed.receipt.request_hash = changed.request_hash();
    assert!(db.begin_external_recovery(&changed).is_err());
    db.set_status("recover", OperationStatus::Executing, "archive")
        .unwrap();
    drop(db);
    let db = DatabaseWorker::start(root.path()).unwrap();
    let operations = db.recover_on_startup().unwrap();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].status, OperationStatus::OutcomeUnknown);
    assert_eq!(
        db.begin_external_recovery(&request).unwrap(),
        request.receipt
    );
    let record = db.external_recovery("recover").unwrap();
    assert_eq!(record.confirmation_hash, request.confirmation_hash);
    assert_eq!(record.replacement_artifact_id, "recover-restored");
}

#[test]
fn stale_or_wrong_artifact_confirmation_leaves_no_operation() {
    let (_root, db, mut request) = fixture();
    request.receipt.confirmed_revision = 2;
    request.receipt.request_hash = request.request_hash();
    assert!(db.begin_external_recovery(&request).is_err());
    request.receipt.confirmed_revision = 1;
    request.source_artifact_id = "other".into();
    request.receipt.request_hash = request.request_hash();
    assert!(db.begin_external_recovery(&request).is_err());
    assert!(db.external_recovery("recover").is_err());
    assert!(db.receipt("scope", "recover-request").unwrap().is_none());
}

#[test]
fn completion_requires_published_artifact_and_current_journal_observation() {
    let (root, db, request) = fixture();
    db.begin_external_recovery(&request).unwrap();
    let container = "b".repeat(64);
    db.set_status("recover", OperationStatus::Executing, "generate")
        .unwrap();
    assert!(
        db.complete_external_recovery("recover", &container, RuntimeStatus::Stopped)
            .is_err()
    );
    ArtifactStore::new(root.path(), &db)
        .publish(
            "recover",
            ArtifactInput {
                id: "recover-restored".into(),
                instance_id: "instance".into(),
                spec_revision: 1,
                generator_version: "compose-v1".into(),
                files: BTreeMap::from([("compose.yaml".into(), b"saved-secret".to_vec())]),
            },
        )
        .unwrap();
    assert!(
        db.complete_external_recovery("recover", &container, RuntimeStatus::Stopped)
            .is_err()
    );
    db.record_step(&StepIntent {
        operation_id: "recover".into(),
        sequence: 1,
        attempt: 1,
        command_kind: StepCommand::Observe,
        resource_id: container.clone(),
        expected_result: ExpectedResult::StateObserved,
    })
    .unwrap();
    db.finish_step("recover", 1, StepOutcome::Succeeded)
        .unwrap();
    db.complete_external_recovery("recover", &container, RuntimeStatus::Stopped)
        .unwrap();
    assert_eq!(
        db.selected_artifact("instance", 1).unwrap(),
        "recover-restored"
    );
    assert_eq!(db.selected_artifact("instance", 2).unwrap(), "instance-r2");
    let state = db.read(|db| {
        Ok(db.query_row("SELECT i.applied_spec_revision, s.inputs_json, (SELECT status FROM port_reservations WHERE id = 'port') FROM instances i JOIN instance_specs s ON s.instance_id = i.id AND s.revision = i.applied_spec_revision WHERE i.id = 'instance'", [], |row| Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))?)
    }).unwrap();
    assert_eq!(
        state,
        (
            1,
            "{\"password\":\"saved-secret\"}".into(),
            "committed".into()
        )
    );
    assert_eq!(
        db.begin_external_recovery(&request).unwrap(),
        request.receipt
    );
}
