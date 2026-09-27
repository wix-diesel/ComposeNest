use std::{cell::Cell, collections::BTreeMap, fs, time::SystemTime};

use composenest_adapters::sqlite::DatabaseWorker;
use composenest_application::{
    Clock,
    create_plan::{CreatePlans, PlanEdit, PrepareCreate, get_default_storage, set_default_storage},
    host_ports::{PortCheck, PortInspector},
    state_store::{StateStore, StorageMethod, TemplateFile},
    template_catalog::{TemplateOrigin, TemplatePackage, prepare_revision},
};
use composenest_domain::clone_policy::{RandomError, RandomSource};
use serde_json::json;

const MANIFEST: &str = "schemaVersion: 1\nid: example.test\ntemplateVersion: \"1.0.0\"\nname: Test\ndescription: Test\ndefaultVersion: \"1\"\nversions:\n  \"1\": versions/1.yaml\n  \"2\": versions/2.yaml\n";
const V1: &str = "image: example:1\nplatforms: [linux/amd64]\ninputs:\n  retained:\n    label: Retained\n    type: string\n    default: first\n  removed:\n    label: Removed\n    type: boolean\n    default: true\n  password:\n    label: Password\n    type: secret\nservice:\n  environment:\n    RETAINED: { input: retained }\n    PASSWORD: { input: password }\n  ports:\n    db:\n      label: DB\n      container: 5432\n      defaultHost: 5432\n  storage:\n    data:\n      label: Data\n      container: /data\n  healthcheck:\n    command: [check]\n";
const V2: &str = "image: example:2\nplatforms: [linux/amd64]\ninputs:\n  retained:\n    label: Retained\n    type: string\n    default: second\n    validation: { minLength: 6 }\n  password:\n    label: Password\n    type: secret\n    initial: ask\n    clone: ask\n    validation: { minLength: 33 }\n  added:\n    label: Added\n    type: select\n    default: green\n    options:\n      - { value: green, label: Green }\n      - { value: blue, label: Blue }\nservice:\n  environment:\n    RETAINED: { input: retained }\n    PASSWORD: { input: password }\n    ADDED: { input: added }\n  ports:\n    api:\n      label: API\n      container: 9000\n      defaultHost: 9000\n  storage:\n    cache:\n      label: Cache\n      container: /cache\n  healthcheck:\n    command: [check]\n";

struct TestClock(Cell<u64>);
impl Clock for TestClock {
    fn unix_seconds(&self) -> u64 {
        self.0.get()
    }
}

#[derive(Default)]
struct TestRandom(u8);
impl RandomSource for TestRandom {
    fn fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), RandomError> {
        self.0 += 1;
        bytes.fill(self.0);
        Ok(())
    }
}

struct FreePorts;
impl PortInspector for FreePorts {
    fn inspect(&self, _port: u16) -> PortCheck {
        PortCheck {
            result: Ok(()),
            observed_at: SystemTime::UNIX_EPOCH,
        }
    }
}

fn store() -> (tempfile::TempDir, DatabaseWorker, String) {
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
        ("template.yaml", MANIFEST),
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

fn request(template: &str) -> PrepareCreate {
    PrepareCreate {
        scope_id: "scope".into(),
        template_revision_id: template.into(),
        version: None,
    }
}

fn edit(
    revision: u64,
    version: Option<&str>,
    inputs: BTreeMap<String, serde_json::Value>,
) -> PlanEdit {
    PlanEdit {
        expected_revision: revision,
        version: version.map(str::to_owned),
        storage_method: None,
        inputs,
        ports: BTreeMap::new(),
    }
}

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
    assert_eq!(first.storage_method, StorageMethod::Bind);
    assert_eq!(first.ports["db"], 5432);
    assert_eq!(first.storage_slots, ["data"]);
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
    assert_eq!(random.0, generated_count);
    assert_eq!(next.plan_revision, 3);
    assert_eq!(
        next.inputs
            .iter()
            .map(|input| input.key.as_str())
            .collect::<Vec<_>>(),
        ["retained", "password", "added"]
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
