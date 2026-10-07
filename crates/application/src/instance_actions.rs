//! Scoped action views and idempotent lifecycle acceptance for desktop transports.
use crate::{
    RequestContext,
    operation_journal::{OperationIntent, OperationJournal, OperationKind, RequestReceipt},
    query_service::{InstanceView, QueryService, QueryStore},
    state_store::{StateStore, StoreConflict},
};
use composenest_domain::clone_policy::RandomSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Reads the action state of one instance in the trusted scope.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstanceActionRequest {
    /// Validated transport context.
    pub context: RequestContext,
    /// Stable target identifier.
    pub instance_id: String,
}
/// Changes the display name against the version shown to the user.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameInstanceRequest {
    /// Validated transport context.
    pub context: RequestContext,
    /// Stable target identifier.
    pub instance_id: String,
    /// Optimistic lock revision.
    pub expected_revision: u64,
    /// User supplied display name; Core normalizes it.
    pub name: String,
}
/// Accepts one fixed lifecycle action; no external arguments are exposed.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeInstanceRequest {
    /// Stable request context, reused after an uncertain response.
    pub context: RequestContext,
    /// Stable target identifier.
    pub instance_id: String,
    /// Optimistic lock revision.
    pub expected_revision: u64,
    /// Start, stop, restart, or data-preserving delete.
    pub action: String,
    /// Required explicit confirmation for Delete; omitted by older lifecycle clients.
    #[serde(default)]
    pub retain_data_confirmed: bool,
}
/// Non-sensitive state used by the header and name editor.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceActionView {
    /// Stable target identifier.
    pub id: String,
    /// Confirmed display name.
    pub name: String,
    /// Current optimistic lock revision.
    pub revision: u64,
    /// Last persisted runtime state, never an optimistic UI result.
    pub runtime_status: String,
    /// Timestamp of the persisted observation, if any.
    pub observed_at: Option<String>,
    /// Most recent operation ID, if any.
    pub operation_id: Option<String>,
    /// Persisted operation outcome, including unresolved failure.
    pub operation_status: Option<String>,
    /// Fixed operation kind.
    pub operation_kind: Option<String>,
    /// Persisted progress phase without external arguments.
    pub operation_phase: Option<String>,
    /// Actions offered by Core; external prerequisites are rechecked on execution.
    pub actions: Vec<String>,
}
impl From<InstanceView> for InstanceActionView {
    fn from(view: InstanceView) -> Self {
        let mut actions = vec![];
        if view.lifecycle == "managed"
            && view
                .last_operation
                .as_ref()
                .is_none_or(|op| matches!(op.status.as_str(), "Succeeded" | "Abandoned"))
        {
            actions.extend(["rename".into(), "delete".into()]);
            if matches!(view.runtime_status.as_str(), "stopped" | "absent") {
                actions.push("start".into());
            }
            if matches!(
                view.runtime_status.as_str(),
                "ready" | "preparing" | "unhealthy" | "stopped"
            ) {
                actions.extend(["stop".into(), "restart".into()]);
            }
        }
        Self {
            id: view.id,
            name: view.name,
            revision: view.revision,
            runtime_status: view.runtime_status,
            observed_at: view.observation.map(|item| item.observed_at),
            operation_id: view.last_operation.as_ref().map(|op| op.id.clone()),
            operation_status: view.last_operation.as_ref().map(|op| op.status.clone()),
            operation_kind: view.last_operation.as_ref().map(|op| op.kind.clone()),
            operation_phase: view.last_operation.map(|op| op.phase),
            actions,
        }
    }
}
/// Reads committed action state without contacting Docker.
pub fn view<S: QueryStore>(
    store: &S,
    scope: &str,
    id: &str,
) -> Result<InstanceActionView, StoreConflict> {
    store
        .get_instance(scope, id)?
        .map(Into::into)
        .ok_or(StoreConflict::Missing)
}
/// Returns a prior identical acceptance or atomically accepts the current version.
/// The boolean is true only for work requiring a new detached worker.
pub fn accept<S: QueryStore + StateStore + OperationJournal>(
    store: &S,
    scope: &str,
    request: &ChangeInstanceRequest,
    random: &mut impl RandomSource,
) -> Result<(RequestReceipt, OperationKind, bool), StoreConflict> {
    let kind = match request.action.as_str() {
        "start" => OperationKind::Start,
        "stop" => OperationKind::Stop,
        "restart" => OperationKind::Restart,
        "delete" if request.retain_data_confirmed => OperationKind::Delete,
        _ => return Err(StoreConflict::InvalidInput),
    };
    let hash = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                request.instance_id.as_str(),
                request.expected_revision,
                request.action.as_str()
            ))
            .map_err(|_| StoreConflict::InvalidInput)?
        )
    );
    if let Some(receipt) = store.receipt(scope, &request.context.request_id)? {
        if receipt.request_hash != hash
            || receipt.instance_id != request.instance_id
            || receipt.confirmed_revision != request.expected_revision
            || receipt.plan_id.is_some()
        {
            return Err(StoreConflict::Duplicate);
        }
        return Ok((receipt, kind, false));
    }
    let current = QueryService::new(store)
        .get_instance(scope, &request.instance_id)?
        .ok_or(StoreConflict::Missing)?;
    if current.revision != request.expected_revision {
        return Err(StoreConflict::StaleRevision);
    }
    if !InstanceActionView::from(current.clone())
        .actions
        .contains(&request.action)
    {
        return Err(StoreConflict::UnresolvedOperation);
    }
    let mut bytes = [0; 16];
    random
        .fill_bytes(&mut bytes)
        .map_err(|_| StoreConflict::Backend)?;
    let operation_id = format!("{:032x}", u128::from_be_bytes(bytes));
    let receipt = RequestReceipt {
        scope_id: scope.into(),
        request_id: request.context.request_id.clone(),
        plan_id: None,
        confirmed_revision: request.expected_revision,
        request_hash: hash,
        instance_id: request.instance_id.clone(),
        operation_id: operation_id.clone(),
    };
    let intent = OperationIntent {
        id: operation_id,
        instance_id: request.instance_id.clone(),
        kind,
        phase: "inspect".into(),
        expected_revision: request.expected_revision,
        old_spec_revision: (kind != OperationKind::Delete).then_some(current.spec_revision),
        new_spec_revision: None,
    };
    store
        .accept(&intent, &receipt)
        .map(|receipt| (receipt, kind, true))
}
