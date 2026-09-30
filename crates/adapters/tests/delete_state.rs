mod support;

use composenest_application::{
    delete_operation::{DeleteError, DeleteOperation, DeleteStages, DeleteState, StorageCheck},
    lifecycle_operation::LifecycleEffectError,
    operation_journal::{
        ExpectedResult, OperationIntent, OperationJournal, OperationKind, OperationStatus,
        RequestReceipt, StepCommand, StepIntent, StepOutcome,
    },
    operation_runner::OperationRunner,
    state_store::{StateStore, StoreConflict},
};
use composenest_domain::instance::StoragePresence;
use support::{setup_source, store};

fn request() -> (OperationIntent, RequestReceipt) {
    (
        OperationIntent {
            id: "delete".into(),
            instance_id: "source".into(),
            kind: OperationKind::Delete,
            phase: "retiring".into(),
            expected_revision: 1,
            old_spec_revision: None,
            new_spec_revision: None,
        },
        RequestReceipt {
            scope_id: "scope".into(),
            request_id: "delete-request".into(),
            plan_id: None,
            confirmed_revision: 1,
            request_hash: "a".repeat(64),
            instance_id: "source".into(),
            operation_id: "delete".into(),
        },
    )
}

struct Stages(Result<(), LifecycleEffectError>, StoragePresence);
impl DeleteStages for Stages {
    async fn runtime_absent(&self) -> Result<(), LifecycleEffectError> {
        self.0
    }
    async fn remove_runtime(&self, _: &RequestReceipt) -> Result<(), LifecycleEffectError> {
        self.0
    }
    async fn inspect_storage(&self) -> Result<Vec<StorageCheck>, LifecycleEffectError> {
        Ok(vec![StorageCheck {
            slot: "data".into(),
            presence: self.1,
        }])
    }
}

#[tokio::test]
async fn retirement_preserves_history_reports_actual_presence_and_replays_receipt() {
    for presence in [
        StoragePresence::Present,
        StoragePresence::Missing,
        StoragePresence::NotMaterialized,
        StoragePresence::Unverified,
    ] {
        let (_root, db, template) = store();
        setup_source(&db, &template);
        let (intent, receipt) = request();
        let runner = OperationRunner::new();
        let stages = Stages(Ok(()), presence);
        let service = DeleteOperation {
            state: &db,
            runner: &runner,
            stages: &stages,
        };
        assert_eq!(
            service.run(&intent, &receipt, true).await,
            Ok(receipt.clone())
        );
        assert_eq!(service.run(&intent, &receipt, true).await, Ok(receipt));
        let entry = db.storage_allocation("source", "data").unwrap().unwrap();
        assert_eq!(entry.presence, presence);
        assert_eq!(
            entry.ownership,
            composenest_domain::instance::StorageOwnership::Retained
        );
        assert_eq!(db.source_revision("scope", "source"), Ok(Some(3)));
        db.read(|db| {
            assert_eq!(db.query_row("SELECT lifecycle FROM instances WHERE id='source'", [], |r| r.get::<_, String>(0))?, "retired");
            assert_eq!(db.query_row("SELECT COUNT(*) FROM template_snapshots WHERE instance_id='source'", [], |r| r.get::<_, i64>(0))?, 1);
            assert_eq!(db.query_row("SELECT COUNT(*) FROM instance_specs WHERE instance_id='source'", [], |r| r.get::<_, i64>(0))?, 1);
            assert_eq!(db.query_row("SELECT COUNT(*) FROM port_reservations WHERE instance_id='source' AND status!='released'", [], |r| r.get::<_, i64>(0))?, 0);
            Ok(())
        }).unwrap();
        let mut replacement = support::source_instance(&template);
        replacement.id = "replacement".into();
        replacement.project_name = "cn-replacement".into();
        replacement.storage[0].resource_identity = "data/replacement/data".into();
        db.commit_instance(&replacement).unwrap();
    }
}

