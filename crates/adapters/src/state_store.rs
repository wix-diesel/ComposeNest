//! SQLite implementation of the confirmed state ledgers.

use composenest_application::state_store::{
    InstanceRecord, RuntimeTarget, StateStore, StorageMethod, StoreConflict, TemplateCatalogItem,
    TemplateFile, TemplateRevision,
};
use composenest_domain::identity::DisplayName;
use rusqlite::{Connection, Error, ErrorCode, TransactionBehavior, params};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

use crate::docker_cli::is_local_endpoint;
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

fn valid_canonical_revision(revision: &TemplateRevision) -> bool {
    let Ok(value) = serde_json::from_str::<JsonValue>(&revision.canonical_json) else {
        return false;
    };
    if revision.normalization != "template-normalization-v1"
        || value.get("normalization").and_then(JsonValue::as_str)
            != Some(revision.normalization.as_str())
        || value
            .pointer("/manifest/schemaVersion")
            .and_then(JsonValue::as_i64)
            != Some(1)
        || value.pointer("/manifest/id").and_then(JsonValue::as_str)
            != Some(revision.template_id.as_str())
        || value
            .pointer("/manifest/templateVersion")
            .and_then(JsonValue::as_str)
            != Some(revision.version.as_str())
    {
        return false;
    }
    let Some(versions) = value.get("versions").and_then(JsonValue::as_array) else {
        return false;
    };
    !versions.is_empty()
        && versions.iter().all(|version| {
            version
                .get("key")
                .and_then(JsonValue::as_str)
                .is_some_and(|key| !key.is_empty())
        })
}

