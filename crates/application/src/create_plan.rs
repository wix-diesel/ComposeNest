//! In-memory preparation of new instances from immutable Template revisions.

use std::collections::{BTreeMap, BTreeSet};

use composenest_domain::clone_policy::{RandomSource, generate_secret};
use composenest_domain::identity::DisplayName;
use composenest_domain::instance::OperationKind;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    Clock,
    host_ports::{PortCursor, PortInspector, PortPlan, PortSlot, plan_ports},
    operation_journal::{OperationIntent, PlanCommitStore, RequestReceipt},
    state_store::{
        InstanceRecord, PortAllocation, StateStore, StorageAllocation, StorageMethod, StoreConflict,
    },
};

const MAX_PLANS: usize = 16;
const IDLE_SECONDS: u64 = 30 * 60;

/// An input or plan failure without sensitive candidate values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError {
    /// Stable error code for the caller.
    pub code: &'static str,
    /// Form field that needs correction, if any.
    pub field_path: Option<String>,
}

fn error(code: &'static str, path: impl Into<Option<String>>) -> PlanError {
    PlanError {
        code,
        field_path: path.into(),
    }
}

/// One input in the selected complete Version definition.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputView {
    /// Stable input key.
    pub key: String,
    /// Validated Template form definition; never contains a secret value.
    pub definition: Value,
    /// Nonsecret candidate. Secret candidates are always omitted.
    pub value: Option<Value>,
    /// Whether a secret has been initialized or entered.
    pub has_secret: bool,
}

/// A field or slot awaiting a corrected value or external check.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanConcern {
    /// Stable code for the confirmation UI.
    pub code: &'static str,
    /// Path in the selected form.
    pub field_path: String,
}

/// Secret-safe preview of a new instance plan.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePlanView {
    /// Opaque identifier scoped to this process.
    pub plan_id: String,
    /// Version required by a later update or confirmation.
    pub plan_revision: u64,
    /// Normalized display name, or the entered text awaiting correction.
    pub display_name: String,
    /// Immutable Template revision used by this plan.
    pub template_revision_id: String,
    /// Selected service Version.
    pub version: String,
    /// Version keys in Template display order.
    pub versions: Vec<String>,
    /// Complete selected Version definition without candidate values.
    pub form: Value,
    /// Storage method selected for all active slots.
    pub storage_method: StorageMethod,
    /// Form fields in Template display order.
    pub inputs: Vec<InputView>,
    /// Previewed host ports, which must be checked again at commit.
    pub ports: BTreeMap<String, u16>,
    /// Active writable storage slots, without allocating them.
    pub storage_slots: Vec<String>,
    /// Reasons confirmation is unavailable.
    pub concerns: Vec<PlanConcern>,
}

/// Changes to a plan; omitted fields retain their current candidate.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanEdit {
    /// Expected plan revision for optimistic concurrency.
    pub expected_revision: u64,
    /// Optional replacement for the new instance display name.
    pub display_name: Option<String>,
    /// Optional service Version selection.
    pub version: Option<String>,
    /// Optional method for all selected storage slots.
    pub storage_method: Option<StorageMethod>,
    /// Input answers. Null clears an optional input.
    #[serde(default)]
    pub inputs: BTreeMap<String, Value>,
    /// Secret keys to regenerate explicitly using secret-v1.
    #[serde(default)]
    pub regenerate_secrets: Vec<String>,
    /// Explicit decimal host ports. Null returns a slot to automatic selection.
    #[serde(default)]
    pub ports: BTreeMap<String, Option<String>>,
}

/// Selects a registered Template revision and optional initial service Version.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareCreate {
    /// Management scope whose persisted default applies.
    pub scope_id: String,
    /// User-entered purpose name for the new instance.
    pub display_name: String,
    /// Immutable registered Template revision identifier.
    pub template_revision_id: String,
    /// Optional Version key; the Template default applies when omitted.
    pub version: Option<String>,
}

