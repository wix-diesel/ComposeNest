use std::cell::Cell;

use composenest_application::{
    clone_plan::{CloneAnswer, ClonePlans, PrepareClone, UpdateClone},
    create_plan::CommitCreate,
    state_store::{StateStore, StorageAllocation, StorageMethod},
};
use serde_json::json;

mod support;
use support::*;

#[test]
fn clone_version_switch_uses_snapshot_and_shows_removed_fields_and_slots() {
    let (_root, store, template) = store();
    setup_source(&store, &template);
    store
        .set_default_storage_method("scope", StorageMethod::Volume)
        .unwrap();
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = ClonePlans::default();
    let first = plans
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
    assert_eq!(first.storage_method, StorageMethod::Bind);
    assert_eq!(first.ports["db"], 5433);
    assert_eq!(first.versions, ["1", "2"]);
    assert!(first.concerns.is_empty());
    assert_eq!(
        first
            .inputs
            .iter()
            .find(|input| input.key == "retained")
            .unwrap()
            .origin,
        composenest_application::clone_plan::ValueOrigin::Inherited
    );
    assert!(
        first
            .inputs
            .iter()
            .find(|input| input.key == "password")
            .unwrap()
            .changed
    );
    let count = random.0;
    let redraw = plans
        .view_plan("scope", &first.plan_id, &store, &clock, &FreePorts)
        .unwrap();
    assert_eq!(redraw.plan_revision, 1);
    assert_eq!(random.0, count);

    store
        .write(|db| {
            db.execute("DELETE FROM template_revision_files", [])?;
            db.execute("DELETE FROM template_revisions", [])?;
            Ok(())
        })
        .unwrap();
    let mut edit = clone_edit(1);
    edit.version = Some("2".into());
    let next = update_clone(
        &mut plans,
        &first.plan_id,
        edit,
        &store,
        &clock,
        &mut random,
    );
    assert_eq!(next.added_ports, ["api"]);
    assert_eq!(
        next.template_form
            .inputs
            .iter()
            .map(|input| input.key.as_str())
            .collect::<Vec<_>>(),
        ["retained", "fresh", "password", "added"]
    );
    assert_eq!(next.template_form.ports[0].key, "api");
    assert_eq!(next.template_form.storage[0].key, "cache");
    assert_eq!(next.template_form.name, "Test");
    assert!(
        !serde_json::to_string(&next.template_form)
            .unwrap()
            .contains("source-secret")
    );
    assert_eq!(next.removed_ports, ["db"]);
    assert_eq!(next.added_storage, ["cache"]);
    assert_eq!(next.removed_storage, ["data"]);
    assert!(
        next.inputs
            .iter()
            .find(|input| input.key == "removed")
            .unwrap()
            .removed
    );
    assert!(
        next.inputs
            .iter()
            .find(|input| input.key == "added")
            .unwrap()
            .needs_answer
    );
    assert!(
        next.concerns
            .iter()
            .any(|concern| concern.field_path == "inputs.retained")
    );
    assert!(
        next.inputs
            .iter()
            .find(|input| input.key == "retained")
            .unwrap()
            .needs_answer
    );
    assert!(
        next.concerns
            .iter()
            .any(|concern| concern.field_path == "inputs.password")
    );
    assert!(
        next.inputs
            .iter()
            .find(|input| input.key == "password")
            .unwrap()
            .needs_answer
    );
    assert!(
        !serde_json::to_string(&next)
            .unwrap()
            .contains(&"p".repeat(32))
    );
    assert_eq!(
        plans
            .update_plan(
                UpdateClone {
                    scope_id: "scope".into(),
                    plan_id: first.plan_id.clone(),
                    edit: clone_edit(1)
                },
                &store,
                &clock,
                &mut random,
                &FreePorts
            )
            .unwrap_err()
            .code,
        "PLAN_STALE"
    );
}

