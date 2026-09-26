//! Docker named-volume creation with journal and ownership-label verification.

use std::time::Duration;

use composenest_application::{
    named_volumes::{NamedVolumeError, NamedVolumePort, named_volume_name, valid_label_value},
    operation_journal::{
        ExpectedResult, OperationJournal, StepCommand, StepIntent, StepOutcome, StepRecord,
    },
    state_store::{StateStore, StorageLedgerEntry, StorageMethod},
};
use composenest_domain::{
    identity::{InstanceId, SlotId},
    instance::StoragePresence,
};
use serde_json::Value;

use crate::docker_target::BoundDocker;

const INSPECT_FORMAT: &str = r#"{"Name":{{json .Name}},"Driver":{{json .Driver}},"Labels":{"io.composenest.scope":{{if .Labels}}{{json (index .Labels "io.composenest.scope")}}{{else}}null{{end}},"io.composenest.instance":{{if .Labels}}{{json (index .Labels "io.composenest.instance")}}{{else}}null{{end}},"io.composenest.slot":{{if .Labels}}{{json (index .Labels "io.composenest.slot")}}{{else}}null{{end}},"io.composenest.allocation-operation":{{if .Labels}}{{json (index .Labels "io.composenest.allocation-operation")}}{{else}}null{{end}}}}}"#;
const LABEL_SCOPE: &str = "io.composenest.scope";
const LABEL_INSTANCE: &str = "io.composenest.instance";
const LABEL_SLOT: &str = "io.composenest.slot";
const LABEL_OPERATION: &str = "io.composenest.allocation-operation";
const DOCKER_TIMEOUT: Duration = Duration::from_secs(30);

/// Creates and verifies named volumes through one registered Engine.
pub struct DockerNamedVolumes {
    docker: BoundDocker,
}

impl DockerNamedVolumes {
    /// Binds named-volume operations to a previously registered Docker target.
    #[must_use]
    pub fn new(docker: BoundDocker) -> Self {
        Self { docker }
    }

