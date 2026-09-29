//! Compose and Docker expectation projection from confirmed records.

use std::{collections::BTreeMap, ffi::OsString};

use composenest_application::{
    create_operation::CreateEffectError,
    create_state::ConfirmedCreate,
    image_resolution::ImageResolution,
    state_store::{StateStore, StorageMethod},
    storage::StoragePort,
};
use composenest_domain::{
    compose::{self, ConfirmedCompose, InputValue, Storage},
    identity::InstanceId,
    instance::StoragePresence,
    template::{Node, ResolvedTemplate, Value, parse_manifest, parse_version, resolve_template},
};
use serde_json::Value as JsonValue;

use crate::{
    docker_observation::{ExpectedContainer, ExpectedHealthcheck, ExpectedMount, PortBinding},
    docker_target::BoundDocker,
    sqlite::DatabaseWorker,
    storage::BindStorage,
};

/// Rebuilds the exact Compose and Docker expectations from immutable inputs.
pub struct CreateProjection<'a> {
    database: &'a DatabaseWorker,
    docker: &'a BoundDocker,
    binds: &'a BindStorage,
    project: &'a str,
    instance_id: &'a str,
}

impl<'a> CreateProjection<'a> {
    /// Uses the fixed adapters and confirmed project identity.
    pub fn new(
        database: &'a DatabaseWorker,
        docker: &'a BoundDocker,
        binds: &'a BindStorage,
        project: &'a str,
        instance_id: &'a str,
    ) -> Self {
        Self {
            database,
            docker,
            binds,
            project,
            instance_id,
        }
    }

    fn check_confirmed(
        &self,
        confirmed: &ConfirmedCreate,
    ) -> Result<InstanceId, CreateEffectError> {
        if confirmed.instance_id != self.instance_id || confirmed.project_name != self.project {
            return Err(CreateEffectError::Failed);
        }
        parse_instance_id(&confirmed.instance_id)
    }

    /// Parses the private immutable package copied at confirmation.
    pub fn snapshot(confirmed: &ConfirmedCreate) -> Result<ResolvedTemplate, CreateEffectError> {
        let manifest = confirmed
            .snapshot_files
            .iter()
            .find(|file| file.relative_path == "template.yaml")
            .ok_or(CreateEffectError::Failed)?;
        let manifest = parse_manifest("snapshot", &manifest.contents)
            .map_err(|_| CreateEffectError::Failed)?;
        let definitions = manifest
            .versions
            .iter()
            .map(|(key, path)| {
                let file = confirmed
                    .snapshot_files
                    .iter()
                    .find(|file| &file.relative_path == path)
                    .ok_or(CreateEffectError::Failed)?;
                Ok((
                    key.clone(),
                    parse_version(&manifest.id, key, path, &file.contents)
                        .map_err(|_| CreateEffectError::Failed)?,
                ))
            })
            .collect::<Result<Vec<_>, CreateEffectError>>()?;
        resolve_template(manifest, &definitions).map_err(|_| CreateEffectError::Failed)
    }

