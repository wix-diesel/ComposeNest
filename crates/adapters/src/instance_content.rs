//! Scoped explicit content reads; no sensitive result is logged or emitted as an event.

use crate::{
    artifact_store::{ArtifactError, ArtifactStore},
    query_service::read_instance,
    sqlite::{DatabaseError, DatabaseWorker},
    state_store::map_error,
};
use composenest_application::{
    instance_content::{
        InstanceComposeRequest, InstanceComposeView, InstanceSecretRequest, InstanceSecretView,
    },
    query_service::InstanceView,
    state_store::StoreConflict,
};
use rusqlite::{OptionalExtension, params};
use serde_json::Value;

fn saved(
    database: &DatabaseWorker,
    scope: &str,
    id: &str,
    revision: u64,
) -> Result<(InstanceView, Value), StoreConflict> {
    database
        .read(|db| {
            db.query_row(
                "SELECT 1 FROM instances WHERE id=?1 AND scope_id=?2 AND lifecycle!='retired'",
                params![id, scope],
                |_| Ok(()),
            )
            .optional()?
            .ok_or(DatabaseError::Missing)?;
            let view = read_instance(db, scope, id)?;
            let inputs: String = db.query_row(
                "SELECT inputs_json FROM instance_specs WHERE instance_id=?1 AND revision=?2",
                params![id, view.spec_revision],
                |row| row.get(0),
            )?;
            Ok((
                view,
                serde_json::from_str(&inputs).map_err(|_| DatabaseError::InvalidInput)?,
            ))
        })
        .map_err(map_error)
        .and_then(|(view, inputs)| {
            if view.spec_revision != revision {
                return Err(StoreConflict::StaleRevision);
            }
            Ok((view, inputs))
        })
}

/// Resolves one saved secret input used by a connection in the committed template.
pub fn secret(
    database: &DatabaseWorker,
    scope: &str,
    request: &InstanceSecretRequest,
) -> Result<InstanceSecretView, StoreConflict> {
    let (view, inputs) = saved(
        database,
        scope,
        &request.instance_id,
        request.expected_spec_revision,
    )?;
    if !view
        .inputs
        .iter()
        .any(|input| input.slot == request.slot && input.secret)
        || !view
            .connections
            .iter()
            .any(|connection| connection.input_slots.contains(&request.slot))
    {
        return Err(StoreConflict::InvalidInput);
    }
    let value = inputs[&request.slot]
        .as_str()
        .ok_or(StoreConflict::InvalidInput)?
        .to_owned();
    Ok(InstanceSecretView {
        instance_id: view.id,
        spec_revision: view.spec_revision,
        slot: request.slot.clone(),
        value,
    })
}

/// Reads the selected artifact, preserving raw bytes or masking known secret encodings.
pub fn compose(
    database: &DatabaseWorker,
    scope: &str,
    request: &InstanceComposeRequest,
) -> Result<InstanceComposeView, StoreConflict> {
    let (view, inputs) = saved(
        database,
        scope,
        &request.instance_id,
        request.expected_spec_revision,
    )?;
    let artifact = database.selected_artifact(&view.id, view.spec_revision)?;
    // A selected identity must still belong to the scoped instance and revision.
    database.read(|db| {
        db.query_row("SELECT 1 FROM artifacts WHERE id=?1 AND instance_id=?2 AND spec_revision=?3 AND placement='published'",
            params![artifact, view.id, view.spec_revision], |_| Ok(())).optional()?.ok_or(DatabaseError::Missing)?;
        Ok(())
    }).map_err(|error| match error {
        DatabaseError::Missing => StoreConflict::ArtifactUnavailable,
        error => map_error(error),
    })?;
    let (path, mut content) = ArtifactStore::new(database.management_root(), database)
        .read_compose(&artifact)
        .map_err(content_error)?;
    if !request.reveal {
        let secrets = view
            .inputs
            .iter()
            .filter(|input| input.secret)
            .map(|input| match &inputs[&input.slot] {
                Value::Null => Ok(String::new()),
                Value::String(value) => Ok(value.clone()),
                _ => Err(StoreConflict::InvalidInput),
            })
            .collect::<Result<Vec<_>, _>>()?;
        content = composenest_domain::compose::mask_yaml(&content, &secrets)
            .map_err(|_| StoreConflict::ArtifactInvalid)?;
    }
    // Reject a concurrent configuration change before returning sensitive content.
    let current = saved(
        database,
        scope,
        &request.instance_id,
        request.expected_spec_revision,
    )?
    .0;
    if current.revision != view.revision
        || database.selected_artifact(&view.id, view.spec_revision)? != artifact
    {
        return Err(StoreConflict::StaleRevision);
    }
    Ok(InstanceComposeView {
        instance_id: view.id,
        spec_revision: view.spec_revision,
        masked: !request.reveal,
        content,
        path: path.to_string_lossy().into_owned(),
    })
}

fn content_error(error: ArtifactError) -> StoreConflict {
    match error {
        ArtifactError::Modified | ArtifactError::UnsafePath(_) | ArtifactError::Conflict => {
            StoreConflict::ArtifactModified
        }
        ArtifactError::Unavailable => StoreConflict::ArtifactUnavailable,
        ArtifactError::InvalidInput => StoreConflict::ArtifactInvalid,
        ArtifactError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            StoreConflict::ArtifactUnavailable
        }
        ArtifactError::Database(error) => map_error(error),
        ArtifactError::Io(_) => StoreConflict::Backend,
    }
}
