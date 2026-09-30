//! SQLite implementation of the confirmed state ledgers.

use composenest_application::state_store::{
    CloneSource, InstanceRecord, PortAllocation, RuntimeTarget, StateStore, StorageAllocation,
    StorageLedgerEntry, StorageMethod, StoreConflict, TemplateCatalogItem, TemplateFile,
    TemplateRevision,
};
use composenest_domain::{
    identity::DisplayName,
    instance::{Initialization, StorageOwnership, StoragePresence},
};
#[cfg(test)]
use rusqlite::params;
use rusqlite::{Error, ErrorCode};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

use crate::docker_cli::is_local_endpoint;
use crate::entities::{
    image_resolution, instance as instance_entity, instance_spec, management_scope, operation,
    port_binding, port_reservation, runtime_target as target_entity, storage_allocation,
    template_revision as revision_entity, template_revision_file, template_snapshot,
    template_snapshot_file,
};
use crate::sqlite::{DatabaseError, DatabaseWorker};

fn method_name(method: StorageMethod) -> &'static str {
    match method {
        StorageMethod::Bind => "bind",
        StorageMethod::Volume => "volume",
    }
}

fn method_from_name(value: &str) -> Option<StorageMethod> {
    match value {
        "bind" => Some(StorageMethod::Bind),
        "volume" => Some(StorageMethod::Volume),
        _ => None,
    }
}

pub(crate) fn presence_name(presence: StoragePresence) -> &'static str {
    match presence {
        StoragePresence::NotMaterialized => "not_materialized",
        StoragePresence::Present => "present",
        StoragePresence::Missing => "missing",
        StoragePresence::Unverified => "unverified",
    }
}

fn presence_from_name(value: &str) -> Option<StoragePresence> {
    match value {
        "not_materialized" => Some(StoragePresence::NotMaterialized),
        "present" => Some(StoragePresence::Present),
        "missing" => Some(StoragePresence::Missing),
        "unverified" => Some(StoragePresence::Unverified),
        _ => None,
    }
}

fn ownership_from_name(value: &str) -> Option<StorageOwnership> {
    match value {
        "assigned" => Some(StorageOwnership::Assigned),
        "retained" => Some(StorageOwnership::Retained),
        _ => None,
    }
}

fn initialization_name(initialization: Initialization) -> &'static str {
    match initialization {
        Initialization::NotAttempted => "not_attempted",
        Initialization::MayHaveInitialized => "may_have_initialized",
        Initialization::ReadyObserved => "ready_observed",
    }
}

fn initialization_from_name(value: &str) -> Option<Initialization> {
    match value {
        "not_attempted" => Some(Initialization::NotAttempted),
        "may_have_initialized" => Some(Initialization::MayHaveInitialized),
        "ready_observed" => Some(Initialization::ReadyObserved),
        _ => None,
    }
}

pub(crate) fn storage_entry(
    row: storage_allocation::Model,
    scope_id: String,
) -> Result<StorageLedgerEntry, DatabaseError> {
    Ok(StorageLedgerEntry {
        instance_id: row.instance_id,
        scope_id,
        slot: row.slot.clone(),
        method: method_from_name(&row.method).ok_or(DatabaseError::InvalidInput)?,
        allocation: StorageAllocation {
            slot: row.slot,
            resource_identity: row.resource_identity,
            ownership_evidence: row.ownership_evidence,
        },
        ownership: ownership_from_name(&row.ownership).ok_or(DatabaseError::InvalidInput)?,
        presence: presence_from_name(&row.presence).ok_or(DatabaseError::InvalidInput)?,
        initialization: initialization_from_name(&row.initialization)
            .ok_or(DatabaseError::InvalidInput)?,
    })
}