    async fn ensure(
        &self,
        state: &dyn StateStore,
        journal: &dyn OperationJournal,
        instance_id: InstanceId,
        slot: &SlotId,
        attempt: u64,
        sequence: u64,
    ) -> Result<StorageLedgerEntry, NamedVolumeError> {
        let entry = load_entry(state, instance_id, slot)?;
        let expected = validate_entry(&entry, instance_id, slot)?;
        if entry.presence == StoragePresence::Missing {
            return Err(NamedVolumeError::Missing);
        }

        let steps = allocation_steps(journal, &entry)?;
        match self.inspect_volume(&entry, &expected).await {
            VolumeCheck::Owned => {
                if steps.is_empty() {
                    mark_unverified(state, &entry)?;
                    return Err(NamedVolumeError::Unverified);
                }
                finish_unresolved_step(journal, &entry, &steps)?;
                mark_present(state, &entry)?;
                return load_entry(state, instance_id, slot);
            }
            VolumeCheck::Foreign => {
                mark_unverified(state, &entry)?;
                return Err(NamedVolumeError::Unverified);
            }
            VolumeCheck::Unavailable => {
                mark_unverified(state, &entry)?;
                return Err(NamedVolumeError::Unverified);
            }
            VolumeCheck::Absent => {}
        }

        if entry.presence == StoragePresence::Present
            || steps
                .iter()
                .any(|step| step.outcome == Some(StepOutcome::Succeeded))
        {
            state
                .set_storage_presence(&entry.instance_id, &entry.slot, StoragePresence::Missing)
                .map_err(|_| NamedVolumeError::Backend)?;
            return Err(NamedVolumeError::Missing);
        }
        let retrying_failed_attempt = !steps.is_empty()
            && steps
                .iter()
                .all(|step| step.outcome == Some(StepOutcome::Failed))
            && steps.iter().all(|step| step.attempt < attempt);
        if !steps.is_empty() && !retrying_failed_attempt {
            mark_unverified(state, &entry)?;
            return Err(NamedVolumeError::Unverified);
        }

        let step = StepIntent {
            operation_id: entry.allocation.ownership_evidence.clone(),
            sequence,
            attempt,
            command_kind: StepCommand::CreateVolume,
            resource_id: entry.allocation.resource_identity.clone(),
            expected_result: ExpectedResult::VolumeCreated,
        };
        journal
            .record_step(&step)
            .map_err(|_| NamedVolumeError::Backend)?;

        let create = self.create_volume(&entry, &expected).await;
        let after = self.inspect_volume(&entry, &expected).await;
        match after {
            VolumeCheck::Owned => {
                journal
                    .finish_step(&step.operation_id, step.sequence, StepOutcome::Succeeded)
                    .map_err(|_| NamedVolumeError::Backend)?;
                mark_present(state, &entry)?;
                load_entry(state, instance_id, slot)
            }
            VolumeCheck::Foreign => {
                journal
                    .finish_step(&step.operation_id, step.sequence, StepOutcome::Failed)
                    .map_err(|_| NamedVolumeError::Backend)?;
                mark_unverified(state, &entry)?;
                Err(NamedVolumeError::Unverified)
            }
            VolumeCheck::Absent => {
                let outcome = if create == CreateResult::Uncertain {
                    StepOutcome::Unknown
                } else {
                    StepOutcome::Failed
                };
                journal
                    .finish_step(&step.operation_id, step.sequence, outcome)
                    .map_err(|_| NamedVolumeError::Backend)?;
                if create == CreateResult::Uncertain {
                    mark_unverified(state, &entry)?;
                }
                Err(if create == CreateResult::Uncertain {
                    NamedVolumeError::Unverified
                } else {
                    NamedVolumeError::Backend
                })
            }
            VolumeCheck::Unavailable => {
                journal
                    .finish_step(&step.operation_id, step.sequence, StepOutcome::Unknown)
                    .map_err(|_| NamedVolumeError::Backend)?;
                mark_unverified(state, &entry)?;
                Err(NamedVolumeError::Unverified)
            }
        }
    }

    async fn inspect_only(
        &self,
        state: &dyn StateStore,
        journal: &dyn OperationJournal,
        instance_id: InstanceId,
        slot: &SlotId,
    ) -> Result<StorageLedgerEntry, NamedVolumeError> {
        let entry = load_entry(state, instance_id, slot)?;
        let expected = validate_entry(&entry, instance_id, slot)?;
        if entry.presence == StoragePresence::Missing {
            return Ok(entry);
        }
        let steps = allocation_steps(journal, &entry)?;
        match self.inspect_volume(&entry, &expected).await {
            VolumeCheck::Owned if !steps.is_empty() => {
                finish_unresolved_step(journal, &entry, &steps)?;
                mark_present(state, &entry)?;
            }
            VolumeCheck::Owned | VolumeCheck::Foreign | VolumeCheck::Unavailable => {
                mark_unverified(state, &entry)?;
            }
            VolumeCheck::Absent
                if entry.presence == StoragePresence::Present
                    || steps
                        .iter()
                        .any(|step| step.outcome == Some(StepOutcome::Succeeded)) =>
            {
                state
                    .set_storage_presence(&entry.instance_id, &entry.slot, StoragePresence::Missing)
                    .map_err(|_| NamedVolumeError::Backend)?;
            }
            VolumeCheck::Absent if !steps.is_empty() => {
                mark_unverified(state, &entry)?;
            }
            VolumeCheck::Absent => {}
        }
        load_entry(state, instance_id, slot)
    }

