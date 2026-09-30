//! Owned container and network removal without touching data or Compose files.

use crate::docker_target::BoundDocker;
use composenest_application::{
    lifecycle_operation::LifecycleEffectError as Error,
    named_volumes::valid_label_value,
    operation_journal::{ExpectedResult, RequestReceipt, StepCommand, StepIntent, StepOutcome},
    operation_recovery::RecoveryJournal,
};
use serde_json::Value;
use std::{collections::BTreeSet, ffi::OsString, time::Duration};

const CONTAINER_FORMAT: &str =
    r#"{"Id":{{json .Id}},"Labels":{{json .Config.Labels}},"Status":{{json .State.Status}}}"#;
const NETWORK_FORMAT: &str = r#"{"Id":{{json .Id}},"Name":{{json .Name}},"Labels":{{json .Labels}},"Containers":{{json .Containers}}}"#;

/// Fixed identity used to verify every discovered runtime resource independently.
pub struct DeletionOwner<'a> {
    /// Management scope recorded in the ledger.
    pub scope: &'a str,
    /// Full canonical instance ID.
    pub instance: &'a str,
    /// Dedicated Compose project name.
    pub project: &'a str,
    /// Previously observed full container ID, checked even if its labels changed.
    pub saved_container: Option<&'a str>,
}

trait RemovalPort {
    fn read(&self, args: &[OsString]) -> impl Future<Output = Result<Vec<u8>, Error>>;
    fn change(&self, args: &[OsString]) -> impl Future<Output = Result<(), Error>>;
}

impl RemovalPort for BoundDocker {
    async fn read(&self, args: &[OsString]) -> Result<Vec<u8>, Error> {
        let output = self
            .read_inspection(args)
            .await
            .map_err(|_| Error::OutcomeUnknown)?;
        if output.outcome_unknown
            || output.stdout.truncated
            || output.stderr.truncated
            || !output.status.is_some_and(|status| status.success())
        {
            return Err(Error::OutcomeUnknown);
        }
        Ok(output.stdout.bytes)
    }
    async fn change(&self, args: &[OsString]) -> Result<(), Error> {
        let output = BoundDocker::change(self, args, Duration::from_secs(120))
            .await
            .map_err(|_| Error::OutcomeUnknown)?;
        if output.outcome_unknown || !output.status.is_some_and(|status| status.success()) {
            return Err(Error::OutcomeUnknown);
        }
        Ok(())
    }
}

/// Removes only freshly verified owned resources and then confirms project absence.
/// A foreign network endpoint blocks deletion before any runtime change is sent.
pub async fn remove_owned_runtime<J: RecoveryJournal>(
    docker: &BoundDocker,
    journal: &J,
    receipt: &RequestReceipt,
    owner: &DeletionOwner<'_>,
) -> Result<(), Error> {
    remove_runtime(docker, journal, receipt, owner).await
}

/// Confirms absence on the fixed Engine without sending any changes.
pub async fn verify_runtime_absent(
    docker: &BoundDocker,
    owner: &DeletionOwner<'_>,
) -> Result<(), Error> {
    let filters = vec![
        format!("label=com.docker.compose.project={}", owner.project),
        format!("label=io.composenest.instance={}", owner.instance),
    ];
    let mut containers = filters.clone();
    if let Some(id) = owner.saved_container {
        containers.push(format!("id={id}"));
    }
    let mut networks = filters;
    networks.push(format!("name=^{}_default$", owner.project));
    if !list(docker, "container", &containers).await?.is_empty()
        || !list(docker, "network", &networks).await?.is_empty()
    {
        return Err(Error::OutcomeUnknown);
    }
    Ok(())
}

async fn list(
    port: &impl RemovalPort,
    kind: &str,
    filters: &[String],
) -> Result<BTreeSet<String>, Error> {
    let mut ids = BTreeSet::new();
    for filter in filters {
        let mut args: Vec<OsString> = [kind, "ls", "--quiet", "--no-trunc", "--filter", filter]
            .into_iter()
            .map(Into::into)
            .collect();
        if kind == "container" {
            args.push("--all".into());
        }
        let bytes = port.read(&args).await?;
        for id in std::str::from_utf8(&bytes)
            .map_err(|_| Error::OutcomeUnknown)?
            .lines()
            .filter(|id| !id.is_empty())
        {
            if !valid_id(id) {
                return Err(Error::OutcomeUnknown);
            }
            ids.insert(id.into());
        }
    }
    Ok(ids)
}

async fn inspect(
    port: &impl RemovalPort,
    kind: &str,
    id: &str,
    owner: &DeletionOwner<'_>,
) -> Result<Value, Error> {
    let format = if kind == "container" {
        CONTAINER_FORMAT
    } else {
        NETWORK_FORMAT
    };
    let bytes = port
        .read(&[
            kind.into(),
            "inspect".into(),
            "--format".into(),
            format.into(),
            id.into(),
        ])
        .await?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::OutcomeUnknown)?;
    let labels = &value["Labels"];
    if value["Id"] != id
        || labels["com.docker.compose.project"] != owner.project
        || labels["io.composenest.scope"] != owner.scope
        || labels["io.composenest.instance"] != owner.instance
        || (kind == "container" && labels["com.docker.compose.service"] != "main")
        || (kind == "network"
            && (labels["com.docker.compose.network"] != "default"
                || value["Name"] != format!("{}_default", owner.project)))
    {
        return Err(Error::Rejected);
    }
    Ok(value)
}

