//! Configuration-only clone planning from a committed spec and private Snapshot.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use composenest_domain::clone_policy::RandomSource;
use composenest_domain::identity::DisplayName;
use composenest_domain::instance::OperationKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::Clock;

mod commit;
mod evaluation;
use crate::create_plan::{
    CommitCreate, IDLE_SECONDS, MAX_PLANS, PlanConcern, PlanError, commit_store_error, entries,
    error, generate_input_secret, normalize_name, valid_input, version_definition,
};
use crate::host_ports::{PortCursor, PortInspector, PortPlan, PortReason, PortSlot, plan_ports};
use crate::operation_journal::{
    CloneSourceGuard, OperationIntent, PlanCommitStore, RequestReceipt,
};
use crate::state_store::{CloneSource, InstanceRecord, PortAllocation, StateStore, StorageMethod};
use evaluation::{answer_field, check_source, evaluate_fields, preview};

/// Prepares a configuration-only clone of a managed source.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareClone {
    /// Management scope containing the source.
    pub scope_id: String,
    /// Existing managed instance.
    pub source_id: String,
    /// User-entered new purpose name.
    pub display_name: String,
}

/// Explicit response to a Clone Policy.
#[derive(Clone, Deserialize)]
#[serde(tag = "action", content = "value", rename_all = "snake_case")]
pub enum CloneAnswer {
    /// Copy the committed source value.
    Copy,
    /// Generate a new secret.
    Generate,
    /// Use a user-entered value.
    Input(Value),
    /// Leave an optional input unset.
    Clear,
}

/// Changes to a clone plan; omitted fields retain their current candidate.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloneEdit {
    /// Expected plan revision.
    pub expected_revision: u64,
    /// New display name, if changed.
    pub display_name: Option<String>,
    /// Explicit service Version choice from the private Snapshot.
    pub version: Option<String>,
    /// Explicit storage method choice.
    pub storage_method: Option<StorageMethod>,
    /// Responses or edits for active inputs.
    #[serde(default)]
    pub inputs: BTreeMap<String, CloneAnswer>,
    /// Active secret keys whose unchanged source value was explicitly confirmed.
    #[serde(default)]
    pub confirm_secrets: Vec<String>,
    /// Explicit decimal ports; null returns a slot to automatic selection.
    #[serde(default)]
    pub ports: BTreeMap<String, Option<String>>,
}

/// Identifies the plan and scope to update.
pub struct UpdateClone {
    /// Management scope containing the plan.
    pub scope_id: String,
    /// Plan identifier.
    pub plan_id: String,
    /// Proposed answers and changes.
    pub edit: CloneEdit,
}

/// Where a candidate came from, independently of whether its value changed.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValueOrigin {
    /// Committed source value.
    Inherited,
    /// Built-in secret generator.
    Generated,
    /// User-entered value.
    UserInput,
    /// Deliberately absent value.
    Unset,
}

/// Secret-safe difference for one input in either Version.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDiff {
    /// Stable input key.
    pub key: String,
    /// Active definition, absent for a removed input.
    pub definition: Option<Value>,
    /// Source value, omitted for secrets.
    pub source: Option<Value>,
    /// Candidate value, omitted for secrets.
    pub candidate: Option<Value>,
    /// Effective Clone Policy.
    pub policy: String,
    /// Candidate provenance.
    pub origin: ValueOrigin,
    /// Whether source and candidate differ, including absence.
    pub changed: bool,
    /// Whether this key was added in the selected Version.
    pub added: bool,
    /// Whether this key is absent from the selected Version.
    pub removed: bool,
    /// Whether an explicit answer is still required.
    pub needs_answer: bool,
    /// Whether reuse of a source secret needs explicit confirmation.
    pub needs_secret_confirmation: bool,
    /// Whether a secret candidate is present, without exposing it.
    pub has_secret: bool,
}

/// Secret-safe view of a configuration-only clone plan.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClonePlanView {
    /// Opaque process-local plan ID.
    pub plan_id: String,
    /// Revision required by edits and confirmation.
    pub plan_revision: u64,
    /// Source instance ID.
    pub source_id: String,
    /// Proposed fresh instance ID, stable for this plan.
    pub instance_id: String,
    /// Compose project derived from the new instance ID.
    pub project_name: String,
    /// New purpose name.
    pub display_name: String,
    /// Selected service Version.
    pub version: String,
    /// Committed source service Version.
    pub source_version: String,
    /// Version choices from the source Snapshot only.
    pub versions: Vec<String>,
    /// Selected storage method.
    pub storage_method: StorageMethod,
    /// Committed source storage method.
    pub source_storage_method: StorageMethod,
    /// Ordered active form metadata from the private Snapshot only.
    pub template_form: crate::template_form::TemplateForm,
    /// All input differences, including removed keys.
    pub inputs: Vec<InputDiff>,
    /// Proposed host ports.
    pub ports: BTreeMap<String, u16>,
    /// Committed source port bindings by stable slot.
    pub source_ports: BTreeMap<String, u16>,
    /// Port slots added in the selected Version.
    pub added_ports: Vec<String>,
    /// Port slots removed in the selected Version.
    pub removed_ports: Vec<String>,
    /// Active writable storage slots requiring fresh allocations.
    pub storage_slots: Vec<String>,
    /// Storage slots added in the selected Version.
    pub added_storage: Vec<String>,
    /// Storage slots removed in the selected Version.
    pub removed_storage: Vec<String>,
    /// Reasons this plan cannot yet be confirmed.
    pub concerns: Vec<PlanConcern>,
}

