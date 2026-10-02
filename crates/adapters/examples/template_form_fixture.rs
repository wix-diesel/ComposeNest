//! Emits real Core previews for the browser's shared-form contract tests.

use std::{cell::Cell, collections::BTreeMap};

use composenest_application::{
    clone_plan::{ClonePlans, PrepareClone},
    create_plan::CreatePlans,
};

#[path = "../tests/support/mod.rs"]
mod support;
use support::*;

fn main() {
    let (_root, store, template) = store();
    setup_source(&store, &template);
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut creates = CreatePlans::default();
    let create = creates
        .prepare_create(&request(&template), &store, &clock, &mut random, &FreePorts)
        .unwrap();
    let create_next = creates
        .update_plan(
            "scope",
            &create.plan_id,
            edit(1, Some("2"), BTreeMap::new()),
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    let mut clones = ClonePlans::default();
    let clone = clones
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
    let mut change = clone_edit(1);
    change.version = Some("2".into());
    let clone_next = update_clone(
        &mut clones,
        &clone.plan_id,
        change,
        &store,
        &clock,
        &mut random,
    );
    println!(
        "{}",
        serde_json::json!({
            "create": create, "createNext": create_next, "clone": clone, "cloneNext": clone_next,
        })
    );
}