/// Prevalidated resource identities supplied before the short commit transaction.
pub struct CommitCreate {
    /// Plan to confirm.
    pub plan_id: String,
    /// Plan revision shown to the caller.
    pub revision: u64,
    /// Scope containing the plan and request.
    pub scope_id: String,
    /// Caller-generated idempotency key.
    pub request_id: String,
    /// Registered runtime target selected before confirmation.
    pub target_id: String,
    /// Fresh 32-digit hexadecimal instance identifier.
    pub instance_id: String,
    /// Fresh operation identifier.
    pub operation_id: String,
    /// Host ports explicitly confirmed from the displayed plan revision.
    pub confirmed_ports: BTreeMap<String, u16>,
    /// Allocations checked or prepared outside the database transaction.
    pub storage: Vec<StorageAllocation>,
}

struct Plan {
    scope: String,
    display_name: String,
    revision: u64,
    template_id: String,
    template: Value,
    version: String,
    storage_method: StorageMethod,
    values: BTreeMap<String, Value>,
    explicit_ports: BTreeMap<String, String>,
    port_cursor: PortCursor,
    last_used: u64,
}

/// Process-local, bounded collection of unconfirmed new-instance plans.
#[derive(Default)]
pub struct CreatePlans {
    plans: BTreeMap<String, Plan>,
}