    async fn inspect_volume(
        &self,
        entry: &StorageLedgerEntry,
        expected: &VolumeLabels,
    ) -> VolumeCheck {
        let args = [
            "volume".into(),
            "inspect".into(),
            "--format".into(),
            INSPECT_FORMAT.into(),
            entry.allocation.resource_identity.clone().into(),
        ];
        let outcome = match self.docker.read_inspection(&args).await {
            Ok(outcome) => outcome,
            Err(_) => return VolumeCheck::Unavailable,
        };
        if outcome.outcome_unknown || outcome.stdout.truncated || outcome.stderr.truncated {
            return VolumeCheck::Unavailable;
        }
        if outcome.status.is_some_and(|status| status.success()) {
            return parse_volume(&outcome.stdout.bytes, expected);
        }
        self.confirm_absent(&entry.allocation.resource_identity)
            .await
    }

    async fn confirm_absent(&self, name: &str) -> VolumeCheck {
        let filter = format!("name={name}");
        let args = [
            "volume".into(),
            "ls".into(),
            "--quiet".into(),
            "--filter".into(),
            filter.into(),
            "--format".into(),
            "{{.Name}}".into(),
        ];
        let outcome = match self.docker.read(&args).await {
            Ok(outcome) => outcome,
            Err(_) => return VolumeCheck::Unavailable,
        };
        if outcome.outcome_unknown
            || outcome.stdout.truncated
            || outcome.stderr.truncated
            || !outcome.status.is_some_and(|status| status.success())
        {
            return VolumeCheck::Unavailable;
        }
        let Ok(output) = std::str::from_utf8(&outcome.stdout.bytes) else {
            return VolumeCheck::Unavailable;
        };
        if output.lines().any(|line| line == name) {
            VolumeCheck::Unavailable
        } else {
            VolumeCheck::Absent
        }
    }

    async fn create_volume(
        &self,
        entry: &StorageLedgerEntry,
        labels: &VolumeLabels,
    ) -> CreateResult {
        let args = [
            "volume".into(),
            "create".into(),
            "--driver".into(),
            "local".into(),
            "--label".into(),
            format!("{LABEL_SCOPE}={}", labels.scope).into(),
            "--label".into(),
            format!("{LABEL_INSTANCE}={}", labels.instance).into(),
            "--label".into(),
            format!("{LABEL_SLOT}={}", labels.slot).into(),
            "--label".into(),
            format!("{LABEL_OPERATION}={}", labels.operation).into(),
            entry.allocation.resource_identity.clone().into(),
        ];
        match self.docker.change(&args, DOCKER_TIMEOUT).await {
            Ok(outcome)
                if !outcome.outcome_unknown
                    && !outcome.stdout.truncated
                    && outcome.status.is_some_and(|status| status.success())
                    && std::str::from_utf8(&outcome.stdout.bytes).is_ok_and(|output| {
                        output.trim() == entry.allocation.resource_identity
                    }) =>
            {
                CreateResult::Confirmed
            }
            Ok(outcome) if !outcome.outcome_unknown && outcome.status.is_some() => {
                CreateResult::Rejected
            }
            _ => CreateResult::Uncertain,
        }
    }
}

