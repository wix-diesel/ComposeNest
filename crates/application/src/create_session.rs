//! Secret-safe desktop requests bound to one backend-selected management scope.

use crate::{
    Clock, RequestContext,
    create_plan::{
        CommitCreate, CreatePlanView, CreatePlans, PlanEdit, PlanError, PrepareCreate,
        get_plan_commit,
    },
    host_ports::PortInspector,
    named_volumes::named_volume_allocations,
    operation_journal::{PlanCommitStore, RequestReceipt},
    state_store::{StorageAllocation, StorageMethod},
};
use composenest_domain::{
    clone_policy::RandomSource,
    identity::{InstanceId, SlotId},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Template selection; scope and resource identities are resolved by the backend.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareCreateRequest {
    /// Versioned IPC metadata.
    pub context: RequestContext,
    /// Registered immutable revision selected in navigation.
    pub template_revision_id: String,
}

/// Identity-only request for refreshing, discarding, or reconciling a plan.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreatePlanRequest {
    /// Versioned IPC metadata.
    pub context: RequestContext,
    /// Process-local plan identifier.
    pub plan_id: String,
}

/// Changes to the current version of an existing plan.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateCreateRequest {
    /// Versioned IPC metadata.
    pub context: RequestContext,
    /// Process-local plan identifier.
    pub plan_id: String,
    /// Revision-guarded input changes.
    pub edit: PlanEdit,
}

/// Explicit acknowledgement of the displayed plan and its persistence policy.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmCreateRequest {
    /// Stable idempotency metadata reused after response loss.
    pub context: RequestContext,
    /// Reviewed plan identifier.
    pub plan_id: String,
    /// Reviewed plan revision.
    pub revision: u64,
    /// Exact loopback port candidates shown in the confirmation dialog.
    pub confirmed_ports: BTreeMap<String, u16>,
    /// Explicit consent to plaintext local settings and Compose artifacts.
    pub accept_plaintext: bool,
}

/// Non-sensitive accepted result; never contains persisted inputs or request hashes.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateReceipt {
    /// Plan that was accepted.
    pub plan_id: String,
    /// Accepted revision.
    pub confirmed_revision: u64,
    /// Independent operation destination.
    pub operation_id: String,
    /// New instance destination.
    pub instance_id: String,
}

impl From<&RequestReceipt> for CreateReceipt {
    fn from(receipt: &RequestReceipt) -> Self {
        Self {
            plan_id: receipt.plan_id.clone().unwrap_or_default(),
            confirmed_revision: receipt.confirmed_revision,
            operation_id: receipt.operation_id.clone(),
            instance_id: receipt.instance_id.clone(),
        }
    }
}

/// Process-local create plans bound to a trusted backend scope.
pub struct CreateSession {
    scope: String,
    plans: CreatePlans,
}

fn failure(code: &'static str) -> PlanError {
    PlanError {
        code,
        field_path: None,
    }
}

impl CreateSession {
    /// Binds new requests to the scope selected from the protected database.
    pub fn new(scope: String) -> Self {
        Self {
            scope,
            plans: CreatePlans::default(),
        }
    }

    /// Prepares only in-memory candidates from the selected registered revision.
    pub fn prepare(
        &mut self,
        template: String,
        store: &impl PlanCommitStore,
        clock: &impl Clock,
        random: &mut impl RandomSource,
        inspector: &impl PortInspector,
    ) -> Result<CreatePlanView, PlanError> {
        self.plans.prepare_create(
            &PrepareCreate {
                scope_id: self.scope.clone(),
                display_name: String::new(),
                template_revision_id: template,
                version: None,
            },
            store,
            clock,
            random,
            inspector,
        )
    }

    /// Applies a revision-guarded edit to the existing plan.
    pub fn update(
        &mut self,
        id: &str,
        edit: PlanEdit,
        clock: &impl Clock,
        random: &mut impl RandomSource,
        inspector: &impl PortInspector,
    ) -> Result<CreatePlanView, PlanError> {
        self.plans
            .update_plan(&self.scope, id, edit, clock, random, inspector)
    }

    /// Refreshes existing candidates without replacing the plan.
    pub fn view(
        &mut self,
        id: &str,
        clock: &impl Clock,
        inspector: &impl PortInspector,
    ) -> Result<CreatePlanView, PlanError> {
        self.plans.view_plan(&self.scope, id, clock, inspector)
    }

