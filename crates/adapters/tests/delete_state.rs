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

struct CrashStages<'a>(&'a composenest_adapters::sqlite::DatabaseWorker);
impl DeleteStages for CrashStages<'_> {
    async fn remove_runtime(&self, receipt: &RequestReceipt) -> Result<(), LifecycleEffectError> {
        for (sequence, name, command) in [
            (1, "container", StepCommand::RemoveContainer),
            (2, "network", StepCommand::RemoveNetwork),
        ] {
            self.0
                .record_step(&StepIntent {
                    operation_id: receipt.operation_id.clone(),
                    sequence,
                    attempt: 1,
                    command_kind: command,
                    resource_id: name.into(),
                    expected_result: ExpectedResult::ContainerAbsent,
                })
                .unwrap();
            support::interruption::checkpoint(&format!("before:{name}"));
            std::fs::write(self.0.management_root().join(name), "absent").unwrap();
            support::interruption::checkpoint(&format!("after:{name}"));
            self.0
                .finish_step(&receipt.operation_id, sequence, StepOutcome::Succeeded)
                .unwrap();
        }
        Ok(())
    }
    async fn inspect_storage(&self) -> Result<Vec<StorageCheck>, LifecycleEffectError> {
        support::interruption::checkpoint("before:storage");
        let checks = vec![StorageCheck {
            slot: "data".into(),
            presence: StoragePresence::Present,
        }];
        support::interruption::checkpoint("after:storage");
        Ok(checks)
    }
    async fn runtime_absent(&self) -> Result<(), LifecycleEffectError> {
        support::interruption::checkpoint("before:absence");
        support::interruption::checkpoint("after:absence");
        support::interruption::checkpoint("before:commit");
        if std::env::var("COMPOSENEST_CRASH_BOUNDARY").as_deref() == Ok("full") {
            support::interruption::full_at_completion(self.0);
        }
        Ok(())
    }
}

#[tokio::test]
async fn delete_interruption_worker() {
    let Some(root) = std::env::var_os("COMPOSENEST_CRASH_ROOT") else {
        return;
    };
    let db =
        composenest_adapters::sqlite::DatabaseWorker::start(std::path::Path::new(&root)).unwrap();
    let (intent, receipt) = request();
    let result = DeleteOperation {
        state: &db,
        runner: &OperationRunner::new(),
        stages: &CrashStages(&db),
    }
    .run(&intent, &receipt, true)
    .await;
    if result.is_ok() {
        support::interruption::checkpoint("after:commit");
    }
    assert_eq!(result, Err(DeleteError::Store(StoreConflict::Backend)));
    support::interruption::checkpoint("full");
    panic!("crash boundary was not reached");
}

