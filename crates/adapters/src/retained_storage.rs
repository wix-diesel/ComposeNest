//! Locations and fresh storage observations for data-preserving deletion.

use crate::{
    artifact_store::validate_id,
    create_projection::parse_instance_id,
    entities::{instance, storage_allocation},
    query_service::read_instance,
    sqlite::{DatabaseError, DatabaseWorker},
    state_store::{map_error, presence_name},
    storage::BindStorage,
};
use composenest_application::{
    delete_operation::StorageCheck,
    retained_storage::{RetainedArtifact, RetainedInstance, RetainedLocation, RetainedStore},
    state_store::{StorageLedgerEntry, StorageMethod, StoreConflict},
    storage::StoragePort,
};
use composenest_domain::{identity::SlotId, instance::StoragePresence};
use rusqlite::params;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, TransactionTrait, sea_query::Expr};
use std::{fs, io, path::Path};

impl RetainedStore for DatabaseWorker {
    fn list_retained_storage(
        &self,
        scope_id: &str,
    ) -> Result<Vec<RetainedInstance>, StoreConflict> {
        let root = self.management_root();
        self.read(|db| {
            let mut query = db.prepare("SELECT i.id, o.completed_at FROM instances i JOIN operations o ON o.instance_id=i.id AND o.kind='delete' AND o.status='Succeeded' WHERE i.scope_id=?1 AND i.lifecycle='retired' ORDER BY o.completed_at DESC, i.id")?;
            let ids = query.query_map([scope_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
            ids.into_iter().map(|(id, deleted_at)| {
                parse_instance_id(&id).map_err(|_| DatabaseError::InvalidInput)?;
                let view = read_instance(db, scope_id, &id)?;
                let (snapshot_id, canonical): (String, String) = db.query_row("SELECT id, canonical_json FROM template_snapshots WHERE instance_id=?1", [&id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                let canonical: serde_json::Value = serde_json::from_str(&canonical).map_err(|_| DatabaseError::InvalidInput)?;
                let service_name = canonical["manifest"]["name"].as_str().unwrap_or(&view.template_id).to_owned();
                let mut locations = Vec::new();
                for storage in &view.storage {
                    let (resource, ownership, observed_at): (String, String, Option<String>) = db.query_row("SELECT resource_identity, ownership, observed_at FROM storage_allocations WHERE instance_id=?1 AND slot=?2", params![id, storage.slot], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                    SlotId::parse(&storage.slot).map_err(|_| DatabaseError::InvalidInput)?;
                    let location = if storage.method == "bind" {
                        if resource != format!("data/{id}/{}", storage.slot) { return Err(DatabaseError::InvalidInput); }
                        root.join(resource).to_string_lossy().into_owned()
                    } else { resource };
                    locations.push(RetainedLocation { storage: storage.clone(), location, ownership, ownership_verified: storage.presence == "present", observed_at });
                }
                let mut query = db.prepare("SELECT id, spec_revision, placement FROM artifacts WHERE instance_id=?1 ORDER BY spec_revision, id")?;
                let artifacts = query.query_map([&id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?, r.get::<_, String>(2)?)))?
                    .map(|row| {
                        let (artifact, spec_revision, placement) = row?;
                        validate_id(&artifact).map_err(|_| DatabaseError::InvalidInput)?;
                        let directory = (placement != "staged").then(|| root.join("instances").join(&id).join("artifacts").join(&artifact).to_string_lossy().into_owned());
                        Ok(RetainedArtifact { id: artifact, spec_revision, placement, directory })
                    }).collect::<Result<Vec<_>, DatabaseError>>()?;
                Ok(RetainedInstance { instance: view, service_name, deleted_at, locations, settings_database: root.join("state/composenest.sqlite").to_string_lossy().into_owned(), snapshot_id, artifacts })
            }).collect()
        }).map_err(map_error)
    }

    fn update_retained_checks(
        &self,
        scope_id: &str,
        instance_id: &str,
        expected_revision: u64,
        checks: &[StorageCheck],
    ) -> Result<(), StoreConflict> {
        let (scope_id, id, checks) = (scope_id.to_owned(), instance_id.to_owned(), checks.to_vec());
        self.orm_write(move |db| {
            let tx = db.begin()?;
            let owned = instance::Entity::find_by_id(&id)
                .one(&tx)?
                .ok_or(DatabaseError::Missing)?;
            if owned.scope_id != scope_id
                || owned.lifecycle != "retired"
                || u64::try_from(owned.revision).ok() != Some(expected_revision)
            {
                return Err(DatabaseError::InvalidInput);
            }
            let allocations = storage_allocation::Entity::find()
                .filter(storage_allocation::Column::InstanceId.eq(&id))
                .all(&tx)?;
            if allocations.len() != checks.len()
                || allocations.iter().any(|allocation| {
                    checks
                        .iter()
                        .filter(|check| check.slot == allocation.slot)
                        .count()
                        != 1
                })
            {
                return Err(DatabaseError::InvalidInput);
            }
            for check in checks {
                storage_allocation::Entity::update_many()
                    .col_expr(
                        storage_allocation::Column::Presence,
                        Expr::value(presence_name(check.presence)),
                    )
                    .col_expr(
                        storage_allocation::Column::ObservedAt,
                        Expr::cust("CURRENT_TIMESTAMP"),
                    )
                    .filter(storage_allocation::Column::InstanceId.eq(&id))
                    .filter(storage_allocation::Column::Slot.eq(check.slot))
                    .exec(&tx)?;
            }
            tx.commit()?;
            Ok(())
        })
        .map_err(map_error)
    }
}

/// Observes a bind allocation without creating a never-materialized directory.
pub fn inspect_saved_bind(root: &Path, entry: &StorageLedgerEntry) -> StoragePresence {
    if !root.is_absolute() {
        return StoragePresence::Unverified;
    }
    let Ok(id) = parse_instance_id(&entry.instance_id) else {
        return StoragePresence::Unverified;
    };
    if entry.method != StorageMethod::Bind
        || SlotId::parse(&entry.slot).is_err()
        || entry.allocation.resource_identity
            != format!("data/{}/{}", entry.instance_id, entry.slot)
    {
        return StoragePresence::Unverified;
    }
    if entry.presence == StoragePresence::NotMaterialized
        && entry.allocation.ownership_evidence.is_empty()
    {
        let mut path = root.to_owned();
        for component in ["", "data", &entry.instance_id, &entry.slot] {
            path.push(component);
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return StoragePresence::NotMaterialized;
                }
                Ok(meta) if plain_directory(&meta) => {}
                _ => return StoragePresence::Unverified,
            }
        }
        return StoragePresence::Unverified;
    }
    BindStorage::new(root).inspect_bind(id, &entry.allocation)
}

fn plain_directory(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.is_dir()
            && meta.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                == 0
    }
    #[cfg(not(windows))]
    {
        meta.is_dir() && !meta.file_type().is_symlink()
    }
}
