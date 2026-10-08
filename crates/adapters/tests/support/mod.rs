// Each integration test binary uses a different subset of these shared fixtures.
#![allow(dead_code)]

pub(super) mod create_stages;
pub(super) mod interruption;

use std::{cell::Cell, collections::BTreeMap, fs, time::SystemTime};

use composenest_adapters::sqlite::DatabaseWorker;
use composenest_application::{
    Clock,
    clone_plan::{CloneEdit, ClonePlans, UpdateClone},
    create_plan::{PlanEdit, PrepareCreate},
    host_ports::{PortCheck, PortInspector, PortReason},
    state_store::{
        InstanceRecord, PortAllocation, StateStore, StorageAllocation, StorageMethod, TemplateFile,
    },
    template_catalog::{TemplateOrigin, TemplatePackage, prepare_revision},
};
use composenest_domain::clone_policy::{RandomError, RandomSource};
use serde_json::json;

pub(super) const MANIFEST: &str = "schemaVersion: 1\nid: example.test\ntemplateVersion: \"1.0.0\"\nname: Test\ndescription: Test\ndefaultVersion: \"1\"\nversions:\n  \"1\": versions/1.yaml\n  \"2\": versions/2.yaml\n";
pub(super) const V1: &str = "image: example:1\nplatforms: [linux/amd64]\ninputs:\n  retained:\n    label: Retained\n    type: string\n    default: first\n  removed:\n    label: Removed\n    type: boolean\n    required: false\n    default: true\n  password:\n    label: Password\n    type: secret\nservice:\n  environment:\n    RETAINED: { input: retained }\n    PASSWORD: { input: password }\n  ports:\n    db:\n      label: DB\n      container: 5432\n      defaultHost: 5432\n  storage:\n    data:\n      label: Data\n      container: /data\n  healthcheck:\n    command: [check]\n";
pub(super) const V2: &str = "image: example:2\nplatforms: [linux/amd64]\ninputs:\n  retained:\n    label: Retained\n    type: string\n    default: second\n    validation: { minLength: 6 }\n  fresh:\n    label: Fresh\n    type: secret\n  password:\n    label: Password\n    type: secret\n    initial: ask\n    clone: ask\n    validation: { minLength: 33 }\n  added:\n    label: Added\n    type: select\n    default: green\n    options:\n      - { value: green, label: Green }\n      - { value: blue, label: Blue }\nservice:\n  environment:\n    RETAINED: { input: retained }\n    FRESH: { input: fresh }\n    PASSWORD: { input: password }\n    ADDED: { input: added }\n  ports:\n    api:\n      label: API\n      container: 9000\n      defaultHost: 9000\n  storage:\n    cache:\n      label: Cache\n      container: /cache\n  healthcheck:\n    command: [check]\n";

pub(super) struct TestClock(pub(super) Cell<u64>);
impl Clock for TestClock {
    fn unix_seconds(&self) -> u64 {
        self.0.get()
    }
}

#[derive(Default)]
pub(super) struct TestRandom(pub(super) u8);
impl RandomSource for TestRandom {
    fn fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), RandomError> {
        self.0 += 1;
        bytes.fill(self.0);
        Ok(())
    }
}

pub(super) struct FreePorts;
impl PortInspector for FreePorts {
    fn inspect(&self, _port: u16) -> PortCheck {
        PortCheck {
            result: Ok(()),
            observed_at: SystemTime::UNIX_EPOCH,
        }
    }
}

pub(super) struct OccupiedPort(pub(super) Cell<u16>);
impl PortInspector for OccupiedPort {
    fn inspect(&self, port: u16) -> PortCheck {
        PortCheck {
            result: if port == self.0.get() {
                Err(PortReason::Host)
            } else {
                Ok(())
            },
            observed_at: SystemTime::UNIX_EPOCH,
        }
    }
}

pub(super) fn store() -> (tempfile::TempDir, DatabaseWorker, String) {
    store_with_manifest(MANIFEST)
}

pub(super) fn store_with_manifest(manifest: &str) -> (tempfile::TempDir, DatabaseWorker, String) {
    let root = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    for name in ["state", "locks"] {
        fs::create_dir(root.path().join(name)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.path().join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let store = DatabaseWorker::start(root.path()).unwrap();
    store.create_scope("scope", "owner", "root").unwrap();
    let files = [
        ("template.yaml", manifest),
        ("versions/1.yaml", V1),
        ("versions/2.yaml", V2),
    ]
    .into_iter()
    .map(|(relative_path, contents)| TemplateFile {
        relative_path: relative_path.into(),
        contents: contents.as_bytes().to_vec(),
    })
    .collect();
    let revision = prepare_revision(TemplatePackage {
        name: "test".into(),
        origin: TemplateOrigin::Local,
        files,
        warnings: Vec::new(),
    })
    .unwrap();
    let id = revision.id.clone();
    store.register_template(&revision).unwrap();
    (root, store, id)
}

pub(super) fn request(template: &str) -> PrepareCreate {
    PrepareCreate {
        scope_id: "scope".into(),
        display_name: " Database ".into(),
        template_revision_id: template.into(),
        version: None,
    }
}

pub(super) fn edit(
    revision: u64,
    version: Option<&str>,
    inputs: BTreeMap<String, serde_json::Value>,
) -> PlanEdit {
    PlanEdit {
        expected_revision: revision,
        display_name: None,
        version: version.map(str::to_owned),
        storage_method: None,
        inputs,
        regenerate_secrets: Vec::new(),
        ports: BTreeMap::new(),
    }
}

pub(super) fn source_instance(template: &str) -> InstanceRecord {
    InstanceRecord {
        id: "source".into(),
        scope_id: "scope".into(),
        target_id: "target".into(),
        name: "Original".into(),
        project_name: "cn-source".into(),
        clone_source_id: None,
        template_revision_id: template.into(),
        selected_version: "1".into(),
        storage_method: StorageMethod::Bind,
        inputs_json: serde_json::to_string(&json!({
            "retained": "first", "removed": true, "password": "p".repeat(32)
        }))
        .unwrap(),
        ports: vec![PortAllocation {
            slot: "db".into(),
            host_ip: "127.0.0.1".into(),
            host_port: 5432,
            container_port: 5432,
        }],
        storage: vec![StorageAllocation {
            slot: "data".into(),
            resource_identity: "root/source/data".into(),
            ownership_evidence: "source-proof".into(),
        }],
    }
}

pub(super) fn setup_source(store: &DatabaseWorker, template: &str) {
    store
        .create_target(
            "target",
            "scope",
            "unix:///var/run/docker.sock",
            "engine",
            "linux/amd64",
        )
        .unwrap();
    store.commit_instance(&source_instance(template)).unwrap();
}

pub(super) fn clone_edit(revision: u64) -> CloneEdit {
    CloneEdit {
        expected_revision: revision,
        display_name: None,
        version: None,
        storage_method: None,
        inputs: BTreeMap::new(),
        confirm_secrets: Vec::new(),
        ports: BTreeMap::new(),
    }
}

pub(super) fn update_clone(
    plans: &mut ClonePlans,
    id: &str,
    edit: CloneEdit,
    store: &DatabaseWorker,
    clock: &TestClock,
    random: &mut TestRandom,
) -> composenest_application::clone_plan::ClonePlanView {
    plans
        .update_plan(
            UpdateClone {
                scope_id: "scope".into(),
                plan_id: id.into(),
                edit,
            },
            store,
            clock,
            random,
            &FreePorts,
        )
        .unwrap()
}
