//! Data-preserving deletion of runtime resources and atomic retirement.

use crate::{
    lifecycle_operation::LifecycleEffectError,
    operation_journal::{
        OperationIntent, OperationJournal, OperationKind, OperationStatus, RequestReceipt,
    },
    operation_runner::{OperationReservation, OperationRunner, RunnerError},
    state_store::StoreConflict,
};
use composenest_domain::instance::StoragePresence;

/// A fresh read-only check of a saved data allocation.
#[derive(Debug, Clone)]
pub struct StorageCheck {
    /// Stable template storage slot.
    pub slot: String,
    /// Verified presence; missing or unverified data is never reported as present.
    pub presence: StoragePresence,
}

/// Atomic retirement boundary, independent of Docker and the filesystem.
pub trait DeleteState {
    /// Validates the receipt and permits accepted work, a reconciled retry, or completed replay.
    fn delete_pending(&self, receipt: &RequestReceipt) -> Result<bool, StoreConflict>;
    /// Completes deletion after runtime absence and all allocation checks are established.
    fn complete_delete(
        &self,
        receipt: &RequestReceipt,
        storage: &[StorageCheck],
    ) -> Result<(), StoreConflict>;
}

/// Fixed adapters that preserve all data and generated configuration.
pub trait DeleteStages {
    /// Journals owned runtime removal and freshly confirms container and network absence.
    fn remove_runtime(
        &self,
        receipt: &RequestReceipt,
    ) -> impl Future<Output = Result<(), LifecycleEffectError>>;
    /// Inspects every saved allocation without creating or deleting data.
    fn inspect_storage(
        &self,
    ) -> impl Future<Output = Result<Vec<StorageCheck>, LifecycleEffectError>>;
    /// Freshly confirms runtime absence after storage checks and before retirement.
    fn runtime_absent(&self) -> impl Future<Output = Result<(), LifecycleEffectError>>;
}

/// A deletion could not establish its safe completion conditions.
#[derive(Debug, PartialEq, Eq)]
pub enum DeleteError {
    /// The deletion confirmation or an ownership prerequisite was rejected.
    Rejected,
    /// Docker is unavailable or a changing command requires reconciliation.
    OutcomeUnknown,
    /// A persistence or optimistic-lock check failed.
    Store(StoreConflict),
    /// The operation gate rejected concurrent execution.
    Runner(RunnerError),
}

/// Coordinates a confirmed delete request through the existing operation gate.
pub struct DeleteOperation<'a, S, A> {
    /// Durable state and effect journal.
    pub state: &'a S,
    /// Backend change gate.
    pub runner: &'a OperationRunner,
    /// Runtime and storage checks for the requested instance.
    pub stages: &'a A,
}

impl<S: DeleteState + OperationJournal, A: DeleteStages> DeleteOperation<'_, S, A> {
    /// Accepts deletion only with explicit data-retention confirmation and the current revision.
    pub async fn run(
        &self,
        intent: &OperationIntent,
        receipt: &RequestReceipt,
        retain_data_confirmed: bool,
    ) -> Result<RequestReceipt, DeleteError> {
        self.runner
            .run_exclusive(&receipt.instance_id, || {
                self.run_locked(intent, receipt, retain_data_confirmed)
            })
            .await
            .map_err(DeleteError::Runner)?
    }

    /// Executes confirmed deletion with capacity reserved before durable acceptance.
    pub async fn run_reserved(
        &self,
        intent: &OperationIntent,
        receipt: &RequestReceipt,
        retain_data_confirmed: bool,
        reservation: OperationReservation,
    ) -> Result<RequestReceipt, DeleteError> {
        self.runner
            .run_reserved(reservation, &receipt.instance_id, || {
                self.run_locked(intent, receipt, retain_data_confirmed)
            })
            .await
            .map_err(DeleteError::Runner)?
    }

    async fn run_locked(
        &self,
        intent: &OperationIntent,
        receipt: &RequestReceipt,
        retain_data_confirmed: bool,
    ) -> Result<RequestReceipt, DeleteError> {
        if !retain_data_confirmed
            || intent.kind != OperationKind::Delete
            || receipt.plan_id.is_some()
            || intent.instance_id != receipt.instance_id
            || intent.id != receipt.operation_id
            || intent.expected_revision != receipt.confirmed_revision
            || intent.old_spec_revision.is_some()
            || intent.new_spec_revision.is_some()
        {
            return Err(DeleteError::Rejected);
        }
        let accepted = self
            .state
            .accept(intent, receipt)
            .map_err(DeleteError::Store)?;
        if !self
            .state
            .delete_pending(&accepted)
            .map_err(DeleteError::Store)?
        {
            return Ok(accepted);
        }
        self.state
            .set_status(
                &accepted.operation_id,
                OperationStatus::Executing,
                "remove_runtime",
            )
            .map_err(DeleteError::Store)?;
        let result = async {
            self.stages.remove_runtime(&accepted).await?;
            let storage = self.stages.inspect_storage().await?;
            self.stages.runtime_absent().await?;
            Ok(storage)
        }
        .await;
        let storage = match result {
            Ok(storage) => storage,
            Err(error) => {
                let (status, result) = match error {
                    LifecycleEffectError::Rejected => {
                        (OperationStatus::Failed, DeleteError::Rejected)
                    }
                    LifecycleEffectError::OutcomeUnknown => {
                        (OperationStatus::OutcomeUnknown, DeleteError::OutcomeUnknown)
                    }
                };
                self.state
                    .set_status(&accepted.operation_id, status, "delete_check")
                    .map_err(DeleteError::Store)?;
                return Err(result);
            }
        };
        self.state
            .complete_delete(&accepted, &storage)
            .map_err(DeleteError::Store)?;
        Ok(accepted)
    }
}