#[test]
fn delete_crashes_never_release_reservations_before_atomic_retirement() {
    use composenest_application::operation_recovery::RecoveryJournal;
    for boundary in [
        "before:container",
        "after:container",
        "before:network",
        "after:network",
        "before:storage",
        "after:storage",
        "before:absence",
        "after:absence",
        "before:commit",
        "after:commit",
        "full",
    ] {
        let (root, db, template) = store();
        setup_source(&db, &template);
        std::fs::write(root.path().join("data-sentinel"), "retained").unwrap();
        drop(db);
        support::interruption::child("delete_interruption_worker", root.path(), boundary);
        let db = composenest_adapters::sqlite::DatabaseWorker::start(root.path()).unwrap();
        let operations = db.recover_on_startup().unwrap();
        let completed = boundary == "after:commit";
        assert_eq!(operations.len(), usize::from(!completed));
        db.read(|db| {
            let (lifecycle, reservation, secret): (String, String, String) = db.query_row(
                "SELECT i.lifecycle, p.status, s.inputs_json FROM instances i JOIN port_reservations p ON p.instance_id=i.id JOIN instance_specs s ON s.instance_id=i.id WHERE i.id='source'", [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            assert_eq!(lifecycle, if completed { "retired" } else { "retiring" });
            assert_eq!(reservation, if completed { "released" } else { "committed" });
            assert!(secret.contains(&"p".repeat(32)));
            Ok(())
        }).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("data-sentinel")).unwrap(),
            "retained"
        );
        if boundary == "full" {
            support::interruption::restore_capacity(&db);
            let (_, receipt) = request();
            db.set_status("delete", OperationStatus::Executing, "reconcile")
                .unwrap();
            db.complete_delete(
                &receipt,
                &[StorageCheck {
                    slot: "data".into(),
                    presence: StoragePresence::Present,
                }],
            )
            .unwrap();
            assert!(!db.delete_pending(&receipt).unwrap());
        }
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

#[tokio::test]
async fn desktop_delete_requires_confirmation_replays_and_uses_reserved_capacity() {
    use composenest_application::{
        RequestContext,
        instance_actions::{self, ChangeInstanceRequest},
        query_service::{QueryService, QueryStore},
    };
    use std::{sync::Arc, time::Duration};
    let (_root, db, template) = store();
    setup_source(&db, &template);
    let mut request = ChangeInstanceRequest {
        context: RequestContext {
            api_version: 1,
            request_id: "desktop-delete".into(),
        },
        instance_id: "source".into(),
        expected_revision: 1,
        action: "delete".into(),
        retain_data_confirmed: false,
    };
    let accept = |scope: &str, request: &ChangeInstanceRequest| {
        instance_actions::accept(&db, scope, request, &mut composenest_adapters::SystemRandom)
    };
    assert!(matches!(
        accept("scope", &request),
        Err(StoreConflict::InvalidInput)
    ));
    assert!(db.receipt("scope", "desktop-delete").unwrap().is_none());
    request.retain_data_confirmed = true;
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
    let runner = Arc::new(OperationRunner::new());
    let reservation = runner.reserve().unwrap();
    let (receipt, kind, fresh) = accept("scope", &request).unwrap();
    assert!(fresh);
    assert_eq!(kind, OperationKind::Delete);
    assert!(
        instance_actions::view(&db, "scope", "source")
            .unwrap()
            .actions
            .is_empty()
    );
    assert_eq!(
        QueryService::new(&db)
            .list_instances("scope")
            .unwrap()
            .len(),
        1
    );
    let (repeated, _, fresh) = accept("scope", &request).unwrap();
    assert_eq!(repeated, receipt);
    assert!(!fresh);
    request.context.request_id = "different-request".into();
    request.expected_revision = 2;
    assert!(matches!(
        accept("scope", &request),
        Err(StoreConflict::UnresolvedOperation)
    ));
    request.context.request_id = "desktop-delete".into();
    request.expected_revision = 1;
    assert!(!runner.shutdown(Duration::ZERO));
    let intent = OperationIntent {
        id: receipt.operation_id.clone(),
        instance_id: receipt.instance_id.clone(),
        kind,
        phase: "inspect".into(),
        expected_revision: 1,
        old_spec_revision: None,
        new_spec_revision: None,
    };
    let stages = Stages(Ok(()), StoragePresence::Present);
    assert_eq!(
        DeleteOperation {
            state: &db,
            runner: &runner,
            stages: &stages
        }
        .run_reserved(&intent, &receipt, true, reservation)
        .await,
        Ok(receipt.clone())
    );
    assert!(runner.shutdown(Duration::ZERO));
    assert!(db.list_instances("scope").unwrap().is_empty());
    let progress =
        composenest_adapters::query_service::view_operation(&db, "scope", &receipt.operation_id)
            .unwrap();
    assert_eq!(progress.instance.lifecycle, "retired");
    assert_eq!(progress.operation.status, "Succeeded");
    assert!(progress.completed_at.is_some());
    let (repeated, _, fresh) = accept("scope", &request).unwrap();
    assert_eq!(repeated, receipt);
    assert!(!fresh);
    request.retain_data_confirmed = false;
    assert!(matches!(
        accept("scope", &request),
        Err(StoreConflict::InvalidInput)
    ));
}