fn insert_template(
    connection: &mut Connection,
    revision: &TemplateRevision,
) -> Result<(), DatabaseError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing = transaction.query_row(
        "SELECT semantic_hash FROM template_revisions WHERE template_id = ?1 AND template_version = ?2",
        params![revision.template_id, revision.version],
        |row| row.get::<_, String>(0),
    );
    match existing {
        Ok(hash) if hash == revision.semantic_hash => return Ok(()),
        Ok(_) => {
            return Err(DatabaseError::Sqlite(Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE),
                None,
            )));
        }
        Err(Error::QueryReturnedNoRows) => {}
        Err(error) => return Err(DatabaseError::Sqlite(error)),
    }
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
    let exists: i64 = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM template_revisions r, json_each(r.canonical_json, '$.versions') v \
         WHERE r.id = ?1 AND json_extract(v.value, '$.key') = ?2)",
        params![instance.template_revision_id, instance.selected_version], |row| row.get(0),
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
    // A same-version clone inherits only evidence for the same requested image and Engine target.
    if let Some(source) = &instance.clone_source_id {
        transaction.execute(
            "INSERT INTO image_resolutions \
             SELECT ?1, 1, r.image_ref, r.digest, r.image_id, r.platform, r.first_operation_id \
             FROM image_resolutions r JOIN instances parent ON parent.id = r.instance_id \
             JOIN instance_specs original ON original.instance_id = parent.id AND original.revision = r.spec_revision \
             JOIN template_snapshots copied ON copied.instance_id = ?1 \
             JOIN json_each(copied.canonical_json, '$.versions') v \
             WHERE parent.id = ?2 AND parent.target_id = ?3 \
             AND original.revision = (SELECT MAX(revision) FROM instance_specs WHERE instance_id = parent.id) \
             AND original.selected_version = ?4 AND v.value ->> '$.key' = ?4 \
             AND v.value ->> '$.definition.image' = r.image_ref \
             AND r.platform = (SELECT platform FROM runtime_targets WHERE id = ?3)",
            params![instance.id, source, instance.target_id, instance.selected_version],
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
        if !is_local_endpoint(endpoint.as_ref()) || engine_id.is_empty() || platform.is_empty() {
            return Err(StoreConflict::InvalidInput);
        }
        let values = (
            id.to_owned(),
            scope_id.to_owned(),
            endpoint.to_owned(),
            engine_id.to_owned(),
            platform.to_owned(),
        );
        self.write(move |db| {
            let count: i64 = db.query_row(
                "SELECT COUNT(*) FROM runtime_targets WHERE scope_id = ?1",
                [&values.1],
                |row| row.get(0),
            )?;
            if count != 0 {
                return Err(DatabaseError::Sqlite(Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE),
                    None,
                )));
            }
            db.execute(
                "INSERT INTO runtime_targets VALUES (?1, ?2, ?3, ?4, ?5)",
                params![values.0, values.1, values.2, values.3, values.4],
            )?;
            Ok(())
        })
        .map_err(map_error)
    }

    fn runtime_target(&self, scope_id: &str) -> Result<Option<RuntimeTarget>, StoreConflict> {
        let scope_id = scope_id.to_owned();
        self.read(move |db| {
            let mut statement = db.prepare(
                "SELECT id, scope_id, endpoint, engine_id, platform FROM runtime_targets WHERE scope_id = ?1",
            )?;
            let mut rows = statement.query([scope_id])?;
            let target = rows.next()?.map(|row| {
                Ok::<RuntimeTarget, Error>(RuntimeTarget {
                    id: row.get(0)?,
                    scope_id: row.get(1)?,
                    endpoint: row.get(2)?,
                    engine_id: row.get(3)?,
                    platform: row.get(4)?,
                })
            }).transpose()?;
            if rows.next()?.is_some() {
                return Err(DatabaseError::Sqlite(Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE),
                    None,
                )));
            }
            Ok(target)
        }).map_err(map_error)
    }

    fn register_template(&self, revision: &TemplateRevision) -> Result<(), StoreConflict> {
        if !valid_files(&revision.files)
            || !valid_canonical_revision(revision)
            || revision.semantic_hash
                != format!("{:x}", Sha256::digest(revision.canonical_json.as_bytes()))
            || revision.id
                != format!(
                    "{}:{}:{}",
                    revision.template_id, revision.version, revision.semantic_hash
                )
        {
            return Err(StoreConflict::InvalidInput);
        }
        let revision = revision.clone();
        self.write(move |db| insert_template(db, &revision))
            .map_err(map_error)
    }

    fn list_templates(&self) -> Result<Vec<TemplateCatalogItem>, StoreConflict> {
        self.read(|db| {
            let mut query = db.prepare(
                "SELECT id, template_id, template_version, origin, semantic_hash, canonical_json \
                 FROM template_revisions ORDER BY template_id, template_version",
            )?;
            let rows = query.query_map([], |row| {
                Ok(TemplateCatalogItem {
                    id: row.get(0)?,
                    template_id: row.get(1)?,
                    version: row.get(2)?,
                    origin: row.get(3)?,
                    semantic_hash: row.get(4)?,
                    canonical_json: row.get(5)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
        })
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
    use composenest_application::template_catalog::{
        CatalogError, TemplateOrigin, TemplatePackage, register_packages,
    };
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
            .create_target(
                "target",
                "scope",
                "unix:///var/run/docker.sock",
                "engine",
                "linux",
            )
            .unwrap();
        (root, worker)
    }

    #[test]
    fn image_resolution_is_immutable_and_same_version_clone_inherits_it() {
        use composenest_application::image_resolution::{ImageResolution, ImageResolutionStore};
        let (_root, worker) = store();
        worker
            .write(|db| {
                db.execute(
                    "UPDATE runtime_targets SET platform = 'linux/amd64' WHERE id = 'target'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let mut template = revision();
        template.canonical_json = r#"{"manifest":{"id":"redis","schemaVersion":1,"templateVersion":"1"},"normalization":"template-normalization-v1","versions":[{"key":"8","definition":{"image":"redis:8"}},{"key":"9","definition":{"image":"redis:9"}}]}"#.into();
        template.semantic_hash =
            format!("{:x}", Sha256::digest(template.canonical_json.as_bytes()));
        template.id = format!("redis:1:{}", template.semantic_hash);
        worker.register_template(&template).unwrap();
        let mut source = instance("source", "Source", 13000);
        source.template_revision_id = template.id.clone();
        worker.commit_instance(&source).unwrap();
        worker.write(|db| {
            db.execute("INSERT INTO operations (id, instance_id, kind, status, phase, expected_instance_revision, new_spec_revision) VALUES ('op', 'source', 'create', 'Executing', 'resolve_image', 1, 1)", [])?;
            db.execute("INSERT INTO operation_steps (operation_id, sequence, attempt, command_kind, resource_id, expected_result) VALUES ('op', 1, 1, 'resolve_image', 'source', 'image_resolved')", [])?;
            Ok(())
        }).unwrap();
        let id = format!("sha256:{}", "a".repeat(64));
        let resolution = ImageResolution {
            instance_id: "source".into(),
            spec_revision: 1,
            requested: "redis:8".into(),
            digest: format!("docker.io/library/redis@{id}"),
            image_id: id,
            platform: "linux/amd64".into(),
            operation_id: "op".into(),
        };
        worker.record_image_resolution(&resolution).unwrap();
        worker.record_image_resolution(&resolution).unwrap();
        worker
            .write(|db| {
                db.execute(
                    "UPDATE operations SET status = 'Failed' WHERE id = 'op'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            worker.record_image_resolution(&resolution),
            Err(StoreConflict::InvalidInput)
        );
        let mut changed = resolution.clone();
        changed.digest = format!("docker.io/library/redis@sha256:{}", "b".repeat(64));
        assert_eq!(
            worker.record_image_resolution(&changed),
            Err(StoreConflict::InvalidInput)
        );
        let mut same = instance("clone", "Clone", 13001);
        same.clone_source_id = Some("source".into());
        same.template_revision_id = template.id.clone();
        worker.commit_instance(&same).unwrap();
        let inherited = worker.image_resolution("clone", 1).unwrap().unwrap();
        assert_eq!(inherited.digest, resolution.digest);
        assert_eq!(inherited.requested, "redis:8");
        let mut different = instance("next", "Next", 13002);
        different.clone_source_id = Some("source".into());
        different.selected_version = "9".into();
        different.template_revision_id = template.id.clone();
        worker.commit_instance(&different).unwrap();
        assert_eq!(worker.image_resolution("next", 1).unwrap(), None);
    }

    #[test]
    fn target_is_readable_offline_and_scope_accepts_only_one_local_engine() {
        let (root, worker) = store();
        let target = worker.runtime_target("scope").unwrap().unwrap();
        assert_eq!(target.endpoint, "unix:///var/run/docker.sock");
        assert_eq!(target.engine_id, "engine");
        assert_eq!(worker.runtime_target("missing").unwrap(), None);
        assert_eq!(
            worker.create_target("remote", "scope", "ssh://host", "other", "linux/arm64"),
            Err(StoreConflict::InvalidInput)
        );
        assert_eq!(
            worker.create_target(
                "second",
                "scope",
                "unix:///tmp/other.sock",
                "other",
                "linux/arm64"
            ),
            Err(StoreConflict::Duplicate)
        );
        drop(worker);
        let reopened = DatabaseWorker::start(root.path()).unwrap();
        assert_eq!(reopened.runtime_target("scope").unwrap(), Some(target));
    }

    #[test]
    fn ambiguous_persisted_targets_are_rejected() {
        let (_root, worker) = store();
        worker.write(|db| {
            db.execute(
                "INSERT INTO runtime_targets (id, scope_id, endpoint, engine_id, platform) VALUES (?1, ?2, ?3, ?4, ?5)",
                params!["second", "scope", "unix:///tmp/other.sock", "other", "linux/amd64"],
            )?;
            Ok(())
        }).unwrap();
        assert_eq!(
            worker.runtime_target("scope"),
            Err(StoreConflict::Duplicate)
        );
    }

    fn revision() -> TemplateRevision {
        let canonical_json = r#"{"manifest":{"id":"redis","schemaVersion":1,"templateVersion":"1"},"normalization":"template-normalization-v1","versions":[{"key":"8","definition":"complete"}]}"#;
        let semantic_hash = format!("{:x}", Sha256::digest(canonical_json.as_bytes()));
        TemplateRevision {
            id: format!("redis:1:{semantic_hash}"),
            template_id: "redis".into(),
            version: "1".into(),
            normalization: "template-normalization-v1".into(),
            semantic_hash,
            canonical_json: canonical_json.into(),
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
            template_revision_id: revision().id,
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
    fn unsupported_canonical_shape_is_rejected_before_registration() {
        let (_root, worker) = store();
        let mut unsupported = revision();
        unsupported.canonical_json = unsupported.canonical_json.replace(
            r#""versions":[{"key":"8","definition":"complete"}]"#,
            r#""versions":{"8":{"definition":"complete"}}"#,
        );
        unsupported.semantic_hash = format!(
            "{:x}",
            Sha256::digest(unsupported.canonical_json.as_bytes())
        );
        assert_eq!(
            worker.register_template(&unsupported),
            Err(StoreConflict::InvalidInput)
        );

        let mut unsupported = revision();
        unsupported.normalization = "template-normalization-v2".into();
        assert_eq!(
            worker.register_template(&unsupported),
            Err(StoreConflict::InvalidInput)
        );
        let mut unsupported = revision();
        unsupported.id = "unrelated-revision".into();
        assert_eq!(
            worker.register_template(&unsupported),
            Err(StoreConflict::InvalidInput)
        );
        assert_eq!(count(&worker, "template_revisions"), 0);
    }

    #[test]
    fn same_meaning_keeps_first_bytes_and_origin_but_changed_meaning_conflicts() {
        let (_root, worker) = store();
        let first = revision();
        worker.register_template(&first).unwrap();
        let mut equivalent = first.clone();
        equivalent.origin = "bundled".into();
        equivalent.files[0].contents = b"# reformatted manifest".to_vec();
        worker.register_template(&equivalent).unwrap();
        assert_eq!(count(&worker, "template_revisions"), 1);
        let (origin, contents): (String, Vec<u8>) = worker
            .read(|db| {
                Ok(db.query_row(
                    "SELECT r.origin, f.contents FROM template_revisions r JOIN template_revision_files f ON r.id = f.revision_id WHERE f.relative_path = 'template.yaml'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?)
            })
            .unwrap();
        assert_eq!(origin, first.origin);
        assert_eq!(contents, first.files[0].contents);
        let catalog = worker.list_templates().unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].origin, first.origin);
        assert_eq!(catalog[0].semantic_hash, first.semantic_hash);

        let mut changed = first.clone();
        changed.canonical_json = changed.canonical_json.replace("complete", "changed");
        changed.semantic_hash = format!("{:x}", Sha256::digest(changed.canonical_json.as_bytes()));
        changed.id = format!("redis:1:{}", changed.semantic_hash);
        assert_eq!(
            worker.register_template(&changed),
            Err(StoreConflict::Duplicate)
        );
        assert_eq!(count(&worker, "template_revisions"), 1);
    }

    #[test]
    fn reload_rejects_every_simultaneous_duplicate_and_keeps_other_templates() {
        fn package(name: &str, id: &str, version: &str) -> TemplatePackage {
            let manifest = format!(
                "schemaVersion: 1\nid: {id}\ntemplateVersion: \"1.0.0\"\nname: Test\ndescription: Test template\ndefaultVersion: \"1\"\nversions:\n  \"1\": versions/1.yaml\n"
            );
            TemplatePackage {
                name: name.into(),
                origin: TemplateOrigin::Local,
                files: vec![
                    TemplateFile {
                        relative_path: "template.yaml".into(),
                        contents: manifest.into_bytes(),
                    },
                    TemplateFile {
                        relative_path: "versions/1.yaml".into(),
                        contents: format!(
                            "image: example:{version}\nplatforms: [linux/amd64]\nservice:\n  healthcheck:\n    command: [check]\n"
                        )
                        .into_bytes(),
                    },
                ],
                warnings: Vec::new(),
            }
        }

        let (_root, worker) = store();
        let results = register_packages(
            &worker,
            vec![
                package("first", "example.same", "1"),
                package("other", "example.other", "1"),
                package("second", "example.same", "2"),
            ],
        );
        assert!(matches!(
            results[0].result,
            Err(CatalogError::AmbiguousRevision)
        ));
        assert!(results[1].result.is_ok());
        assert!(matches!(
            results[2].result,
            Err(CatalogError::AmbiguousRevision)
        ));
        assert_eq!(count(&worker, "template_revisions"), 1);

        let reloaded = register_packages(&worker, vec![package("other", "example.other", "2")]);
        assert!(matches!(
            reloaded[0].result,
            Err(CatalogError::Store(StoreConflict::Duplicate))
        ));
        assert_eq!(count(&worker, "template_revisions"), 1);
    }

    #[test]
    fn snapshot_keeps_original_versions_when_catalog_adds_another_revision() {
        let (_root, worker) = store();
        let mut original = revision();
        original.files[1].relative_path = "versions/custom.yaml".into();
        worker.register_template(&original).unwrap();
        worker
            .commit_instance(&instance("one", "One", 6379))
            .unwrap();

        let mut newer = original.clone();
        newer.version = "2".into();
        newer.canonical_json = newer
            .canonical_json
            .replace("\"templateVersion\":\"1\"", "\"templateVersion\":\"2\"")
            .replace(
                r#"{"key":"8","definition":"complete"}"#,
                r#"{"key":"8","definition":"complete"},{"key":"9","definition":"new"}"#,
            );
        newer.semantic_hash = format!("{:x}", Sha256::digest(newer.canonical_json.as_bytes()));
        newer.id = format!("redis:2:{}", newer.semantic_hash);
        worker.register_template(&newer).unwrap();
        let snapshot: String = worker
            .read(|db| {
                Ok(db.query_row(
                    "SELECT canonical_json FROM template_snapshots WHERE instance_id = 'one'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(snapshot, original.canonical_json);
        assert_eq!(count(&worker, "template_snapshot_files"), 2);
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
