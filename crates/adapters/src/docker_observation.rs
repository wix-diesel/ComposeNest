//! Bounded Docker inspection with ownership and configuration evidence.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    time::{SystemTime, UNIX_EPOCH},
};

use composenest_domain::instance::{ObservationFailure, RuntimeStatus};
use serde_json::Value;

use crate::docker_target::BoundDocker;

// Docker formats each selected field as JSON; health logs and unrelated inspect data stay out.
const INSPECT_FORMAT: &str = concat!(
    r#"{"Id":{{json .Id}},"Image":{{json .Image}},"Config":{"Labels":{"com.docker.compose.project":{{json (index .Config.Labels "com.docker.compose.project")}},"com.docker.compose.service":{{json (index .Config.Labels "com.docker.compose.service")}},"io.composenest.scope":{{json (index .Config.Labels "io.composenest.scope")}},"io.composenest.instance":{{json (index .Config.Labels "io.composenest.instance")}},"io.composenest.spec-revision":{{json (index .Config.Labels "io.composenest.spec-revision")}}},"Cmd":{{json .Config.Cmd}},"Env":{{json .Config.Env}},"Healthcheck":{{json .Config.Healthcheck}}},"HostConfig":{"PortBindings":{{json .HostConfig.PortBindings}}},"Mounts":{{json .Mounts}},"NetworkSettings":{"Networks":{{json .NetworkSettings.Networks}}},"State":{"Status":{{json .State.Status}},"Health":{"Status":{{if .State.Health}}{{json .State.Health.Status}}{{else}}null{{end}}}}}"#
);

/// Expected Docker state built from the confirmed spec and recorded allocations.
/// Environment and command values may contain secrets; keep this input private.
pub struct ExpectedContainer {
    /// Full recorded Docker container ID, never a mutable name or label.
    pub container_id: String,
    /// Stable Compose project name.
    pub project: String,
    /// Registered management scope.
    pub scope: String,
    /// Immutable instance identifier used in the application label.
    pub instance: String,
    /// Resolved image ID, not a mutable image tag.
    pub image_id: String,
    /// Every expected mount, including read-only mounts.
    pub mounts: Vec<ExpectedMount>,
    /// All published port bindings, keyed by container port and protocol.
    pub ports: BTreeMap<String, Vec<PortBinding>>,
    /// All expected Docker network names.
    pub networks: Vec<String>,
    /// Complete effective command, including any embedded secrets.
    pub command: Vec<String>,
    /// Complete effective environment, including image defaults and secrets.
    pub environment: Vec<String>,
    /// Effective Docker healthcheck configuration, or None when disabled.
    pub healthcheck: Option<ExpectedHealthcheck>,
    /// Confirmed spec revision for the application label.
    pub spec_revision: u64,
}

/// A mount as projected from Docker inspect (no host path is returned in observations).
#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub struct ExpectedMount {
    /// Docker mount type (bind or volume).
    pub kind: String,
    /// Recorded source path or volume name.
    pub source: String,
    /// Destination inside the container.
    pub destination: String,
    /// Whether Docker mounted the resource read-write.
    pub read_write: bool,
}

/// A published port binding in Docker's canonical host representation.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub struct PortBinding {
    /// Canonical HostIp returned by Docker (for example 0.0.0.0).
    pub host_ip: String,
    /// Decimal host port.
    pub host_port: String,
}

/// Effective healthcheck fields as reported by Docker, including defaults.
#[derive(PartialEq, Eq)]
pub struct ExpectedHealthcheck {
    /// Exec or shell test argv.
    pub test: Vec<String>,
    /// Interval in nanoseconds.
    pub interval: u64,
    /// Timeout in nanoseconds.
    pub timeout: u64,
    /// Number of retries.
    pub retries: u64,
    /// Grace period in nanoseconds.
    pub start_period: u64,
    /// Start interval in nanoseconds.
    pub start_interval: u64,
}