#[derive(Clone)]
struct FieldState {
    kind: String,
    policy: String,
    options: Option<Value>,
    value: Option<Value>,
    origin: ValueOrigin,
    answered: bool,
    secret_confirmed: bool,
}

struct Plan {
    source: CloneSource,
    instance_id: String,
    template: Value,
    source_values: BTreeMap<String, Value>,
    source_definition: Value,
    display_name: String,
    version: String,
    version_answered: bool,
    storage_method: StorageMethod,
    storage_answered: bool,
    fields: BTreeMap<String, FieldState>,
    explicit_ports: BTreeMap<String, String>,
    port_cursor: PortCursor,
    last_ports: BTreeMap<String, u16>,
    revision: u64,
    last_used: u64,
}

/// Bounded process-local collection of unconfirmed clone plans.
#[derive(Default)]
pub struct ClonePlans {
    plans: BTreeMap<String, Plan>,
}

impl ClonePlans {
    /// Prepares a plan from the source's committed spec and private Snapshot.
    pub fn prepare_clone(
        &mut self,
        request: &PrepareClone,
        store: &impl StateStore,
        clock: &impl Clock,
        random: &mut impl RandomSource,
        inspector: &impl PortInspector,
    ) -> Result<ClonePlanView, PlanError> {
        self.expire(clock.unix_seconds());
        if self.plans.len() >= MAX_PLANS {
            return Err(error("PLAN_LIMIT", None));
        }
        let source = store
            .clone_source(&request.scope_id, &request.source_id)
            .map_err(commit_store_error)?
            .ok_or_else(|| error("SOURCE_UNAVAILABLE", None))?;
        let template: Value = serde_json::from_str(&source.snapshot_json)
            .map_err(|_| error("SNAPSHOT_INVALID", None))?;
        let mut source_values: BTreeMap<String, Value> = serde_json::from_str(&source.inputs_json)
            .map_err(|_| error("SOURCE_SPEC_INVALID", None))?;
        source_values.retain(|_, value| !value.is_null());
        let source_definition = version_definition(&template, &source.selected_version)?.clone();
        let version = source.selected_version.clone();
        let fields = evaluate_fields(
            &source_definition,
            &source_definition,
            &source_values,
            &BTreeMap::new(),
            random,
        )?;
        let id = self.unique_id(random)?;
        let instance_id = self.unique_instance_id(random, store)?;
        let mut plan = Plan {
            display_name: normalize_name(&request.display_name),
            version_answered: template["rules"]["versionClone"] != "ask",
            storage_answered: template["rules"]["storageClone"] != "ask",
            storage_method: source.storage_method,
            source,
            instance_id,
            template,
            source_values,
            source_definition,
            version,
            fields,
            explicit_ports: BTreeMap::new(),
            port_cursor: PortCursor::default(),
            last_ports: BTreeMap::new(),
            revision: 1,
            last_used: clock.unix_seconds(),
        };
        let view = preview(&id, &mut plan, inspector, false);
        self.plans.insert(id, plan);
        Ok(view)
    }

