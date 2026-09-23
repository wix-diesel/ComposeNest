//! SQLite implementation of the confirmed state ledgers.

use composenest_application::state_store::{
    InstanceRecord, StateStore, StorageMethod, StoreConflict, TemplateFile, TemplateRevision,
};
use composenest_domain::identity::DisplayName;
use rusqlite::{Connection, Error, ErrorCode, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::sqlite::{DatabaseError, DatabaseWorker};

fn method_name(method: StorageMethod) -> &'static str {
    match method {
        StorageMethod::Bind => "bind",
        StorageMethod::Volume => "volume",
    }
}

pub(crate) fn map_error(error: DatabaseError) -> StoreConflict {
    match error {
        DatabaseError::Sqlite(Error::SqliteFailure(code, _)) => match code.extended_code {
            rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY => StoreConflict::Missing,
            rusqlite::ffi::SQLITE_CONSTRAINT_CHECK => StoreConflict::InvalidInput,
            _ if code.code == ErrorCode::ConstraintViolation => StoreConflict::Duplicate,
            _ => StoreConflict::Backend,
        },
        _ => StoreConflict::Backend,
    }
}

fn valid_files(files: &[TemplateFile]) -> bool {
    files
        .iter()
        .any(|file| file.relative_path == "template.yaml")
        && files
            .iter()
            .any(|file| file.relative_path.starts_with("versions/"))
        && files.iter().all(|file| {
            let path = file.relative_path.as_str();
            !file.contents.is_empty()
                && (path == "template.yaml"
                    || (path.starts_with("versions/") && path.ends_with(".yaml")))
                && !path.contains("..")
                && !path.contains('\\')
                && !path.contains('\0')
        })
}