/// Ownership evidence; Unknown must prohibit every Docker change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
    /// The recorded ID and independent identity evidence agree.
    Verified,
    /// Docker returned a resource whose identity evidence conflicts.
    Foreign,
    /// Inspection did not establish ownership.
    Unknown,
}

/// Comparison result without environment, command, paths or healthcheck output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerObservation {
    /// Time at which the observation completed, in Unix seconds.
    pub observed_at_unix_seconds: u64,
    /// Current running and health status.
    pub status: RuntimeStatus,
    /// Ownership decision for all change operations.
    pub ownership: Ownership,
    /// Whether every expected configuration field agrees.
    pub configuration_matches: Option<bool>,
}

impl ContainerObservation {
    /// Returns true only while verified evidence is sufficiently recent.
    pub fn can_change(&self, now_unix_seconds: u64, max_age_seconds: u64) -> bool {
        self.ownership == Ownership::Verified
            && self.configuration_matches == Some(true)
            && !matches!(
                self.status,
                RuntimeStatus::Unknown(_) | RuntimeStatus::Absent
            )
            && now_unix_seconds.saturating_sub(self.observed_at_unix_seconds) <= max_age_seconds
            && now_unix_seconds >= self.observed_at_unix_seconds
    }

    /// Returns the observation age, or None for a future timestamp.
    pub fn age_seconds(&self, now_unix_seconds: u64) -> Option<u64> {
        now_unix_seconds.checked_sub(self.observed_at_unix_seconds)
    }
}

impl BoundDocker {
    /// Inspects a recorded container without exposing raw CLI output or secrets.
    pub async fn observe(&self, expected: &ExpectedContainer) -> ContainerObservation {
        let result = self.inspect_container(expected).await;
        let observed_at_unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        match result {
            Some((status, ownership, configuration_matches)) => ContainerObservation {
                observed_at_unix_seconds,
                status,
                ownership,
                configuration_matches,
            },
            None => ContainerObservation {
                observed_at_unix_seconds,
                status: RuntimeStatus::Unknown(ObservationFailure::Unreachable),
                ownership: Ownership::Unknown,
                configuration_matches: None,
            },
        }
    }

    async fn inspect_container(
        &self,
        expected: &ExpectedContainer,
    ) -> Option<(RuntimeStatus, Ownership, Option<bool>)> {
        if !valid_id(&expected.container_id) {
            return None;
        }
        let args = [
            "container".into(),
            "inspect".into(),
            "--format".into(),
            INSPECT_FORMAT.into(),
            OsString::from(&expected.container_id),
        ];
        let outcome = self.read_inspection(&args).await.ok()?;
        if outcome.outcome_unknown || outcome.stdout.truncated || outcome.stderr.truncated {
            return None;
        }
        if !outcome.status?.success() {
            // A failed inspect alone cannot distinguish a missing container from a disconnected Engine.
            let args = [
                "container".into(),
                "ls".into(),
                "--all".into(),
                "--no-trunc".into(),
                "--quiet".into(),
                "--filter".into(),
                OsString::from(format!("id={}", expected.container_id)),
            ];
            let listing = self.read(&args).await.ok()?;
            if listing.outcome_unknown
                || listing.stdout.truncated
                || listing.stderr.truncated
                || !listing.status?.success()
            {
                return None;
            }
            let listed = std::str::from_utf8(&listing.stdout.bytes).ok()?;
            if listed.trim().is_empty() {
                return Some((RuntimeStatus::Absent, Ownership::Unknown, None));
            }
            return None;
        }
        let json: Value = serde_json::from_slice(&outcome.stdout.bytes).ok()?;
        let ownership = ownership(&json, expected);
        let configuration_matches =
            (ownership == Ownership::Verified).then(|| configuration_matches(&json, expected));
        let status = runtime_status(&json);
        Some((status, ownership, configuration_matches))
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn field<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
}

fn string<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    field(value, path)?.as_str()
}

fn zero_if_omitted(value: &Value, key: &str) -> Option<u64> {
    match value.get(key) {
        None => Some(0),
        Some(number) => number.as_u64(),
    }
}