    /// Reconciles a lost response using durable state, even after process restart.
    pub fn receipt(
        &self,
        id: &str,
        store: &impl PlanCommitStore,
    ) -> Result<Option<RequestReceipt>, PlanError> {
        get_plan_commit(store, &self.scope, id)
    }

    /// Discards only an unconfirmed plan; committed resources remain owned.
    pub fn discard(
        &mut self,
        id: &str,
        store: &impl PlanCommitStore,
        clock: &impl Clock,
    ) -> Result<(), PlanError> {
        if self.receipt(id, store)?.is_some() {
            return Err(failure("PLAN_ALREADY_COMMITTED"));
        }
        self.plans.discard_plan(&self.scope, id, clock)
    }

    /// Returns an identical acceptance before external checks or ID generation.
    pub fn accepted(
        &self,
        request: &ConfirmCreateRequest,
        store: &impl PlanCommitStore,
    ) -> Result<Option<RequestReceipt>, PlanError> {
        if !request.accept_plaintext {
            return Err(failure("PLAINTEXT_CONFIRMATION_REQUIRED"));
        }
        if let Some(previous) = store
            .receipt(&self.scope, &request.context.request_id)
            .map_err(|_| failure("STORE_UNAVAILABLE"))?
            && (previous.plan_id.as_deref() != Some(request.plan_id.as_str())
                || previous.confirmed_revision != request.revision)
        {
            return Err(failure("REQUEST_ALREADY_USED"));
        }
        let previous = self.receipt(&request.plan_id, store)?;
        if previous
            .as_ref()
            .is_some_and(|receipt| receipt.confirmed_revision != request.revision)
        {
            return Err(failure("PLAN_ALREADY_COMMITTED"));
        }
        Ok(previous)
    }

    /// Allocates identities, then atomically accepts the exact reviewed plan.
    /// The caller must serialize requests and freshly verify the fixed runtime target.
    pub fn confirm(
        &mut self,
        request: ConfirmCreateRequest,
        store: &impl PlanCommitStore,
        clock: &impl Clock,
        random: &mut impl RandomSource,
        inspector: &impl PortInspector,
    ) -> Result<(RequestReceipt, bool), PlanError> {
        if let Some(previous) = self.accepted(&request, store)? {
            return Ok((previous, false));
        }
        let view = self.view(&request.plan_id, clock, inspector)?;
        let target = store
            .runtime_target(&self.scope)
            .map_err(|_| failure("STORE_UNAVAILABLE"))?
            .ok_or_else(|| failure("TARGET_UNAVAILABLE"))?;
        let instance_id = random_id(random)?;
        let operation_id = random_id(random)?;
        let id = InstanceId::from_u128(
            u128::from_str_radix(&instance_id, 16).map_err(|_| failure("RANDOM_UNAVAILABLE"))?,
        );
        let slots = view
            .storage_slots
            .iter()
            .map(|slot| SlotId::parse(slot).map_err(|_| failure("STORAGE_INVALID")))
            .collect::<Result<Vec<_>, _>>()?;
        let storage = match view.storage_method {
            StorageMethod::Volume => named_volume_allocations(id, &operation_id, &slots)
                .map_err(|_| failure("STORAGE_INVALID"))?,
            StorageMethod::Bind => slots
                .iter()
                .map(|slot| StorageAllocation {
                    slot: slot.as_str().into(),
                    resource_identity: format!("data/{instance_id}/{}", slot.as_str()),
                    ownership_evidence: operation_id.clone(),
                })
                .collect(),
        };
        let receipt = self.plans.commit_plan(
            CommitCreate {
                plan_id: request.plan_id,
                revision: request.revision,
                scope_id: self.scope.clone(),
                request_id: request.context.request_id,
                target_id: target.id,
                instance_id,
                operation_id,
                confirmed_ports: request.confirmed_ports,
                storage,
            },
            store,
            clock,
            inspector,
        )?;
        Ok((receipt, true))
    }
}

fn random_id(random: &mut impl RandomSource) -> Result<String, PlanError> {
    let mut bytes = [0; 16];
    random
        .fill_bytes(&mut bytes)
        .map_err(|_| failure("RANDOM_UNAVAILABLE"))?;
    Ok(format!("{:032x}", u128::from_be_bytes(bytes)))
}
