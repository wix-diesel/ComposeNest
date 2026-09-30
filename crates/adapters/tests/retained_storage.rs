mod support;
use composenest_adapters::{
    delete_stages::refresh_retained_storage, docker_target::DockerProbe,
    retained_storage::inspect_saved_bind, sqlite::DatabaseWorker, storage::BindStorage,
};
use composenest_application::{
    delete_operation::{DeleteState, StorageCheck},
    operation_journal::{
        OperationIntent, OperationJournal, OperationKind, OperationStatus, RequestReceipt,
    },
    query_service::QueryStore,
    retained_storage::RetainedStore,
    state_store::StateStore,
    storage::StoragePort,
};
use composenest_domain::{
    identity::{InstanceId, SlotId},
    instance::StoragePresence,
};

const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn fixture() -> (tempfile::TempDir, DatabaseWorker) {
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
    record.id = ID.into();
    record.project_name = format!("cn-{ID}");
    record.storage[0].resource_identity = format!("data/{ID}/data");
    record.storage[0].ownership_evidence.clear();
    db.commit_instance(&record).unwrap();
    let receipt = RequestReceipt {
        scope_id: "scope".into(),
        request_id: "delete-request".into(),
        plan_id: None,
        confirmed_revision: 1,
        request_hash: "a".repeat(64),
        instance_id: ID.into(),
        operation_id: "delete".into(),
    };
    db.accept(
        &OperationIntent {
            id: "delete".into(),
            instance_id: ID.into(),
            kind: OperationKind::Delete,
            phase: "retiring".into(),
            expected_revision: 1,
            old_spec_revision: None,
            new_spec_revision: None,
        },
        &receipt,
    )
    .unwrap();
    db.set_status("delete", OperationStatus::Executing, "check")
        .unwrap();
    db.complete_delete(
        &receipt,
        &[StorageCheck {
            slot: "data".into(),
            presence: StoragePresence::NotMaterialized,
        }],
    )
    .unwrap();
    (root, db)
}

#[test]
fn retained_views_are_scoped_masked_and_include_original_settings_and_artifact_locations() {
    let (root, db) = fixture();
    db.write(move |db| {
        db.execute("INSERT INTO artifacts (id, instance_id, spec_revision, generator_version, manifest_hash, placement) VALUES ('artifact', ?1, 1, 'v1', 'hash', 'retained')", [ID])?;
        Ok(())
    }).unwrap();
    assert!(db.list_instances("scope").unwrap().is_empty());
    assert!(db.list_retained_storage("other").unwrap().is_empty());
    let items = db.list_retained_storage("scope").unwrap();
    let item = &items[0];
    assert_eq!(item.instance.name, "Original");
    assert_eq!(item.instance.selected_version, "1");
    assert_eq!(item.locations[0].storage.presence, "not_materialized");
    assert!(!item.locations[0].ownership_verified);
    assert_eq!(item.locations[0].ownership, "retained");
    assert!(!item.deleted_at.is_empty());
    assert!(item.locations[0].observed_at.is_some());
    assert_eq!(
        item.locations[0].location,
        root.path()
            .join(format!("data/{ID}/data"))
            .to_string_lossy()
    );
    assert_eq!(
        item.artifacts[0].directory.as_deref(),
        Some(
            root.path()
                .join("instances")
                .join(ID)
                .join("artifacts")
                .join("artifact")
                .to_string_lossy()
                .as_ref()
        )
    );
    assert_eq!(
        item.settings_database,
        root.path()
            .join("state/composenest.sqlite")
            .to_string_lossy()
    );
    assert!(
        !serde_json::to_string(&items)
            .unwrap()
            .contains(&"p".repeat(32))
    );
}

#[tokio::test]
async fn fresh_checks_never_create_data_and_distinguish_never_materialized_from_unverified() {
    let (root, db) = fixture();
    let entry = db.storage_allocation(ID, "data").unwrap().unwrap();
    assert_eq!(
        inspect_saved_bind(root.path(), &entry),
        StoragePresence::NotMaterialized
    );
    assert!(!root.path().join("data").exists());
    let offline = DockerProbe {
        executable: root.path().join("missing-docker.exe"),
        directory: root.path().into(),
        config_directory: root.path().into(),
    };
    let refreshed = refresh_retained_storage(&db, &offline, "scope", ID)
        .await
        .unwrap();
    assert_eq!(refreshed.locations[0].storage.presence, "not_materialized");
    assert_eq!(refreshed.instance.runtime_status, "absent");
    std::fs::create_dir_all(root.path().join(format!("data/{ID}/data"))).unwrap();
    assert_eq!(
        inspect_saved_bind(root.path(), &entry),
        StoragePresence::Unverified
    );
    db.update_retained_checks(
        "scope",
        ID,
        3,
        &[StorageCheck {
            slot: "data".into(),
            presence: StoragePresence::Unverified,
        }],
    )
    .unwrap();
    let view = db.list_retained_storage("scope").unwrap().remove(0);
    assert_eq!(view.locations[0].storage.presence, "unverified");
    assert!(!view.locations[0].ownership_verified);
    assert!(db.update_retained_checks("other", ID, 3, &[]).is_err());
    assert!(db.update_retained_checks("scope", ID, 2, &[]).is_err());
    assert!(db.update_retained_checks("scope", ID, 3, &[]).is_err());
}

#[test]
fn owned_bind_data_is_observed_again_and_missing_data_is_never_recreated() {
    let (root, db) = fixture();
    let bind = BindStorage::new(root.path());
    let allocation = bind
        .create_bind_set(
            InstanceId::from_u128(u128::from_str_radix(ID, 16).unwrap()),
            &[SlotId::parse("data").unwrap()],
        )
        .unwrap()
        .remove(0);
    db.write(move |db| {
        db.execute("UPDATE storage_allocations SET ownership_evidence=?1, presence='present' WHERE instance_id=?2", rusqlite::params![allocation.ownership_evidence, ID])?;
        Ok(())
    }).unwrap();
    let entry = db.storage_allocation(ID, "data").unwrap().unwrap();
    let data = root.path().join(&entry.allocation.resource_identity);
    std::fs::write(data.join("sentinel"), "keep").unwrap();
    assert_eq!(
        inspect_saved_bind(root.path(), &entry),
        StoragePresence::Present
    );
    assert_eq!(
        std::fs::read_to_string(data.join("sentinel")).unwrap(),
        "keep"
    );
    std::fs::remove_file(data.join("sentinel")).unwrap();
    std::fs::remove_dir(&data).unwrap();
    assert_eq!(
        inspect_saved_bind(root.path(), &entry),
        StoragePresence::Missing
    );
    assert!(!data.exists());
}
