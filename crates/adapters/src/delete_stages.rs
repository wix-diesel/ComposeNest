//! Fixed-target wiring for deletion and read-only retained storage refresh.

use crate::{
    docker_delete::{DeletionOwner, remove_owned_runtime, verify_runtime_absent},
    docker_target::DockerProbe,
    entities::{instance, runtime_observation, runtime_target, storage_allocation},
    named_volumes::DockerNamedVolumes,
    retained_storage::inspect_saved_bind,
    sqlite::{DatabaseError, DatabaseWorker},
    state_store::{map_error, storage_entry},
};
use composenest_application::{
    delete_operation::{DeleteError, DeleteOperation, DeleteStages, StorageCheck},
    lifecycle_operation::LifecycleEffectError,
    operation_journal::{OperationIntent, RequestReceipt},
    operation_runner::OperationRunner,
    retained_storage::{RetainedInstance, RetainedStore},
    state_store::{RuntimeTarget, StorageLedgerEntry, StorageMethod, StoreConflict},
};
use composenest_domain::instance::StoragePresence;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};

struct Context {
    project: String,
    target: RuntimeTarget,
    container: Option<String>,
    storage: Vec<StorageLedgerEntry>,
}

fn context(database: &DatabaseWorker, scope: &str, id: &str) -> Result<Context, StoreConflict> {
    database
        .orm_read(|db| {
            let owned = instance::Entity::find_by_id(id)
                .one(db)?
                .filter(|owned| owned.scope_id == scope)
                .ok_or(DatabaseError::Missing)?;
            let target = runtime_target::Entity::find_by_id(&owned.target_id)
                .one(db)?
                .filter(|target| target.scope_id == scope)
                .ok_or(DatabaseError::Missing)?;
            let container = runtime_observation::Entity::find_by_id(id)
                .one(db)?
                .and_then(|observation| observation.container_id);
            let storage = storage_allocation::Entity::find()
                .filter(storage_allocation::Column::InstanceId.eq(id))
                .order_by_asc(storage_allocation::Column::Slot)
                .all(db)?
                .into_iter()
                .map(|entry| storage_entry(entry, scope.into()))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Context {
                project: owned.project_name,
                target: RuntimeTarget {
                    id: target.id,
                    scope_id: target.scope_id,
                    endpoint: target.endpoint,
                    engine_id: target.engine_id,
                    platform: target.platform,
                },
                container,
                storage,
            })
        })
        .map_err(map_error)
}

struct Stages<'a> {
    database: &'a DatabaseWorker,
    probe: Option<&'a DockerProbe>,
    scope: &'a str,
    id: &'a str,
}

impl Stages<'_> {
    fn owner<'a>(&'a self, saved: &'a Context) -> DeletionOwner<'a> {
        DeletionOwner {
            scope: self.scope,
            instance: self.id,
            project: &saved.project,
            saved_container: saved.container.as_deref(),
        }
    }

    async fn checks(
        &self,
        require_engine: bool,
    ) -> Result<Vec<StorageCheck>, LifecycleEffectError> {
        let saved = context(self.database, self.scope, self.id)
            .map_err(|_| LifecycleEffectError::OutcomeUnknown)?;
        let mut checks = Vec::new();
        for entry in &saved.storage {
            let presence = match entry.method {
                StorageMethod::Bind => inspect_saved_bind(self.database.management_root(), entry),
                StorageMethod::Volume => {
                    let observed = match self
                        .probe
                        .and_then(|probe| probe.bind(saved.target.clone()).ok())
                    {
                        Some(docker) => {
                            DockerNamedVolumes::new(docker)
                                .observe_saved_volume(entry, self.database)
                                .await
                        }
                        None => {
                            Err(composenest_application::named_volumes::NamedVolumeError::Backend)
                        }
                    };
                    match observed {
                        Ok(presence) => presence,
                        Err(_) if !require_engine => StoragePresence::Unverified,
                        Err(_) => return Err(LifecycleEffectError::OutcomeUnknown),
                    }
                }
            };
            checks.push(StorageCheck {
                slot: entry.slot.clone(),
                presence,
            });
        }
        Ok(checks)
    }
}

impl DeleteStages for Stages<'_> {
    async fn remove_runtime(&self, receipt: &RequestReceipt) -> Result<(), LifecycleEffectError> {
        let saved = context(self.database, self.scope, self.id)
            .map_err(|_| LifecycleEffectError::OutcomeUnknown)?;
        let docker = self
            .probe
            .ok_or(LifecycleEffectError::OutcomeUnknown)?
            .bind(saved.target.clone())
            .map_err(|_| LifecycleEffectError::OutcomeUnknown)?;
        remove_owned_runtime(&docker, self.database, receipt, &self.owner(&saved)).await
    }
    async fn inspect_storage(&self) -> Result<Vec<StorageCheck>, LifecycleEffectError> {
        self.checks(true).await
    }
    async fn runtime_absent(&self) -> Result<(), LifecycleEffectError> {
        let saved = context(self.database, self.scope, self.id)
            .map_err(|_| LifecycleEffectError::OutcomeUnknown)?;
        let docker = self
            .probe
            .ok_or(LifecycleEffectError::OutcomeUnknown)?
            .bind(saved.target.clone())
            .map_err(|_| LifecycleEffectError::OutcomeUnknown)?;
        verify_runtime_absent(&docker, &self.owner(&saved)).await
    }
}

/// Accepts a confirmed Delete and preserves all data, original settings, and artifacts.
/// Unresolved attempts must be reconciled before the existing journal permits a retry.
pub async fn run_delete(
    database: &DatabaseWorker,
    probe: &DockerProbe,
    runner: &OperationRunner,
    intent: &OperationIntent,
    receipt: &RequestReceipt,
    retain_data_confirmed: bool,
) -> Result<RequestReceipt, DeleteError> {
    let stages = Stages {
        database,
        probe: Some(probe),
        scope: &receipt.scope_id,
        id: &receipt.instance_id,
    };
    DeleteOperation {
        state: database,
        runner,
        stages: &stages,
    }
    .run(intent, receipt, retain_data_confirmed)
    .await
}

/// Refreshes every retained slot without creating resources or requiring Docker for bind data.
/// An absent CLI or unavailable Engine is shown as unverified for volume data.
pub async fn refresh_retained_storage(
    database: &DatabaseWorker,
    probe: Option<&DockerProbe>,
    scope: &str,
    id: &str,
) -> Result<RetainedInstance, StoreConflict> {
    let saved = database
        .list_retained_storage(scope)?
        .into_iter()
        .find(|item| item.instance.id == id)
        .ok_or(StoreConflict::Missing)?;
    let stages = Stages {
        database,
        probe,
        scope,
        id,
    };
    let checks = stages
        .checks(false)
        .await
        .map_err(|_| StoreConflict::Backend)?;
    database.update_retained_checks(scope, id, saved.instance.revision, &checks)?;
    database
        .list_retained_storage(scope)?
        .into_iter()
        .find(|item| item.instance.id == id)
        .ok_or(StoreConflict::Missing)
}
