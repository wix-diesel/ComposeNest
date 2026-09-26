use std::collections::BTreeMap;
use std::fs;

use composenest_adapters::artifact_store::{ArtifactError, ArtifactInput, ArtifactStore};
use composenest_adapters::sqlite::DatabaseWorker;
use tempfile::TempDir;

fn fixture() -> (TempDir, DatabaseWorker) {
    let root = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    for name in ["state", "locks"] {
        let path = root.path().join(name);
        fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let database = DatabaseWorker::start(root.path()).unwrap();
    database.write(|db| {
        db.execute_batch("INSERT INTO management_scopes (id, owner_id, root_identity) VALUES ('scope', 'owner', 'root');
            INSERT INTO runtime_targets (id, scope_id, endpoint, engine_id, platform) VALUES ('target', 'scope', 'local', 'engine', 'linux');
            INSERT INTO instances (id, scope_id, target_id, display_name, normalized_name, project_name)
                VALUES ('instance', 'scope', 'target', 'Test', 'Test', 'cn-test');
            INSERT INTO instance_specs (instance_id, revision, selected_version, storage_method, inputs_json)
                VALUES ('instance', 1, '1', 'bind', '{}');
            INSERT INTO operations (id, instance_id, kind, phase, expected_instance_revision)
                VALUES ('operation', 'instance', 'create', 'artifact', 1);")?;
        Ok(())
    }).unwrap();
    (root, database)
}

fn input() -> ArtifactInput {
    ArtifactInput {
        id: "artifact".into(),
        instance_id: "instance".into(),
        spec_revision: 1,
        generator_version: "compose-v1".into(),
        files: BTreeMap::from([("compose.yaml".into(), b"services: {}\n".to_vec())]),
    }
}

#[test]
fn publishes_and_reconciles_from_recorded_bytes() {
    let (root, database) = fixture();
    let store = ArtifactStore::new(root.path(), &database);
    let path = store.publish("operation", input()).unwrap();
    assert_eq!(
        store.verified_compose_path("artifact").unwrap(),
        path.join("compose.yaml")
    );
    database
        .write(|db| {
            db.execute(
                "UPDATE artifacts SET placement = 'staged' WHERE id = 'artifact'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        store.verified_compose_path("artifact"),
        Err(ArtifactError::Unavailable)
    ));
    assert_eq!(store.reconcile("artifact").unwrap(), path);
    assert_eq!(store.publish("operation", input()).unwrap(), path);
}

#[test]
fn detects_modification_missing_file_and_extra_file() {
    let (root, database) = fixture();
    let store = ArtifactStore::new(root.path(), &database);
    let path = store.publish("operation", input()).unwrap();
    let manifest = fs::read(path.join("manifest.json")).unwrap();
    fs::write(path.join("compose.yaml"), b"changed").unwrap();
    assert!(matches!(
        store.verified_compose_path("artifact"),
        Err(ArtifactError::Modified)
    ));
    fs::write(path.join("compose.yaml"), b"services: {}\n").unwrap();
    fs::remove_file(path.join("manifest.json")).unwrap();
    assert!(matches!(
        store.verified_compose_path("artifact"),
        Err(ArtifactError::Modified)
    ));
    fs::write(path.join("manifest.json"), manifest).unwrap();
    fs::write(path.join("extra"), b"unexpected").unwrap();
    assert!(matches!(
        store.verified_compose_path("artifact"),
        Err(ArtifactError::Modified)
    ));
}

#[test]
fn existing_target_is_never_overwritten_and_staging_is_never_executable() {
    let (root, database) = fixture();
    let store = ArtifactStore::new(root.path(), &database);
    let target = root.path().join("instances/instance/artifacts/artifact");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("compose.yaml"), b"foreign").unwrap();
    assert!(store.publish("operation", input()).is_err());
    assert_eq!(fs::read(target.join("compose.yaml")).unwrap(), b"foreign");
    assert!(store.verified_compose_path("artifact").is_err());
    assert!(!root.path().join("staging/operation").exists());
}

#[test]
fn rejects_unsafe_references_and_links() {
    let (root, database) = fixture();
    let store = ArtifactStore::new(root.path(), &database);
    let mut unsafe_input = input();
    unsafe_input
        .files
        .insert("../outside".into(), b"x".to_vec());
    assert!(matches!(
        store.publish("operation", unsafe_input),
        Err(ArtifactError::InvalidInput)
    ));
    let path = store.publish("operation", input()).unwrap();
    fs::remove_file(path.join("compose.yaml")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        root.path().join("state/composenest.sqlite"),
        path.join("compose.yaml"),
    )
    .unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(
        root.path().join("state/composenest.sqlite"),
        path.join("compose.yaml"),
    )
    .unwrap();
    assert!(matches!(
        store.verified_compose_path("artifact"),
        Err(ArtifactError::UnsafePath(_))
    ));
}

#[test]
fn rejects_unrelated_operation_and_out_of_scope_database_path() {
    let (root, database) = fixture();
    let store = ArtifactStore::new(root.path(), &database);
    assert!(matches!(
        store.publish("other", input()),
        Err(ArtifactError::Conflict)
    ));
    store.publish("operation", input()).unwrap();
    database.write(|db| {
        db.execute("UPDATE artifact_files SET relative_path = '../outside' WHERE artifact_id = 'artifact' AND relative_path = 'compose.yaml'", [])?;
        Ok(())
    }).unwrap();
    assert!(matches!(
        store.verified_compose_path("artifact"),
        Err(ArtifactError::Modified)
    ));
}

#[test]
fn missing_published_artifact_is_not_silently_regenerated() {
    let (root, database) = fixture();
    let store = ArtifactStore::new(root.path(), &database);
    let path = store.publish("operation", input()).unwrap();
    fs::remove_dir_all(path).unwrap();
    assert!(matches!(
        store.publish("operation", input()),
        Err(ArtifactError::Unavailable)
    ));
    assert!(matches!(
        store.verified_compose_path("artifact"),
        Err(ArtifactError::Unavailable)
    ));
}

#[test]
fn resumes_complete_staging_after_interrupted_publish() {
    let (root, database) = fixture();
    let store = ArtifactStore::new(root.path(), &database);
    let target = store.publish("operation", input()).unwrap();
    let staging = root.path().join("staging/operation");
    fs::rename(&target, &staging).unwrap();
    database
        .write(|db| {
            db.execute(
                "UPDATE artifacts SET placement = 'staged' WHERE id = 'artifact'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(store.publish("operation", input()).unwrap(), target);
    assert!(!staging.exists());
}

#[test]
fn regenerates_partial_staging_for_the_same_operation() {
    let (root, database) = fixture();
    let store = ArtifactStore::new(root.path(), &database);
    let target = root.path().join("instances/instance/artifacts/artifact");
    fs::create_dir_all(&target).unwrap();
    assert!(store.publish("operation", input()).is_err());
    fs::remove_dir(&target).unwrap();
    let staging = root.path().join("staging/operation");
    fs::create_dir_all(&staging).unwrap();
    fs::write(staging.join("compose.yaml"), b"partial").unwrap();
    fs::write(staging.join("foreign"), b"keep").unwrap();
    assert!(matches!(
        store.publish("operation", input()),
        Err(ArtifactError::Conflict)
    ));
    assert_eq!(fs::read(staging.join("foreign")).unwrap(), b"keep");
    fs::remove_file(staging.join("foreign")).unwrap();
    assert_eq!(store.publish("operation", input()).unwrap(), target);
    assert_eq!(
        fs::read(target.join("compose.yaml")).unwrap(),
        b"services: {}\n"
    );
}