    /// Generates Compose solely from saved inputs, pinned Image and verified storage.
    pub fn model(
        &self,
        confirmed: &ConfirmedCreate,
        image: &ImageResolution,
    ) -> Result<compose::ComposeModel, CreateEffectError> {
        let snapshot = Self::snapshot(confirmed)?;
        let inputs: BTreeMap<String, InputValue> =
            serde_json::from_str::<BTreeMap<String, JsonValue>>(&confirmed.inputs_json)
                .map_err(|_| CreateEffectError::Failed)?
                .into_iter()
                .map(|(key, value)| {
                    let value = match value {
                        JsonValue::String(value) => InputValue::String(value),
                        JsonValue::Number(value) => {
                            InputValue::Integer(value.as_i64().ok_or(CreateEffectError::Failed)?)
                        }
                        JsonValue::Bool(value) => InputValue::Boolean(value),
                        _ => return Err(CreateEffectError::Failed),
                    };
                    Ok((key, value))
                })
                .collect::<Result<_, _>>()?;
        let ports = confirmed
            .ports
            .iter()
            .map(|port| (port.slot.clone(), port.host_port))
            .collect();
        let id = self.check_confirmed(confirmed)?;
        let mut storage = BTreeMap::new();
        for saved in &confirmed.storage {
            let current = self
                .database
                .storage_allocation(&confirmed.instance_id, &saved.slot)
                .map_err(|_| CreateEffectError::OutcomeUnknown)?
                .ok_or(CreateEffectError::Failed)?;
            if current.presence != StoragePresence::Present {
                return Err(CreateEffectError::Failed);
            }
            let value = match current.method {
                StorageMethod::Bind => Storage::Bind(
                    self.binds
                        .bind_path(id, &current.allocation)
                        .map_err(|_| CreateEffectError::Failed)?
                        .to_string_lossy()
                        .into_owned(),
                ),
                StorageMethod::Volume => Storage::Volume {
                    name: current.allocation.resource_identity,
                    presence: current.presence,
                },
            };
            storage.insert(saved.slot.clone(), value);
        }
        compose::generate(&ConfirmedCompose {
            snapshot: &snapshot,
            version: &confirmed.selected_version,
            instance_id: id,
            scope_id: &confirmed.scope_id,
            spec_revision: confirmed.spec_revision,
            inputs: &inputs,
            ports: &ports,
            storage: &storage,
            source_image: &image.requested,
            execution_image: &image.digest,
            platform: &image.platform,
        })
        .map_err(|_| CreateEffectError::Failed)
    }

    /// Builds complete Docker inspect expectations, including Image defaults.
    pub async fn expected(
        &self,
        model: &compose::ComposeModel,
        image: &ImageResolution,
        container_id: &str,
        confirmed: &ConfirmedCreate,
    ) -> Result<ExpectedContainer, CreateEffectError> {
        let (mut environment, image_command) = image_defaults(self.docker, &image.digest).await?;
        environment.extend(model.service.environment.clone());
        let mounts = model
            .service
            .mounts
            .iter()
            .map(|(storage, destination)| {
                let (kind, source) = match storage {
                    Storage::Bind(source) => ("bind", source.clone()),
                    Storage::Volume { name, .. } => ("volume", name.clone()),
                };
                ExpectedMount {
                    kind: kind.into(),
                    source,
                    destination: destination.clone(),
                    read_write: true,
                }
            })
            .collect();
        let ports = model
            .service
            .ports
            .iter()
            .map(|(host, container)| {
                (
                    format!("{container}/tcp"),
                    vec![PortBinding {
                        host_ip: "127.0.0.1".into(),
                        host_port: host.to_string(),
                    }],
                )
            })
            .collect();
        let timing = model.service.health_timing;
        let healthcheck = ExpectedHealthcheck {
            test: std::iter::once("CMD".into())
                .chain(model.service.healthcheck.iter().cloned())
                .collect(),
            interval: nanos(timing[0])?,
            timeout: nanos(timing[1])?,
            start_period: nanos(timing[2])?,
            retries: u64::try_from(timing[3]).map_err(|_| CreateEffectError::Failed)?,
            start_interval: 5_000_000_000,
        };
        Ok(ExpectedContainer {
            container_id: container_id.into(),
            project: self.project.into(),
            scope: confirmed.scope_id.clone(),
            instance: self.instance_id.into(),
            image_id: image.image_id.clone(),
            mounts,
            ports,
            networks: vec![format!("{}_default", self.project)],
            command: model.service.command.clone().unwrap_or(image_command),
            environment: environment
                .into_iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect(),
            healthcheck: Some(healthcheck),
            spec_revision: confirmed.spec_revision,
        })
    }
}

pub(crate) fn parse_instance_id(value: &str) -> Result<InstanceId, CreateEffectError> {
    if value.len() != 32 {
        return Err(CreateEffectError::Failed);
    }
    u128::from_str_radix(value, 16)
        .map(InstanceId::from_u128)
        .map_err(|_| CreateEffectError::Failed)
}

/// Returns every writable destination declared by the selected saved version.
pub fn storage_destinations(
    snapshot: &ResolvedTemplate,
    selected_version: &str,
) -> Result<Vec<String>, CreateEffectError> {
    let version = snapshot
        .versions
        .iter()
        .find(|version| version.key == selected_version)
        .ok_or(CreateEffectError::Failed)?;
    map_field(&version.definition.document, "service")
        .and_then(|service| map_field(service, "storage"))
        .and_then(|storage| match &storage.value {
            Value::Map(slots) => Some(slots),
            _ => None,
        })
        .ok_or(CreateEffectError::Failed)?
        .iter()
        .map(|(_, slot)| {
            map_field(slot, "container")
                .and_then(|node| match &node.value {
                    Value::String(value) => Some(value.clone()),
                    _ => None,
                })
                .ok_or(CreateEffectError::Failed)
        })
        .collect()
}

