//! SQLite-backed image evidence, separate from the mutable request reference.

use composenest_application::{
    image_resolution::{ImageResolution, ImageResolutionStore, valid_hash},
    state_store::StoreConflict,
};
use rusqlite::{OptionalExtension, params};

use crate::{sqlite::DatabaseWorker, state_store::map_error};

impl ImageResolutionStore for DatabaseWorker {
    fn image_resolution(
        &self,
        id: &str,
        revision: u64,
    ) -> Result<Option<ImageResolution>, StoreConflict> {
        let revision = i64::try_from(revision).map_err(|_| StoreConflict::InvalidInput)?;
        self.read(|db| {
            db.query_row(
                "SELECT instance_id, spec_revision, image_ref, digest, image_id, platform, first_operation_id \
                 FROM image_resolutions WHERE instance_id = ?1 AND spec_revision = ?2",
                params![id, revision],
                |row| Ok(ImageResolution {
                    instance_id: row.get(0)?, spec_revision: row.get::<_, i64>(1)? as u64, requested: row.get(2)?,
                    digest: row.get(3)?, image_id: row.get(4)?, platform: row.get(5)?, operation_id: row.get(6)?,
                }),
            ).optional().map_err(Into::into)
        }).map_err(map_error)
    }

    fn record_image_resolution(&self, resolution: &ImageResolution) -> Result<(), StoreConflict> {
        if resolution.spec_revision == 0
            || resolution.spec_revision > i64::MAX as u64
            || resolution.requested.is_empty()
            || !valid_hash(&resolution.image_id)
            || !resolution
                .digest
                .rsplit_once('@')
                .is_some_and(|(_, hash)| valid_hash(hash))
        {
            return Err(StoreConflict::InvalidInput);
        }
        let resolution = resolution.clone();
        self.write(move |db| {
            let tx = db.transaction()?;
            let expected: Option<(String, String)> = tx.query_row(
                "SELECT json_extract(v.value, '$.definition.image'), t.platform \
                 FROM instances i JOIN runtime_targets t ON t.id = i.target_id \
                 JOIN template_snapshots s ON s.instance_id = i.id \
                 JOIN instance_specs spec ON spec.instance_id = i.id \
                 JOIN json_each(s.canonical_json, '$.versions') v \
                 JOIN operations o ON o.id = ?3 AND o.instance_id = i.id \
                     AND o.status IN ('Accepted', 'Executing') \
                     AND COALESCE(o.new_spec_revision, o.old_spec_revision) = spec.revision \
                 JOIN operation_steps step ON step.operation_id = o.id \
                     AND step.attempt = o.attempt AND step.outcome IS NULL \
                     AND step.command_kind = 'resolve_image' AND step.expected_result = 'image_resolved' \
                 WHERE i.id = ?1 AND spec.revision = ?2 AND v.value ->> '$.key' = spec.selected_version",
                params![resolution.instance_id, resolution.spec_revision as i64, resolution.operation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            if expected.as_ref() != Some(&(resolution.requested.clone(), resolution.platform.clone())) {
                return Err(rusqlite::Error::InvalidQuery.into());
            }
            let current: Option<(String, String, String, String, String)> = tx.query_row(
                "SELECT image_ref, digest, image_id, platform, first_operation_id FROM image_resolutions \
                 WHERE instance_id = ?1 AND spec_revision = ?2",
                params![resolution.instance_id, resolution.spec_revision as i64],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            ).optional()?;
            if let Some(current) = current {
                if current != (resolution.requested, resolution.digest, resolution.image_id, resolution.platform, resolution.operation_id) {
                    return Err(rusqlite::Error::InvalidQuery.into());
                }
                return Ok(());
            }
            tx.execute("INSERT INTO image_resolutions VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![resolution.instance_id, resolution.spec_revision as i64, resolution.requested,
                    resolution.digest, resolution.image_id, resolution.platform, resolution.operation_id])?;
            tx.commit()?;
            Ok(())
        }).map_err(|error| match error {
            crate::sqlite::DatabaseError::Sqlite(rusqlite::Error::InvalidQuery) => StoreConflict::InvalidInput,
            other => map_error(other),
        })
    }
}
