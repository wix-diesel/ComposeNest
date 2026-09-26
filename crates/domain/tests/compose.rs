use std::collections::BTreeMap;

use composenest_domain::{
    compose::{ComposeError, ConfirmedCompose, InputValue, Storage, generate, to_yaml},
    identity::InstanceId,
    instance::StoragePresence,
    template::{ResolvedTemplate, parse_manifest, parse_version, resolve_template},
};
use yaml_rust2::YamlLoader;

fn snapshot() -> ResolvedTemplate {
    let manifest = parse_manifest(
        "postgres",
        include_bytes!("../../../docs/template-examples/postgresql/template.yaml"),
    )
    .unwrap();
    let definitions = ["17", "18"].map(|key| {
        let file = format!("versions/{key}.yaml");
        let bytes: &[u8] = if key == "17" {
            include_bytes!("../../../docs/template-examples/postgresql/versions/17.yaml")
        } else {
            include_bytes!("../../../docs/template-examples/postgresql/versions/18.yaml")
        };
        (
            key.to_owned(),
            parse_version("postgres", key, &file, bytes).unwrap(),
        )
    });
    resolve_template(manifest, &definitions).unwrap()
}

fn inputs() -> BTreeMap<String, InputValue> {
    BTreeMap::from([
        ("database".into(), InputValue::String("日本語 db".into())),
        ("username".into(), InputValue::String("admin".into())),
        ("password".into(), InputValue::String("$' \"日本語".into())),
    ])
}

#[test]
fn each_snapshot_version_uses_its_own_storage_target_and_preserves_values() {
    let snapshot = snapshot();
    let inputs = inputs();
    let ports = BTreeMap::from([("database".into(), 15432)]);
    let storage = BTreeMap::from([("data".into(), Storage::Bind("/tmp/cn-data".into()))]);
    for (version, target) in [
        ("17", "/var/lib/postgresql/data"),
        ("18", "/var/lib/postgresql"),
    ] {
        let source = ConfirmedCompose {
            snapshot: &snapshot,
            version,
            instance_id: InstanceId::from_u128(42),
            scope_id: "scope-a",
            spec_revision: 7,
            inputs: &inputs,
            ports: &ports,
            storage: &storage,
            source_image: if version == "17" {
                "postgres:17"
            } else {
                "postgres:18"
            },
            execution_image: &format!("postgres@sha256:{}", "a".repeat(64)),
            platform: "linux/amd64",
        };
        let model = generate(&source).unwrap();
        assert_eq!(model.service.mounts[0].1, target);
        assert_eq!(
            model.service.environment["POSTGRES_PASSWORD"],
            "$' \"日本語"
        );
        assert_eq!(
            model.service.healthcheck,
            ["pg_isready", "-U", "admin", "-d", "日本語 db"]
        );
        let yaml = to_yaml(&model).unwrap();
        let doc = &YamlLoader::load_from_str(&yaml).unwrap()[0];
        let main = &doc["services"]["main"];
        assert_eq!(
            doc["name"].as_str(),
            Some("cn-0000000000000000000000000000002a")
        );
        assert_eq!(
            main["environment"]["POSTGRES_PASSWORD"].as_str(),
            Some("$$' \"日本語")
        );
        assert_eq!(main["ports"][0]["host_ip"].as_str(), Some("127.0.0.1"));
        assert_eq!(main["ports"][0]["protocol"].as_str(), Some("tcp"));
        assert_eq!(
            main["volumes"][0]["bind"]["create_host_path"].as_bool(),
            Some(false)
        );
        assert_eq!(main["healthcheck"]["test"][0].as_str(), Some("CMD"));
        assert_eq!(main["volumes"][0]["target"].as_str(), Some(target));
        assert!(main["container_name"].is_badvalue());
    }
}

