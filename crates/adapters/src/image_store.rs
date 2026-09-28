//! Persisted image evidence, separate from the mutable request reference.

use composenest_application::{
    image_resolution::{ImageResolution, ImageResolutionStore, valid_hash},
    state_store::StoreConflict,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set, TransactionTrait};

use crate::{
    entities::{
        image_resolution, instance, instance_spec, operation, operation_step, runtime_target,
        template_snapshot,
    },
    sqlite::{DatabaseError, DatabaseWorker},
    state_store::map_error,
};

impl ImageResolutionStore for DatabaseWorker {
    fn image_resolution(
        &self,
        id: &str,
        revision: u64,
    ) -> Result<Option<ImageResolution>, StoreConflict> {
        let revision = i64::try_from(revision).map_err(|_| StoreConflict::InvalidInput)?;
        self.orm_read(|db| {
            let stored = image_resolution::Entity::find()
                .filter(image_resolution::Column::InstanceId.eq(id))
                .filter(image_resolution::Column::SpecRevision.eq(revision))
                .one(db)?;
            stored.map(image_resolution_from_model).transpose()
        })
        .map_err(map_error)
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
        self.orm_write(move |db| {
            let tx = db.begin()?;
            let revision = resolution.spec_revision as i64;
            let owner = instance::Entity::find_by_id(&resolution.instance_id).one(&tx)?;
            let Some(owner) = owner else {
                return Err(DatabaseError::InvalidInput);
            };
            let target = runtime_target::Entity::find_by_id(&owner.target_id).one(&tx)?;
            let snapshot = template_snapshot::Entity::find()
                .filter(template_snapshot::Column::InstanceId.eq(&resolution.instance_id))
                .one(&tx)?;
            let spec =
                instance_spec::Entity::find_by_id((resolution.instance_id.clone(), revision))
                    .one(&tx)?;
            let operation = operation::Entity::find_by_id(&resolution.operation_id).one(&tx)?;
            let valid_operation = operation.as_ref().is_some_and(|operation| {
                operation.instance_id == resolution.instance_id
                    && matches!(operation.status.as_str(), "Accepted" | "Executing")
                    && operation.new_spec_revision.or(operation.old_spec_revision) == Some(revision)
            });
            let valid_step = operation_step::Entity::find()
                .filter(operation_step::Column::OperationId.eq(&resolution.operation_id))
                .filter(
                    operation_step::Column::Attempt
                        .eq(operation.as_ref().map_or(0, |operation| operation.attempt)),
                )
                .filter(operation_step::Column::Outcome.is_null())
                .filter(operation_step::Column::CommandKind.eq("resolve_image"))
                .filter(operation_step::Column::ExpectedResult.eq("image_resolved"))
                .one(&tx)?
                .is_some();
            let expected_image =
                snapshot
                    .as_ref()
                    .zip(spec.as_ref())
                    .and_then(|(snapshot, spec)| {
                        let value: serde_json::Value =
                            serde_json::from_str(&snapshot.canonical_json).ok()?;
                        value
                            .get("versions")?
                            .as_array()?
                            .iter()
                            .find(|version| {
                                version.get("key").and_then(serde_json::Value::as_str)
                                    == Some(&spec.selected_version)
                            })?
                            .get("definition")?
                            .get("image")?
                            .as_str()
                            .map(str::to_owned)
                    });
            if !valid_operation
                || !valid_step
                || expected_image.as_deref() != Some(&resolution.requested)
                || target.as_ref().map(|target| target.platform.as_str())
                    != Some(&resolution.platform)
            {
                return Err(DatabaseError::InvalidInput);
            }
            let current = image_resolution::Entity::find()
                .filter(image_resolution::Column::InstanceId.eq(&resolution.instance_id))
                .filter(image_resolution::Column::SpecRevision.eq(revision))
                .one(&tx)?;
            if let Some(current) = current {
                if image_resolution_from_model(current)? != resolution {
                    return Err(DatabaseError::InvalidInput);
                }
                return Ok(());
            }
            image_resolution::Entity::insert(image_resolution::ActiveModel {
                instance_id: Set(resolution.instance_id),
                spec_revision: Set(revision),
                image_ref: Set(resolution.requested),
                digest: Set(resolution.digest),
                image_id: Set(resolution.image_id),
                platform: Set(resolution.platform),
                first_operation_id: Set(resolution.operation_id),
            })
            .exec(&tx)?;
            tx.commit()?;
            Ok(())
        })
        .map_err(|error| match error {
            DatabaseError::InvalidInput => StoreConflict::InvalidInput,
            other => map_error(other),
        })
    }
}

fn image_resolution_from_model(
    model: image_resolution::Model,
) -> Result<ImageResolution, DatabaseError> {
    Ok(ImageResolution {
        instance_id: model.instance_id,
        spec_revision: u64::try_from(model.spec_revision)
            .map_err(|_| DatabaseError::InvalidInput)?,
        requested: model.image_ref,
        digest: model.digest,
        image_id: model.image_id,
        platform: model.platform,
        operation_id: model.first_operation_id,
    })
}
