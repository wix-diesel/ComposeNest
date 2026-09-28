//! Clone Policy evaluation and secret-safe difference previews.

use super::*;

pub(super) fn policy(input: &Value) -> &str {
    input["clone"]
        .as_str()
        .unwrap_or(if input["type"] == "secret" {
            "regenerate"
        } else {
            "copy"
        })
}

pub(super) fn evaluate_fields(
    source_definition: &Value,
    selected_definition: &Value,
    source_values: &BTreeMap<String, Value>,
    previous: &BTreeMap<String, FieldState>,
    random: &mut impl RandomSource,
) -> Result<BTreeMap<String, FieldState>, PlanError> {
    let source_types: BTreeMap<_, _> = entries(&source_definition["inputs"])
        .into_iter()
        .map(|(key, input)| (key, input["type"].clone()))
        .collect();
    let active = entries(&selected_definition["inputs"]);
    let mut fields = BTreeMap::new();
    for (key, input) in &active {
        let kind = input["type"].as_str().unwrap_or("").to_owned();
        if let Some(state) = previous.get(key).filter(|state| state.kind == kind) {
            let mut retained = state.clone();
            if policy(input) == "ask"
                && (state.policy != "ask" || state.options != input.get("options").cloned())
            {
                retained.answered = false;
            }
            retained.policy = policy(input).into();
            retained.options = input.get("options").cloned();
            fields.insert(key.clone(), retained);
            continue;
        }
        let source = source_values
            .get(key)
            .filter(|_| source_types.get(key) == Some(&input["type"]));
        let chosen = policy(input);
        let (value, origin, answered) = match chosen {
            "copy" => (
                source.cloned(),
                if source.is_some() {
                    ValueOrigin::Inherited
                } else {
                    ValueOrigin::Unset
                },
                source.map_or(input["required"] == false, |value| {
                    valid_input(input, Some(value))
                }),
            ),
            "regenerate" => {
                let values = fields
                    .iter()
                    .filter_map(|(key, state): (&String, &FieldState)| {
                        state
                            .value
                            .as_ref()
                            .map(|value| (key.clone(), value.clone()))
                    })
                    .collect();
                let secret = generate_input_secret(
                    key,
                    input,
                    &values,
                    &active,
                    source.and_then(Value::as_str),
                    random,
                )?;
                (Some(Value::String(secret)), ValueOrigin::Generated, true)
            }
            "clear" => (None, ValueOrigin::Unset, true),
            "ask" => (
                source.cloned(),
                if source.is_some() {
                    ValueOrigin::Inherited
                } else {
                    ValueOrigin::Unset
                },
                false,
            ),
            _ => return Err(error("SNAPSHOT_INVALID", Some(format!("inputs.{key}")))),
        };
        fields.insert(
            key.clone(),
            FieldState {
                kind,
                policy: chosen.into(),
                options: input.get("options").cloned(),
                value,
                origin,
                answered,
                secret_confirmed: false,
            },
        );
    }
    Ok(fields)
}

pub(super) fn answer_field(
    key: &str,
    input: &Value,
    source: Option<&Value>,
    answer: CloneAnswer,
    fields: &BTreeMap<String, FieldState>,
    active: &[(String, Value)],
    random: &mut impl RandomSource,
) -> Result<FieldState, PlanError> {
    let invalid = || error("INPUT_REQUIRED_OR_INVALID", Some(format!("inputs.{key}")));
    let (value, origin) = match answer {
        CloneAnswer::Copy => {
            let source = source
                .filter(|value| valid_input(input, Some(value)))
                .ok_or_else(invalid)?;
            (Some(source.clone()), ValueOrigin::Inherited)
        }
        CloneAnswer::Generate if input["type"] == "secret" => {
            let values = fields
                .iter()
                .filter_map(|(key, state)| {
                    state
                        .value
                        .as_ref()
                        .map(|value| (key.clone(), value.clone()))
                })
                .collect();
            let secret = generate_input_secret(
                key,
                input,
                &values,
                active,
                source.and_then(Value::as_str),
                random,
            )?;
            (Some(Value::String(secret)), ValueOrigin::Generated)
        }
        CloneAnswer::Input(value)
            if value.as_str().is_none_or(|text| text.len() <= 16 * 1024)
                && valid_input(input, Some(&value)) =>
        {
            (Some(value), ValueOrigin::UserInput)
        }
        CloneAnswer::Clear if input["required"] == false => (None, ValueOrigin::Unset),
        _ => return Err(invalid()),
    };
    Ok(FieldState {
        kind: input["type"].as_str().unwrap_or("").into(),
        policy: policy(input).into(),
        options: input.get("options").cloned(),
        value,
        origin,
        answered: true,
        secret_confirmed: false,
    })
}

