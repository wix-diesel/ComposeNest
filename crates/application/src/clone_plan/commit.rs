//! Atomic confirmation of a reviewed clone plan.

use super::*;

impl ClonePlans {
    /// Confirms a reviewed clone atomically with its source revision guard.
    pub fn commit_plan(
        &mut self,
        request: CommitCreate,
        store: &impl PlanCommitStore,
        clock: &impl Clock,
        inspector: &impl PortInspector,
    ) -> Result<RequestReceipt, PlanError> {
        self.expire(clock.unix_seconds());
        let hash = format!(
            "{:x}",
            Sha256::digest(format!("clone:{}:{}", request.plan_id, request.revision).as_bytes())
        );
        if let Some(previous) = store
            .receipt(&request.scope_id, &request.request_id)
            .map_err(commit_store_error)?
        {
            return if previous.plan_id.as_deref() == Some(&request.plan_id)
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
        let plan = self.plan_mut(&request.scope_id, &request.plan_id)?;
        check_source(plan, store)?;
        if plan.revision != request.revision {
            return Err(error("PLAN_STALE", Some("revision".into())));
        }
        let view = preview(&request.plan_id, plan, inspector, true);
        if plan.revision != request.revision {
            return Err(error("PLAN_RECONFIRM", Some("ports".into())));
        }
        if let Some(concern) = view.concerns.first() {
            return Err(error(concern.code, Some(concern.field_path.clone())));
        }
        if view.ports != request.confirmed_ports {
            return Err(error("PLAN_RECONFIRM", Some("ports".into())));
        }
        if request.target_id != plan.source.target_id
            || request.instance_id != plan.instance_id
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
        let expected: BTreeSet<_> = view.storage_slots.iter().map(String::as_str).collect();
        let actual: BTreeSet<_> = request
            .storage
            .iter()
            .map(|item| item.slot.as_str())
            .collect();
        if expected != actual
            || actual.len() != request.storage.len()
            || request.storage.iter().any(|item| {
                !valid_storage_identity(
                    &item.resource_identity,
                    plan.storage_method,
                    &plan.instance_id,
                    &item.slot,
                ) || item.ownership_evidence.is_empty()
                    || plan.source.storage.iter().any(|source| {
                        overlaps_resource(&source.resource_identity, &item.resource_identity)
                            || source.ownership_evidence == item.ownership_evidence
                    })
            })
            || request.storage.iter().enumerate().any(|(index, item)| {
                request.storage[index + 1..].iter().any(|other| {
                    overlaps_resource(&item.resource_identity, &other.resource_identity)
                })
            })
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
                        .and_then(|number| u16::try_from(number).ok())
                        .ok_or_else(|| error("SNAPSHOT_INVALID", None))?,
                    host_ip: "127.0.0.1".into(),
                    slot,
                })
            })
            .collect::<Result<Vec<_>, PlanError>>()?;
        let values: BTreeMap<_, _> = plan
            .fields
            .iter()
            .filter_map(|(key, field)| field.value.as_ref().map(|value| (key, value)))
            .collect();
        let instance = InstanceRecord {
            id: request.instance_id.clone(),
            scope_id: request.scope_id.clone(),
            target_id: request.target_id,
            name: plan.display_name.clone(),
            project_name: format!("cn-{}", request.instance_id),
            clone_source_id: Some(plan.source.id.clone()),
            template_revision_id: String::new(),
            selected_version: plan.version.clone(),
            storage_method: plan.storage_method,
            inputs_json: serde_json::to_string(&values)
                .map_err(|_| error("COMMIT_INVALID", None))?,
            ports,
            storage: request.storage,
        };
        let intent = OperationIntent {
            id: request.operation_id.clone(),
            instance_id: instance.id.clone(),
            kind: OperationKind::Clone,
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
        let guard = CloneSourceGuard {
            instance_id: plan.source.id.clone(),
            revision: plan.source.revision,
            spec_revision: plan.source.spec_revision,
        };
        let result = store
            .commit_plan(&instance, &intent, &receipt, &target, Some(&guard))
            .map_err(commit_store_error)?;
        self.plans.remove(&request.plan_id);
        Ok(result)
    }
}

fn valid_storage_identity(
    identity: &str,
    method: StorageMethod,
    instance_id: &str,
    slot: &str,
) -> bool {
    match method {
        StorageMethod::Volume => identity == format!("cn-{instance_id}-{slot}"),
        StorageMethod::Bind => {
            !identity.is_empty()
                && !Path::new(identity).components().any(|component| {
                    matches!(
                        component,
                        std::path::Component::ParentDir | std::path::Component::CurDir
                    )
                })
        }
    }
}
