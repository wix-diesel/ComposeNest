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


fn recovery(database: &DatabaseWorker) {
    database.write(|db| {
        db.execute("UPDATE operations SET kind = 'recover' WHERE id = 'operation'", [])?;
        Ok(())
    }).unwrap();
}

#[test]
fn confirmation_covers_added_removed_files_and_never_returns_secrets() {
    let (root, database) = fixture();
    let store = ArtifactStore::new(root.path(), &database);
    let path = store.publish("operation", input()).unwrap();
    let original = store.inspect_external("artifact").unwrap();
    assert!(original.differences.is_empty());
    fs::write(path.join("compose.yaml"), b"PASSWORD: secret-value").unwrap();
    fs::remove_file(path.join("manifest.json")).unwrap();
    fs::write(path.join("extra"), b"other-secret").unwrap();
    let changed = store.inspect_external("artifact").unwrap();
    assert_eq!(changed.differences.len(), 3);
    assert_ne!(changed.confirmation_hash, original.confirmation_hash);
    assert!(!format!("{changed:?}").contains("secret-value"));
    recovery(&database);
    fs::write(path.join("extra"), b"edited-again").unwrap();
    assert!(matches!(store.archive_external("operation", "artifact", &changed.confirmation_hash), Err(ArtifactError::Modified)));
    assert!(path.join("compose.yaml").exists());
}

#[test]
fn archives_confirmed_bytes_and_reconciles_move_before_database_update() {
    let (root, database) = fixture();
    let store = ArtifactStore::new(root.path(), &database);
    let source = store.publish("operation", input()).unwrap();
    fs::write(source.join("compose.yaml"), b"external-secret").unwrap();
    let changed = store.inspect_external("artifact").unwrap();
    recovery(&database);
    let target = store.archive_external("operation", "artifact", &changed.confirmation_hash).unwrap();
    assert!(!source.exists());
    assert_eq!(fs::read(target.join("compose.yaml")).unwrap(), b"external-secret");
    assert!(store.verified_compose_path("artifact").is_err());
    database.write(|db| {
        db.execute("UPDATE artifacts SET placement = 'published' WHERE id = 'artifact'", [])?;
        Ok(())
    }).unwrap();
    assert_eq!(store.archive_external("operation", "artifact", &changed.confirmation_hash).unwrap(), target);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(target.join("compose.yaml")).unwrap().permissions().mode() & 0o777, 0o600);
    }
    fs::write(target.join("compose.yaml"), b"archive-edited").unwrap();
    assert!(matches!(store.archive_external("operation", "artifact", &changed.confirmation_hash), Err(ArtifactError::Modified)));
}

#[cfg(unix)]
#[test]
fn archive_rejects_links_and_never_changes_another_hardlink_owner() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (root, database) = fixture();
    let store = ArtifactStore::new(root.path(), &database);
    let source = store.publish("operation", input()).unwrap();
    recovery(&database);
    let outside = root.path().join("outside");
    fs::write(&outside, b"shared").unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
    fs::remove_file(source.join("compose.yaml")).unwrap();
    fs::hard_link(&outside, source.join("compose.yaml")).unwrap();
    let changed = store.inspect_external("artifact").unwrap();
    assert!(matches!(store.archive_external("operation", "artifact", &changed.confirmation_hash), Err(ArtifactError::UnsafePath(_))));
    assert_eq!(fs::metadata(&outside).unwrap().permissions().mode() & 0o777, 0o644);
    fs::remove_file(source.join("compose.yaml")).unwrap();
    symlink(&outside, source.join("compose.yaml")).unwrap();
    assert!(matches!(store.inspect_external("artifact"), Err(ArtifactError::UnsafePath(_))));
}