impl NamedVolumePort for DockerNamedVolumes {
    fn ensure_named_volume<'a>(
        &'a self,
        state: &'a dyn StateStore,
        journal: &'a dyn OperationJournal,
        instance_id: InstanceId,
        slot: &'a SlotId,
        attempt: u64,
        sequence: u64,
    ) -> impl std::future::Future<Output = Result<StorageLedgerEntry, NamedVolumeError>> + Send + 'a
    {
        self.ensure(state, journal, instance_id, slot, attempt, sequence)
    }

    fn inspect_named_volume<'a>(
        &'a self,
        state: &'a dyn StateStore,
        journal: &'a dyn OperationJournal,
        instance_id: InstanceId,
        slot: &'a SlotId,
    ) -> impl std::future::Future<Output = Result<StorageLedgerEntry, NamedVolumeError>> + Send + 'a
    {
        self.inspect_only(state, journal, instance_id, slot)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum VolumeCheck {
    Owned,
    Foreign,
    Absent,
    Unavailable,
}

#[derive(Debug, PartialEq, Eq)]
enum CreateResult {
    Confirmed,
    Rejected,
    Uncertain,
}

struct VolumeLabels {
    scope: String,
    instance: String,
    slot: String,
    operation: String,
}

fn load_entry(
    state: &dyn StateStore,
    instance_id: InstanceId,
    slot: &SlotId,
) -> Result<StorageLedgerEntry, NamedVolumeError> {
    state
        .storage_allocation(&instance_key(instance_id), slot.as_str())
        .map_err(|_| NamedVolumeError::Backend)?
        .ok_or(NamedVolumeError::InvalidAllocation)
}

fn validate_entry(
    entry: &StorageLedgerEntry,
    instance_id: InstanceId,
    slot: &SlotId,
) -> Result<VolumeLabels, NamedVolumeError> {
    let instance = instance_key(instance_id);
    let operation = entry.allocation.ownership_evidence.as_str();
    if entry.instance_id != instance
        || entry.slot != slot.as_str()
        || entry.allocation.slot != slot.as_str()
        || entry.method != StorageMethod::Volume
        || entry.allocation.resource_identity != named_volume_name(instance_id, slot)
        || !valid_label_value(&entry.scope_id)
        || !valid_label_value(operation)
    {
        return Err(NamedVolumeError::InvalidAllocation);
    }
    Ok(VolumeLabels {
        scope: entry.scope_id.clone(),
        instance,
        slot: slot.as_str().to_owned(),
        operation: operation.to_owned(),
    })
}

fn allocation_steps(
    journal: &dyn OperationJournal,
    entry: &StorageLedgerEntry,
) -> Result<Vec<StepRecord>, NamedVolumeError> {
    journal
        .steps_for_resource(
            &entry.allocation.ownership_evidence,
            &entry.allocation.resource_identity,
        )
        .map(|steps| {
            steps
                .into_iter()
                .filter(|step| {
                    step.instance_id == entry.instance_id
                        && step.scope_id == entry.scope_id
                        && step.command_kind == StepCommand::CreateVolume
                        && step.expected_result == ExpectedResult::VolumeCreated
                })
                .collect()
        })
        .map_err(|_| NamedVolumeError::Backend)
}

fn finish_unresolved_step(
    journal: &dyn OperationJournal,
    entry: &StorageLedgerEntry,
    steps: &[StepRecord],
) -> Result<(), NamedVolumeError> {
    for step in steps.iter().filter(|step| step.outcome.is_none()) {
        journal
            .finish_step(
                &entry.allocation.ownership_evidence,
                step.sequence,
                StepOutcome::Succeeded,
            )
            .map_err(|_| NamedVolumeError::Backend)?;
    }
    Ok(())
}

fn mark_present(
    state: &dyn StateStore,
    entry: &StorageLedgerEntry,
) -> Result<(), NamedVolumeError> {
    state
        .set_storage_presence(&entry.instance_id, &entry.slot, StoragePresence::Present)
        .map_err(|_| NamedVolumeError::Backend)
}

fn mark_unverified(
    state: &dyn StateStore,
    entry: &StorageLedgerEntry,
) -> Result<(), NamedVolumeError> {
    state
        .set_storage_presence(&entry.instance_id, &entry.slot, StoragePresence::Unverified)
        .map_err(|_| NamedVolumeError::Backend)
}

fn instance_key(instance_id: InstanceId) -> String {
    format!("{:032x}", instance_id.as_u128())
}

fn parse_volume(bytes: &[u8], expected: &VolumeLabels) -> VolumeCheck {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return VolumeCheck::Unavailable;
    };
    let Some(labels) = value.get("Labels").and_then(Value::as_object) else {
        return VolumeCheck::Foreign;
    };
    let label_matches = [
        (LABEL_SCOPE, expected.scope.as_str()),
        (LABEL_INSTANCE, expected.instance.as_str()),
        (LABEL_SLOT, expected.slot.as_str()),
        (LABEL_OPERATION, expected.operation.as_str()),
    ]
    .into_iter()
    .all(|(key, expected)| labels.get(key).and_then(Value::as_str) == Some(expected));
    if value.get("Name").and_then(Value::as_str) == Some(expected_name(expected).as_str())
        && value.get("Driver").and_then(Value::as_str) == Some("local")
        && label_matches
    {
        VolumeCheck::Owned
    } else {
        VolumeCheck::Foreign
    }
}

