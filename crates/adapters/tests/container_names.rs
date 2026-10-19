use composenest_application::{
    create_plan::CreatePlans,
    state_store::{StateStore, StoreConflict},
};
use std::{cell::Cell, collections::BTreeMap};
mod support;
use support::*;

#[test]
fn container_name_edits_are_validated_and_advance_the_reviewed_revision() {
    let (_root, db, template) = store();
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = CreatePlans::default();
    let initial = plans
        .prepare_create(&request(&template), &db, &clock, &mut random, &FreePorts)
        .unwrap();
    let mut change = edit(initial.plan_revision, None, BTreeMap::new());
    change.container_name = Some("invalid name".into());
    let invalid = plans
        .update_plan(
            "scope",
            &initial.plan_id,
            change,
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    assert!(
        invalid
            .concerns
            .iter()
            .any(|concern| concern.code == "CONTAINER_NAME_INVALID")
    );
    let mut change = edit(invalid.plan_revision, None, BTreeMap::new());
    change.container_name = Some("my-db_1.test".into());
    let valid = plans
        .update_plan(
            "scope",
            &initial.plan_id,
            change,
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    assert_eq!(valid.container_name, "my-db_1.test");
    assert!(valid.concerns.is_empty());
    assert!(valid.plan_revision > invalid.plan_revision);
}

#[test]
fn active_container_names_are_reserved_per_target_and_legacy_records_remain_unnamed() {
    let (_root, db, template) = store();
    setup_source(&db, &template);
    let saved: Option<String> = db
        .read(|db| {
            Ok(db.query_row(
                "SELECT container_name FROM instances WHERE id='source'",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert!(saved.is_none());
    let mut first = source_instance(&template);
    first.id = "first".into();
    first.name = "First".into();
    first.project_name = "cn-first".into();
    first.container_name = Some("custom-db".into());
    first.ports[0].host_port = 5433;
    first.storage[0].resource_identity.push_str("-first");
    db.commit_instance(&first).unwrap();
    let mut second = first.clone();
    second.id = "second".into();
    second.name = "Second".into();
    second.project_name = "cn-second".into();
    second.ports[0].host_port = 5434;
    second.storage[0].resource_identity.push_str("-second");
    assert_eq!(db.commit_instance(&second), Err(StoreConflict::Duplicate));
    second.container_name = Some("different-db".into());
    db.commit_instance(&second).unwrap();
}