#[test]
fn clone_secret_reuse_needs_confirmation_even_when_entered_by_hand() {
    let (_root, store, template) = store();
    setup_source(&store, &template);
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = ClonePlans::default();
    let first = plans
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
    let mut edit = clone_edit(1);
    edit.inputs
        .insert("password".into(), CloneAnswer::Input(json!("p".repeat(32))));
    let reused = update_clone(
        &mut plans,
        &first.plan_id,
        edit,
        &store,
        &clock,
        &mut random,
    );
    let password = reused
        .inputs
        .iter()
        .find(|input| input.key == "password")
        .unwrap();
    assert!(!password.changed);
    assert!(password.can_copy);
    assert_eq!(
        password.origin,
        composenest_application::clone_plan::ValueOrigin::UserInput
    );
    assert!(password.needs_secret_confirmation);
    let mut confirm = clone_edit(2);
    confirm.confirm_secrets.push("password".into());
    let ready = update_clone(
        &mut plans,
        &first.plan_id,
        confirm,
        &store,
        &clock,
        &mut random,
    );
    assert!(ready.concerns.is_empty());
    let mut switch = clone_edit(ready.plan_revision);
    switch.version = Some("2".into());
    let switched = update_clone(
        &mut plans,
        &first.plan_id,
        switch,
        &store,
        &clock,
        &mut random,
    );
    assert!(
        switched
            .inputs
            .iter()
            .find(|item| item.key == "password")
            .unwrap()
            .needs_secret_confirmation
    );

    assert!(
        !serde_json::to_string(&ready)
            .unwrap()
            .contains(&"p".repeat(32))
    );
}