fn map_field<'a>(node: &'a Node, key: &str) -> Option<&'a Node> {
    match &node.value {
        Value::Map(fields) => fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value),
        _ => None,
    }
}

fn nanos(seconds: i64) -> Result<u64, CreateEffectError> {
    u64::try_from(seconds)
        .ok()
        .and_then(|value| value.checked_mul(1_000_000_000))
        .ok_or(CreateEffectError::Failed)
}

async fn image_defaults(
    docker: &BoundDocker,
    digest: &str,
) -> Result<(BTreeMap<String, String>, Vec<String>), CreateEffectError> {
    let args: [OsString; 5] = [
        "image".into(),
        "inspect".into(),
        "--format".into(),
        r#"{"Env":{{json .Config.Env}},"Cmd":{{json .Config.Cmd}}}"#.into(),
        digest.into(),
    ];
    let result = docker
        .read_inspection(&args)
        .await
        .map_err(|_| CreateEffectError::OutcomeUnknown)?;
    if result.outcome_unknown
        || result.stdout.truncated
        || result.stderr.truncated
        || !result.status.is_some_and(|status| status.success())
    {
        return Err(CreateEffectError::OutcomeUnknown);
    }
    let value: JsonValue = serde_json::from_slice(&result.stdout.bytes)
        .map_err(|_| CreateEffectError::OutcomeUnknown)?;
    let environment = value["Env"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let raw = item.as_str().ok_or(CreateEffectError::Failed)?;
                    let (name, value) = raw.split_once('=').ok_or(CreateEffectError::Failed)?;
                    Ok((name.to_owned(), value.to_owned()))
                })
                .collect::<Result<BTreeMap<_, _>, CreateEffectError>>()
        })
        .transpose()?
        .unwrap_or_default();
    let command = value["Cmd"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_owned)
                        .ok_or(CreateEffectError::Failed)
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok((environment, command))
}

#[cfg(test)]
mod tests {
    use super::*;
    use composenest_application::state_store::{RuntimeTarget, TemplateFile};

    #[test]
    fn snapshot_uses_saved_package_files_and_rejects_missing_versions() {
        let files = [
            (
                "template.yaml",
                include_bytes!("../../../docs/template-examples/postgresql/template.yaml")
                    .as_slice(),
            ),
            (
                "versions/17.yaml",
                include_bytes!("../../../docs/template-examples/postgresql/versions/17.yaml")
                    .as_slice(),
            ),
            (
                "versions/18.yaml",
                include_bytes!("../../../docs/template-examples/postgresql/versions/18.yaml")
                    .as_slice(),
            ),
        ];
        let mut confirmed = ConfirmedCreate {
            instance_id: "a".repeat(32),
            scope_id: "scope".into(),
            project_name: format!("cn-{}", "a".repeat(32)),
            target: RuntimeTarget {
                id: "target".into(),
                scope_id: "scope".into(),
                endpoint: "local".into(),
                engine_id: "engine".into(),
                platform: "linux/amd64".into(),
            },
            spec_revision: 1,
            selected_version: "17".into(),
            snapshot_files: files
                .into_iter()
                .map(|(path, bytes)| TemplateFile {
                    relative_path: path.into(),
                    contents: bytes.to_vec(),
                })
                .collect(),
            inputs_json: "{}".into(),
            ports: vec![],
            storage: vec![],
        };
        let snapshot = CreateProjection::snapshot(&confirmed).unwrap();
        assert_eq!(snapshot.versions.len(), 2);
        assert_eq!(snapshot.versions[0].definition.image, "postgres:18");
        assert_eq!(
            storage_destinations(&snapshot, "17").unwrap(),
            ["/var/lib/postgresql/data"]
        );
        confirmed.snapshot_files.pop();
        assert_eq!(
            CreateProjection::snapshot(&confirmed),
            Err(CreateEffectError::Failed)
        );
    }
}