#[test]
fn rejects_stale_slots_inputs_and_unverified_image_or_storage() {
    let snapshot = snapshot();
    let mut inputs = inputs();
    let mut ports = BTreeMap::from([("database".into(), 15432)]);
    let mut storage = BTreeMap::from([(
        "data".into(),
        Storage::Volume {
            name: "cn-0000000000000000000000000000002a-data".into(),
            presence: StoragePresence::Present,
        },
    )]);
    let make =
        |inputs: &BTreeMap<_, _>, ports: &BTreeMap<_, _>, storage: &BTreeMap<_, _>, image: &str| {
            let source = ConfirmedCompose {
                snapshot: &snapshot,
                version: "18",
                instance_id: InstanceId::from_u128(42),
                scope_id: "scope-a",
                spec_revision: 7,
                inputs,
                ports,
                storage,
                source_image: "postgres:18",
                execution_image: image,
                platform: "linux/amd64",
            };
            generate(&source)
        };
    let digest = format!("postgres@sha256:{}", "a".repeat(64));
    let model = make(&inputs, &ports, &storage, &digest).unwrap();
    let normalized = format!("docker.io/library/postgres@sha256:{}", "a".repeat(64));
    assert_eq!(
        make(&inputs, &ports, &storage, &normalized)
            .unwrap()
            .service
            .image,
        normalized
    );
    let wrong_repository = format!("docker.io/library/redis@sha256:{}", "a".repeat(64));
    assert_eq!(
        make(&inputs, &ports, &storage, &wrong_repository),
        Err(ComposeError::InvalidImage)
    );
    let yaml = to_yaml(&model).unwrap();
    let doc = &YamlLoader::load_from_str(&yaml).unwrap()[0];
    assert_eq!(
        doc["volumes"]["cn-0000000000000000000000000000002a-data"]["external"].as_bool(),
        Some(true)
    );
    inputs.insert("deleted-input".into(), InputValue::String("stale".into()));
    assert_eq!(
        make(&inputs, &ports, &storage, &digest),
        Err(ComposeError::InvalidSpec)
    );
    inputs.remove("deleted-input");
    ports.insert("deleted-port".into(), 15433);
    assert_eq!(
        make(&inputs, &ports, &storage, &digest),
        Err(ComposeError::InvalidAllocation)
    );
    ports.remove("deleted-port");
    storage.insert("deleted-slot".into(), Storage::Bind("/tmp/stale".into()));
    assert_eq!(
        make(&inputs, &ports, &storage, &digest),
        Err(ComposeError::InvalidAllocation)
    );
    storage.remove("deleted-slot");
    assert_eq!(
        make(&inputs, &ports, &storage, "postgres:18"),
        Err(ComposeError::InvalidImage)
    );
    storage.insert(
        "data".into(),
        Storage::Volume {
            name: "cn-0000000000000000000000000000002a-data".into(),
            presence: StoragePresence::Missing,
        },
    );
    assert_eq!(
        make(&inputs, &ports, &storage, &digest),
        Err(ComposeError::InvalidAllocation)
    );
    storage.insert("data".into(), Storage::Bind("../untrusted".into()));
    assert_eq!(
        make(&inputs, &ports, &storage, &digest),
        Err(ComposeError::InvalidAllocation)
    );
}