impl CreatePlans {
    /// Confirms a ready plan and returns its durable result, including after plan expiration.
    pub fn commit_plan(
        &mut self,
        request: CommitCreate,
        store: &impl PlanCommitStore,
        clock: &impl Clock,
        inspector: &impl PortInspector,
    ) -> Result<RequestReceipt, PlanError> {
        self.expire(clock.unix_seconds());
        let hash = commit_hash(&request.plan_id, request.revision);
        if let Some(previous) = store
            .receipt(&request.scope_id, &request.request_id)
            .map_err(commit_store_error)?
        {
            return if previous.plan_id.as_deref() == Some(request.plan_id.as_str())
                && previous.confirmed_revision == request.revision
                && previous.request_hash == hash
            {
                Ok(previous)
            } else {
                Err(error("REQUEST_ALREADY_USED", None))
            };
        }
        if let Some(previous) = store
            .plan_receipt(&request.scope_id, &request.plan_id)
            .map_err(commit_store_error)?
        {
            return if previous.confirmed_revision == request.revision
                && previous.request_hash == hash
            {
                Ok(previous)
            } else {
                Err(error("PLAN_ALREADY_COMMITTED", None))
            };
        }
        let plan = self
            .plans
            .get_mut(&request.plan_id)
            .filter(|plan| plan.scope == request.scope_id)
            .ok_or_else(|| error("PLAN_NOT_FOUND", None))?;
        if plan.revision != request.revision {
            return Err(error("PLAN_STALE", Some("revision".into())));
        }
        let view = preview(&request.plan_id, plan, inspector);
        if let Some(concern) = view.concerns.first() {
            return Err(error(concern.code, Some(concern.field_path.clone())));
        }
        if request.confirmed_ports != view.ports {
            return Err(error("PLAN_RECONFIRM", Some("ports".into())));
        }
        if request.target_id.is_empty()
            || request.request_id.is_empty()
            || request.operation_id.is_empty()
            || request.instance_id.len() != 32
            || !request
                .instance_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(error("COMMIT_INVALID", None));
        }
        let expected_slots: BTreeSet<_> = view.storage_slots.iter().map(String::as_str).collect();
        let actual_slots: BTreeSet<_> = request
            .storage
            .iter()
            .map(|item| item.slot.as_str())
            .collect();
        if expected_slots != actual_slots
            || actual_slots.len() != request.storage.len()
            || request
                .storage
                .iter()
                .any(|item| item.resource_identity.is_empty() || item.ownership_evidence.is_empty())
        {
            return Err(error("STORAGE_ALLOCATION_INVALID", None));
        }
        let definition = version_definition(&plan.template, &plan.version)?;
        let ports = entries(&definition["service"]["ports"])
            .into_iter()
            .map(|(slot, port)| {
                Ok(PortAllocation {
                    host_port: *view
                        .ports
                        .get(&slot)
                        .ok_or_else(|| error("PORT_CHECK_UNAVAILABLE", None))?,
                    container_port: port["container"]
                        .as_u64()
                        .and_then(|value| u16::try_from(value).ok())
                        .ok_or_else(|| error("TEMPLATE_INVALID", None))?,
                    host_ip: "127.0.0.1".into(),
                    slot,
                })
            })
            .collect::<Result<Vec<_>, PlanError>>()?;
        let instance = InstanceRecord {
            id: request.instance_id.clone(),
            scope_id: request.scope_id.clone(),
            target_id: request.target_id,
            name: plan.display_name.clone(),
            project_name: format!("cn-{}", request.instance_id),
            clone_source_id: None,
            template_revision_id: plan.template_id.clone(),
            selected_version: plan.version.clone(),
            storage_method: plan.storage_method,
            inputs_json: serde_json::to_string(&plan.values)
                .map_err(|_| error("COMMIT_INVALID", None))?,
            ports,
            storage: request.storage,
        };
        let intent = OperationIntent {
            id: request.operation_id.clone(),
            instance_id: instance.id.clone(),
            kind: OperationKind::Create,
            phase: "accepted".into(),
            expected_revision: 1,
            old_spec_revision: None,
            new_spec_revision: Some(1),
        };
        let receipt = RequestReceipt {
            scope_id: instance.scope_id.clone(),
            request_id: request.request_id,
            plan_id: Some(request.plan_id.clone()),
            confirmed_revision: request.revision,
            request_hash: hash,
            instance_id: instance.id.clone(),
            operation_id: intent.id.clone(),
        };
        let target = store
            .runtime_target(&instance.scope_id)
            .map_err(commit_store_error)?
            .filter(|target| target.id == instance.target_id)
            .ok_or_else(|| error("RUNTIME_TARGET_MISMATCH", None))?;
        let result = store
            .commit_plan(&instance, &intent, &receipt, &target, None)
            .map_err(commit_store_error)?;
        self.plans.remove(&request.plan_id);
        Ok(result)
    }

    /// Builds a plan from a registered, complete Template revision without reserving resources.
    pub fn prepare_create(
        &mut self,
        request: &PrepareCreate,
        store: &impl StateStore,
        clock: &impl Clock,
        random: &mut impl RandomSource,
        inspector: &impl PortInspector,
    ) -> Result<CreatePlanView, PlanError> {
        self.expire(clock.unix_seconds());
        if self.plans.len() >= MAX_PLANS {
            return Err(error("PLAN_LIMIT", None));
        }
        let revision = store
            .list_templates()
            .map_err(store_error)?
            .into_iter()
            .find(|item| item.id == request.template_revision_id)
            .ok_or_else(|| error("TEMPLATE_NOT_FOUND", Some("templateRevisionId".into())))?;
        let template: Value = serde_json::from_str(&revision.canonical_json)
            .map_err(|_| error("TEMPLATE_INVALID", None))?;
        let selected = request
            .version
            .as_deref()
            .unwrap_or_else(|| {
                template["manifest"]["defaultVersion"]
                    .as_str()
                    .unwrap_or("")
            })
            .to_owned();
        let definition = version_definition(&template, &selected)?;
        let values = initialize_inputs(definition, random)?;
        let storage_method = store
            .default_storage_method(&request.scope_id)
            .map_err(store_error)?;
        let id = self.unique_id(random)?;
        let mut plan = Plan {
            scope: request.scope_id.clone(),
            display_name: normalize_name(&request.display_name),
            revision: 1,
            template_id: revision.id,
            template,
            version: selected,
            storage_method,
            values,
            explicit_ports: BTreeMap::new(),
            port_cursor: PortCursor::default(),
            last_used: clock.unix_seconds(),
        };
        let view = preview(&id, &mut plan, inspector);
        self.plans.insert(id, plan);
        Ok(view)
    }

    /// Applies answers and a Version switch, then rechecks every active input and port slot.
    pub fn update_plan(
        &mut self,
        scope: &str,
        id: &str,
        edit: PlanEdit,
        clock: &impl Clock,
        random: &mut impl RandomSource,
        inspector: &impl PortInspector,
    ) -> Result<CreatePlanView, PlanError> {
        self.expire(clock.unix_seconds());
        let plan = self
            .plans
            .get_mut(id)
            .ok_or_else(|| error("PLAN_NOT_FOUND", None))?;
        if plan.scope != scope {
            return Err(error("PLAN_NOT_FOUND", None));
        }
        if plan.revision != edit.expected_revision {
            return Err(error("PLAN_STALE", Some("expectedRevision".into())));
        }
        let next_revision = plan
            .revision
            .checked_add(1)
            .ok_or_else(|| error("PLAN_REVISION_EXHAUSTED", None))?;
        let selected = edit.version.as_deref().unwrap_or(&plan.version);
        let definition = version_definition(&plan.template, selected)?;
        let active_inputs = entries(&definition["inputs"]);
        let active_ports = entries(&definition["service"]["ports"]);
        let regenerate: BTreeSet<_> = edit.regenerate_secrets.iter().collect();
        if regenerate.len() != edit.regenerate_secrets.len() {
            return Err(error(
                "DUPLICATE_SECRET_ACTION",
                Some("regenerateSecrets".into()),
            ));
        }
        for key in edit.inputs.keys() {
            if !active_inputs.iter().any(|(name, _)| name == key) {
                return Err(error("UNKNOWN_INPUT", Some(format!("inputs.{key}"))));
            }
            if edit.inputs[key]
                .as_str()
                .is_some_and(|value| value.len() > 16 * 1024)
            {
                return Err(error("INPUT_TOO_LARGE", Some(format!("inputs.{key}"))));
            }
        }
        for key in edit.ports.keys() {
            if !active_ports.iter().any(|(name, _)| name == key) {
                return Err(error("UNKNOWN_PORT", Some(format!("ports.{key}"))));
            }
        }
        for key in &regenerate {
            if edit.inputs.contains_key(*key)
                || !active_inputs
                    .iter()
                    .any(|(name, input)| name == *key && input["type"] == "secret")
            {
                return Err(error(
                    "INVALID_SECRET_ACTION",
                    Some(format!("regenerateSecrets.{key}")),
                ));
            }
        }
        let mut values = BTreeMap::new();
        let switching = selected != plan.version;
        let mut generate = Vec::new();
        for (key, input) in &active_inputs {
            if regenerate.contains(key) {
                generate.push(key.clone());
                continue;
            }
            if let Some(value) = edit.inputs.get(key) {
                values.insert(key.clone(), value.clone());
            } else if let Some(value) = plan.values.get(key) {
                values.insert(key.clone(), value.clone());
            } else if switching {
                if input["type"] == "secret" && input["initial"] != "ask" {
                    generate.push(key.clone());
                } else if let Some(value) = input.get("default") {
                    values.insert(key.clone(), value.clone());
                }
            }
        }
        for key in generate {
            let input = &active_inputs
                .iter()
                .find(|(name, _)| name == &key)
                .expect("generation key belongs to the selected form")
                .1;
            let source = plan.values.get(&key).and_then(Value::as_str);
            let candidate =
                generate_input_secret(&key, input, &values, &active_inputs, source, random)?;
            values.insert(key, Value::String(candidate));
        }
        if let Some(name) = edit.display_name {
            plan.display_name = normalize_name(&name);
        }
        plan.values = values;
        plan.version = selected.into();
        if let Some(method) = edit.storage_method {
            plan.storage_method = method;
        }
        plan.explicit_ports
            .retain(|key, _| active_ports.iter().any(|(name, _)| name == key));
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
        Ok(preview(id, plan, inspector))
    }

    /// Removes an unconfirmed plan and its secrets from process memory.
    pub fn discard_plan(
        &mut self,
        scope: &str,
        id: &str,
        clock: &impl Clock,
    ) -> Result<(), PlanError> {
        self.expire(clock.unix_seconds());
        if self.plans.get(id).is_none_or(|plan| plan.scope != scope) {
            return Err(error("PLAN_NOT_FOUND", None));
        }
        self.plans.remove(id);
        Ok(())
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
}