pub(super) fn check_source(plan: &Plan, store: &impl StateStore) -> Result<(), PlanError> {
    let source = store
        .clone_source(&plan.source.scope_id, &plan.source.id)
        .map_err(commit_store_error)?;
    let Some(source) = source else {
        let revision = store
            .source_revision(&plan.source.scope_id, &plan.source.id)
            .map_err(commit_store_error)?;
        return if revision == Some(plan.source.revision) {
            Err(error("SOURCE_UNAVAILABLE", None))
        } else {
            Err(error("PLAN_STALE", Some("sourceId".into())))
        };
    };
    if source.revision != plan.source.revision || source.target_id != plan.source.target_id {
        return Err(error("PLAN_STALE", Some("sourceId".into())));
    }
    Ok(())
}

fn difference(source: &[String], current: &[String]) -> (Vec<String>, Vec<String>) {
    let old: BTreeSet<_> = source.iter().cloned().collect();
    let new: BTreeSet<_> = current.iter().cloned().collect();
    (
        new.difference(&old).cloned().collect(),
        old.difference(&new).cloned().collect(),
    )
}

pub(super) fn preview(
    id: &str,
    plan: &mut Plan,
    inspector: &impl PortInspector,
    advance_for_port_change: bool,
) -> ClonePlanView {
    let definition = version_definition(&plan.template, &plan.version)
        .expect("selected Version remains in the private Snapshot");
    let mut concerns = Vec::new();
    if DisplayName::parse(&plan.display_name).is_err() || plan.display_name == plan.source.name {
        concerns.push(PlanConcern {
            code: "DISPLAY_NAME_INVALID",
            field_path: "displayName".into(),
        });
    }
    if !plan.version_answered {
        concerns.push(PlanConcern {
            code: "VERSION_NEEDS_ANSWER",
            field_path: "version".into(),
        });
    }
    if !plan.storage_answered {
        concerns.push(PlanConcern {
            code: "STORAGE_NEEDS_ANSWER",
            field_path: "storageMethod".into(),
        });
    }
    let source_inputs = entries(&plan.source_definition["inputs"]);
    let active_inputs = entries(&definition["inputs"]);
    let mut inputs = Vec::new();
    for (key, input) in &active_inputs {
        let source_input = source_inputs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, input)| input);
        let source = plan.source_values.get(key);
        let state = &plan.fields[key];
        let secret = input["type"] == "secret";
        let type_changed = source_input.is_some_and(|old| old["type"] != input["type"]);
        let invalid = !valid_input(input, state.value.as_ref());
        if type_changed || invalid || !state.answered {
            concerns.push(PlanConcern {
                code: if type_changed {
                    "INPUT_TYPE_CHANGED"
                } else {
                    "INPUT_REQUIRED_OR_INVALID"
                },
                field_path: format!("inputs.{key}"),
            });
        }
        let needs_confirmation = secret
            && state.value.is_some()
            && state.value.as_ref() == source
            && !state.secret_confirmed;
        if needs_confirmation {
            concerns.push(PlanConcern {
                code: "SECRET_REUSE_NEEDS_CONFIRMATION",
                field_path: format!("inputs.{key}"),
            });
        }
        inputs.push(InputDiff {
            key: key.clone(),
            definition: Some(input.clone()),
            source: if secret { None } else { source.cloned() },
            candidate: if secret { None } else { state.value.clone() },
            policy: policy(input).into(),
            origin: state.origin,
            changed: state.value.as_ref() != source,
            added: source_input.is_none(),
            removed: false,
            needs_answer: !state.answered,
            needs_secret_confirmation: needs_confirmation,
            has_secret: secret && state.value.is_some(),
        });
    }
    for (key, old) in &source_inputs {
        if active_inputs.iter().any(|(name, _)| name == key) {
            continue;
        }
        let secret = old["type"] == "secret";
        inputs.push(InputDiff {
            key: key.clone(),
            definition: None,
            source: if secret {
                None
            } else {
                plan.source_values.get(key).cloned()
            },
            candidate: None,
            policy: policy(old).into(),
            origin: ValueOrigin::Unset,
            changed: plan.source_values.contains_key(key),
            added: false,
            removed: true,
            needs_answer: false,
            needs_secret_confirmation: false,
            has_secret: false,
        });
    }
    let source_ports: Vec<_> = plan
        .source
        .ports
        .iter()
        .map(|port| port.slot.clone())
        .collect();
    let current_ports: Vec<_> = entries(&definition["service"]["ports"])
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    let (added_ports, removed_ports) = difference(&source_ports, &current_ports);
    let slots: Vec<_> = entries(&definition["service"]["ports"])
        .into_iter()
        .map(|(key, port)| PortSlot {
            source: plan
                .source
                .ports
                .iter()
                .find(|item| item.slot == key)
                .map(|item| item.host_port),
            recommended: port["defaultHost"]
                .as_u64()
                .and_then(|n| u16::try_from(n).ok()),
            explicit: plan.explicit_ports.get(&key).cloned(),
            min: 1024,
            max: 65535,
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
                    PortReason::InvalidInput | PortReason::DuplicateInput => "PORT_INVALID",
                    PortReason::Unavailable => "PORT_CHECK_UNAVAILABLE",
                    _ => "PORT_CONFLICT",
                },
                field_path: format!("ports.{slot}"),
            });
            BTreeMap::new()
        }
    };
    for (slot, port) in &ports {
        if plan
            .source
            .ports
            .iter()
            .any(|source| source.host_port == *port)
        {
            concerns.push(PlanConcern {
                code: "PORT_CONFLICT",
                field_path: format!("ports.{slot}"),
            });
        }
    }
    if !ports.is_empty() {
        if advance_for_port_change && !plan.last_ports.is_empty() && plan.last_ports != ports {
            plan.revision = plan.revision.saturating_add(1);
        }
        plan.last_ports = ports.clone();
    }
    let source_storage: Vec<_> = entries(&plan.source_definition["service"]["storage"])
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    let storage_slots: Vec<_> = entries(&definition["service"]["storage"])
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    let (added_storage, removed_storage) = difference(&source_storage, &storage_slots);
    ClonePlanView {
        plan_id: id.into(),
        plan_revision: plan.revision,
        source_id: plan.source.id.clone(),
        instance_id: plan.instance_id.clone(),
        project_name: format!("cn-{}", plan.instance_id),
        display_name: plan.display_name.clone(),
        version: plan.version.clone(),
        source_version: plan.source.selected_version.clone(),
        versions: plan.template["manifest"]["versions"]
            .as_array()
            .map_or_else(Vec::new, |versions| {
                versions
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            }),
        storage_method: plan.storage_method,
        source_storage_method: plan.source.storage_method,
        inputs,
        ports,
        source_ports: plan
            .source
            .ports
            .iter()
            .map(|port| (port.slot.clone(), port.host_port))
            .collect(),
        added_ports,
        removed_ports,
        storage_slots,
        added_storage,
        removed_storage,
        concerns,
    }
}
