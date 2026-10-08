use std::{fs, path::Path};

use composenest_adapters::sqlite::DatabaseWorker;
use composenest_application::{
    create_operation::{CreateOperation, CreateOperationError},
    create_state::CreateStateStore,
    operation_journal::{OperationJournal, OperationStatus},
    operation_recovery::{
        CurrentRuntime, RecoveryDecision, RecoveryEvidence, RecoveryJournal, classify_recovery,
    },
    operation_runner::OperationRunner,
    state_store::StoreConflict,
};

#[path = "interruption.rs"]
mod interruption;
use interruption as crash;
#[path = "create_stages.rs"]
mod create_stages;
use create_stages::{Progress, Stages};

#[tokio::test]
async fn worker() {
    let Some(root) = std::env::var_os("COMPOSENEST_CRASH_ROOT") else {
        return;
    };
    let db = DatabaseWorker::start(Path::new(&root)).unwrap();
    let receipt = db.receipt("scope", "request").unwrap().unwrap();
    let result = CreateOperation {
        state: &db,
        journal: &db,
        runner: &OperationRunner::new(),
        stages: &Stages(Path::new(&root), &"a".repeat(64)),
        progress: &Progress(&db),
    }
    .run(&receipt)
    .await;
    assert_eq!(
        result,
        Err(CreateOperationError::Store(StoreConflict::Backend))
    );
    crash::checkpoint("full");
    panic!("crash boundary was not reached");
}

#[test]
fn create_and_clone_crashes_preserve_confirmed_identity_and_ready_evidence() {
    for kind in ["create", "clone"] {
        for boundary in [
            "storage", "image", "artifact", "config", "create", "inspect", "start", "ready",
            "commit",
        ]
        .into_iter()
        .flat_map(|name| [format!("before:{name}"), format!("after:{name}")])
        .chain(["full".into()])
        {
            let (root, db, receipt) = super::fixture(kind);
            let original = db.confirmed_create(&receipt).unwrap();
            db.write(|db| {
                db.execute_batch("INSERT INTO instances (id, scope_id, target_id, display_name, normalized_name, project_name)
                    VALUES ('source', 'scope', 'target', 'Original', 'Original', 'cn-source'),
                           ('other', 'scope', 'target', 'Other', 'Other', 'cn-other');
                    UPDATE instances SET clone_source_id='source' WHERE id='instance';")?;
                Ok(())
            }).unwrap();
            fs::write(root.path().join("source-data"), "original").unwrap();
            fs::write(root.path().join("other-data"), "unrelated").unwrap();
            drop(db);
            crash::child("create_interruption::worker", root.path(), &boundary);
            let db = DatabaseWorker::start(root.path()).unwrap();
            assert_eq!(
                db.plan_receipt("scope", "plan").unwrap(),
                Some(receipt.clone())
            );
            let unresolved = db.recover_on_startup().unwrap();
            assert_eq!(unresolved.len(), usize::from(boundary != "after:commit"));
            if let Some(operation) = unresolved.first() {
                assert_eq!(operation.id, receipt.operation_id);
                assert_eq!(operation.status, OperationStatus::OutcomeUnknown);
                let saved = db.confirmed_create(&receipt).unwrap();
                assert_eq!(saved.inputs_json, original.inputs_json);
                assert_eq!(saved.project_name, original.project_name);
                assert_eq!(saved.storage[0].allocation, original.storage[0].allocation);
                let ready = root.path().join("ready").exists();
                let decision = classify_recovery(
                    operation,
                    RecoveryEvidence {
                        previous_cli_exited: true,
                        target_matches: true,
                        artifact_matches: true,
                        storage_verified: true,
                        docker_verified: true,
                        runtime: if ready {
                            CurrentRuntime::Ready
                        } else {
                            CurrentRuntime::Unknown
                        },
                    },
                );
                assert_eq!(
                    decision,
                    if ready {
                        RecoveryDecision::Complete
                    } else {
                        RecoveryDecision::Hold
                    }
                );
            }
            if boundary == "full" {
                crash::restore_capacity(&db);
            }
            db.read(|db| {
                assert_eq!(db.query_row("SELECT count(*) FROM instances", [], |r| r.get::<_, i64>(0))?, 3);
                assert_eq!(db.query_row("SELECT count(*) FROM port_reservations WHERE status='committed'", [], |r| r.get::<_, i64>(0))?, 1);
                assert_eq!(db.query_row("SELECT count(*) FROM instances WHERE id IN ('source','other') AND revision=1 AND applied_spec_revision IS NULL", [], |r| r.get::<_, i64>(0))?, 2);
                Ok(())
            }).unwrap();
            assert_eq!(
                fs::read_to_string(root.path().join("source-data")).unwrap(),
                "original"
            );
            assert_eq!(
                fs::read_to_string(root.path().join("other-data")).unwrap(),
                "unrelated"
            );
        }
    }
}