/// Finds a committed plan even when its in-memory candidate has disappeared.
pub fn get_plan_commit(
    store: &impl PlanCommitStore,
    scope: &str,
    plan_id: &str,
) -> Result<Option<RequestReceipt>, PlanError> {
    store
        .plan_receipt(scope, plan_id)
        .map_err(commit_store_error)
}

fn commit_hash(plan_id: &str, revision: u64) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("create:{plan_id}:{revision}").as_bytes())
    )
}

fn commit_store_error(conflict: StoreConflict) -> PlanError {
    error(
        match conflict {
            StoreConflict::Duplicate => "RESOURCE_CONFLICT",
            StoreConflict::StaleRevision => "PLAN_STALE",
            StoreConflict::Missing => "RESOURCE_MISSING",
            StoreConflict::InvalidLifecycle => "SOURCE_UNAVAILABLE",
            StoreConflict::InvalidInput => "COMMIT_INVALID",
            StoreConflict::Backend => "STORE_UNAVAILABLE",
        },
        None,
    )
}

/// Reads the persisted default used only for future new-instance forms.
pub fn get_default_storage(
    store: &impl StateStore,
    scope: &str,
) -> Result<StorageMethod, PlanError> {
    store.default_storage_method(scope).map_err(store_error)
}

/// Persists the default for future forms and returns the committed value.
pub fn set_default_storage(
    store: &impl StateStore,
    scope: &str,
    method: StorageMethod,
) -> Result<StorageMethod, PlanError> {
    store
        .set_default_storage_method(scope, method)
        .map_err(store_error)?;
    get_default_storage(store, scope)
}

