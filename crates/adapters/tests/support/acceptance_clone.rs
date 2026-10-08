use std::{cell::Cell, path::Path};

use composenest_adapters::template_package::read_packages;
use composenest_application::{
    clone_plan::{ClonePlans, PrepareClone},
    create_plan::CommitCreate,
    create_state::{ConfirmedCreate, CreateStateStore},
    state_store::{InstanceRecord, StateStore, StorageAllocation, StorageMethod},
    template_catalog::{TemplateOrigin, prepare_revision},
};

use super::{FreePorts, TestClock, TestRandom, clone_edit, store, update_clone};

// Exercise the real Clone policy and durable confirmation before generating Compose.
pub(crate) fn confirm_clone(
    package_name: &str,
    source: &InstanceRecord,
    method: StorageMethod,
    port: u16,
    bind_path: &Path,
) -> ConfirmedCreate {
    let (_root, db, _) = store();
    let packages = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/template-candidates");
    let package = read_packages(&packages, TemplateOrigin::Local)
        .unwrap()
        .into_iter()
        .map(Result::unwrap)
        .find(|package| package.name == package_name)
        .unwrap();
    let revision = prepare_revision(package).unwrap();
    db.register_template(&revision).unwrap();
    db.create_target(
        "target",
        "scope",
        "unix:///var/run/docker.sock",
        "engine",
        "linux/amd64",
    )
    .unwrap();
    let mut saved_source = source.clone();
    saved_source.template_revision_id = revision.id;
    db.commit_instance(&saved_source).unwrap();
    let before = db.clone_source("scope", &source.id).unwrap().unwrap();
    let clock = TestClock(Cell::new(0));
    let mut random = TestRandom::default();
    let mut plans = ClonePlans::default();
    let first = plans
        .prepare_clone(
            &PrepareClone {
                scope_id: "scope".into(),
                source_id: source.id.clone(),
                display_name: "受入確認用の複製先".into(),
            },
            &db,
            &clock,
            &mut random,
            &FreePorts,
        )
        .unwrap();
    assert_eq!(first.storage_method, source.storage_method);
    let mut edit = clone_edit(first.plan_revision);
    edit.storage_method = Some(method);
    edit.ports
        .insert(source.ports[0].slot.clone(), Some(port.to_string()));
    let ready = update_clone(&mut plans, &first.plan_id, edit, &db, &clock, &mut random);
    assert!(ready.concerns.is_empty());
    let identity = match method {
        StorageMethod::Bind => bind_path.to_str().unwrap().to_owned(),
        StorageMethod::Volume => format!("cn-{}-data", ready.instance_id),
    };
    let receipt = plans
        .commit_plan(
            CommitCreate {
                plan_id: first.plan_id,
                revision: ready.plan_revision,
                scope_id: "scope".into(),
                request_id: "acceptance-clone".into(),
                target_id: "target".into(),
                instance_id: ready.instance_id,
                operation_id: "acceptance-operation".into(),
                confirmed_ports: ready.ports,
                storage: vec![StorageAllocation {
                    slot: "data".into(),
                    resource_identity: identity,
                    ownership_evidence: "clone-proof".into(),
                }],
            },
            &db,
            &clock,
            &FreePorts,
        )
        .unwrap();
    let confirmed = db.confirmed_create(&receipt).unwrap();
    assert_ne!(confirmed.instance_id, source.id);
    assert_ne!(confirmed.project_name, source.project_name);
    assert_ne!(confirmed.ports[0].host_port, source.ports[0].host_port);
    assert_ne!(
        confirmed.storage[0].allocation.resource_identity,
        source.storage[0].resource_identity
    );
    let after = db.clone_source("scope", &source.id).unwrap().unwrap();
    assert_eq!(before.inputs_json, after.inputs_json);
    assert_eq!(before.revision, after.revision);
    assert_eq!(
        before.storage[0].resource_identity,
        after.storage[0].resource_identity
    );
    confirmed
}

pub(crate) fn source_record(
    id: composenest_domain::identity::InstanceId,
    version: &str,
    inputs: &std::collections::BTreeMap<String, composenest_domain::compose::InputValue>,
    port: composenest_application::state_store::PortAllocation,
    bind: bool,
    identity: String,
) -> InstanceRecord {
    let values = inputs
        .iter()
        .map(|(key, value)| {
            let composenest_domain::compose::InputValue::String(value) = value else {
                unreachable!()
            };
            (key, value)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    InstanceRecord {
        id: format!("{:032x}", id.as_u128()),
        scope_id: "scope".into(),
        target_id: "target".into(),
        name: "受入確認用の元環境".into(),
        project_name: id.compose_project_name(),
        clone_source_id: None,
        template_revision_id: String::new(),
        selected_version: version.into(),
        storage_method: if bind {
            StorageMethod::Bind
        } else {
            StorageMethod::Volume
        },
        inputs_json: serde_json::to_string(&values).unwrap(),
        ports: vec![port],
        storage: vec![StorageAllocation {
            slot: "data".into(),
            resource_identity: identity,
            ownership_evidence: "source-proof".into(),
        }],
    }
}