fn expected_name(labels: &VolumeLabels) -> String {
    format!("cn-{}-{}", labels.instance, labels.slot)
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use composenest_application::{
        named_volumes::{NamedVolumeError, NamedVolumePort, named_volume_allocations},
        operation_journal::{
            ExpectedResult, OperationIntent, OperationJournal, OperationKind, RequestReceipt,
            StepCommand, StepIntent, StepOutcome,
        },
        state_store::{
            InstanceRecord, RuntimeTarget, StateStore, StorageMethod, TemplateFile,
            TemplateRevision,
        },
    };
    use composenest_domain::{
        identity::{InstanceId, SlotId},
        instance::StoragePresence,
    };
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use crate::{
        docker_target::DockerProbe, named_volumes::DockerNamedVolumes, sqlite::DatabaseWorker,
    };

    const INSTANCE: &str = "0000000000000000000000000000002a";
    const OPERATION: &str = "op-42";
    const VOLUME: &str = "cn-0000000000000000000000000000002a-data";
    const VOLUME_JSON: &str = r#"{"Name":"cn-0000000000000000000000000000002a-data","Driver":"local","Labels":{"io.composenest.scope":"scope","io.composenest.instance":"0000000000000000000000000000002a","io.composenest.slot":"data","io.composenest.allocation-operation":"op-42"}}"#;
    const EMPTY_VOLUME_JSON: &str =
        r#"{"Name":"cn-0000000000000000000000000000002a-data","Driver":"local","Labels":null}"#;

    struct Fixture {
        root: TempDir,
        worker: DatabaseWorker,
        storage: DockerNamedVolumes,
        instance_id: InstanceId,
        slot: SlotId,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        for path in ["state", "locks"] {
            fs::create_dir(root.path().join(path)).unwrap();
            fs::set_permissions(root.path().join(path), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let worker = DatabaseWorker::start(root.path()).unwrap();
        worker.create_scope("scope", "owner", "root").unwrap();
        worker
            .create_target(
                "target",
                "scope",
                "unix:///tmp/composenest-volume-test.sock",
                "engine-a",
                "linux/amd64",
            )
            .unwrap();

        let canonical_json = r#"{"normalization":"template-normalization-v1","manifest":{"schemaVersion":1,"id":"sample","templateVersion":"1"},"versions":[{"key":"1","definition":"complete"}]}"#;
        let semantic_hash = format!("{:x}", Sha256::digest(canonical_json.as_bytes()));
        let revision = TemplateRevision {
            id: format!("sample:1:{semantic_hash}"),
            template_id: "sample".into(),
            version: "1".into(),
            normalization: "template-normalization-v1".into(),
            semantic_hash,
            canonical_json: canonical_json.into(),
            origin: "local".into(),
            files: vec![
                TemplateFile {
                    relative_path: "template.yaml".into(),
                    contents: b"manifest".to_vec(),
                },
                TemplateFile {
                    relative_path: "versions/1.yaml".into(),
                    contents: b"version".to_vec(),
                },
            ],
        };
        worker.register_template(&revision).unwrap();
        let instance_id = InstanceId::from_u128(42);
        let slot = SlotId::parse("data").unwrap();
        let allocations =
            named_volume_allocations(instance_id, OPERATION, std::slice::from_ref(&slot)).unwrap();
        worker
            .commit_instance(&InstanceRecord {
                id: INSTANCE.into(),
                scope_id: "scope".into(),
                target_id: "target".into(),
                name: "Example".into(),
                project_name: "cn-0000000000000000000000000000002a".into(),
                clone_source_id: None,
                template_revision_id: revision.id,
                selected_version: "1".into(),
                storage_method: StorageMethod::Volume,
                inputs_json: "{}".into(),
                ports: vec![],
                storage: allocations,
            })
            .unwrap();
        worker
            .accept(
                &OperationIntent {
                    id: OPERATION.into(),
                    instance_id: INSTANCE.into(),
                    kind: OperationKind::Create,
                    phase: "prepare_storage".into(),
                    expected_revision: 1,
                    old_spec_revision: None,
                    new_spec_revision: None,
                },
                &RequestReceipt {
                    scope_id: "scope".into(),
                    request_id: "request-42".into(),
                    plan_id: None,
                    confirmed_revision: 1,
                    request_hash: "a".repeat(64),
                    instance_id: INSTANCE.into(),
                    operation_id: OPERATION.into(),
                },
            )
            .unwrap();

        let executable = root.path().join("docker-mock");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\n\
                 shift 2\n\
                 if [ \"$1\" = info ]; then printf '{{\"ID\":\"engine-a\"}}\\n'; exit 0; fi\n\
                 if [ \"$1\" != volume ]; then exit 9; fi\n\
                 if [ \"$2\" = inspect ]; then if [ -f volume.json ]; then /bin/cat volume.json; else exit 1; fi; exit 0; fi\n\
                 if [ \"$2\" = ls ]; then if [ -f volume.json ]; then printf '{VOLUME}\\n'; fi; exit 0; fi\n\
                 if [ \"$2\" = create ]; then printf '%s\\n' \"$@\" > create-args; printf x >> create-count; printf '%s\\n' '{VOLUME_JSON}' > volume.json; printf '{VOLUME}\\n'; exit 0; fi\n\
                 exit 9\n",
                VOLUME = VOLUME,
                VOLUME_JSON = VOLUME_JSON
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let docker = DockerProbe {
            executable,
            directory: root.path().into(),
            config_directory: root.path().into(),
        }
        .bind(RuntimeTarget {
            id: "target".into(),
            scope_id: "scope".into(),
            endpoint: "unix:///tmp/composenest-volume-test.sock".into(),
            engine_id: "engine-a".into(),
            platform: "linux/amd64".into(),
        })
        .unwrap();

        Fixture {
            root,
            worker,
            storage: DockerNamedVolumes::new(docker),
            instance_id,
            slot,
        }
    }

    #[tokio::test]
    async fn create_is_journaled_labeled_and_idempotently_verified() {
        let fixture = fixture();
        let first = fixture
            .storage
            .ensure_named_volume(
                &fixture.worker,
                &fixture.worker,
                fixture.instance_id,
                &fixture.slot,
                1,
                1,
            )
            .await
            .unwrap();
        assert_eq!(first.allocation.resource_identity, VOLUME);
        assert_eq!(first.presence, StoragePresence::Present);

        let second = fixture
            .storage
            .ensure_named_volume(
                &fixture.worker,
                &fixture.worker,
                fixture.instance_id,
                &fixture.slot,
                1,
                2,
            )
            .await
            .unwrap();
        assert_eq!(second.presence, StoragePresence::Present);
        assert_eq!(
            fs::read_to_string(fixture.root.path().join("create-count")).unwrap(),
            "x"
        );
        let steps = fixture
            .worker
            .steps_for_resource(OPERATION, VOLUME)
            .unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].instance_id, INSTANCE);
        assert_eq!(steps[0].scope_id, "scope");
        assert_eq!(steps[0].outcome, Some(StepOutcome::Succeeded));
        let args = fs::read_to_string(fixture.root.path().join("create-args")).unwrap();
        assert!(args.contains("io.composenest.scope=scope"));
        assert!(args.contains(&format!("io.composenest.instance={INSTANCE}")));
        assert!(args.contains("io.composenest.slot=data"));
        assert!(args.contains(&format!("io.composenest.allocation-operation={OPERATION}")));
    }

    #[tokio::test]
    async fn does_not_adopt_existing_volume_without_matching_journal_and_labels() {
        let fixture = fixture();
        fs::write(fixture.root.path().join("volume.json"), VOLUME_JSON).unwrap();

        let result = fixture
            .storage
            .ensure_named_volume(
                &fixture.worker,
                &fixture.worker,
                fixture.instance_id,
                &fixture.slot,
                1,
                1,
            )
            .await;
        assert_eq!(result, Err(NamedVolumeError::Unverified));
        assert_eq!(
            fixture
                .worker
                .storage_allocation(INSTANCE, "data")
                .unwrap()
                .unwrap()
                .presence,
            StoragePresence::Unverified
        );
        assert!(!fixture.root.path().join("create-count").exists());
    }

    #[tokio::test]
    async fn does_not_adopt_a_preexisting_empty_volume() {
        let fixture = fixture();
        fs::write(fixture.root.path().join("volume.json"), EMPTY_VOLUME_JSON).unwrap();

        assert_eq!(
            fixture
                .storage
                .ensure_named_volume(
                    &fixture.worker,
                    &fixture.worker,
                    fixture.instance_id,
                    &fixture.slot,
                    1,
                    1,
                )
                .await,
            Err(NamedVolumeError::Unverified)
        );
        assert!(!fixture.root.path().join("create-count").exists());
    }

    #[tokio::test]
    async fn never_recreates_a_volume_that_was_previously_present() {
        let fixture = fixture();
        fixture
            .storage
            .ensure_named_volume(
                &fixture.worker,
                &fixture.worker,
                fixture.instance_id,
                &fixture.slot,
                1,
                1,
            )
            .await
            .unwrap();
        fs::remove_file(fixture.root.path().join("volume.json")).unwrap();

        assert_eq!(
            fixture
                .storage
                .ensure_named_volume(
                    &fixture.worker,
                    &fixture.worker,
                    fixture.instance_id,
                    &fixture.slot,
                    1,
                    2,
                )
                .await,
            Err(NamedVolumeError::Missing)
        );
        assert_eq!(
            fixture
                .worker
                .storage_allocation(INSTANCE, "data")
                .unwrap()
                .unwrap()
                .presence,
            StoragePresence::Missing
        );
        assert_eq!(
            fs::read_to_string(fixture.root.path().join("create-count")).unwrap(),
            "x"
        );
    }

    #[tokio::test]
    async fn marks_volume_missing_if_creation_succeeded_before_presence_was_saved() {
        let fixture = fixture();
        let step = StepIntent {
            operation_id: OPERATION.into(),
            sequence: 1,
            attempt: 1,
            command_kind: StepCommand::CreateVolume,
            resource_id: VOLUME.into(),
            expected_result: ExpectedResult::VolumeCreated,
        };
        fixture.worker.record_step(&step).unwrap();
        fixture
            .worker
            .finish_step(OPERATION, 1, StepOutcome::Succeeded)
            .unwrap();

        assert_eq!(
            fixture
                .storage
                .ensure_named_volume(
                    &fixture.worker,
                    &fixture.worker,
                    fixture.instance_id,
                    &fixture.slot,
                    1,
                    2,
                )
                .await,
            Err(NamedVolumeError::Missing)
        );
        assert_eq!(
            fixture
                .worker
                .storage_allocation(INSTANCE, "data")
                .unwrap()
                .unwrap()
                .presence,
            StoragePresence::Missing
        );
        assert!(!fixture.root.path().join("create-count").exists());
    }
}