fn store_error(conflict: StoreConflict) -> PlanError {
    error(
        match conflict {
            StoreConflict::Missing => "SCOPE_NOT_FOUND",
            _ => "STORE_UNAVAILABLE",
        },
        None,
    )
}

fn version_definition<'a>(template: &'a Value, key: &str) -> Result<&'a Value, PlanError> {
    template["versions"]
        .as_array()
        .and_then(|versions| {
            versions
                .iter()
                .find(|version| version["key"] == key)
                .map(|version| &version["definition"])
        })
        .ok_or_else(|| error("VERSION_NOT_FOUND", Some("version".into())))
}

fn entries(value: &Value) -> Vec<(String, Value)> {
    value["order"].as_array().map_or_else(Vec::new, |order| {
        order
            .iter()
            .filter_map(|key| {
                key.as_str()
                    .map(|key| (key.into(), value["values"][key].clone()))
            })
            .collect()
    })
}

fn initialize_inputs(
    definition: &Value,
    random: &mut impl RandomSource,
) -> Result<BTreeMap<String, Value>, PlanError> {
    let mut values = BTreeMap::new();
    let inputs = entries(&definition["inputs"]);
    for (key, input) in &inputs {
        if let Some(value) = input.get("default") {
            values.insert(key.clone(), value.clone());
        }
    }
    for (key, input) in &inputs {
        if input["type"] == "secret" && input["initial"] != "ask" {
            let secret = generate_input_secret(key, input, &values, &inputs, None, random)?;
            values.insert(key.clone(), Value::String(secret));
        }
    }
    Ok(values)
}

fn generate_input_secret(
    key: &str,
    input: &Value,
    values: &BTreeMap<String, Value>,
    inputs: &[(String, Value)],
    source: Option<&str>,
    random: &mut impl RandomSource,
) -> Result<String, PlanError> {
    let others: Vec<_> = inputs
        .iter()
        .filter(|(name, definition)| name != key && definition["type"] == "secret")
        .filter_map(|(name, _)| values.get(name).and_then(Value::as_str))
        .collect();
    let secret = generate_secret(random, source, &others)
        .map_err(|_| error("RANDOM_UNAVAILABLE", Some(format!("inputs.{key}"))))?;
    if !valid_input(input, Some(&Value::String(secret.clone()))) {
        return Err(error(
            "SECRET_GENERATOR_INCOMPATIBLE",
            Some(format!("inputs.{key}")),
        ));
    }
    Ok(secret)
}

fn normalize_name(input: &str) -> String {
    DisplayName::parse(input).map_or_else(|_| input.to_owned(), |name| name.as_str().to_owned())
}