#[test]
fn clone_commit_is_idempotent_and_source_revision_is_guarded() {
    let (_root, store, template) = store();
    setup_source(&store, &template);
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = ClonePlans::default();
    let first = plans
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
    let request = CommitCreate {
        plan_id: first.plan_id.clone(),
        revision: 1,
        scope_id: "scope".into(),
        request_id: "request".into(),
        target_id: "target".into(),
        instance_id: first.instance_id.clone(),
        operation_id: "clone-op".into(),
        confirmed_ports: first.ports.clone(),
        storage: vec![StorageAllocation {
            slot: "data".into(),
            resource_identity: "root/clone/data".into(),
            ownership_evidence: "clone-proof".into(),
        }],
    };
    let mut shared = request.clone();
    shared.storage[0].resource_identity = "root/source/data".into();
    assert_eq!(
        plans
            .commit_plan(shared, &store, &clock, &FreePorts)
            .unwrap_err()
            .code,
        "STORAGE_ALLOCATION_INVALID"
    );
    let mut nested = request.clone();
    nested.storage[0].resource_identity = "root/source/data/child".into();
    assert_eq!(
        plans
            .commit_plan(nested, &store, &clock, &FreePorts)
            .unwrap_err()
            .code,
        "STORAGE_ALLOCATION_INVALID"
    );
    let mut disguised = request.clone();
    disguised.storage[0].resource_identity = "root/clone/../source/data".into();
    assert_eq!(
        plans
            .commit_plan(disguised, &store, &clock, &FreePorts)
            .unwrap_err()
            .code,
        "STORAGE_ALLOCATION_INVALID"
    );
    let mut reused_proof = request.clone();
    reused_proof.storage[0].ownership_evidence = "source-proof".into();
    assert_eq!(
        plans
            .commit_plan(reused_proof, &store, &clock, &FreePorts)
            .unwrap_err()
            .code,
        "STORAGE_ALLOCATION_INVALID"
    );
    let receipt = plans
        .commit_plan(request.clone(), &store, &clock, &FreePorts)
        .unwrap();
    assert_eq!(receipt.instance_id, first.instance_id);
    let resources: (String, String, String) = store
        .read(|db| {
            Ok(db.query_row(
                "SELECT i.project_name, i.clone_source_id, a.resource_identity \
            FROM instances i JOIN storage_allocations a ON a.instance_id = i.id WHERE i.id = ?1",
                [&receipt.instance_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        })
        .unwrap();
    assert_eq!(resources.0, first.project_name);
    assert_eq!(resources.1, "source");
    assert_eq!(resources.2, "root/clone/data");
    assert_eq!(
        plans
            .commit_plan(request, &store, &clock, &FreePorts)
            .unwrap(),
        receipt
    );
    let clone = store.clone_source("scope", &receipt.instance_id).unwrap();
    assert!(clone.is_none(), "accepted operation is still unresolved");

    let second = plans
        .prepare_clone(
            &PrepareClone {
                scope_id: "scope".into(),
                source_id: "source".into(),
                display_name: "Another".into(),
            },
            &store,
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    store.rename_instance("source", 1, "Renamed").unwrap();
    let second_request = CommitCreate {
        plan_id: second.plan_id,
        revision: 1,
        scope_id: "scope".into(),
        request_id: "other".into(),
        target_id: "target".into(),
        instance_id: second.instance_id,
        operation_id: "another-op".into(),
        confirmed_ports: second.ports,
        storage: vec![StorageAllocation {
            slot: "data".into(),
            resource_identity: "root/another/data".into(),
            ownership_evidence: "another-proof".into(),
        }],
    };
    assert_eq!(
        plans
            .commit_plan(second_request, &store, &clock, &FreePorts)
            .unwrap_err()
            .code,
        "PLAN_STALE"
    );
    let committed_before: (String, String) = store
        .read(|db| {
            Ok(db.query_row(
                "SELECT s.inputs_json, t.canonical_json FROM instance_specs s \
            JOIN template_snapshots t ON t.instance_id = s.instance_id WHERE s.instance_id = ?1",
                [&receipt.instance_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();
    store.retire_instance("source", 2, true).unwrap();
    let committed_after: (String, String) = store
        .read(|db| {
            Ok(db.query_row(
                "SELECT s.inputs_json, t.canonical_json FROM instance_specs s \
            JOIN template_snapshots t ON t.instance_id = s.instance_id WHERE s.instance_id = ?1",
                [&receipt.instance_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(committed_before, committed_after);
}

#[test]
fn clone_version_change_requires_answers_and_commits_only_active_inputs() {
    let (_root, store, template) = store();
    setup_source(&store, &template);
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = ClonePlans::default();
    let first = plans
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
    store
        .write(|db| {
            db.execute("DELETE FROM template_revision_files", [])?;
            db.execute("DELETE FROM template_revisions", [])?;
            Ok(())
        })
        .unwrap();
    let mut switch = clone_edit(1);
    switch.version = Some("2".into());
    let random_before_switch = random.0;
    let next = update_clone(
        &mut plans,
        &first.plan_id,
        switch,
        &store,
        &clock,
        &mut random,
    );
    let random_after_switch = random.0;
    assert_eq!(
        random_after_switch,
        random_before_switch + 1,
        "the invalid retained secret must not be regenerated"
    );
    assert!(
        next.inputs
            .iter()
            .find(|input| input.key == "password")
            .unwrap()
            .has_secret
    );
    let mut answer = clone_edit(2);
    answer
        .inputs
        .insert("retained".into(), CloneAnswer::Input(json!("longer")));
    answer.inputs.insert(
        "password".into(),
        CloneAnswer::Input(json!("new-password".repeat(3))),
    );
    answer
        .inputs
        .insert("added".into(), CloneAnswer::Input(json!("blue")));
    let ready = update_clone(
        &mut plans,
        &first.plan_id,
        answer,
        &store,
        &clock,
        &mut random,
    );
    assert!(ready.concerns.is_empty());
    assert_eq!(random.0, random_after_switch);
    let receipt = plans
        .commit_plan(
            CommitCreate {
                plan_id: first.plan_id,
                revision: ready.plan_revision,
                scope_id: "scope".into(),
                request_id: "version-change".into(),
                target_id: "target".into(),
                instance_id: first.instance_id.clone(),
                operation_id: "version-op".into(),
                confirmed_ports: ready.ports,
                storage: vec![StorageAllocation {
                    slot: "cache".into(),
                    resource_identity: "root/clone/cache".into(),
                    ownership_evidence: "proof".into(),
                }],
            },
            &store,
            &clock,
            &FreePorts,
        )
        .unwrap();
    let (version, inputs, snapshot): (String, String, String) = store.read(|db| {
        Ok(db.query_row(
            "SELECT s.selected_version, s.inputs_json, t.canonical_json FROM instance_specs s JOIN template_snapshots t ON t.instance_id = s.instance_id WHERE s.instance_id = ?1",
            [&receipt.instance_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?)
    }).unwrap();
    let values: serde_json::Value = serde_json::from_str(&inputs).unwrap();
    assert_eq!(version, "2");
    assert_eq!(values["retained"], "longer");
    assert_eq!(values["added"], "blue");
    assert!(values.get("removed").is_none());
    assert_eq!(
        snapshot,
        store
            .clone_source("scope", "source")
            .unwrap()
            .unwrap()
            .snapshot_json
    );
}

#[test]
fn clone_plan_survives_observation_and_temporary_source_operation() {
    let (_root, store, template) = store();
    setup_source(&store, &template);
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = ClonePlans::default();
    let first = plans
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
    store.write(|db| {
        db.execute("INSERT INTO runtime_observations (instance_id, runtime_state, freshness) VALUES ('source', 'stopped', 'fresh')", [])?;
        Ok(())
    }).unwrap();
    assert!(
        plans
            .view_plan("scope", &first.plan_id, &store, &clock, &FreePorts)
            .is_ok()
    );
    store.write(|db| {
        db.execute("INSERT INTO operations (id, instance_id, kind, phase, expected_instance_revision) VALUES ('stop-op', 'source', 'stop', 'accepted', 1)", [])?;
        Ok(())
    }).unwrap();
    assert_eq!(
        plans
            .view_plan("scope", &first.plan_id, &store, &clock, &FreePorts)
            .unwrap_err()
            .code,
        "SOURCE_UNAVAILABLE"
    );
    store.write(|db| {
        db.execute("UPDATE operations SET status = 'Succeeded', completed_at = CURRENT_TIMESTAMP WHERE id = 'stop-op'", [])?;
        Ok(())
    }).unwrap();
    assert!(
        plans
            .view_plan("scope", &first.plan_id, &store, &clock, &FreePorts)
            .is_ok()
    );
}

#[test]
fn clone_port_reproposal_advances_revision_and_requires_review() {
    let (_root, store, template) = store();
    setup_source(&store, &template);
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = ClonePlans::default();
    let ports = OccupiedPort(Cell::new(0));
    let first = plans
        .prepare_clone(
            &PrepareClone {
                scope_id: "scope".into(),
                source_id: "source".into(),
                display_name: "Copy".into(),
            },
            &store,
            &clock,
            &mut random,
            &ports,
        )
        .unwrap();
    assert_eq!(first.ports["db"], 5433);
    ports.0.set(5433);
    let changed = plans
        .view_plan("scope", &first.plan_id, &store, &clock, &ports)
        .unwrap();
    assert_eq!(changed.ports["db"], 5434);
    assert_eq!(changed.plan_revision, 2);
    let redraw = plans
        .view_plan("scope", &first.plan_id, &store, &clock, &ports)
        .unwrap();
    assert_eq!(redraw.plan_revision, 2);
}

#[test]
fn clone_commit_requires_review_after_a_port_changes() {
    let (_root, store, template) = store();
    setup_source(&store, &template);
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = ClonePlans::default();
    let ports = OccupiedPort(Cell::new(0));
    let first = plans
        .prepare_clone(
            &PrepareClone {
                scope_id: "scope".into(),
                source_id: "source".into(),
                display_name: "Copy".into(),
            },
            &store,
            &clock,
            &mut random,
            &ports,
        )
        .unwrap();
    let mut request = CommitCreate {
        plan_id: first.plan_id.clone(),
        revision: first.plan_revision,
        scope_id: "scope".into(),
        request_id: "port-review".into(),
        target_id: "target".into(),
        instance_id: first.instance_id,
        operation_id: "port-review-op".into(),
        confirmed_ports: first.ports,
        storage: vec![StorageAllocation {
            slot: "data".into(),
            resource_identity: "root/clone/data".into(),
            ownership_evidence: "proof".into(),
        }],
    };
    ports.0.set(5433);
    assert_eq!(
        plans
            .commit_plan(request.clone(), &store, &clock, &ports)
            .unwrap_err()
            .code,
        "PLAN_RECONFIRM"
    );
    let changed = plans
        .view_plan("scope", &first.plan_id, &store, &clock, &ports)
        .unwrap();
    assert_eq!(changed.plan_revision, 2);
    assert_eq!(changed.ports["db"], 5434);
    request.revision = changed.plan_revision;
    request.confirmed_ports = changed.ports;
    assert_eq!(
        plans
            .commit_plan(request, &store, &clock, &ports)
            .unwrap()
            .instance_id,
        changed.instance_id
    );
}

#[test]
fn clone_ask_version_and_storage_need_explicit_answers() {
    let manifest = format!("{MANIFEST}versionClone: ask\nstorageClone: ask\n");
    let (_root, store, template) = store_with_manifest(&manifest);
    setup_source(&store, &template);
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = ClonePlans::default();
    let first = plans
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
    assert!(
        first
            .concerns
            .iter()
            .any(|concern| concern.code == "VERSION_NEEDS_ANSWER")
    );
    assert!(
        first
            .concerns
            .iter()
            .any(|concern| concern.code == "STORAGE_NEEDS_ANSWER")
    );
    let mut edit = clone_edit(1);
    edit.version = Some("1".into());
    edit.storage_method = Some(StorageMethod::Bind);
    let answered = update_clone(
        &mut plans,
        &first.plan_id,
        edit,
        &store,
        &clock,
        &mut random,
    );
    assert!(answered.concerns.is_empty());
}

#[test]
fn clone_storage_switch_keeps_source_allocation_and_uses_new_method() {
    for (source_method, clone_method) in [
        (StorageMethod::Bind, StorageMethod::Volume),
        (StorageMethod::Volume, StorageMethod::Bind),
    ] {
        let (_root, store, template) = store();
        store
            .create_target(
                "target",
                "scope",
                "unix:///var/run/docker.sock",
                "engine",
                "linux/amd64",
            )
            .unwrap();
        let mut source = source_instance(&template);
        source.storage_method = source_method;
        source.storage[0].resource_identity = if source_method == StorageMethod::Volume {
            "cn-source-data".into()
        } else {
            "root/source/data".into()
        };
        store.commit_instance(&source).unwrap();
        let clock = TestClock(Cell::new(0));
        let mut random = TestRandom::default();
        let mut plans = ClonePlans::default();
        let first = plans
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
        assert_eq!(first.storage_method, source_method);
        let mut edit = clone_edit(1);
        edit.storage_method = Some(clone_method);
        let ready = update_clone(
            &mut plans,
            &first.plan_id,
            edit,
            &store,
            &clock,
            &mut random,
        );
        assert_eq!(ready.storage_method, clone_method);
        let identity = if clone_method == StorageMethod::Volume {
            format!("cn-{}-data", ready.instance_id)
        } else {
            format!("root/{}/data", ready.instance_id)
        };
        if clone_method == StorageMethod::Volume {
            let invalid = CommitCreate {
                plan_id: first.plan_id.clone(),
                revision: ready.plan_revision,
                scope_id: "scope".into(),
                request_id: "wrong-volume".into(),
                target_id: "target".into(),
                instance_id: ready.instance_id.clone(),
                operation_id: "wrong-volume-op".into(),
                confirmed_ports: ready.ports.clone(),
                storage: vec![StorageAllocation {
                    slot: "data".into(),
                    resource_identity: "unrelated-volume".into(),
                    ownership_evidence: "proof".into(),
                }],
            };
            assert_eq!(
                plans
                    .commit_plan(invalid, &store, &clock, &FreePorts)
                    .unwrap_err()
                    .code,
                "STORAGE_ALLOCATION_INVALID"
            );
        }
        let receipt = plans
            .commit_plan(
                CommitCreate {
                    plan_id: first.plan_id,
                    revision: ready.plan_revision,
                    scope_id: "scope".into(),
                    request_id: "switch".into(),
                    target_id: "target".into(),
                    instance_id: ready.instance_id,
                    operation_id: "switch-op".into(),
                    confirmed_ports: ready.ports,
                    storage: vec![StorageAllocation {
                        slot: "data".into(),
                        resource_identity: identity.clone(),
                        ownership_evidence: "proof".into(),
                    }],
                },
                &store,
                &clock,
                &FreePorts,
            )
            .unwrap();
        let clone_storage = store
            .storage_allocation(&receipt.instance_id, "data")
            .unwrap()
            .unwrap();
        let source_storage = store.storage_allocation("source", "data").unwrap().unwrap();
        assert_eq!(clone_storage.method, clone_method);
        assert_eq!(clone_storage.allocation.resource_identity, identity);
        assert_eq!(source_storage.method, source_method);
        assert_eq!(
            source_storage.allocation.resource_identity,
            source.storage[0].resource_identity
        );
    }
}

#[test]
fn null_input_is_rejected_and_clear_omits_optional_value() {
    let (_root, store, template) = store();
    setup_source(&store, &template);
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = ClonePlans::default();
    let first = plans
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
    let mut invalid = clone_edit(1);
    invalid
        .inputs
        .insert("removed".into(), CloneAnswer::Input(json!(null)));
    assert_eq!(
        plans
            .update_plan(
                UpdateClone {
                    scope_id: "scope".into(),
                    plan_id: first.plan_id.clone(),
                    edit: invalid
                },
                &store,
                &clock,
                &mut random,
                &FreePorts
            )
            .unwrap_err()
            .code,
        "INPUT_REQUIRED_OR_INVALID"
    );
    let mut clear = clone_edit(1);
    clear.inputs.insert("removed".into(), CloneAnswer::Clear);
    let ready = update_clone(
        &mut plans,
        &first.plan_id,
        clear,
        &store,
        &clock,
        &mut random,
    );
    assert!(ready.concerns.is_empty());
    let receipt = plans
        .commit_plan(
            CommitCreate {
                plan_id: first.plan_id,
                revision: ready.plan_revision,
                scope_id: "scope".into(),
                request_id: "clear".into(),
                target_id: "target".into(),
                instance_id: ready.instance_id,
                operation_id: "clear-op".into(),
                confirmed_ports: ready.ports,
                storage: vec![StorageAllocation {
                    slot: "data".into(),
                    resource_identity: "root/cleared/data".into(),
                    ownership_evidence: "proof".into(),
                }],
            },
            &store,
            &clock,
            &FreePorts,
        )
        .unwrap();
    let inputs: String = store
        .read(|db| {
            Ok(db.query_row(
                "SELECT inputs_json FROM instance_specs WHERE instance_id = ?1",
                [&receipt.instance_id],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    let values: serde_json::Value = serde_json::from_str(&inputs).unwrap();
    assert!(values.get("removed").is_none());
}

#[test]
fn committed_spec_change_stales_plan_even_without_instance_revision_change() {
    let (_root, store, template) = store();
    setup_source(&store, &template);
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = ClonePlans::default();
    let first = plans
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
    store.write(|db| {
        db.execute("INSERT INTO instance_specs VALUES ('source', 2, '1', 'bind', '{}')", [])?;
        db.execute("INSERT INTO operations (id, instance_id, kind, status, phase, expected_instance_revision, old_spec_revision, new_spec_revision, completed_at) \
            VALUES ('updated', 'source', 'edit_port', 'Succeeded', 'done', 1, 1, 2, CURRENT_TIMESTAMP)", [])?;
        Ok(())
    }).unwrap();
    assert_eq!(store.source_revision("scope", "source").unwrap(), Some(1));
    assert_eq!(
        plans
            .view_plan("scope", &first.plan_id, &store, &clock, &FreePorts)
            .unwrap_err()
            .code,
        "PLAN_STALE"
    );
}
