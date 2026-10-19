//! Protected execution artifacts for named containers without Compose grouping labels.

use composenest_domain::compose::{ComposeModel, Storage};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) fn artifact_files(model: &ComposeModel, yaml: String) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::from([("compose.yaml".into(), yaml.into_bytes())]);
    if let Some(name) = &model.container_name {
        files.insert(
            "container-create.json".into(),
            json!({
                "name": name, "platform": model.service.platform, "config": configuration(model)
            })
            .to_string()
            .into_bytes(),
        );
    }
    files
}

fn configuration(model: &ComposeModel) -> Value {
    let service = &model.service;
    let mut labels = service.labels.clone();
    labels.insert("io.composenest.project".into(), model.name.clone());
    labels.insert("io.composenest.service".into(), "main".into());
    let ports: BTreeMap<_, _> = service
        .ports
        .iter()
        .map(|(host, container)| {
            (
                format!("{container}/tcp"),
                json!([{ "HostIp": "127.0.0.1", "HostPort": host.to_string() }]),
            )
        })
        .collect();
    let exposed: BTreeMap<_, _> = ports.keys().map(|port| (port.clone(), json!({}))).collect();
    let mounts: Vec<_> = service
        .mounts
        .iter()
        .map(|(storage, target)| {
            let (kind, source) = match storage {
                Storage::Bind(path) => ("bind", path),
                Storage::Volume { name, .. } => ("volume", name),
            };
            json!({ "Type": kind, "Source": source, "Target": target, "ReadOnly": false })
        })
        .collect();
    let timing = service.health_timing;
    let mut config = json!({
        "Image": service.image, "Labels": labels,
        "Env": service.environment.iter().map(|(key,value)| format!("{key}={value}")).collect::<Vec<_>>(),
        "ExposedPorts": exposed,
        "Healthcheck": { "Test": std::iter::once("CMD".to_owned()).chain(service.healthcheck.iter().cloned()).collect::<Vec<_>>(),
            "Interval": timing[0] * 1_000_000_000, "Timeout": timing[1] * 1_000_000_000,
            "StartPeriod": timing[2] * 1_000_000_000, "StartInterval": 5_000_000_000_i64, "Retries": timing[3] },
        "HostConfig": { "PortBindings": ports, "Mounts": mounts, "NetworkMode": format!("{}_default", model.name),
            "RestartPolicy": { "Name": service.restart }, "Memory": service.mem_limit.unwrap_or(0) },
        "NetworkingConfig": { "EndpointsConfig": { format!("{}_default", model.name): {} } }
    });
    if let Some(command) = &service.command {
        config["Cmd"] = json!(command);
    }
    config
}

#[cfg(test)]
#[path = "standalone_container_tests.rs"]
mod tests;