    /// Applies explicit answers and revalidates the current source revision.
    pub fn update_plan(
        &mut self,
        request: UpdateClone,
        store: &impl StateStore,
        clock: &impl Clock,
        random: &mut impl RandomSource,
        inspector: &impl PortInspector,
    ) -> Result<ClonePlanView, PlanError> {
        self.expire(clock.unix_seconds());
        let id = request.plan_id.as_str();
        let edit = request.edit;
        let plan = self.plan_mut(&request.scope_id, id)?;
        check_source(plan, store)?;
        if plan.revision != edit.expected_revision {
            return Err(error("PLAN_STALE", Some("expectedRevision".into())));
        }
        let next_revision = plan
            .revision
            .checked_add(1)
            .ok_or_else(|| error("PLAN_REVISION_EXHAUSTED", None))?;
        let version = edit.version.as_deref().unwrap_or(&plan.version);
        let definition = version_definition(&plan.template, version)?.clone();
        let active = entries(&definition["inputs"]);
        let port_keys: BTreeSet<_> = entries(&definition["service"]["ports"])
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        for key in edit.inputs.keys() {
            if !active.iter().any(|(name, _)| name == key) {
                return Err(error("UNKNOWN_INPUT", Some(format!("inputs.{key}"))));
            }
        }
        for key in edit.ports.keys() {
            if !port_keys.contains(key) {
                return Err(error("UNKNOWN_PORT", Some(format!("ports.{key}"))));
            }
        }
        let switching = version != plan.version;
        let mut fields = if switching {
            evaluate_fields(
                &plan.source_definition,
                &definition,
                &plan.source_values,
                &plan.fields,
                random,
            )?
        } else {
            plan.fields.clone()
        };
        for (key, answer) in edit.inputs {
            let input = &active
                .iter()
                .find(|(name, _)| name == &key)
                .expect("validated active input")
                .1;
            let source = plan.source_values.get(&key);
            let state = answer_field(&key, input, source, answer, &fields, &active, random)?;
            fields.insert(key, state);
        }
        let confirmations: BTreeSet<_> = edit.confirm_secrets.iter().collect();
        if confirmations.len() != edit.confirm_secrets.len() {
            return Err(error(
                "DUPLICATE_SECRET_ACTION",
                Some("confirmSecrets".into()),
            ));
        }
        for key in confirmations {
            let input = &active
                .iter()
                .find(|(name, _)| name == key)
                .ok_or_else(|| {
                    error(
                        "INVALID_SECRET_ACTION",
                        Some(format!("confirmSecrets.{key}")),
                    )
                })?
                .1;
            let field = fields.get_mut(key).ok_or_else(|| {
                error(
                    "INVALID_SECRET_ACTION",
                    Some(format!("confirmSecrets.{key}")),
                )
            })?;
            if input["type"] != "secret"
                || field.value.is_none()
                || field.value.as_ref() != plan.source_values.get(key)
            {
                return Err(error(
                    "INVALID_SECRET_ACTION",
                    Some(format!("confirmSecrets.{key}")),
                ));
            }
            field.secret_confirmed = true;
        }
        if let Some(name) = edit.display_name {
            plan.display_name = normalize_name(&name);
        }
        if edit.version.is_some() {
            plan.version_answered = true;
        }
        if let Some(method) = edit.storage_method {
            plan.storage_method = method;
            plan.storage_answered = true;
        }
        plan.fields = fields;
        plan.version = version.into();
        plan.explicit_ports.retain(|key, _| port_keys.contains(key));
        if switching || !edit.ports.is_empty() {
            plan.port_cursor = PortCursor::default();
        }
        for (key, value) in edit.ports {
            if let Some(value) = value {
                plan.explicit_ports.insert(key, value);
            } else {
                plan.explicit_ports.remove(&key);
            }
        }
        plan.revision = next_revision;
        plan.last_used = clock.unix_seconds();
        Ok(preview(id, plan, inspector, false))
    }

    /// Returns a fresh preview without changing candidates or answers.
    pub fn view_plan(
        &mut self,
        scope: &str,
        id: &str,
        store: &impl StateStore,
        clock: &impl Clock,
        inspector: &impl PortInspector,
    ) -> Result<ClonePlanView, PlanError> {
        self.expire(clock.unix_seconds());
        let plan = self.plan_mut(scope, id)?;
        check_source(plan, store)?;
        plan.last_used = clock.unix_seconds();
        Ok(preview(id, plan, inspector, true))
    }

    /// Removes an unconfirmed clone plan and its secret candidates.
    pub fn discard_plan(
        &mut self,
        scope: &str,
        id: &str,
        clock: &impl Clock,
    ) -> Result<(), PlanError> {
        self.expire(clock.unix_seconds());
        self.plan_mut(scope, id)?;
        self.plans.remove(id);
        Ok(())
    }

    fn plan_mut(&mut self, scope: &str, id: &str) -> Result<&mut Plan, PlanError> {
        self.plans
            .get_mut(id)
            .filter(|plan| plan.source.scope_id == scope)
            .ok_or_else(|| error("PLAN_NOT_FOUND", None))
    }

    fn expire(&mut self, now: u64) {
        self.plans
            .retain(|_, plan| now.saturating_sub(plan.last_used) < IDLE_SECONDS);
    }

    fn unique_id(&self, random: &mut impl RandomSource) -> Result<String, PlanError> {
        for _ in 0..8 {
            let mut bytes = [0; 16];
            random
                .fill_bytes(&mut bytes)
                .map_err(|_| error("RANDOM_UNAVAILABLE", None))?;
            let id = format!("{:032x}", u128::from_be_bytes(bytes));
            if !self.plans.contains_key(&id) {
                return Ok(id);
            }
        }
        Err(error("RANDOM_UNAVAILABLE", None))
    }

    fn unique_instance_id(
        &self,
        random: &mut impl RandomSource,
        store: &impl StateStore,
    ) -> Result<String, PlanError> {
        for _ in 0..8 {
            let candidate = self.unique_id(random)?;
            if !self
                .plans
                .values()
                .any(|plan| plan.instance_id == candidate)
                && !store
                    .instance_id_exists(&candidate)
                    .map_err(commit_store_error)?
            {
                return Ok(candidate);
            }
        }
        Err(error("IDENTITY_CONFLICT", None))
    }
}

fn overlaps_resource(first: &str, second: &str) -> bool {
    let first = Path::new(first);
    let second = Path::new(second);
    first.starts_with(second) || second.starts_with(first)
}