fn ownership(value: &Value, expected: &ExpectedContainer) -> Ownership {
    let label = |key| field(value, &["Config", "Labels", key]).and_then(Value::as_str);
    let evidence = [
        string(value, &["Id"]) == Some(expected.container_id.as_str()),
        label("com.docker.compose.project") == Some(expected.project.as_str()),
        label("com.docker.compose.service") == Some("main"),
        label("io.composenest.scope") == Some(expected.scope.as_str()),
        label("io.composenest.instance") == Some(expected.instance.as_str()),
    ];
    if evidence.iter().all(|matches| *matches) {
        Ownership::Verified
    } else {
        Ownership::Foreign
    }
}

fn configuration_matches(value: &Value, expected: &ExpectedContainer) -> bool {
    let mounts = field(value, &["Mounts"])
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .map(|item| {
                    Some(ExpectedMount {
                        kind: string(item, &["Type"])?.to_owned(),
                        source: match string(item, &["Type"])? {
                            "volume" => string(item, &["Name"])?.to_owned(),
                            "bind" => string(item, &["Source"])?.to_owned(),
                            _ => return None,
                        },
                        destination: string(item, &["Destination"])?.to_owned(),
                        read_write: field(item, &["RW"])?.as_bool()?,
                    })
                })
                .collect::<Option<Vec<_>>>()
        });
    let ports = field(value, &["HostConfig", "PortBindings"]).and_then(|value| {
        if value.is_null() {
            return Some(BTreeMap::new());
        }
        let entries = value.as_object()?;
        entries
            .iter()
            .map(|(key, bindings)| {
                Some((
                    key.clone(),
                    bindings
                        .as_array()?
                        .iter()
                        .map(|binding| {
                            Some(PortBinding {
                                host_ip: match string(binding, &["HostIp"])? {
                                    "" => "0.0.0.0".to_owned(),
                                    address => address.to_owned(),
                                },
                                host_port: string(binding, &["HostPort"])?.to_owned(),
                            })
                        })
                        .collect::<Option<Vec<_>>>()?,
                ))
            })
            .collect::<Option<BTreeMap<_, _>>>()
    });
    let networks = field(value, &["NetworkSettings", "Networks"])
        .and_then(Value::as_object)
        .map(|map| map.keys().cloned().collect::<Vec<_>>());
    let strings = |path: &[&str]| {
        let value = field(value, path)?;
        if value.is_null() {
            return Some(Vec::new());
        }
        value
            .as_array()?
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
    };
    let actual_health = field(value, &["Config", "Healthcheck"]);
    let health_matches = match (&expected.healthcheck, actual_health) {
        (None, None | Some(Value::Null)) => true,
        (Some(expected), Some(actual)) => {
            strings(&["Config", "Healthcheck", "Test"]) == Some(expected.test.clone())
                && zero_if_omitted(actual, "Interval") == Some(expected.interval)
                && zero_if_omitted(actual, "Timeout") == Some(expected.timeout)
                && zero_if_omitted(actual, "Retries") == Some(expected.retries)
                && zero_if_omitted(actual, "StartPeriod") == Some(expected.start_period)
                && zero_if_omitted(actual, "StartInterval") == Some(expected.start_interval)
        }
        _ => false,
    };
    let mut expected_mounts = expected.mounts.iter().collect::<Vec<_>>();
    expected_mounts.sort();
    let mut actual_mounts = mounts
        .as_ref()
        .map(|mounts| mounts.iter().collect::<Vec<_>>());
    if let Some(mounts) = &mut actual_mounts {
        mounts.sort();
    }
    let mut expected_networks = expected.networks.clone();
    expected_networks.sort();
    let mut expected_env = expected.environment.clone();
    expected_env.sort();
    let mut actual_env = strings(&["Config", "Env"]);
    if let Some(env) = &mut actual_env {
        env.sort();
    }
    string(value, &["Image"]) == Some(expected.image_id.as_str())
        && mounts.is_some()
        && actual_mounts == Some(expected_mounts)
        && ports.as_ref() == Some(&expected.ports)
        && networks == Some(expected_networks)
        && strings(&["Config", "Cmd"]) == Some(expected.command.clone())
        && actual_env == Some(expected_env)
        && health_matches
        && field(value, &["Config", "Labels", "io.composenest.spec-revision"])
            .and_then(Value::as_str)
            == Some(expected.spec_revision.to_string().as_str())
}