fn endpoints(value: &Value) -> Result<BTreeSet<String>, Error> {
    value["Containers"]
        .as_object()
        .ok_or(Error::OutcomeUnknown)
        .map(|items| items.keys().cloned().collect())
}

fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn effect(
    port: &impl RemovalPort,
    journal: &impl RecoveryJournal,
    receipt: &RequestReceipt,
    kind: &str,
    action: &str,
    id: &str,
) -> Result<(), Error> {
    let op = journal
        .recoverable(&receipt.operation_id)
        .map_err(|_| Error::OutcomeUnknown)?;
    let sequence = op
        .steps
        .last()
        .map_or(Some(1), |step| step.sequence.checked_add(1))
        .ok_or(Error::Rejected)?;
    let (command_kind, expected_result) = match (kind, action) {
        ("container", "stop") => (StepCommand::ComposeStop, ExpectedResult::ContainerStopped),
        ("container", "rm") => (
            StepCommand::RemoveContainer,
            ExpectedResult::ContainerAbsent,
        ),
        ("network", "rm") => (StepCommand::RemoveNetwork, ExpectedResult::StateObserved),
        _ => return Err(Error::Rejected),
    };
    journal
        .record_step(&StepIntent {
            operation_id: op.id,
            sequence,
            attempt: op.attempt,
            command_kind,
            resource_id: id.into(),
            expected_result,
        })
        .map_err(|_| Error::OutcomeUnknown)?;
    let mut args: Vec<OsString> = [kind, action].into_iter().map(Into::into).collect();
    if action == "stop" {
        args.extend(["--time".into(), "30".into()]);
    }
    args.push(id.into());
    // Keep the step uncertain until its postcondition is independently observed.
    if port.change(&args).await.is_err() {
        journal
            .finish_step(&receipt.operation_id, sequence, StepOutcome::Unknown)
            .map_err(|_| Error::OutcomeUnknown)?;
        return Err(Error::OutcomeUnknown);
    }
    Ok(())
}

async fn finish(journal: &impl RecoveryJournal, receipt: &RequestReceipt) -> Result<(), Error> {
    let op = journal
        .recoverable(&receipt.operation_id)
        .map_err(|_| Error::OutcomeUnknown)?;
    let last = op.steps.last().ok_or(Error::OutcomeUnknown)?;
    journal
        .finish_step(&op.id, last.sequence, StepOutcome::Succeeded)
        .map_err(|_| Error::OutcomeUnknown)
}

async fn remove_runtime(
    port: &impl RemovalPort,
    journal: &impl RecoveryJournal,
    receipt: &RequestReceipt,
    owner: &DeletionOwner<'_>,
) -> Result<(), Error> {
    if owner.instance.len() != 32
        || !owner
            .instance
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || owner.project != format!("cn-{}", owner.instance)
        || !valid_label_value(owner.scope)
        || owner.instance != receipt.instance_id
        || owner.scope != receipt.scope_id
        || owner.saved_container.is_some_and(|id| !valid_id(id))
    {
        return Err(Error::Rejected);
    }
    let filters = vec![
        format!("label=com.docker.compose.project={}", owner.project),
        format!("label=io.composenest.instance={}", owner.instance),
    ];
    let mut container_filters = filters.clone();
    if let Some(id) = owner.saved_container {
        container_filters.push(format!("id={id}"));
    }
    let mut network_filters = filters;
    network_filters.push(format!("name=^{}_default$", owner.project));
    let containers = list(port, "container", &container_filters).await?;
    let networks = list(port, "network", &network_filters).await?;
    for id in &containers {
        inspect(port, "container", id, owner).await?;
    }
    for id in &networks {
        if !endpoints(&inspect(port, "network", id, owner).await?)?.is_subset(&containers) {
            return Err(Error::Rejected);
        }
    }
    for id in containers {
        let current = inspect(port, "container", &id, owner).await?;
        if !matches!(
            current["Status"].as_str(),
            Some("exited" | "created" | "dead")
        ) {
            effect(port, journal, receipt, "container", "stop", &id).await?;
            let stopped = inspect(port, "container", &id, owner).await?;
            if !matches!(
                stopped["Status"].as_str(),
                Some("exited" | "created" | "dead")
            ) {
                return Err(Error::OutcomeUnknown);
            }
            finish(journal, receipt).await?;
        }
        inspect(port, "container", &id, owner).await?;
        effect(port, journal, receipt, "container", "rm", &id).await?;
        if !list(port, "container", &[format!("id={id}")])
            .await?
            .is_empty()
        {
            return Err(Error::OutcomeUnknown);
        }
        finish(journal, receipt).await?;
    }
    for id in networks {
        if !endpoints(&inspect(port, "network", &id, owner).await?)?.is_empty() {
            return Err(Error::Rejected);
        }
        effect(port, journal, receipt, "network", "rm", &id).await?;
        if !list(port, "network", &[format!("id={id}")])
            .await?
            .is_empty()
        {
            return Err(Error::OutcomeUnknown);
        }
        finish(journal, receipt).await?;
    }
    if !list(port, "container", &container_filters)
        .await?
        .is_empty()
        || !list(port, "network", &network_filters).await?.is_empty()
    {
        return Err(Error::OutcomeUnknown);
    }
    Ok(())
}

#[cfg(test)]
#[path = "docker_delete_tests.rs"]
mod tests;
