use std::{cell::Cell, collections::BTreeMap};

use composenest_application::{
    create_plan::{CreatePlans, get_default_storage, set_default_storage},
    state_store::StorageMethod,
};
use serde_json::json;

mod support;
use support::*;

#[test]
fn version_switch_preserves_candidates_and_revalidates_only_active_definition() {
    let (_root, store, template) = store();
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = CreatePlans::default();
    let first = plans
        .prepare_create(&request(&template), &store, &clock, &mut random, &FreePorts)
        .unwrap();
    assert_eq!(first.version, "1");
    assert_eq!(first.display_name, "Database");
    assert_eq!(first.storage_method, StorageMethod::Bind);
    assert_eq!(first.ports["db"], 5432);
    assert_eq!(first.storage_slots, ["data"]);
    assert_eq!(first.template_form.name, "Test");
    assert_eq!(first.template_form.template_version, "1.0.0");
    assert_eq!(first.template_form.ports[0].label, "DB");
    assert!(first.concerns.is_empty());
    assert!(
        first
            .inputs
            .iter()
            .find(|input| input.key == "password")
            .unwrap()
            .has_secret
    );
    assert!(!serde_json::to_string(&first).unwrap().contains("BBBB"));
    let generated_count = random.0;
    let redraw = plans
        .update_plan(
            "scope",
            &first.plan_id,
            edit(1, None, BTreeMap::new()),
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    assert_eq!(random.0, generated_count);
    assert_eq!(redraw.plan_revision, 2);

    let next = plans
        .update_plan(
            "scope",
            &first.plan_id,
            edit(2, Some("2"), BTreeMap::new()),
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    assert!(random.0 > generated_count);
    assert_eq!(next.plan_revision, 3);
    assert_eq!(
        next.template_form
            .inputs
            .iter()
            .map(|input| input.key.as_str())
            .collect::<Vec<_>>(),
        ["retained", "fresh", "password", "added"]
    );
    assert_eq!(next.template_form.inputs[0].validation["minLength"], 6);
    assert!(!next.template_form.inputs[2].can_generate);
    assert_eq!(next.template_form.inputs[3].options[0].value, "green");
    assert_eq!(next.template_form.ports[0].key, "api");
    assert_eq!(next.template_form.storage[0].container, "/cache");
    assert_eq!(
        next.inputs
            .iter()
            .map(|input| input.key.as_str())
            .collect::<Vec<_>>(),
        ["retained", "fresh", "password", "added"]
    );
    assert_eq!(
        next.inputs
            .iter()
            .find(|input| input.key == "retained")
            .unwrap()
            .value,
        Some(json!("first"))
    );
    assert_eq!(
        next.inputs
            .iter()
            .find(|input| input.key == "added")
            .unwrap()
            .value,
        Some(json!("green"))
    );
    assert_eq!(
        next.concerns
            .iter()
            .map(|item| item.field_path.as_str())
            .collect::<Vec<_>>(),
        ["inputs.retained", "inputs.password"]
    );
    assert_eq!(
        next.ports.keys().map(String::as_str).collect::<Vec<_>>(),
        ["api"]
    );
    assert_eq!(next.storage_slots, ["cache"]);
    assert_eq!(
        plans
            .update_plan(
                "scope",
                &first.plan_id,
                edit(2, None, BTreeMap::new()),
                &clock,
                &mut random,
                &FreePorts
            )
            .unwrap_err()
            .code,
        "PLAN_STALE"
    );

    let answers = BTreeMap::from([
        ("retained".into(), json!("longer")),
        ("password".into(), json!("a".repeat(33))),
    ]);
    let ready = plans
        .update_plan(
            "scope",
            &first.plan_id,
            edit(3, None, answers),
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    assert!(ready.concerns.is_empty());
    assert!(
        !serde_json::to_string(&ready)
            .unwrap()
            .contains(&"a".repeat(33))
    );
    assert_eq!(
        plans
            .update_plan(
                "scope",
                &first.plan_id,
                edit(4, None, BTreeMap::from([("removed".into(), json!(true))])),
                &clock,
                &mut random,
                &FreePorts
            )
            .unwrap_err()
            .code,
        "UNKNOWN_INPUT"
    );
}

#[test]
fn display_name_and_secret_regeneration_are_explicit_plan_updates() {
    let (_root, store, template) = store();
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = CreatePlans::default();
    let first = plans
        .prepare_create(&request(&template), &store, &clock, &mut random, &FreePorts)
        .unwrap();
    let generated_count = random.0;

    let mut name_edit = edit(1, None, BTreeMap::new());
    name_edit.display_name = Some(" \u{3000}".into());
    let invalid = plans
        .update_plan(
            "scope",
            &first.plan_id,
            name_edit,
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    assert_eq!(invalid.concerns[0].field_path, "displayName");
    assert_eq!(random.0, generated_count);

    let mut regenerate = edit(2, None, BTreeMap::new());
    regenerate.display_name = Some(" Second ".into());
    regenerate.regenerate_secrets.push("password".into());
    let next = plans
        .update_plan(
            "scope",
            &first.plan_id,
            regenerate,
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    assert_eq!(next.display_name, "Second");
    assert!(next.concerns.is_empty());
    assert_eq!(random.0, generated_count + 1);
    assert_eq!(next.plan_revision, 3);
    assert_eq!(
        next.inputs
            .iter()
            .find(|input| input.key == "retained")
            .unwrap()
            .value,
        Some(json!("first"))
    );

    let mut invalid_action = edit(3, None, BTreeMap::new());
    invalid_action.regenerate_secrets.push("retained".into());
    assert_eq!(
        plans
            .update_plan(
                "scope",
                &first.plan_id,
                invalid_action,
                &clock,
                &mut random,
                &FreePorts
            )
            .unwrap_err()
            .code,
        "INVALID_SECRET_ACTION"
    );
    assert_eq!(random.0, generated_count + 1);
}

#[test]
fn new_secret_before_retained_secret_retries_a_duplicate_candidate() {
    let (_root, store, template) = store();
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = CreatePlans::default();
    let first = plans
        .prepare_create(&request(&template), &store, &clock, &mut random, &FreePorts)
        .unwrap();
    random.0 = 0;
    let switched = plans
        .update_plan(
            "scope",
            &first.plan_id,
            edit(1, Some("2"), BTreeMap::new()),
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    assert_eq!(
        random.0, 2,
        "the first generated candidate matches the retained secret"
    );
    assert!(
        switched
            .inputs
            .iter()
            .find(|input| input.key == "fresh")
            .unwrap()
            .has_secret
    );
    assert!(
        !serde_json::to_string(&switched)
            .unwrap()
            .contains(&"C".repeat(32))
    );
}

#[test]
fn plans_expire_and_never_reserve_or_materialize_resources() {
    let (root, store, template) = store();
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = CreatePlans::default();
    let first = plans
        .prepare_create(&request(&template), &store, &clock, &mut random, &FreePorts)
        .unwrap();
    for _ in 1..16 {
        plans
            .prepare_create(&request(&template), &store, &clock, &mut random, &FreePorts)
            .unwrap();
    }
    assert_eq!(
        plans
            .prepare_create(&request(&template), &store, &clock, &mut random, &FreePorts)
            .unwrap_err()
            .code,
        "PLAN_LIMIT"
    );
    let counts: (i64, i64) = store
        .read(|db| {
            Ok((
                db.query_row("SELECT count(*) FROM port_reservations", [], |row| {
                    row.get(0)
                })?,
                db.query_row("SELECT count(*) FROM storage_allocations", [], |row| {
                    row.get(0)
                })?,
            ))
        })
        .unwrap();
    assert_eq!(counts, (0, 0));
    assert!(!root.path().join("data").exists());
    clock.0.set(1800);
    assert_eq!(
        plans
            .discard_plan("scope", &first.plan_id, &clock)
            .unwrap_err()
            .code,
        "PLAN_NOT_FOUND"
    );
    let discarded = plans
        .prepare_create(&request(&template), &store, &clock, &mut random, &FreePorts)
        .unwrap();
    plans
        .discard_plan("scope", &discarded.plan_id, &clock)
        .unwrap();
    assert_eq!(
        plans
            .discard_plan("scope", &discarded.plan_id, &clock)
            .unwrap_err()
            .code,
        "PLAN_NOT_FOUND"
    );
}

#[test]
fn default_storage_setting_affects_future_plans_only() {
    let (_root, store, template) = store();
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = CreatePlans::default();
    let old = plans
        .prepare_create(&request(&template), &store, &clock, &mut random, &FreePorts)
        .unwrap();
    assert_eq!(
        get_default_storage(&store, "scope").unwrap(),
        StorageMethod::Bind
    );
    assert_eq!(
        set_default_storage(&store, "scope", StorageMethod::Volume).unwrap(),
        StorageMethod::Volume
    );
    let new = plans
        .prepare_create(&request(&template), &store, &clock, &mut random, &FreePorts)
        .unwrap();
    assert_eq!(old.storage_method, StorageMethod::Bind);
    assert_eq!(new.storage_method, StorageMethod::Volume);
}