#[test]
fn redis_secret_is_preserved_in_environment_and_exec_argv() {
    let manifest = parse_manifest(
        "redis",
        include_bytes!("../../../docs/template-examples/redis/template.yaml"),
    )
    .unwrap();
    let definition = parse_version(
        "redis",
        "8.2",
        "versions/8.2.yaml",
        include_bytes!("../../../docs/template-examples/redis/versions/8.2.yaml"),
    )
    .unwrap();
    let snapshot = resolve_template(manifest, &[("8.2".into(), definition)]).unwrap();
    let secret = "$' \"日本語";
    let inputs = BTreeMap::from([("password".into(), InputValue::String(secret.into()))]);
    let ports = BTreeMap::from([("redis".into(), 16379)]);
    let storage = BTreeMap::from([("data".into(), Storage::Bind("/tmp/redis-data".into()))]);
    let digest = format!("redis@sha256:{}", "a".repeat(64));
    let model = generate(&ConfirmedCompose {
        snapshot: &snapshot,
        version: "8.2",
        instance_id: InstanceId::from_u128(42),
        scope_id: "scope-a",
        spec_revision: 1,
        inputs: &inputs,
        ports: &ports,
        storage: &storage,
        source_image: "redis:8.2",
        execution_image: &digest,
        platform: "linux/amd64",
    })
    .unwrap();
    assert_eq!(
        model.service.command.as_ref().unwrap().last().unwrap(),
        secret
    );
    let yaml = to_yaml(&model).unwrap();
    let doc = &YamlLoader::load_from_str(&yaml).unwrap()[0]["services"]["main"];
    assert_eq!(
        doc["environment"]["REDISCLI_AUTH"].as_str(),
        Some("$$' \"日本語")
    );
    assert_eq!(doc["command"][4].as_str(), Some("$$' \"日本語"));
    assert_eq!(doc["healthcheck"]["test"][0].as_str(), Some("CMD"));
}

#[test]
fn removed_version_fields_are_never_inherited() {
    let manifest = parse_manifest(
        "postgres",
        include_bytes!("../../../docs/template-examples/postgresql/template.yaml"),
    )
    .unwrap();
    let older = include_str!("../../../docs/template-examples/postgresql/versions/17.yaml")
        .replace(
            "  username:\n",
            "  legacy:\n    label: Legacy\n    type: string\n  username:\n",
        )
        .replace(
            "    POSTGRES_USER:",
            "    OLD_ENV: { input: legacy }\n    POSTGRES_USER:",
        )
        .replace(
            "  storage:\n",
            "  storage:\n    old:\n      label: Old\n      container: /var/lib/old\n",
        );
    let definitions = [
        (
            "17".into(),
            parse_version("postgres", "17", "versions/17.yaml", older.as_bytes()).unwrap(),
        ),
        (
            "18".into(),
            parse_version(
                "postgres",
                "18",
                "versions/18.yaml",
                include_bytes!("../../../docs/template-examples/postgresql/versions/18.yaml"),
            )
            .unwrap(),
        ),
    ];
    let snapshot = resolve_template(manifest, &definitions).unwrap();
    let inputs = inputs();
    let ports = BTreeMap::from([("database".into(), 15432)]);
    let storage = BTreeMap::from([("data".into(), Storage::Bind("/tmp/cn-data".into()))]);
    let digest = format!("postgres@sha256:{}", "a".repeat(64));
    let model = generate(&ConfirmedCompose {
        snapshot: &snapshot,
        version: "18",
        instance_id: InstanceId::from_u128(42),
        scope_id: "scope-a",
        spec_revision: 1,
        inputs: &inputs,
        ports: &ports,
        storage: &storage,
        source_image: "postgres:18",
        execution_image: &digest,
        platform: "linux/amd64",
    })
    .unwrap();
    assert!(!model.service.environment.contains_key("OLD_ENV"));
    assert_eq!(model.service.mounts.len(), 1);
}

#[test]
fn omitted_healthcheck_timing_uses_snapshot_rule() {
    let manifest = parse_manifest(
        "redis",
        include_bytes!("../../../docs/template-examples/redis/template.yaml"),
    )
    .unwrap();
    let definition = include_str!("../../../docs/template-examples/redis/versions/8.2.yaml")
        .replace("    startPeriodSeconds: 10\n", "");
    let version =
        parse_version("redis", "8.2", "versions/8.2.yaml", definition.as_bytes()).unwrap();
    let snapshot = resolve_template(manifest, &[("8.2".into(), version)]).unwrap();
    let inputs = BTreeMap::from([("password".into(), InputValue::String("secret".into()))]);
    let ports = BTreeMap::from([("redis".into(), 16379)]);
    let storage = BTreeMap::from([("data".into(), Storage::Bind("/tmp/redis-data".into()))]);
    let digest = format!("redis@sha256:{}", "a".repeat(64));
    let model = generate(&ConfirmedCompose {
        snapshot: &snapshot,
        version: "8.2",
        instance_id: InstanceId::from_u128(42),
        scope_id: "scope-a",
        spec_revision: 1,
        inputs: &inputs,
        ports: &ports,
        storage: &storage,
        source_image: "redis:8.2",
        execution_image: &digest,
        platform: "linux/amd64",
    })
    .unwrap();
    assert_eq!(model.service.health_timing, [5, 3, 10, 12]);
}

