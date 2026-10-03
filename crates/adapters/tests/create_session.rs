use composenest_application::{
    RequestContext,
    create_session::{ConfirmCreateRequest, CreateReceipt, CreateSession},
    state_store::StateStore,
};
use std::{cell::Cell, collections::BTreeMap};
#[path = "support/mod.rs"]
mod support;
use support::*;

fn confirmation(id: &str, revision: u64, ports: BTreeMap<String, u16>) -> ConfirmCreateRequest {
    ConfirmCreateRequest {
        context: RequestContext {
            api_version: 1,
            request_id: "request".into(),
        },
        plan_id: id.into(),
        revision,
        confirmed_ports: ports,
        accept_plaintext: true,
    }
}

#[test]
fn cancel_and_confirmation_guards_never_allocate_resources() {
    let (root, store, template) = store();
    store
        .create_target(
            "target",
            "scope",
            "unix:///tmp/test.sock",
            "engine",
            "linux/amd64",
        )
        .unwrap();
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut session = CreateSession::new("scope".into());
    let plan = session
        .prepare(template, &store, &clock, &mut random, &FreePorts)
        .unwrap();
    assert_eq!(plan.template_origin, "local");
    assert!(
        plan.inputs
            .iter()
            .filter(|input| input.has_secret)
            .all(|input| input.value.is_none())
    );
    let mut change = edit(1, None, BTreeMap::new());
    change.display_name = Some("Database".into());
    let plan = session
        .update(&plan.plan_id, change, &clock, &mut random, &FreePorts)
        .unwrap();
    let mut request = confirmation(&plan.plan_id, plan.plan_revision, plan.ports.clone());
    request.accept_plaintext = false;
    assert_eq!(
        session
            .confirm(request, &store, &clock, &mut random, &FreePorts)
            .unwrap_err()
            .code,
        "PLAINTEXT_CONFIRMATION_REQUIRED"
    );
    assert_eq!(
        session
            .confirm(
                confirmation(&plan.plan_id, 1, plan.ports.clone()),
                &store,
                &clock,
                &mut random,
                &FreePorts
            )
            .unwrap_err()
            .code,
        "PLAN_STALE"
    );
    assert_eq!(
        session
            .confirm(
                confirmation(&plan.plan_id, 2, BTreeMap::new()),
                &store,
                &clock,
                &mut random,
                &FreePorts
            )
            .unwrap_err()
            .code,
        "PLAN_RECONFIRM"
    );
    assert!(session.receipt(&plan.plan_id, &store).unwrap().is_none());
    session.discard(&plan.plan_id, &store, &clock).unwrap();
    assert_eq!(
        session
            .view(&plan.plan_id, &clock, &FreePorts)
            .unwrap_err()
            .code,
        "PLAN_NOT_FOUND"
    );
    let count = store
        .read(|db| {
            Ok(db.query_row("SELECT count(*) FROM instances", [], |row| {
                row.get::<_, i64>(0)
            })?)
        })
        .unwrap();
    assert_eq!(count, 0);
    assert!(!root.path().join("data").exists());
    assert!(!root.path().join("ownership").exists());
}

#[test]
fn acceptance_is_reconciled_after_plan_loss_without_new_ids_or_resources() {
    let (root, store, template) = store();
    store
        .create_target(
            "target",
            "scope",
            "unix:///tmp/test.sock",
            "engine",
            "linux/amd64",
        )
        .unwrap();
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut session = CreateSession::new("scope".into());
    let initial = session
        .prepare(template, &store, &clock, &mut random, &FreePorts)
        .unwrap();
    let mut change = edit(1, None, BTreeMap::new());
    change.display_name = Some("Database".into());
    let plan = session
        .update(&initial.plan_id, change, &clock, &mut random, &FreePorts)
        .unwrap();
    let (first, fresh) = session
        .confirm(
            confirmation(&plan.plan_id, 2, plan.ports.clone()),
            &store,
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    assert!(fresh);
    let generated = random.0;
    let mut restarted = CreateSession::new("scope".into());
    let (second, fresh) = restarted
        .confirm(
            confirmation(&plan.plan_id, 2, plan.ports),
            &store,
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    assert!(!fresh);
    assert_eq!(first, second);
    assert_eq!(random.0, generated);
    assert_eq!(
        restarted.receipt(&plan.plan_id, &store).unwrap(),
        Some(first.clone())
    );
    assert_eq!(
        restarted
            .discard(&plan.plan_id, &store, &clock)
            .unwrap_err()
            .code,
        "PLAN_ALREADY_COMMITTED"
    );
    let public = serde_json::to_value(CreateReceipt::from(&first)).unwrap();
    assert_eq!(public.as_object().unwrap().len(), 4);
    assert!(public.get("requestHash").is_none());
    assert_eq!(
        CreateSession::new("other".into())
            .receipt(&plan.plan_id, &store)
            .unwrap(),
        None
    );
    assert!(!root.path().join("data").exists());
    let count = store
        .read(|db| {
            Ok(db.query_row("SELECT count(*) FROM operations", [], |row| {
                row.get::<_, i64>(0)
            })?)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn clone_acceptance_cannot_be_returned_by_create_commands() {
    use composenest_application::{
        clone_plan::{ClonePlans, PrepareClone},
        create_plan::CommitCreate,
        state_store::StorageAllocation,
    };
    let (_root, store, template) = store();
    setup_source(&store, &template);
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut clones = ClonePlans::default();
    let plan = clones
        .prepare_clone(
            &PrepareClone {
                scope_id: "scope".into(),
                source_id: "source".into(),
                display_name: "Copy".into(),
            },
            &store,
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    clones
        .commit_plan(
            CommitCreate {
                plan_id: plan.plan_id.clone(),
                revision: plan.plan_revision,
                scope_id: "scope".into(),
                request_id: "request".into(),
                target_id: "target".into(),
                instance_id: plan.instance_id,
                operation_id: "clone-op".into(),
                confirmed_ports: plan.ports.clone(),
                storage: vec![StorageAllocation {
                    slot: "data".into(),
                    resource_identity: "root/clone/data".into(),
                    ownership_evidence: "clone-proof".into(),
                }],
            },
            &store,
            &clock,
            &FreePorts,
        )
        .unwrap();
    let mut session = CreateSession::new("scope".into());
    assert_eq!(
        session.receipt(&plan.plan_id, &store).unwrap_err().code,
        "PLAN_ALREADY_COMMITTED"
    );
    for (request_id, expected) in [
        ("request", "REQUEST_ALREADY_USED"),
        ("new-request", "PLAN_ALREADY_COMMITTED"),
    ] {
        let mut request = confirmation(&plan.plan_id, plan.plan_revision, plan.ports.clone());
        request.context.request_id = request_id.into();
        assert_eq!(
            session.accepted(&request, &store).unwrap_err().code,
            expected
        );
        assert_eq!(
            session
                .confirm(request, &store, &clock, &mut random, &FreePorts)
                .unwrap_err()
                .code,
            expected
        );
    }
}