fn insert_template(
    connection: &mut Connection,
    revision: &TemplateRevision,
) -> Result<(), DatabaseError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT INTO template_revisions VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7)",
        params![
            revision.id,
            revision.template_id,
            revision.version,
            revision.normalization,
            revision.semantic_hash,
            revision.canonical_json,
            revision.origin
        ],
    )?;
    for file in &revision.files {
        let digest = format!("{:x}", Sha256::digest(&file.contents));
        transaction.execute(
            "INSERT INTO template_revision_files VALUES (?1, ?2, ?3, ?4)",
            params![revision.id, file.relative_path, file.contents, digest],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn insert_instance(
    connection: &mut Connection,
    instance: &InstanceRecord,
) -> Result<(), DatabaseError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version_path = format!("versions/{}.yaml", instance.selected_version);
    let exists: i64 = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM template_revision_files WHERE revision_id = ?1 AND relative_path = ?2)",
        params![instance.template_revision_id, version_path], |row| row.get(0),
    )?;
    if exists == 0 {
        return Err(DatabaseError::Sqlite(Error::QueryReturnedNoRows));
    }
    transaction.execute(
        "INSERT INTO instances (id, scope_id, target_id, display_name, normalized_name, project_name, clone_source_id) VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6)",
        params![instance.id, instance.scope_id, instance.target_id, instance.name,
            instance.project_name, instance.clone_source_id],
    )?;
    transaction.execute(
        "INSERT INTO template_snapshots SELECT ?1, ?2, id, template_id, template_version, ?3, schema_version, normalization, semantic_hash, canonical_json FROM template_revisions WHERE id = ?4",
        params![instance.id, instance.id, instance.selected_version, instance.template_revision_id],
    )?;
    if transaction.changes() != 1 {
        return Err(DatabaseError::Sqlite(Error::QueryReturnedNoRows));
    }
    transaction.execute(
        "INSERT INTO template_snapshot_files SELECT ?1, relative_path, contents, sha256 FROM template_revision_files WHERE revision_id = ?2",
        params![instance.id, instance.template_revision_id],
    )?;
    transaction.execute(
        "INSERT INTO instance_specs VALUES (?1, 1, ?2, ?3, ?4)",
        params![
            instance.id,
            instance.selected_version,
            method_name(instance.storage_method),
            instance.inputs_json
        ],
    )?;
    for port in &instance.ports {
        transaction.execute(
            "INSERT INTO port_bindings VALUES (?1, 1, ?2, ?3, ?4, ?5)",
            params![
                instance.id,
                port.slot,
                port.host_ip,
                port.host_port,
                port.container_port
            ],
        )?;
        transaction.execute(
            "INSERT INTO port_reservations VALUES (?1, ?2, ?3, ?4, 'tcp', ?5, 'committed')",
            params![
                format!("{}:{}", instance.id, port.slot),
                instance.scope_id,
                instance.id,
                port.host_ip,
                port.host_port
            ],
        )?;
    }
    for allocation in &instance.storage {
        transaction.execute(
            "INSERT INTO storage_allocations VALUES (?1, ?2, ?3, ?4, ?5, 'assigned', 'not_materialized', 'not_attempted')",
            params![instance.id, allocation.slot, method_name(instance.storage_method),
                allocation.resource_identity, allocation.ownership_evidence],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

impl StateStore for DatabaseWorker {
    fn create_scope(
        &self,
        id: &str,
        owner_id: &str,
        root_identity: &str,
    ) -> Result<(), StoreConflict> {
        let (id, owner_id, root_identity) =
            (id.to_owned(), owner_id.to_owned(), root_identity.to_owned());
        self.write(move |db| {
            db.execute(
                "INSERT INTO management_scopes (id, owner_id, root_identity) VALUES (?1, ?2, ?3)",
                params![id, owner_id, root_identity],
            )?;
            Ok(())
        })
        .map_err(map_error)
    }

    fn create_target(
        &self,
        id: &str,
        scope_id: &str,
        endpoint: &str,
        engine_id: &str,
        platform: &str,
    ) -> Result<(), StoreConflict> {
        let values = (
            id.to_owned(),
            scope_id.to_owned(),
            endpoint.to_owned(),
            engine_id.to_owned(),
            platform.to_owned(),
        );
        self.write(move |db| {
            db.execute(
                "INSERT INTO runtime_targets VALUES (?1, ?2, ?3, ?4, ?5)",
                params![values.0, values.1, values.2, values.3, values.4],
            )?;
            Ok(())
        })
        .map_err(map_error)
    }

    fn register_template(&self, revision: &TemplateRevision) -> Result<(), StoreConflict> {
        if !valid_files(&revision.files)
            || revision.semantic_hash
                != format!("{:x}", Sha256::digest(revision.canonical_json.as_bytes()))
        {
            return Err(StoreConflict::InvalidInput);
        }
        let revision = revision.clone();
        self.write(move |db| insert_template(db, &revision))
            .map_err(map_error)
    }

    fn commit_instance(&self, instance: &InstanceRecord) -> Result<(), StoreConflict> {
        if instance.id.is_empty()
            || instance.selected_version.is_empty()
            || DisplayName::parse(&instance.name)
                .map(|name| name.as_str() != instance.name)
                .unwrap_or(true)
        {
            return Err(StoreConflict::InvalidInput);
        }
        let instance = instance.clone();
        self.write(move |db| insert_instance(db, &instance))
            .map_err(|error| match error {
                DatabaseError::Sqlite(Error::QueryReturnedNoRows) => StoreConflict::Missing,
                other => map_error(other),
            })
    }

    fn rename_instance(&self, id: &str, expected: u64, name: &str) -> Result<(), StoreConflict> {
        let (id, name) = (id.to_owned(), name.to_owned());
        if DisplayName::parse(&name)
            .map(|parsed| parsed.as_str() != name)
            .unwrap_or(true)
        {
            return Err(StoreConflict::InvalidInput);
        }
        self.write(move |db| update_instance(db, &id, expected, Some(&name)))
            .map_err(map_error)?
    }

    fn retire_instance(
        &self,
        id: &str,
        expected: u64,
        absence_verified: bool,
    ) -> Result<(), StoreConflict> {
        if !absence_verified {
            return Err(StoreConflict::InvalidInput);
        }
        let id = id.to_owned();
        self.write(move |db| update_instance(db, &id, expected, None))
            .map_err(map_error)?
    }

    fn default_storage_method(&self, scope_id: &str) -> Result<StorageMethod, StoreConflict> {
        self.read(|db| {
            let value: String = db.query_row(
                "SELECT default_storage_method FROM management_scopes WHERE id = ?1",
                [scope_id],
                |row| row.get(0),
            )?;
            Ok(value)
        })
        .map_err(|error| match error {
            DatabaseError::Sqlite(Error::QueryReturnedNoRows) => StoreConflict::Missing,
            other => map_error(other),
        })
        .map(|value| {
            if value == "bind" {
                StorageMethod::Bind
            } else {
                StorageMethod::Volume
            }
        })
    }

    fn set_default_storage_method(
        &self,
        scope_id: &str,
        method: StorageMethod,
    ) -> Result<(), StoreConflict> {
        let scope_id = scope_id.to_owned();
        self.write(move |db| {
            let count = db.execute(
                "UPDATE management_scopes SET default_storage_method = ?1 WHERE id = ?2",
                params![method_name(method), scope_id],
            )?;
            if count == 0 {
                return Err(DatabaseError::Sqlite(Error::QueryReturnedNoRows));
            }
            Ok(())
        })
        .map_err(|error| match error {
            DatabaseError::Sqlite(Error::QueryReturnedNoRows) => StoreConflict::Missing,
            other => map_error(other),
        })
    }
}

fn update_instance(
    db: &mut Connection,
    id: &str,
    expected: u64,
    name: Option<&str>,
) -> Result<Result<(), StoreConflict>, DatabaseError> {
    let expected =
        i64::try_from(expected).map_err(|_| DatabaseError::Sqlite(Error::InvalidQuery))?;
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let count = if let Some(name) = name {
        transaction.execute("UPDATE instances SET display_name = ?1, normalized_name = ?1, revision = revision + 1 WHERE id = ?2 AND revision = ?3 AND lifecycle = 'managed'",
            params![name, id, expected])?
    } else {
        transaction.execute("UPDATE instances SET lifecycle = 'retired', revision = revision + 1 WHERE id = ?1 AND revision = ?2 AND lifecycle = 'managed'",
            params![id, expected])?
    };
    if count == 0 {
        let current = transaction.query_row(
            "SELECT revision, lifecycle FROM instances WHERE id = ?1",
            [id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        );
        return Ok(Err(match current {
            Err(Error::QueryReturnedNoRows) => StoreConflict::Missing,
            Ok((revision, _)) if revision != expected => StoreConflict::StaleRevision,
            _ => StoreConflict::InvalidLifecycle,
        }));
    }
    transaction.commit()?;
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use composenest_application::state_store::{PortAllocation, StorageAllocation};
    use std::fs;
    use tempfile::TempDir;

    fn store() -> (TempDir, DatabaseWorker) {
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::create_dir(root.path().join("state")).unwrap();
        fs::create_dir(root.path().join("locks")).unwrap();
        #[cfg(unix)]
        for path in ["state", "locks"] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.path().join(path), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let worker = DatabaseWorker::start(root.path()).unwrap();
        worker.create_scope("scope", "owner", "root").unwrap();
        worker
            .create_target("target", "scope", "local", "engine", "linux")
            .unwrap();
        (root, worker)
    }

    fn revision() -> TemplateRevision {
        TemplateRevision {
            id: "revision".into(),
            template_id: "redis".into(),
            version: "1".into(),
            normalization: "schema-1".into(),
            semantic_hash: format!("{:x}", Sha256::digest(br#"{"versions":{"8":"complete"}}"#)),
            canonical_json: r#"{"versions":{"8":"complete"}}"#.into(),
            origin: "bundled".into(),
            files: vec![
                TemplateFile {
                    relative_path: "template.yaml".into(),
                    contents: b"manifest".to_vec(),
                },
                TemplateFile {
                    relative_path: "versions/8.yaml".into(),
                    contents: b"version".to_vec(),
                },
            ],
        }
    }

    fn instance(id: &str, name: &str, port: u16) -> InstanceRecord {
        InstanceRecord {
            id: id.into(),
            scope_id: "scope".into(),
            target_id: "target".into(),
            name: name.into(),
            project_name: format!("cn-{id}"),
            clone_source_id: None,
            template_revision_id: "revision".into(),
            selected_version: "8".into(),
            storage_method: StorageMethod::Bind,
            inputs_json: "{}".into(),
            ports: vec![PortAllocation {
                slot: "main".into(),
                host_ip: "127.0.0.1".into(),
                host_port: port,
                container_port: 6379,
            }],
            storage: vec![StorageAllocation {
                slot: "data".into(),
                resource_identity: format!("data/{id}"),
                ownership_evidence: format!("owner/{id}"),
            }],
        }
    }

    fn count(worker: &DatabaseWorker, table: &str) -> i64 {
        let sql = format!("SELECT count(*) FROM {table}");
        worker
            .read(|db| Ok(db.query_row(&sql, [], |row| row.get(0))?))
            .unwrap()
    }

    #[test]
    fn template_registration_and_snapshot_copy_are_atomic() {
        let (_root, worker) = store();
        let mut invalid = revision();
        invalid.files.push(invalid.files[0].clone());
        assert_eq!(
            worker.register_template(&invalid),
            Err(StoreConflict::Duplicate)
        );
        assert_eq!(count(&worker, "template_revisions"), 0);
        assert_eq!(count(&worker, "template_revision_files"), 0);
        worker.register_template(&revision()).unwrap();
        let mut invalid_instance = instance("one", "One", 6379);
        invalid_instance
            .storage
            .push(invalid_instance.storage[0].clone());
        assert_eq!(
            worker.commit_instance(&invalid_instance),
            Err(StoreConflict::Duplicate)
        );
        assert_eq!(count(&worker, "instances"), 0);
        assert_eq!(count(&worker, "template_snapshots"), 0);
        worker
            .commit_instance(&instance("one", "One", 6379))
            .unwrap();
        let copied: Vec<u8> = worker.read(|db| Ok(db.query_row(
            "SELECT contents FROM template_snapshot_files WHERE relative_path = 'versions/8.yaml'", [], |row| row.get(0))?)).unwrap();
        assert_eq!(copied, b"version");
        worker
            .write(|db| {
                db.execute("DELETE FROM template_revision_files", [])?;
                db.execute("DELETE FROM template_revisions", [])?;
                Ok(())
            })
            .unwrap();
        assert_eq!(count(&worker, "template_snapshot_files"), 2);
        assert_eq!(count(&worker, "template_snapshots"), 1);
    }

    #[test]
    fn uniqueness_revision_and_retirement_preserve_history() {
        let (_root, worker) = store();
        worker.register_template(&revision()).unwrap();
        worker
            .commit_instance(&instance("one", "One", 6379))
            .unwrap();
        assert_eq!(
            worker.commit_instance(&instance("two", "One", 6380)),
            Err(StoreConflict::Duplicate)
        );
        assert_eq!(
            worker.commit_instance(&instance("two", "Two", 6379)),
            Err(StoreConflict::Duplicate)
        );
        assert_eq!(
            worker.rename_instance("one", 0, "Changed"),
            Err(StoreConflict::StaleRevision)
        );
        worker.rename_instance("one", 1, "Changed").unwrap();
        assert_eq!(
            worker.retire_instance("one", 2, false),
            Err(StoreConflict::InvalidInput)
        );
        worker.retire_instance("one", 2, true).unwrap();
        assert_eq!(count(&worker, "port_reservations"), 1);
        assert_eq!(count(&worker, "storage_allocations"), 1);
        assert_eq!(count(&worker, "template_snapshots"), 1);
        let mut reused = instance("two", "One", 6380);
        reused.project_name = "cn-one".into();
        assert_eq!(
            worker.commit_instance(&reused),
            Err(StoreConflict::Duplicate)
        );
        worker
            .commit_instance(&instance("two", "One", 6380))
            .unwrap();
        assert_eq!(count(&worker, "instances"), 2);
    }

    #[test]
    fn default_storage_method_survives_reopen() {
        let (root, worker) = store();
        assert_eq!(
            worker.default_storage_method("scope"),
            Ok(StorageMethod::Bind)
        );
        worker
            .set_default_storage_method("scope", StorageMethod::Volume)
            .unwrap();
        drop(worker);
        let reopened = DatabaseWorker::start(root.path()).unwrap();
        assert_eq!(
            reopened.default_storage_method("scope"),
            Ok(StorageMethod::Volume)
        );
    }
}