// Run explicitly on a Docker host: cargo test -p composenest-domain --test compose -- --ignored
#[test]
#[ignore = "requires Docker Compose, a local Engine, and permission to pull redis:8.2"]
fn docker_config_and_container_preserve_actual_values() {
    use std::{
        fs,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };
    fn docker(args: &[&str]) -> String {
        let output = Command::new("docker").args(args).output().unwrap();
        assert!(output.status.success(), "Docker contract step failed");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
    docker(&["pull", "redis:8.2"]);
    let digest = docker(&[
        "image",
        "inspect",
        "redis:8.2",
        "--format",
        "{{index .RepoDigests 0}}",
    ]);
    let snapshot = {
        let manifest = parse_manifest(
            "redis",
            include_bytes!("../../../docs/template-examples/redis/template.yaml"),
        )
        .unwrap();
        let version = parse_version(
            "redis",
            "8.2",
            "versions/8.2.yaml",
            include_bytes!("../../../docs/template-examples/redis/versions/8.2.yaml"),
        )
        .unwrap();
        resolve_template(manifest, &[("8.2".into(), version)]).unwrap()
    };
    let secret = "$' \"日本語";
    let inputs = BTreeMap::from([("password".into(), InputValue::String(secret.into()))]);
    let ports = BTreeMap::from([("redis".into(), 16379)]);
    let directory = std::env::temp_dir().join(format!(
        "cn-compose-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&directory).unwrap();
    let data = directory.join("data");
    fs::create_dir(&data).unwrap();
    let storage = BTreeMap::from([("data".into(), Storage::Bind(data.to_str().unwrap().into()))]);
    let model = generate(&ConfirmedCompose {
        snapshot: &snapshot,
        version: "8.2",
        instance_id: InstanceId::from_u128(42),
        scope_id: "scope-a",
        spec_revision: 1,
        inputs: &inputs,
        ports: &ports,
        storage: &storage,
        source_image: "redis:8.2",
        execution_image: &digest,
        platform: "linux/amd64",
    })
    .unwrap();
    let file = directory.join("compose.yaml");
    fs::write(&file, to_yaml(&model).unwrap()).unwrap();
    let file = file.to_str().unwrap();
    let config = docker(&["compose", "-f", file, "config", "--format", "json"]);
    let config: serde_json::Value = serde_json::from_str(&config).unwrap();
    let service = &config["services"]["main"];
    let escaped = secret.replace('$', "$$");
    assert_eq!(service["environment"]["REDISCLI_AUTH"], escaped);
    assert_eq!(service["command"][4], escaped);
    struct Cleanup<'a>(&'a str, &'a std::path::Path);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = Command::new("docker")
                .args(["compose", "-f", self.0, "down", "-v"])
                .status();
            let _ = fs::remove_dir_all(self.1);
        }
    }
    let _cleanup = Cleanup(file, &directory);
    docker(&[
        "compose", "-f", file, "up", "-d", "--pull", "never", "--wait",
    ]);
    let id = docker(&["compose", "-f", file, "ps", "-q", "main"]);
    let inspect = docker(&["inspect", &id]);
    let inspect: serde_json::Value = serde_json::from_str(&inspect).unwrap();
    let container = &inspect[0]["Config"];
    assert_eq!(container["Cmd"][4], secret);
    let expected_env = format!("REDISCLI_AUTH={secret}");
    assert!(
        container["Env"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry.as_str() == Some(expected_env.as_str()))
    );
}
