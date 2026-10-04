use composenest_application::{
    RequestContext,
    clone_session::{CloneSession, ConfirmCloneRequest},
    create_session::CreateSession,
    state_store::{StateStore, StorageMethod},
};
use std::cell::Cell;
mod support;
use support::*;

fn confirmation(plan: &composenest_application::clone_plan::ClonePlanView) -> ConfirmCloneRequest {
    ConfirmCloneRequest {
        context: RequestContext {
            api_version: 1,
            request_id: "clone-request".into(),
        },
        plan_id: plan.plan_id.clone(),
        revision: plan.plan_revision,
        confirmed_ports: plan.ports.clone(),
        accept_plaintext: true,
        accept_configuration_only: true,
    }
}

#[test]
fn clone_confirmation_guards_source_and_candidates_and_restores_only_clone_receipts() {
    let (root, store, template) = store();
    setup_source(&store, &template);
    store
        .set_default_storage_method("scope", StorageMethod::Volume)
        .unwrap();
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut session = CloneSession::new("scope".into());
    let first = session
        .prepare("source".into(), &store, &clock, &mut random, &FreePorts)
        .unwrap();
    assert_eq!(first.source_name, "Original");
    assert_eq!(first.storage_method, StorageMethod::Bind);
    assert_eq!(first.versions, ["1", "2"]);
    assert!(
        !serde_json::to_string(&first)
            .unwrap()
            .contains(&"p".repeat(32))
    );
    let mut edit = clone_edit(first.plan_revision);
    edit.display_name = Some("Copy".into());
    let plan = session
        .update(
            &first.plan_id,
            edit,
            &store,
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    let mut request = confirmation(&plan);
    request.accept_configuration_only = false;
    assert_eq!(
        session
            .confirm(request, &store, &clock, &mut random, &FreePorts)
            .unwrap_err()
            .code,
        "CONFIGURATION_ONLY_CONFIRMATION_REQUIRED"
    );
    let mut request = confirmation(&plan);
    request.accept_plaintext = false;
    assert_eq!(
        session
            .confirm(request, &store, &clock, &mut random, &FreePorts)
            .unwrap_err()
            .code,
        "PLAINTEXT_CONFIRMATION_REQUIRED"
    );
    let mut request = confirmation(&plan);
    request.confirmed_ports.clear();
    assert_eq!(
        session
            .confirm(request, &store, &clock, &mut random, &FreePorts)
            .unwrap_err()
            .code,
        "PLAN_RECONFIRM"
    );
    let (receipt, fresh) = session
        .confirm(confirmation(&plan), &store, &clock, &mut random, &FreePorts)
        .unwrap();
    assert!(fresh);
    assert_eq!(receipt.instance_id, plan.instance_id);
    let generated = random.0;
    let mut restarted = CloneSession::new("scope".into());
    let (same, fresh) = restarted
        .confirm(confirmation(&plan), &store, &clock, &mut random, &FreePorts)
        .unwrap();
    assert!(!fresh);
    assert_eq!(receipt, same);
    assert_eq!(generated, random.0);
    assert_eq!(
        restarted.receipt(&plan.plan_id, &store).unwrap(),
        Some(receipt.clone())
    );
    assert_eq!(
        CreateSession::new("scope".into())
            .receipt(&plan.plan_id, &store)
            .unwrap_err()
            .code,
        "PLAN_ALREADY_COMMITTED"
    );
    assert!(
        CloneSession::new("other".into())
            .receipt(&plan.plan_id, &store)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        restarted
            .discard(&plan.plan_id, &store, &clock)
            .unwrap_err()
            .code,
        "PLAN_ALREADY_COMMITTED"
    );
    let original = store.clone_source("scope", "source").unwrap().unwrap();
    assert_eq!(original.name, "Original");
    assert_eq!(original.ports[0].host_port, 5432);
    assert!(!root.path().join("data").exists());
}

#[test]
fn source_changes_and_port_reproposal_require_new_review() {
    let (_root, store, template) = store();
    setup_source(&store, &template);
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut session = CloneSession::new("scope".into());
    let first = session
        .prepare("source".into(), &store, &clock, &mut random, &FreePorts)
        .unwrap();
    let mut edit = clone_edit(first.plan_revision);
    edit.display_name = Some("Copy".into());
    let plan = session
        .update(
            &first.plan_id,
            edit,
            &store,
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    let occupied = OccupiedPort(Cell::new(plan.ports["db"]));
    assert_eq!(
        session
            .confirm(confirmation(&plan), &store, &clock, &mut random, &occupied)
            .unwrap_err()
            .code,
        "PLAN_STALE"
    );
    let current = session
        .view(&plan.plan_id, &store, &clock, &occupied)
        .unwrap();
    assert_ne!(current.ports, plan.ports);
    assert!(current.plan_revision > plan.plan_revision);
    store
        .write(|db| {
            db.execute(
                "UPDATE instances SET revision = revision + 1 WHERE id = 'source'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let error = session
        .view(&plan.plan_id, &store, &clock, &FreePorts)
        .unwrap_err();
    assert_eq!(error.code, "PLAN_STALE");
    assert_eq!(error.field_path.as_deref(), Some("sourceId"));
    session.discard(&plan.plan_id, &store, &clock).unwrap();
}

#[test]
fn source_secret_stays_masked_and_type_change_blocks_confirmation_after_explicit_answer() {
    let (_root, store, template) = store();
    setup_source(&store, &template);
    let source = store.clone_source("scope", "source").unwrap().unwrap();
    let mut snapshot: serde_json::Value = serde_json::from_str(&source.snapshot_json).unwrap();
    snapshot["versions"][1]["definition"]["inputs"]["values"]["password"] =
        serde_json::json!({"type": "string", "label": "Text", "required": true});
    store
        .write(move |db| {
            db.execute(
                "UPDATE template_snapshots SET canonical_json = ?1 WHERE instance_id = 'source'",
                [snapshot.to_string()],
            )?;
            Ok(())
        })
        .unwrap();
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut session = CloneSession::new("scope".into());
    let first = session
        .prepare("source".into(), &store, &clock, &mut random, &FreePorts)
        .unwrap();
    let mut change = clone_edit(first.plan_revision);
    change.version = Some("2".into());
    let next = session
        .update(
            &first.plan_id,
            change,
            &store,
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    assert!(
        !serde_json::to_string(&next)
            .unwrap()
            .contains(&"p".repeat(32))
    );
    let password = next
        .inputs
        .iter()
        .find(|item| item.key == "password")
        .unwrap();
    assert!(password.source_has_secret);
    assert!(!password.can_copy);
    assert!(password.needs_answer);
    let mut copy = clone_edit(next.plan_revision);
    copy.inputs.insert(
        "password".into(),
        composenest_application::clone_plan::CloneAnswer::Copy,
    );
    assert_eq!(
        session
            .update(
                &first.plan_id,
                copy,
                &store,
                &clock,
                &mut random,
                &FreePorts
            )
            .unwrap_err()
            .code,
        "INPUT_TYPE_CHANGED"
    );
    let mut answer = clone_edit(next.plan_revision);
    answer.display_name = Some("Copy".into());
    for (key, value) in [("retained", "long enough"), ("added", "blue")] {
        answer.inputs.insert(
            key.into(),
            composenest_application::clone_plan::CloneAnswer::Input(serde_json::json!(value)),
        );
    }
    answer.inputs.insert(
        "password".into(),
        composenest_application::clone_plan::CloneAnswer::Input(serde_json::json!("new text")),
    );
    let answered = session
        .update(
            &first.plan_id,
            answer,
            &store,
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    assert_eq!(answered.concerns.len(), 1);
    assert_eq!(answered.concerns[0].code, "INPUT_TYPE_CHANGED");
    assert_eq!(answered.concerns[0].field_path, "inputs.password");
    assert!(
        !serde_json::to_string(&answered)
            .unwrap()
            .contains(&"p".repeat(32))
    );
    assert_eq!(
        session
            .confirm(
                confirmation(&answered),
                &store,
                &clock,
                &mut random,
                &FreePorts
            )
            .unwrap_err()
            .code,
        "INPUT_TYPE_CHANGED"
    );
    assert!(session.receipt(&first.plan_id, &store).unwrap().is_none());
}
