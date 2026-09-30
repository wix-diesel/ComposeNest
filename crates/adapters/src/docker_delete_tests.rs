use super::*;
use composenest_application::{
    operation_journal::{OperationIntent, OperationJournal, OperationKind},
    state_store::StateStore,
};
use serde_json::json;
use std::sync::Mutex;
#[path = "../tests/support/mod.rs"]
mod support;

const INSTANCE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CONTAINER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NETWORK: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

struct Fake {
    state: Mutex<(bool, bool, bool)>,
    changes: Mutex<Vec<Vec<String>>>,
    foreign: bool,
    outsider: bool,
    unavailable: bool,
    change_fails: bool,
}
impl Fake {
    fn new() -> Self {
        Self {
            state: Mutex::new((true, true, true)),
            changes: Mutex::new(vec![]),
            foreign: false,
            outsider: false,
            unavailable: false,
            change_fails: false,
        }
    }
}
impl RemovalPort for Fake {
    async fn read(&self, args: &[OsString]) -> Result<Vec<u8>, Error> {
        if self.unavailable {
            return Err(Error::OutcomeUnknown);
        }
        let args: Vec<_> = args.iter().map(|arg| arg.to_str().unwrap()).collect();
        let state = self.state.lock().unwrap();
        let container = args[0] == "container";
        let present = if container { state.0 } else { state.1 };
        let id = if container { CONTAINER } else { NETWORK };
        if args[1] == "ls" {
            return Ok(if present {
                format!("{id}\n").into_bytes()
            } else {
                vec![]
            });
        }
        let mut labels = json!({"com.docker.compose.project": format!("cn-{INSTANCE}"), "io.composenest.scope": "scope", "io.composenest.instance": INSTANCE,
            "com.docker.compose.service": "main", "com.docker.compose.network": "default"});
        if self.foreign {
            labels["io.composenest.scope"] = json!("other");
        }
        Ok(if container { json!({"Id": id, "Labels": labels, "Status": if state.2 { "running" } else { "exited" }}) }
            else { json!({"Id": id, "Name": format!("cn-{INSTANCE}_default"), "Labels": labels, "Containers":
                if self.outsider { json!({"foreign-container": {}}) } else if state.0 { json!({CONTAINER: {}}) } else { json!({}) } }) }
            .to_string().into_bytes())
    }
    async fn change(&self, args: &[OsString]) -> Result<(), Error> {
        let args: Vec<String> = args
            .iter()
            .map(|arg| arg.to_str().unwrap().into())
            .collect();
        self.changes.lock().unwrap().push(args.clone());
        if self.change_fails {
            return Err(Error::OutcomeUnknown);
        }
        let mut state = self.state.lock().unwrap();
        match (args[0].as_str(), args[1].as_str()) {
            ("container", "stop") => state.2 = false,
            ("container", "rm") => state.0 = false,
            ("network", "rm") => state.1 = false,
            _ => panic!("unexpected changing command"),
        }
        Ok(())
    }
}

fn fixture() -> (
    tempfile::TempDir,
    crate::sqlite::DatabaseWorker,
    RequestReceipt,
) {
    let (root, db, template) = support::store();
    db.create_target(
        "target",
        "scope",
        "unix:///tmp/delete-test.sock",
        "engine",
        "linux/amd64",
    )
    .unwrap();
    let mut record = support::source_instance(&template);
    record.id = INSTANCE.into();
    record.project_name = format!("cn-{INSTANCE}");
    db.commit_instance(&record).unwrap();
    let receipt = RequestReceipt {
        scope_id: "scope".into(),
        request_id: "delete-request".into(),
        plan_id: None,
        confirmed_revision: 1,
        request_hash: "a".repeat(64),
        instance_id: INSTANCE.into(),
        operation_id: "delete".into(),
    };
    db.accept(
        &OperationIntent {
            id: "delete".into(),
            instance_id: INSTANCE.into(),
            kind: OperationKind::Delete,
            phase: "retiring".into(),
            expected_revision: 1,
            old_spec_revision: None,
            new_spec_revision: None,
        },
        &receipt,
    )
    .unwrap();
    (root, db, receipt)
}

#[tokio::test]
async fn removal_is_by_full_id_journaled_and_never_removes_data() {
    let (_root, db, receipt) = fixture();
    let fake = Fake::new();
    let project = format!("cn-{INSTANCE}");
    let owner = DeletionOwner {
        scope: "scope",
        instance: INSTANCE,
        project: &project,
        saved_container: Some(CONTAINER),
    };
    remove_runtime(&fake, &db, &receipt, &owner).await.unwrap();
    let changes = fake.changes.lock().unwrap().clone();
    assert_eq!(
        changes,
        vec![
            vec!["container", "stop", "--time", "30", CONTAINER],
            vec!["container", "rm", CONTAINER],
            vec!["network", "rm", NETWORK]
        ]
    );
    let op = db.recoverable("delete").unwrap();
    assert_eq!(op.steps.len(), 3);
    assert!(
        op.steps
            .iter()
            .all(|step| step.outcome == Some(StepOutcome::Succeeded))
    );
    assert_eq!(op.steps[2].command_kind, StepCommand::RemoveNetwork);
    // Fresh empty listings permit safe completion after a crash following resource removal.
    remove_runtime(&fake, &db, &receipt, &owner).await.unwrap();
    assert_eq!(fake.changes.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn foreign_owner_foreign_endpoint_and_unavailable_engine_send_no_changes() {
    for case in 0..3 {
        let (_root, db, receipt) = fixture();
        let mut fake = Fake::new();
        fake.foreign = case == 0;
        fake.outsider = case == 1;
        fake.unavailable = case == 2;
        let project = format!("cn-{INSTANCE}");
        let owner = DeletionOwner {
            scope: "scope",
            instance: INSTANCE,
            project: &project,
            saved_container: Some(CONTAINER),
        };
        assert!(remove_runtime(&fake, &db, &receipt, &owner).await.is_err());
        assert!(fake.changes.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn uncertain_stop_halts_before_removal_and_keeps_its_effect_in_the_journal() {
    let (_root, db, receipt) = fixture();
    let mut fake = Fake::new();
    fake.change_fails = true;
    let project = format!("cn-{INSTANCE}");
    let owner = DeletionOwner {
        scope: "scope",
        instance: INSTANCE,
        project: &project,
        saved_container: Some(CONTAINER),
    };
    assert_eq!(
        remove_runtime(&fake, &db, &receipt, &owner).await,
        Err(Error::OutcomeUnknown)
    );
    assert_eq!(fake.changes.lock().unwrap().len(), 1);
    assert_eq!(
        db.recoverable("delete").unwrap().steps[0].outcome,
        Some(StepOutcome::Unknown)
    );
}