pub(crate) fn map_error(error: DatabaseError) -> StoreConflict {
    match error {
        DatabaseError::Orm(sea_orm::DbErr::Exec(sea_orm::RuntimeErr::Rusqlite(error)))
        | DatabaseError::Orm(sea_orm::DbErr::Query(sea_orm::RuntimeErr::Rusqlite(error))) => {
            if let Error::SqliteFailure(code, _) = error.as_ref() {
                return match code.extended_code {
                    rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY => StoreConflict::Missing,
                    rusqlite::ffi::SQLITE_CONSTRAINT_CHECK => StoreConflict::InvalidInput,
                    _ if code.code == ErrorCode::ConstraintViolation => StoreConflict::Duplicate,
                    _ => StoreConflict::Backend,
                };
            }
            StoreConflict::Backend
        }
        DatabaseError::InvalidInput => StoreConflict::InvalidInput,
        DatabaseError::Duplicate => StoreConflict::Duplicate,
        DatabaseError::Missing => StoreConflict::Missing,
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
    connection: &sea_orm::DatabaseConnection,
    revision: &TemplateRevision,
) -> Result<(), DatabaseError> {
    let transaction = connection.begin()?;
    let existing = revision_entity::Entity::find()
        .filter(revision_entity::Column::TemplateId.eq(&revision.template_id))
        .filter(revision_entity::Column::TemplateVersion.eq(&revision.version))
        .one(&transaction)?;
    if let Some(existing) = existing {
        return if existing.semantic_hash == revision.semantic_hash {
            Ok(())
        } else {
            Err(DatabaseError::Duplicate)
        };
    }
    revision_entity::Entity::insert(revision_entity::ActiveModel {
        id: Set(revision.id.clone()),
        template_id: Set(revision.template_id.clone()),
        template_version: Set(revision.version.clone()),
        schema_version: Set(1),
        normalization: Set(revision.normalization.clone()),
        semantic_hash: Set(revision.semantic_hash.clone()),
        canonical_json: Set(revision.canonical_json.clone()),
        origin: Set(revision.origin.clone()),
    })
    .exec(&transaction)?;
    for file in &revision.files {
        let digest = format!("{:x}", Sha256::digest(&file.contents));
        template_revision_file::Entity::insert(template_revision_file::ActiveModel {
            revision_id: Set(revision.id.clone()),
            relative_path: Set(file.relative_path.clone()),
            contents: Set(file.contents.clone()),
            sha256: Set(digest),
        })
        .exec(&transaction)?;
    }
    transaction.commit()?;
    Ok(())
}

pub(crate) fn insert_instance(
    transaction: &sea_orm::DatabaseTransaction,
    instance: &InstanceRecord,
) -> Result<(), DatabaseError> {
    let source = if let Some(source_id) = &instance.clone_source_id {
        template_snapshot::Entity::find()
            .filter(template_snapshot::Column::InstanceId.eq(source_id))
            .one(transaction)?
    } else {
        None
    };
    let revision = if source.is_none() && instance.clone_source_id.is_none() {
        revision_entity::Entity::find_by_id(&instance.template_revision_id).one(transaction)?
    } else {
        None
    };
    let canonical_json = source
        .as_ref()
        .map(|item| &item.canonical_json)
        .or_else(|| revision.as_ref().map(|item| &item.canonical_json))
        .ok_or(DatabaseError::Missing)?;
    if !has_version(canonical_json, &instance.selected_version) {
        return Err(DatabaseError::Missing);
    }
    instance_entity::Entity::insert(instance_entity::ActiveModel {
        id: Set(instance.id.clone()),
        scope_id: Set(instance.scope_id.clone()),
        target_id: Set(instance.target_id.clone()),
        display_name: Set(instance.name.clone()),
        normalized_name: Set(instance.name.clone()),
        project_name: Set(instance.project_name.clone()),
        clone_source_id: Set(instance.clone_source_id.clone()),
        ..Default::default()
    })
    .exec(transaction)?;
    let snapshot = if let Some(source) = source {
        template_snapshot::ActiveModel {
            id: Set(instance.id.clone()),
            instance_id: Set(instance.id.clone()),
            source_revision_id: Set(source.source_revision_id),
            template_id: Set(source.template_id),
            template_version: Set(source.template_version),
            selected_version: Set(instance.selected_version.clone()),
            schema_version: Set(source.schema_version),
            normalization: Set(source.normalization),
            semantic_hash: Set(source.semantic_hash),
            canonical_json: Set(source.canonical_json),
        }
    } else if let Some(revision) = revision {
        template_snapshot::ActiveModel {
            id: Set(instance.id.clone()),
            instance_id: Set(instance.id.clone()),
            source_revision_id: Set(Some(revision.id)),
            template_id: Set(revision.template_id),
            template_version: Set(revision.template_version),
            selected_version: Set(instance.selected_version.clone()),
            schema_version: Set(revision.schema_version),
            normalization: Set(revision.normalization),
            semantic_hash: Set(revision.semantic_hash),
            canonical_json: Set(revision.canonical_json),
        }
    } else {
        return Err(DatabaseError::Missing);
    };
    template_snapshot::Entity::insert(snapshot).exec(transaction)?;
    if let Some(source_id) = &instance.clone_source_id {
        for file in template_snapshot_file::Entity::find()
            .filter(template_snapshot_file::Column::SnapshotId.eq(source_id))
            .all(transaction)?
        {
            template_snapshot_file::Entity::insert(template_snapshot_file::ActiveModel {
                snapshot_id: Set(instance.id.clone()),
                relative_path: Set(file.relative_path),
                contents: Set(file.contents),
                sha256: Set(file.sha256),
            })
            .exec(transaction)?;
        }
    } else {
        for file in template_revision_file::Entity::find()
            .filter(template_revision_file::Column::RevisionId.eq(&instance.template_revision_id))
            .all(transaction)?
        {
            template_snapshot_file::Entity::insert(template_snapshot_file::ActiveModel {
                snapshot_id: Set(instance.id.clone()),
                relative_path: Set(file.relative_path),
                contents: Set(file.contents),
                sha256: Set(file.sha256),
            })
            .exec(transaction)?;
        }
    }
    instance_spec::Entity::insert(instance_spec::ActiveModel {
        instance_id: Set(instance.id.clone()),
        revision: Set(1),
        selected_version: Set(instance.selected_version.clone()),
        storage_method: Set(method_name(instance.storage_method).into()),
        inputs_json: Set(instance.inputs_json.clone()),
    })
    .exec(transaction)?;
    for port in &instance.ports {
        port_binding::Entity::insert(port_binding::ActiveModel {
            instance_id: Set(instance.id.clone()),
            spec_revision: Set(1),
            slot: Set(port.slot.clone()),
            host_ip: Set(port.host_ip.clone()),
            host_port: Set(i64::from(port.host_port)),
            container_port: Set(i64::from(port.container_port)),
        })
        .exec(transaction)?;
        port_reservation::Entity::insert(port_reservation::ActiveModel {
            id: Set(format!("{}:{}", instance.id, port.slot)),
            scope_id: Set(instance.scope_id.clone()),
            instance_id: Set(instance.id.clone()),
            host_ip: Set(port.host_ip.clone()),
            protocol: Set("tcp".into()),
            host_port: Set(i64::from(port.host_port)),
            status: Set("committed".into()),
        })
        .exec(transaction)?;
    }
    for allocation in &instance.storage {
        storage_allocation::Entity::insert(storage_allocation::ActiveModel {
            instance_id: Set(instance.id.clone()),
            slot: Set(allocation.slot.clone()),
            method: Set(method_name(instance.storage_method).into()),
            resource_identity: Set(allocation.resource_identity.clone()),
            ownership_evidence: Set(allocation.ownership_evidence.clone()),
            ownership: Set("assigned".into()),
            presence: Set("not_materialized".into()),
            initialization: Set("not_attempted".into()),
        })
        .exec(transaction)?;
    }
    inherit_image_resolution(transaction, instance)?;
    Ok(())
}

fn has_version(canonical_json: &str, selected_version: &str) -> bool {
    serde_json::from_str::<JsonValue>(canonical_json)
        .ok()
        .and_then(|value| value.get("versions")?.as_array().cloned())
        .is_some_and(|versions| {
            versions.iter().any(|version| {
                version.get("key").and_then(JsonValue::as_str) == Some(selected_version)
            })
        })
}

fn selected_image(canonical_json: &str, selected_version: &str) -> Option<String> {
    let value: JsonValue = serde_json::from_str(canonical_json).ok()?;
    value
        .get("versions")?
        .as_array()?
        .iter()
        .find(|version| version.get("key").and_then(JsonValue::as_str) == Some(selected_version))?
        .get("definition")?
        .get("image")?
        .as_str()
        .map(str::to_owned)
}

fn inherit_image_resolution(
    transaction: &sea_orm::DatabaseTransaction,
    instance: &InstanceRecord,
) -> Result<(), DatabaseError> {
    let Some(source_id) = &instance.clone_source_id else {
        return Ok(());
    };
    let Some(source) = instance_entity::Entity::find_by_id(source_id).one(transaction)? else {
        return Ok(());
    };
    if source.target_id != instance.target_id {
        return Ok(());
    }
    let latest = operation::Entity::find()
        .filter(operation::Column::InstanceId.eq(source_id))
        .filter(operation::Column::Status.eq("Succeeded"))
        .all(transaction)?
        .into_iter()
        .filter_map(|operation| operation.new_spec_revision)
        .max()
        .unwrap_or(1);
    let Some(spec) =
        instance_spec::Entity::find_by_id((source_id.clone(), latest)).one(transaction)?
    else {
        return Ok(());
    };
    if spec.selected_version != instance.selected_version {
        return Ok(());
    }
    let Some(snapshot) = template_snapshot::Entity::find()
        .filter(template_snapshot::Column::InstanceId.eq(&instance.id))
        .one(transaction)?
    else {
        return Ok(());
    };
    let Some(image) = selected_image(&snapshot.canonical_json, &instance.selected_version) else {
        return Ok(());
    };
    let Some(target) = target_entity::Entity::find_by_id(&instance.target_id).one(transaction)?
    else {
        return Ok(());
    };
    for resolution in image_resolution::Entity::find()
        .filter(image_resolution::Column::InstanceId.eq(source_id))
        .filter(image_resolution::Column::SpecRevision.eq(latest))
        .filter(image_resolution::Column::ImageRef.eq(image))
        .filter(image_resolution::Column::Platform.eq(&target.platform))
        .all(transaction)?
    {
        image_resolution::Entity::insert(image_resolution::ActiveModel {
            instance_id: Set(instance.id.clone()),
            spec_revision: Set(1),
            image_ref: Set(resolution.image_ref),
            digest: Set(resolution.digest),
            image_id: Set(resolution.image_id),
            platform: Set(resolution.platform),
            first_operation_id: Set(resolution.first_operation_id),
        })
        .exec(transaction)?;
    }
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
        self.orm_write(move |db| {
            management_scope::Entity::insert(management_scope::ActiveModel {
                id: Set(id),
                owner_id: Set(owner_id),
                root_identity: Set(root_identity),
                ..Default::default()
            })
            .exec(db)?;
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
        self.orm_write(move |db| {
            if target_entity::Entity::find()
                .filter(target_entity::Column::ScopeId.eq(&values.1))
                .one(db)?
                .is_some()
            {
                return Err(DatabaseError::Duplicate);
            }
            target_entity::Entity::insert(target_entity::ActiveModel {
                id: Set(values.0),
                scope_id: Set(values.1),
                endpoint: Set(values.2),
                engine_id: Set(values.3),
                platform: Set(values.4),
            })
            .exec(db)?;
            Ok(())
        })
        .map_err(map_error)
    }

    fn runtime_target(&self, scope_id: &str) -> Result<Option<RuntimeTarget>, StoreConflict> {
        self.orm_read(|db| {
            let mut rows = target_entity::Entity::find()
                .filter(target_entity::Column::ScopeId.eq(scope_id))
                .all(db)?
                .into_iter();
            let target = rows.next().map(|row| RuntimeTarget {
                id: row.id,
                scope_id: row.scope_id,
                endpoint: row.endpoint,
                engine_id: row.engine_id,
                platform: row.platform,
            });
            if rows.next().is_some() {
                return Err(DatabaseError::Duplicate);
            }
            Ok(target)
        })
        .map_err(map_error)
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
        self.orm_write(move |db| insert_template(db, &revision))
            .map_err(map_error)
    }

    fn list_templates(&self) -> Result<Vec<TemplateCatalogItem>, StoreConflict> {
        self.orm_read(|db| {
            Ok(revision_entity::Entity::find()
                .order_by_asc(revision_entity::Column::TemplateId)
                .order_by_asc(revision_entity::Column::TemplateVersion)
                .all(db)?
                .into_iter()
                .map(|row| TemplateCatalogItem {
                    id: row.id,
                    template_id: row.template_id,
                    version: row.template_version,
                    origin: row.origin,
                    semantic_hash: row.semantic_hash,
                    canonical_json: row.canonical_json,
                })
                .collect())
        })
        .map_err(map_error)
    }

    fn clone_source(&self, scope_id: &str, id: &str) -> Result<Option<CloneSource>, StoreConflict> {
        self.orm_read(|db| {
            let owner = instance_entity::Entity::find_by_id(id).one(db)?;
            let Some(owner) =
                owner.filter(|owner| owner.scope_id == scope_id && owner.lifecycle == "managed")
            else {
                return Ok(None);
            };
            let operations = operation::Entity::find()
                .filter(operation::Column::InstanceId.eq(id))
                .all(db)?;
            if operations
                .iter()
                .any(|operation| !matches!(operation.status.as_str(), "Succeeded" | "Abandoned"))
            {
                return Ok(None);
            }
            let spec_revision = operations
                .iter()
                .filter(|operation| operation.status == "Succeeded")
                .filter_map(|operation| operation.new_spec_revision)
                .max()
                .unwrap_or(1);
            let Some(snapshot) = template_snapshot::Entity::find()
                .filter(template_snapshot::Column::InstanceId.eq(id))
                .one(db)?
            else {
                return Ok(None);
            };
            let Some(spec) =
                instance_spec::Entity::find_by_id((id.to_owned(), spec_revision)).one(db)?
            else {
                return Ok(None);
            };
            let ports = port_binding::Entity::find()
                .filter(port_binding::Column::InstanceId.eq(id))
                .filter(port_binding::Column::SpecRevision.eq(spec_revision))
                .order_by_asc(port_binding::Column::Slot)
                .all(db)?
                .into_iter()
                .map(|port| {
                    Ok(PortAllocation {
                        slot: port.slot,
                        host_ip: port.host_ip,
                        host_port: u16::try_from(port.host_port)
                            .map_err(|_| DatabaseError::InvalidInput)?,
                        container_port: u16::try_from(port.container_port)
                            .map_err(|_| DatabaseError::InvalidInput)?,
                    })
                })
                .collect::<Result<Vec<_>, DatabaseError>>()?;
            let storage = storage_allocation::Entity::find()
                .filter(storage_allocation::Column::InstanceId.eq(id))
                .order_by_asc(storage_allocation::Column::Slot)
                .all(db)?
                .into_iter()
                .map(|allocation| StorageAllocation {
                    slot: allocation.slot,
                    resource_identity: allocation.resource_identity,
                    ownership_evidence: allocation.ownership_evidence,
                })
                .collect();
            Ok(Some(CloneSource {
                id: owner.id,
                scope_id: owner.scope_id,
                target_id: owner.target_id,
                revision: u64::try_from(owner.revision).map_err(|_| DatabaseError::InvalidInput)?,
                spec_revision: u64::try_from(spec_revision)
                    .map_err(|_| DatabaseError::InvalidInput)?,
                name: owner.display_name,
                snapshot_json: snapshot.canonical_json,
                selected_version: spec.selected_version,
                storage_method: method_from_name(&spec.storage_method)
                    .ok_or(DatabaseError::InvalidInput)?,
                inputs_json: spec.inputs_json,
                ports,
                storage,
            }))
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
        self.orm_write(move |db| {
            let transaction = db.begin()?;
            insert_instance(&transaction, &instance)?;
            transaction.commit()?;
            Ok(())
        })
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
        self.orm_write(move |db| update_instance(db, &id, expected, Some(&name)))
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
        self.orm_write(move |db| update_instance(db, &id, expected, None))
            .map_err(map_error)?
    }

    fn default_storage_method(&self, scope_id: &str) -> Result<StorageMethod, StoreConflict> {
        self.orm_read(|db| {
            management_scope::Entity::find_by_id(scope_id)
                .one(db)?
                .map(|scope| scope.default_storage_method)
                .ok_or(DatabaseError::Missing)
        })
        .map_err(map_error)
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
        self.orm_write(move |db| {
            let scope = management_scope::Entity::find_by_id(&scope_id)
                .one(db)?
                .ok_or(DatabaseError::Missing)?;
            management_scope::ActiveModel {
                id: Set(scope.id),
                default_storage_method: Set(method_name(method).into()),
                ..Default::default()
            }
            .update(db)?;
            Ok(())
        })
        .map_err(map_error)
    }

    fn storage_allocation(
        &self,
        instance_id: &str,
        slot: &str,
    ) -> Result<Option<StorageLedgerEntry>, StoreConflict> {
        self.orm_read(|db| {
            let allocation =
                storage_allocation::Entity::find_by_id((instance_id.to_owned(), slot.to_owned()))
                    .one(db)?;
            allocation
                .map(|allocation| {
                    let owner = instance_entity::Entity::find_by_id(&allocation.instance_id)
                        .one(db)?
                        .ok_or(DatabaseError::Missing)?;
                    storage_entry(allocation, owner.scope_id)
                })
                .transpose()
        })
        .map_err(map_error)
    }

    fn source_revision(&self, scope_id: &str, id: &str) -> Result<Option<u64>, StoreConflict> {
        self.orm_read(|db| {
            let instance = instance_entity::Entity::find_by_id(id).one(db)?;
            instance
                .filter(|instance| instance.scope_id == scope_id)
                .map(|instance| {
                    u64::try_from(instance.revision).map_err(|_| DatabaseError::InvalidInput)
                })
                .transpose()
        })
        .map_err(map_error)
    }

    fn instance_id_exists(&self, id: &str) -> Result<bool, StoreConflict> {
        self.orm_read(|db| Ok(instance_entity::Entity::find_by_id(id).one(db)?.is_some()))
            .map_err(map_error)
    }

    fn set_storage_presence(
        &self,
        instance_id: &str,
        slot: &str,
        presence: StoragePresence,
    ) -> Result<(), StoreConflict> {
        let (instance_id, slot) = (instance_id.to_owned(), slot.to_owned());
        self.orm_write(move |db| {
            let transaction = db.begin()?;
            let allocation =
                storage_allocation::Entity::find_by_id((instance_id.clone(), slot.clone()))
                    .one(&transaction)?
                    .ok_or(DatabaseError::Missing)?;
            let current =
                presence_from_name(&allocation.presence).ok_or(DatabaseError::InvalidInput)?;
            // The named-volume adapter verifies a successful create step before recording this transition.
            let allowed = match current {
                StoragePresence::NotMaterialized => matches!(
                    presence,
                    StoragePresence::NotMaterialized
                        | StoragePresence::Present
                        | StoragePresence::Missing
                        | StoragePresence::Unverified
                ),
                StoragePresence::Present => matches!(
                    presence,
                    StoragePresence::Present
                        | StoragePresence::Missing
                        | StoragePresence::Unverified
                ),
                StoragePresence::Missing => presence == StoragePresence::Missing,
                StoragePresence::Unverified => matches!(
                    presence,
                    StoragePresence::Unverified
                        | StoragePresence::Present
                        | StoragePresence::Missing
                ),
            };
            if !allowed {
                return Err(DatabaseError::InvalidInput);
            }
            storage_allocation::ActiveModel {
                instance_id: Set(instance_id),
                slot: Set(slot),
                presence: Set(presence_name(presence).into()),
                ..Default::default()
            }
            .update(&transaction)?;
            transaction.commit()?;
            Ok(())
        })
        .map_err(map_error)
    }

    fn advance_storage_initialization(
        &self,
        instance_id: &str,
        slot: &str,
        initialization: Initialization,
    ) -> Result<(), StoreConflict> {
        let (instance_id, slot) = (instance_id.to_owned(), slot.to_owned());
        self.orm_write(move |db| {
            let transaction = db.begin()?;
            let allocation =
                storage_allocation::Entity::find_by_id((instance_id.clone(), slot.clone()))
                    .one(&transaction)?
                    .ok_or(DatabaseError::Missing)?;
            if presence_from_name(&allocation.presence) != Some(StoragePresence::Present) {
                return Err(DatabaseError::InvalidInput);
            }
            let current = initialization_from_name(&allocation.initialization)
                .ok_or(DatabaseError::InvalidInput)?;
            if initialization < current {
                return Err(DatabaseError::InvalidInput);
            }
            storage_allocation::ActiveModel {
                instance_id: Set(instance_id),
                slot: Set(slot),
                initialization: Set(initialization_name(initialization).into()),
                ..Default::default()
            }
            .update(&transaction)?;
            transaction.commit()?;
            Ok(())
        })
        .map_err(map_error)
    }
}

fn update_instance(
    db: &sea_orm::DatabaseConnection,
    id: &str,
    expected: u64,
    name: Option<&str>,
) -> Result<Result<(), StoreConflict>, DatabaseError> {
    let expected = i64::try_from(expected).map_err(|_| DatabaseError::InvalidInput)?;
    let transaction = db.begin()?;
    let current = instance_entity::Entity::find_by_id(id).one(&transaction)?;
    let Some(current) = current else {
        return Ok(Err(StoreConflict::Missing));
    };
    if current.revision != expected {
        return Ok(Err(StoreConflict::StaleRevision));
    }
    if current.lifecycle != "managed" {
        return Ok(Err(StoreConflict::InvalidLifecycle));
    }
    if name.is_some()
        && operation::Entity::find()
            .filter(operation::Column::InstanceId.eq(id))
            .all(&transaction)?
            .iter()
            .any(|operation| !matches!(operation.status.as_str(), "Succeeded" | "Abandoned"))
    {
        return Ok(Err(StoreConflict::UnresolvedOperation));
    }
    let next_revision = current
        .revision
        .checked_add(1)
        .ok_or(DatabaseError::InvalidInput)?;
    let mut active = instance_entity::ActiveModel {
        id: Set(current.id),
        revision: Set(next_revision),
        ..Default::default()
    };
    if let Some(name) = name {
        active.display_name = Set(name.to_owned());
        active.normalized_name = Set(name.to_owned());
    } else {
        active.lifecycle = Set("retired".into());
    }
    active.update(&transaction)?;
    transaction.commit()?;
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use composenest_application::operation_journal::{
        OperationIntent, OperationJournal, OperationKind, PlanCommitStore, RequestReceipt,
    };
    use composenest_application::state_store::{PortAllocation, StorageAllocation};
    use composenest_application::template_catalog::{
        CatalogError, TemplateOrigin, TemplatePackage, register_packages,
    };
    use composenest_domain::instance::{Initialization, StoragePresence};
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

    fn plan_intent(id: &str) -> OperationIntent {
        OperationIntent {
            id: format!("op-{id}"),
            instance_id: id.into(),
            kind: OperationKind::Create,
            phase: "accepted".into(),
            expected_revision: 1,
            old_spec_revision: None,
            new_spec_revision: Some(1),
        }
    }

    fn plan_receipt(id: &str, plan: &str, revision: u64) -> RequestReceipt {
        RequestReceipt {
            scope_id: "scope".into(),
            request_id: format!("request-{id}"),
            plan_id: Some(plan.into()),
            confirmed_revision: revision,
            request_hash: "a".repeat(64),
            instance_id: id.into(),
            operation_id: format!("op-{id}"),
        }
    }

    fn plan_target() -> RuntimeTarget {
        RuntimeTarget {
            id: "target".into(),
            scope_id: "scope".into(),
            endpoint: "unix:///var/run/docker.sock".into(),
            engine_id: "engine".into(),
            platform: "linux".into(),
        }
    }

    #[test]
    fn concurrent_plan_retries_return_one_durable_result() {
        use std::sync::Arc;
        let (root, worker) = store();
        worker.register_template(&revision()).unwrap();
        let worker = Arc::new(worker);
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let worker = Arc::clone(&worker);
                std::thread::spawn(move || {
                    worker.commit_plan(
                        &instance("one", "One", 13000),
                        &plan_intent("one"),
                        &plan_receipt("one", "plan", 1),
                        &plan_target(),
                        None,
                    )
                })
            })
            .collect();
        for thread in threads {
            assert_eq!(thread.join().unwrap(), Ok(plan_receipt("one", "plan", 1)));
        }
        assert_eq!(count(&worker, "instances"), 1);
        assert_eq!(count(&worker, "operations"), 1);
        drop(worker);
        let reopened = DatabaseWorker::start(root.path()).unwrap();
        assert_eq!(
            reopened.plan_receipt("scope", "plan"),
            Ok(Some(plan_receipt("one", "plan", 1)))
        );
        assert_eq!(
            reopened.commit_plan(
                &instance("later", "Later", 13001),
                &plan_intent("later"),
                &plan_receipt("later", "plan", 2),
                &plan_target(),
                None
            ),
            Err(StoreConflict::Duplicate)
        );
    }

    #[test]
    fn plan_conflicts_rollback_every_record() {
        let (_root, worker) = store();
        worker.register_template(&revision()).unwrap();
        worker
            .commit_plan(
                &instance("one", "One", 13000),
                &plan_intent("one"),
                &plan_receipt("one", "first", 1),
                &plan_target(),
                None,
            )
            .unwrap();
        for (id, name, port) in [("name", "One", 13001), ("port", "Other", 13000)] {
            assert_eq!(
                worker.commit_plan(
                    &instance(id, name, port),
                    &plan_intent(id),
                    &plan_receipt(id, id, 1),
                    &plan_target(),
                    None
                ),
                Err(StoreConflict::Duplicate)
            );
            assert_eq!(worker.plan_receipt("scope", id), Ok(None));
        }
        let mut changed_target = plan_target();
        changed_target.engine_id = "another-engine".into();
        assert_eq!(
            worker.commit_plan(
                &instance("target", "Target", 13001),
                &plan_intent("target"),
                &plan_receipt("target", "target", 1),
                &changed_target,
                None
            ),
            Err(StoreConflict::StaleRevision)
        );
        let mut clone = instance("clone", "Clone", 13001);
        clone.clone_source_id = Some("one".into());
        let mut clone_intent = plan_intent("clone");
        clone_intent.kind = OperationKind::Clone;
        assert_eq!(
            worker.commit_plan(
                &clone,
                &clone_intent,
                &plan_receipt("clone", "clone", 1),
                &plan_target(),
                Some(
                    &composenest_application::operation_journal::CloneSourceGuard {
                        instance_id: "one".into(),
                        revision: 1,
                        spec_revision: 1,
                    }
                )
            ),
            Err(StoreConflict::InvalidLifecycle)
        );
        for table in [
            "instances",
            "template_snapshots",
            "instance_specs",
            "port_bindings",
            "port_reservations",
            "storage_allocations",
            "operations",
            "request_receipts",
        ] {
            assert_eq!(count(&worker, table), 1, "{table}");
        }
    }

    #[test]
    fn create_plan_confirms_only_current_ready_revision() {
        use composenest_application::create_plan::{
            CommitCreate, CreatePlans, PrepareCreate, get_plan_commit,
        };
        use composenest_application::host_ports::{PortCheck, PortInspector};
        use std::time::SystemTime;

        struct Available;
        impl PortInspector for Available {
            fn inspect(&self, _port: u16) -> PortCheck {
                PortCheck {
                    result: Ok(()),
                    observed_at: SystemTime::now(),
                }
            }
        }
        let (_root, worker) = store();
        let mut template = revision();
        template.canonical_json = r#"{"manifest":{"id":"redis","schemaVersion":1,"templateVersion":"1","defaultVersion":"8","versions":["8"]},"normalization":"template-normalization-v1","versions":[{"key":"8","definition":{"inputs":{"order":[],"values":{}},"service":{"ports":{"order":["main"],"values":{"main":{"container":6379,"defaultHost":13000}}},"storage":{"order":["data"],"values":{"data":{"container":"/data"}}}}}}]}"#.into();
        template.semantic_hash =
            format!("{:x}", Sha256::digest(template.canonical_json.as_bytes()));
        template.id = format!("redis:1:{}", template.semantic_hash);
        worker.register_template(&template).unwrap();
        let mut plans = CreatePlans::default();
        let view = plans
            .prepare_create(
                &PrepareCreate {
                    scope_id: "scope".into(),
                    display_name: "New".into(),
                    template_revision_id: template.id,
                    version: None,
                },
                &worker,
                &crate::SystemClock,
                &mut crate::SystemRandom,
                &Available,
            )
            .unwrap();
        assert!(view.concerns.is_empty());
        let request = |revision| CommitCreate {
            plan_id: view.plan_id.clone(),
            revision,
            scope_id: "scope".into(),
            request_id: "request-new".into(),
            target_id: "target".into(),
            instance_id: "123456789012345678901234567890ab".into(),
            operation_id: "op-new".into(),
            confirmed_ports: view.ports.clone(),
            storage: vec![StorageAllocation {
                slot: "data".into(),
                resource_identity: "data/new".into(),
                ownership_evidence: "proof".into(),
            }],
        };
        assert_eq!(
            plans
                .commit_plan(request(2), &worker, &crate::SystemClock, &Available)
                .unwrap_err()
                .code,
            "PLAN_STALE"
        );
        let mut unconfirmed = request(1);
        unconfirmed.confirmed_ports.clear();
        assert_eq!(
            plans
                .commit_plan(unconfirmed, &worker, &crate::SystemClock, &Available)
                .unwrap_err()
                .code,
            "PLAN_RECONFIRM"
        );
        let committed = plans
            .commit_plan(request(1), &worker, &crate::SystemClock, &Available)
            .unwrap();
        assert_eq!(
            get_plan_commit(&worker, "scope", &view.plan_id).unwrap(),
            Some(committed.clone())
        );
        assert_eq!(
            plans.commit_plan(request(1), &worker, &crate::SystemClock, &Available),
            Ok(committed)
        );
        assert_eq!(
            plans
                .commit_plan(request(2), &worker, &crate::SystemClock, &Available)
                .unwrap_err()
                .code,
            "REQUEST_ALREADY_USED"
        );
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
    fn clone_reads_committed_source_and_copies_snapshot_without_catalog() {
        let (_root, worker) = store();
        let template = revision();
        worker.register_template(&template).unwrap();
        let mut original = instance("one", "One", 6379);
        original.inputs_json = r#"{"password":"secret"}"#.into();
        worker.commit_instance(&original).unwrap();
        worker
            .write(|db| {
                db.execute("DELETE FROM template_revision_files", [])?;
                db.execute("DELETE FROM template_revisions", [])?;
                Ok(())
            })
            .unwrap();

        let source = worker.clone_source("scope", "one").unwrap().unwrap();
        assert_eq!(source.revision, 1);
        assert_eq!(source.snapshot_json, template.canonical_json);
        assert_eq!(source.inputs_json, original.inputs_json);
        assert_eq!(source.ports[0].host_port, 6379);
        assert_eq!(source.storage[0].resource_identity, "data/one");

        let mut clone = instance("two", "Two", 6380);
        clone.clone_source_id = Some(source.id);
        worker.commit_instance(&clone).unwrap();
        let (snapshot, files): (String, i64) =
            worker
                .read(|db| {
                    Ok((db.query_row(
                "SELECT canonical_json FROM template_snapshots WHERE instance_id = 'two'",
                [], |row| row.get(0))?,
                db.query_row(
                    "SELECT count(*) FROM template_snapshot_files WHERE snapshot_id = 'two'",
                    [], |row| row.get(0))?))
                })
                .unwrap();
        assert_eq!(snapshot, template.canonical_json);
        assert_eq!(files, 2);
    }

    #[test]
    fn clone_source_requires_managed_source_without_unresolved_operation() {
        let (_root, worker) = store();
        worker.register_template(&revision()).unwrap();
        worker
            .commit_instance(&instance("one", "One", 6379))
            .unwrap();
        assert!(worker.clone_source("scope", "one").unwrap().is_some());
        worker.write(|db| {
            db.execute(
                "INSERT INTO operations (id, instance_id, kind, phase, expected_instance_revision) VALUES ('busy', 'one', 'stop', 'accepted', 1)",
                [],
            )?;
            Ok(())
        }).unwrap();
        assert!(worker.clone_source("scope", "one").unwrap().is_none());
        assert!(worker.clone_source("other", "one").unwrap().is_none());
    }

    #[test]
    fn clone_source_ignores_abandoned_pending_spec() {
        let (_root, worker) = store();
        worker.register_template(&revision()).unwrap();
        worker
            .commit_instance(&instance("one", "One", 6379))
            .unwrap();
        worker.write(|db| {
            db.execute("INSERT INTO instance_specs VALUES ('one', 2, '8', 'bind', '{\"discarded\":true}')", [])?;
            db.execute("INSERT INTO port_bindings VALUES ('one', 2, 'main', '127.0.0.1', 6380, 6379)", [])?;
            db.execute("INSERT INTO operations (id, instance_id, kind, status, phase, expected_instance_revision, old_spec_revision, new_spec_revision, completed_at) \
                VALUES ('discarded', 'one', 'edit_port', 'Abandoned', 'done', 1, 1, 2, CURRENT_TIMESTAMP)", [])?;
            db.execute("INSERT INTO pending_changes VALUES ('discarded', 'one', 1, 2, 'diff')", [])?;
            Ok(())
        }).unwrap();
        let source = worker.clone_source("scope", "one").unwrap().unwrap();
        assert_eq!(source.spec_revision, 1);
        assert_eq!(source.inputs_json, "{}");
        assert_eq!(source.ports[0].host_port, 6379);
        worker.write(|db| {
            db.execute("INSERT INTO instance_specs VALUES ('one', 3, '8', 'bind', '{\"adopted\":true}')", [])?;
            db.execute("INSERT INTO port_bindings VALUES ('one', 3, 'main', '127.0.0.1', 6390, 6379)", [])?;
            db.execute("INSERT INTO operations (id, instance_id, kind, status, phase, expected_instance_revision, old_spec_revision, new_spec_revision, completed_at) \
                VALUES ('adopted', 'one', 'edit_port', 'Succeeded', 'done', 1, 1, 3, CURRENT_TIMESTAMP)", [])?;
            Ok(())
        }).unwrap();
        let adopted = worker.clone_source("scope", "one").unwrap().unwrap();
        assert_eq!(adopted.spec_revision, 3);
        assert_eq!(adopted.inputs_json, "{\"adopted\":true}");
        assert_eq!(adopted.ports[0].host_port, 6390);
    }

    #[test]
    fn clone_commit_rejects_changed_committed_spec_without_instance_revision_change() {
        let (_root, worker) = store();
        worker.register_template(&revision()).unwrap();
        worker
            .commit_instance(&instance("one", "One", 13000))
            .unwrap();
        worker.write(|db| {
            db.execute("INSERT INTO instance_specs VALUES ('one', 2, '8', 'bind', '{}')", [])?;
            db.execute("INSERT INTO operations (id, instance_id, kind, status, phase, expected_instance_revision, old_spec_revision, new_spec_revision, completed_at) \
                VALUES ('updated', 'one', 'edit_port', 'Succeeded', 'done', 1, 1, 2, CURRENT_TIMESTAMP)", [])?;
            Ok(())
        }).unwrap();
        let mut clone = instance("clone", "Clone", 13001);
        clone.clone_source_id = Some("one".into());
        let mut intent = plan_intent("clone");
        intent.kind = OperationKind::Clone;
        assert_eq!(
            worker.commit_plan(
                &clone,
                &intent,
                &plan_receipt("clone", "clone", 1),
                &plan_target(),
                Some(
                    &composenest_application::operation_journal::CloneSourceGuard {
                        instance_id: "one".into(),
                        revision: 1,
                        spec_revision: 1,
                    }
                )
            ),
            Err(StoreConflict::StaleRevision)
        );
        assert_eq!(count(&worker, "instances"), 1);
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

    #[test]
    fn storage_ledger_tracks_presence_and_monotonic_initialization() {
        let (_root, worker) = store();
        worker.register_template(&revision()).unwrap();
        worker
            .commit_instance(&instance("one", "One", 6379))
            .unwrap();

        let initial = worker.storage_allocation("one", "data").unwrap().unwrap();
        assert_eq!(initial.presence, StoragePresence::NotMaterialized);
        assert_eq!(initial.initialization, Initialization::NotAttempted);
        assert_eq!(initial.allocation.resource_identity, "data/one");
        assert_eq!(initial.scope_id, "scope");

        assert_eq!(
            worker.advance_storage_initialization(
                "one",
                "data",
                Initialization::MayHaveInitialized
            ),
            Err(StoreConflict::InvalidInput)
        );
        worker
            .set_storage_presence("one", "data", StoragePresence::Present)
            .unwrap();
        worker
            .advance_storage_initialization("one", "data", Initialization::MayHaveInitialized)
            .unwrap();
        worker
            .advance_storage_initialization("one", "data", Initialization::ReadyObserved)
            .unwrap();
        worker
            .set_storage_presence("one", "data", StoragePresence::Missing)
            .unwrap();

        assert_eq!(
            worker.set_storage_presence("one", "data", StoragePresence::Present),
            Err(StoreConflict::InvalidInput)
        );
        assert_eq!(
            worker.advance_storage_initialization("one", "data", Initialization::NotAttempted),
            Err(StoreConflict::InvalidInput)
        );
        let missing = worker.storage_allocation("one", "data").unwrap().unwrap();
        assert_eq!(missing.presence, StoragePresence::Missing);
        assert_eq!(missing.initialization, Initialization::ReadyObserved);
    }
}