#[tokio::test]
async fn unavailable_engine_or_rejected_ownership_keeps_retiring_and_reservations() {
    for error in [
        LifecycleEffectError::OutcomeUnknown,
        LifecycleEffectError::Rejected,
    ] {
        let (_root, db, template) = store();
        setup_source(&db, &template);
        let (intent, receipt) = request();
        let runner = OperationRunner::new();
        let stages = Stages(Err(error), StoragePresence::Present);
        let result = DeleteOperation {
            state: &db,
            runner: &runner,
            stages: &stages,
        }
        .run(&intent, &receipt, true)
        .await;
        assert!(matches!(
            result,
            Err(DeleteError::OutcomeUnknown | DeleteError::Rejected)
        ));
        db.read(|db| {
            assert_eq!(db.query_row("SELECT lifecycle FROM instances WHERE id='source'", [], |r| r.get::<_, String>(0))?, "retiring");
            assert_eq!(db.query_row("SELECT COUNT(*) FROM port_reservations WHERE instance_id='source' AND status='committed'", [], |r| r.get::<_, i64>(0))?, 1);
            Ok(())
        }).unwrap();
        assert!(db.delete_pending(&receipt).is_err());
    }
}

#[tokio::test]
async fn confirmation_revision_and_unresolved_operation_are_required_before_retiring() {
    let (_root, db, template) = store();
    setup_source(&db, &template);
    let (mut intent, mut receipt) = request();
    let runner = OperationRunner::new();
    let stages = Stages(Ok(()), StoragePresence::Present);
    let service = DeleteOperation {
        state: &db,
        runner: &runner,
        stages: &stages,
    };
    assert_eq!(
        service.run(&intent, &receipt, false).await,
        Err(DeleteError::Rejected)
    );
    intent.expected_revision = 2;
    receipt.confirmed_revision = 2;
    assert_eq!(
        service.run(&intent, &receipt, true).await,
        Err(DeleteError::Store(StoreConflict::StaleRevision))
    );
    intent.expected_revision = 1;
    receipt.confirmed_revision = 1;
    let mut previous = intent.clone();
    previous.kind = OperationKind::Stop;
    previous.id = "stop".into();
    let mut prior_receipt = receipt.clone();
    prior_receipt.operation_id = "stop".into();
    prior_receipt.request_id = "stop-request".into();
    db.accept(&previous, &prior_receipt).unwrap();
    db.set_status("stop", OperationStatus::Failed, "stop")
        .unwrap();
    assert!(service.run(&intent, &receipt, true).await.is_err());
    assert_eq!(db.source_revision("scope", "source"), Ok(Some(1)));
}

#[test]
fn incomplete_slot_checks_and_unfinished_steps_cannot_release_reservations() {
    let (_root, db, template) = store();
    setup_source(&db, &template);
    let (intent, receipt) = request();
    db.accept(&intent, &receipt).unwrap();
    db.set_status("delete", OperationStatus::Executing, "check")
        .unwrap();
    assert!(db.complete_delete(&receipt, &[]).is_err());
    let check = StorageCheck {
        slot: "data".into(),
        presence: StoragePresence::Missing,
    };
    assert!(
        db.complete_delete(&receipt, &[check.clone(), check.clone()])
            .is_err()
    );
    db.record_step(&StepIntent {
        operation_id: "delete".into(),
        sequence: 1,
        attempt: 1,
        command_kind: StepCommand::Observe,
        resource_id: "source".into(),
        expected_result: ExpectedResult::StateObserved,
    })
    .unwrap();
    assert!(
        db.complete_delete(&receipt, std::slice::from_ref(&check))
            .is_err()
    );
    db.finish_step("delete", 1, StepOutcome::Succeeded).unwrap();
    db.write(|db| {
        db.execute_batch("CREATE TRIGGER reject_retirement BEFORE UPDATE OF lifecycle ON instances WHEN NEW.lifecycle='retired' BEGIN SELECT RAISE(ABORT, 'injected'); END;")?;
        Ok(())
    }).unwrap();
    assert!(db.complete_delete(&receipt, &[check]).is_err());
    assert_eq!(
        db.storage_allocation("source", "data")
            .unwrap()
            .unwrap()
            .presence,
        StoragePresence::NotMaterialized
    );
    assert_eq!(db.source_revision("scope", "source"), Ok(Some(2)));
}