fn runtime_status(value: &Value) -> RuntimeStatus {
    match string(value, &["State", "Status"]) {
        Some("created" | "exited" | "dead") => RuntimeStatus::Stopped,
        Some("running") => match string(value, &["State", "Health", "Status"]) {
            Some("healthy") => RuntimeStatus::Ready,
            Some("unhealthy") => RuntimeStatus::Unhealthy,
            _ => RuntimeStatus::Preparing,
        },
        Some("restarting" | "paused") => RuntimeStatus::Preparing,
        _ => RuntimeStatus::Unknown(ObservationFailure::Inconclusive),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn expected() -> ExpectedContainer {
        ExpectedContainer {
            container_id: "a".repeat(64),
            project: "cn-project".into(),
            scope: "scope".into(),
            instance: "instance".into(),
            image_id: "sha256:image".into(),
            mounts: vec![ExpectedMount {
                kind: "volume".into(),
                source: "owned".into(),
                destination: "/data".into(),
                read_write: true,
            }],
            ports: BTreeMap::from([(
                "8080/tcp".into(),
                vec![PortBinding {
                    host_ip: "127.0.0.1".into(),
                    host_port: "18080".into(),
                }],
            )]),
            networks: vec!["cn-project_default".into()],
            command: vec!["run".into(), "secret".into()],
            environment: vec!["PASSWORD=secret".into(), "PATH=/usr/bin".into()],
            healthcheck: Some(ExpectedHealthcheck {
                test: vec!["CMD".into(), "check".into()],
                interval: 30000000000,
                timeout: 1000000000,
                retries: 3,
                start_period: 0,
                start_interval: 5000000000,
            }),
            spec_revision: 1,
        }
    }

    fn inspected(expected: &ExpectedContainer) -> Value {
        json!({
            "Id": expected.container_id, "Image": "sha256:image",
            "Config": {"Labels": {"com.docker.compose.project": "cn-project", "com.docker.compose.service": "main", "io.composenest.scope": "scope", "io.composenest.instance": "instance", "io.composenest.spec-revision": "1"},
                "Cmd": ["run", "secret"], "Env": ["PATH=/usr/bin", "PASSWORD=secret"],
                "Healthcheck": {"Test": ["CMD", "check"], "Interval": 30000000000_u64, "Timeout": 1000000000_u64, "Retries": 3, "StartPeriod": 0, "StartInterval": 5000000000_u64}},
            "HostConfig": {"PortBindings": {"8080/tcp": [{"HostIp": "127.0.0.1", "HostPort": "18080"}]}},
            "Mounts": [{"Type": "volume", "Name": "owned", "Source": "/var/lib/docker/volumes/owned/_data", "Destination": "/data", "RW": true}],
            "NetworkSettings": {"Networks": {"cn-project_default": {}}},
            "State": {"Status": "running", "Health": {"Status": "starting", "Log": [{"Output": "secret"}]}}
        })
    }

    #[test]
    fn matches_all_evidence_and_requires_healthy_state() {
        let expected = expected();
        let mut actual = inspected(&expected);
        assert_eq!(ownership(&actual, &expected), Ownership::Verified);
        assert!(configuration_matches(&actual, &expected));
        assert_eq!(runtime_status(&actual), RuntimeStatus::Preparing);
        actual["State"]["Health"]["Status"] = json!("healthy");
        assert_eq!(runtime_status(&actual), RuntimeStatus::Ready);
    }

    #[test]
    fn normalizes_omitted_health_zeros_and_unspecified_host_address() {
        let mut expected = expected();
        expected.ports.get_mut("8080/tcp").unwrap()[0].host_ip = "0.0.0.0".into();
        let mut actual = inspected(&expected);
        actual["Config"]["Healthcheck"]
            .as_object_mut()
            .unwrap()
            .remove("StartPeriod");
        actual["HostConfig"]["PortBindings"]["8080/tcp"][0]["HostIp"] = json!("");
        assert!(configuration_matches(&actual, &expected));
    }

    #[test]
    fn compares_bind_source_but_named_volume_name() {
        let mut expected = expected();
        let mut actual = inspected(&expected);
        assert!(configuration_matches(&actual, &expected));
        actual["Mounts"][0]["Name"] = json!("other");
        assert!(!configuration_matches(&actual, &expected));

        expected.mounts[0].kind = "bind".into();
        expected.mounts[0].source = "/private/data".into();
        actual["Mounts"][0] = json!({"Type": "bind", "Source": "/private/data", "Destination": "/data", "RW": true});
        assert!(configuration_matches(&actual, &expected));
        actual["Mounts"][0]["Source"] = json!("/private/other");
        assert!(!configuration_matches(&actual, &expected));
    }

    #[test]
    fn treats_null_command_and_environment_as_empty_lists() {
        let mut expected = expected();
        expected.command.clear();
        expected.environment.clear();
        let mut actual = inspected(&expected);
        actual["Config"]["Cmd"] = Value::Null;
        actual["Config"]["Env"] = Value::Null;
        assert!(configuration_matches(&actual, &expected));
        actual["Config"].as_object_mut().unwrap().remove("Env");
        assert!(!configuration_matches(&actual, &expected));
    }

    #[test]
    fn rejects_each_changed_resource_and_secret_without_returning_values() {
        let expected = expected();
        for (path, replacement) in [
            (vec!["Image"], json!("sha256:other")),
            (vec!["Mounts", "0", "Source"], json!("foreign")),
            (vec!["Config", "Cmd", "1"], json!("different")),
            (vec!["Config", "Env", "1"], json!("PASSWORD=different")),
            (vec!["Config", "Healthcheck", "Retries"], json!(5)),
            (
                vec!["HostConfig", "PortBindings", "8080/tcp", "0", "HostPort"],
                json!(18081),
            ),
            (
                vec!["Config", "Labels", "io.composenest.spec-revision"],
                json!("2"),
            ),
        ] {
            let mut actual = inspected(&expected);
            let mut current = &mut actual;
            for key in &path[..path.len() - 1] {
                current = if let Ok(index) = key.parse::<usize>() {
                    &mut current[index]
                } else {
                    &mut current[*key]
                };
            }
            let last = path[path.len() - 1];
            if let Ok(index) = last.parse::<usize>() {
                current[index] = replacement;
            } else {
                current[last] = replacement;
            }
            assert!(!configuration_matches(&actual, &expected), "field {path:?}");
        }
        let mut actual = inspected(&expected);
        actual["Mounts"].as_array_mut().unwrap().push(
            json!({"Type":"bind", "Source":"/tmp/unexpected", "Destination":"/extra", "RW":true}),
        );
        assert!(!configuration_matches(&actual, &expected));
    }

    #[test]
    fn labels_alone_do_not_prove_ownership_or_allow_changes() {
        let expected = expected();
        let mut actual = inspected(&expected);
        actual["Id"] = json!("b".repeat(64));
        assert_eq!(ownership(&actual, &expected), Ownership::Foreign);
        let observation = ContainerObservation {
            observed_at_unix_seconds: 100,
            status: RuntimeStatus::Ready,
            ownership: Ownership::Unknown,
            configuration_matches: Some(true),
        };
        assert!(!observation.can_change(101, 10));
        assert_eq!(observation.age_seconds(105), Some(5));
        assert_eq!(observation.age_seconds(99), None);
        let observation = ContainerObservation {
            ownership: Ownership::Verified,
            ..observation
        };
        assert!(observation.can_change(105, 10));
        assert!(!observation.can_change(111, 10));
    }
}