fn preview(id: &str, plan: &mut Plan, inspector: &impl PortInspector) -> CreatePlanView {
    let definition = version_definition(&plan.template, &plan.version)
        .expect("selected version remains in the fixed template");
    let mut concerns = Vec::new();
    if DisplayName::parse(&plan.display_name).is_err() {
        concerns.push(PlanConcern {
            code: "DISPLAY_NAME_INVALID",
            field_path: "displayName".into(),
        });
    }
    let inputs = entries(&definition["inputs"])
        .into_iter()
        .map(|(key, input)| {
            let value = plan.values.get(&key);
            if !valid_input(&input, value) {
                concerns.push(PlanConcern {
                    code: "INPUT_REQUIRED_OR_INVALID",
                    field_path: format!("inputs.{key}"),
                });
            }
            let secret = input["type"] == "secret";
            InputView {
                key,
                definition: input,
                value: if secret { None } else { value.cloned() },
                has_secret: secret && value.is_some_and(|value| !value.is_null()),
            }
        })
        .collect();
    let slots: Vec<_> = entries(&definition["service"]["ports"])
        .into_iter()
        .map(|(key, port)| PortSlot {
            explicit: plan.explicit_ports.get(&key).cloned(),
            recommended: port["defaultHost"]
                .as_u64()
                .and_then(|number| u16::try_from(number).ok()),
            min: 1024,
            max: 65535,
            source: None,
            key,
        })
        .collect();
    let ports = match plan_ports(&slots, inspector, plan.port_cursor.clone()) {
        PortPlan::Complete(ports) => {
            plan.port_cursor = PortCursor::default();
            ports
        }
        PortPlan::NeedsInput(slot) => {
            concerns.push(PlanConcern {
                code: "PORT_NEEDS_INPUT",
                field_path: format!("ports.{slot}"),
            });
            BTreeMap::new()
        }
        PortPlan::Incomplete(cursor) => {
            plan.port_cursor = cursor;
            concerns.push(PlanConcern {
                code: "PORT_CHECK_INCOMPLETE",
                field_path: "ports".into(),
            });
            BTreeMap::new()
        }
        PortPlan::Rejected { slot, reason, .. } => {
            concerns.push(PlanConcern {
                code: match reason {
                    crate::host_ports::PortReason::InvalidInput
                    | crate::host_ports::PortReason::DuplicateInput => "PORT_INVALID",
                    crate::host_ports::PortReason::Unavailable => "PORT_CHECK_UNAVAILABLE",
                    _ => "PORT_CONFLICT",
                },
                field_path: format!("ports.{slot}"),
            });
            BTreeMap::new()
        }
    };
    CreatePlanView {
        plan_id: id.into(),
        plan_revision: plan.revision,
        display_name: plan.display_name.clone(),
        template_revision_id: plan.template_id.clone(),
        version: plan.version.clone(),
        versions: plan.template["manifest"]["versions"]
            .as_array()
            .map_or_else(Vec::new, |versions| {
                versions
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            }),
        form: definition.clone(),
        storage_method: plan.storage_method,
        inputs,
        ports,
        storage_slots: entries(&definition["service"]["storage"])
            .into_iter()
            .map(|(key, _)| key)
            .collect(),
        concerns,
    }
}

fn valid_input(input: &Value, value: Option<&Value>) -> bool {
    let Some(value) = value else {
        return input["required"] == false;
    };
    if value.is_null() {
        return input["required"] == false;
    }
    match input["type"].as_str() {
        Some("boolean") => value.is_boolean(),
        Some("integer") => value.as_i64().is_some_and(|number| {
            let validation = &input["validation"];
            number >= validation["min"].as_i64().unwrap_or(-9_007_199_254_740_991)
                && number <= validation["max"].as_i64().unwrap_or(9_007_199_254_740_991)
        }),
        Some("select") => value.as_str().is_some_and(|text| {
            input["options"]
                .as_array()
                .is_some_and(|options| options.iter().any(|option| option["value"] == text))
        }),
        Some("string" | "secret") => value.as_str().is_some_and(|text| {
            let validation = &input["validation"];
            let length = text.chars().count() as u64;
            let min = if input["type"] == "secret" { 1 } else { 0 };
            length >= validation["minLength"].as_u64().unwrap_or(min)
                && length <= validation["maxLength"].as_u64().unwrap_or(4096)
                && !text.chars().any(char::is_control)
                && validation["pattern"].as_str().is_none_or(|pattern| {
                    Regex::new(&format!(r"\A(?:{pattern})\z"))
                        .is_ok_and(|regex| regex.is_match(text))
                })
        }),
        _ => false,
    }
}
