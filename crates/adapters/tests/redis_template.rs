mod support;

use composenest_adapters::{
    template_catalog_view::{catalog_view, reload_results},
    template_package::read_packages,
};
use composenest_application::{
    RequestContext,
    instance_content::InstanceSecretRequest,
    query_service::QueryService,
    state_store::{InstanceRecord, PortAllocation, StateStore, StorageMethod},
    template_catalog::{TemplateOrigin, prepare_revision},
};
use composenest_domain::{
    compose::{ConfirmedCompose, InputValue, Storage, generate, to_yaml},
    identity::InstanceId,
    instance::StoragePresence,
    template::{
        ResolvedTemplate, ResolvedVersion, parse_manifest, parse_version, resolve_template,
    },
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, net::TcpListener, path::PathBuf, process::Command};

fn package_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/template-candidates")
}

fn snapshot() -> ResolvedTemplate {
    snapshot_at(&package_root(), TemplateOrigin::Local)
}

fn snapshot_at(root: &std::path::Path, origin: TemplateOrigin) -> ResolvedTemplate {
    let package = read_packages(root, origin)
        .unwrap()
        .into_iter()
        .map(Result::unwrap)
        .find(|package| package.name == "redis")
        .unwrap();
    let revision = prepare_revision(package.clone()).unwrap();
    assert_eq!(revision.files.len(), 2);
    let manifest = parse_manifest("redis", &package.files[0].contents).unwrap();
    let definitions = manifest
        .versions
        .iter()
        .map(|(key, path)| {
            let file = package
                .files
                .iter()
                .find(|file| &file.relative_path == path)
                .unwrap();
            (
                key.clone(),
                parse_version("redis", key, path, &file.contents).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let snapshot = resolve_template(manifest, &definitions).unwrap();
    assert_eq!(snapshot.manifest.default_version, "8.2");
    assert_eq!(
        snapshot
            .versions
            .iter()
            .map(|version| version.key.as_str())
            .collect::<Vec<_>>(),
        ["8.2"]
    );
    assert!(
        snapshot
            .versions
            .iter()
            .all(|version| version.warnings.is_empty())
    );
    snapshot
}

#[test]
fn redis_package_contains_one_complete_version() {
    let candidate = snapshot();
    let example_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/template-examples");
    let example = snapshot_at(&example_root, TemplateOrigin::Local);
    assert_eq!(
        candidate.versions, example.versions,
        "Acceptance definitions drifted from the specification examples"
    );
    assert_eq!(candidate.manifest.id, "composenest.redis");
}

#[test]
fn unaccepted_redis_candidate_is_not_shipped_or_registered_as_bundled() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let candidate = package_root().join("redis").canonicalize().unwrap();
    let desktop = repository.join("apps/desktop/src-tauri");
    let config: Value =
        serde_json::from_slice(&fs::read(desktop.join("tauri.conf.json")).unwrap()).unwrap();
    for source in config["bundle"]["resources"].as_object().unwrap().keys() {
        let shipped = desktop.join(source).canonicalize().unwrap();
        assert!(
            !candidate.starts_with(shipped),
            "Unaccepted candidate is included in Tauri resources"
        );
    }
    let (root, store, _) = support::store();
    let local = root.path().join("local");
    fs::create_dir(&local).unwrap();
    let bundled = repository.join("resources/templates");
    let results = reload_results(&store, &bundled, &local).unwrap();
    let view = catalog_view(&store, &local, results).unwrap();
    assert!(
        !view
            .templates
            .iter()
            .any(|card| card.template_id == "composenest.redis")
    );

    // A deliberate local fixture import remains local, regardless of the author's ID.
    let package = local.join("redis");
    fs::create_dir_all(package.join("versions")).unwrap();
    for path in ["template.yaml", "versions/8.2.yaml"] {
        fs::copy(candidate.join(path), package.join(path)).unwrap();
    }
    let results = reload_results(&store, &bundled, &local).unwrap();
    let view = catalog_view(&store, &local, results).unwrap();
    let card = view
        .templates
        .iter()
        .find(|card| card.template_id == "composenest.redis")
        .unwrap();
    assert!(card.loaded);
    assert_eq!(card.origin, "local");
    assert_eq!(card.versions, ["8.2"]);
    assert_eq!(card.storage_methods.len(), 2);
}

const PASSWORD: &str = "$' \"\\日本語 acceptance";
const DATA: &str = "persistent_日本語";

fn connection_password(port: u16, bind: bool) -> String {
    let (_root, store, _) = support::store();
    let package = read_packages(&package_root(), TemplateOrigin::Local)
        .unwrap()
        .into_iter()
        .map(Result::unwrap)
        .find(|package| package.name == "redis")
        .unwrap();
    let revision = prepare_revision(package).unwrap();
    store.register_template(&revision).unwrap();
    store
        .create_target(
            "target",
            "scope",
            "unix:///var/run/docker.sock",
            "engine",
            "linux/amd64",
        )
        .unwrap();
    let mut instance = support::source_instance(&revision.id);
    instance.selected_version = "8.2".into();
    instance.storage_method = if bind {
        StorageMethod::Bind
    } else {
        StorageMethod::Volume
    };
    instance.inputs_json = serde_json::to_string(&json!({"password": PASSWORD})).unwrap();
    instance.ports[0].slot = "redis".into();
    instance.ports[0].host_port = port;
    instance.ports[0].container_port = 6379;
    store.commit_instance(&instance).unwrap();
    let view = QueryService::new(&store)
        .get_instance("scope", &instance.id)
        .unwrap()
        .unwrap();
    assert_eq!(view.connections.len(), 1);
    assert_eq!(view.connections[0].port.host_ip, "127.0.0.1");
    assert_eq!(view.connections[0].port.host_port, port);
    assert_eq!(view.connections[0].input_slots, ["password"]);
    assert!(view.inputs[0].secret && view.inputs[0].value.is_none());
    assert!(!serde_json::to_string(&view).unwrap().contains(PASSWORD));
    let revealed = composenest_adapters::instance_content::secret(
        &store,
        "scope",
        &InstanceSecretRequest {
            context: RequestContext {
                api_version: 1,
                request_id: "redis-acceptance".into(),
            },
            instance_id: instance.id,
            expected_spec_revision: 1,
            slot: "password".into(),
        },
    )
    .unwrap();
    assert!(
        revealed.value == PASSWORD,
        "Connection secret differs from the committed input"
    );
    revealed.value
}

#[test]
fn redis_connection_uses_the_committed_secret_and_masks_normal_views() {
    for bind in [true, false] {
        connection_password(16379, bind);
    }
}

fn docker(args: &[&str]) -> String {
    let output = Command::new("docker").args(args).output().unwrap();
    // Inspect/config and Redis errors can contain secrets; do not include output or argv.
    assert!(
        output.status.success(),
        "Redis Docker acceptance step failed: {}",
        args[0]
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

struct Fixture {
    record: InstanceRecord,
    root: tempfile::TempDir,
    volume: String,
    image: String,
    bind: bool,
    port: u16,
    password: String,
}

impl Fixture {
    fn compose(&self, args: &[&str]) -> String {
        let file = self.root.path().join("compose.yaml");
        let mut command = vec!["compose", "-f", file.to_str().unwrap()];
        command.extend_from_slice(args);
        docker(&command)
    }

    fn container(&self) -> String {
        self.compose(&["ps", "-q", "main"])
    }

    fn up(&self, recreate: bool) {
        let mut args = vec![
            "up",
            "-d",
            "--pull",
            "never",
            "--wait",
            "--wait-timeout",
            "180",
        ];
        if recreate {
            args.push("--force-recreate");
        }
        self.compose(&args);
    }

    fn query(&self, args: &[&str]) -> String {
        let output = redis_cli(self.port, Some(&self.password), args);
        assert!(
            output.status.success(),
            "Authenticated Redis TCP query failed"
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .trim_end()
            .to_owned()
    }

    fn check_data(&self) {
        assert_eq!(self.query(&["GET", "acceptance"]), DATA);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let file = self.root.path().join("compose.yaml");
        if file.exists() {
            let _ = Command::new("docker")
                .args([
                    "compose",
                    "-f",
                    file.to_str().unwrap(),
                    "down",
                    "--timeout",
                    "30",
                ])
                .output();
        }
        if !self.bind {
            let _ = Command::new("docker")
                .args(["volume", "rm", &self.volume])
                .output();
        }
        // Restore only this test directory's ownership after the image initializes its bind mount.
        #[cfg(unix)]
        if self.bind {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::metadata(self.root.path()).unwrap();
            let owner = format!("{}:{}", metadata.uid(), metadata.gid());
            let mount = format!(
                "type=bind,source={},target=/fixture",
                self.root.path().join("data").display()
            );
            let _ = Command::new("docker")
                .args([
                    "run",
                    "--rm",
                    "--pull",
                    "never",
                    "--user",
                    "0",
                    "--mount",
                    &mount,
                    "--entrypoint",
                    "chown",
                    &self.image,
                    "-R",
                    &owner,
                    "/fixture",
                ])
                .output();
        }
    }
}

// Use host TCP and an environment credential; never pass secrets through a shell or CLI argv.
fn redis_cli(port: u16, password: Option<&str>, args: &[&str]) -> std::process::Output {
    let executable =
        std::env::var_os("COMPOSENEST_TEST_REDIS_CLI").unwrap_or_else(|| "redis-cli".into());
    let mut command = Command::new(executable);
    command
        .args(["-h", "127.0.0.1", "-p", &port.to_string(), "-e", "--raw"])
        .args(args)
        .env_remove("REDISCLI_AUTH")
        .env("LC_ALL", "C");
    if let Some(password) = password {
        command.env("REDISCLI_AUTH", password);
    }
    command.output().unwrap()
}

fn create_fixture(
    snapshot: &ResolvedTemplate,
    version: &ResolvedVersion,
    image: &str,
    platform: &str,
    bind: bool,
    source: Option<&Fixture>,
) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let id = InstanceId::from_u128(
        root.path()
            .to_string_lossy()
            .bytes()
            .fold(std::process::id() as u128, |value, byte| {
                value.wrapping_mul(31).wrapping_add(byte as u128)
            }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut id = id;
    let mut inputs = BTreeMap::from([(
        "password".into(),
        InputValue::String(connection_password(port, bind)),
    )]);
    let mut volume = format!("cn-{:032x}-data", id.as_u128());
    if let Some(source) = source {
        let confirmed = support::acceptance_clone::confirm_clone(
            "redis",
            &source.record,
            if bind {
                StorageMethod::Bind
            } else {
                StorageMethod::Volume
            },
            port,
            &root.path().join("data"),
        );
        id = InstanceId::from_u128(u128::from_str_radix(&confirmed.instance_id, 16).unwrap());
        volume = format!("cn-{:032x}-data", id.as_u128());
        let values: BTreeMap<String, String> =
            serde_json::from_str(&confirmed.inputs_json).unwrap();
        inputs = values
            .into_iter()
            .map(|(key, value)| (key, InputValue::String(value)))
            .collect();
        assert_eq!(confirmed.ports[0].host_port, port);
    }
    let password = match &inputs["password"] {
        InputValue::String(value) => value.clone(),
        _ => unreachable!(),
    };
    let storage_identity = if bind {
        root.path().join("data").to_str().unwrap().into()
    } else {
        volume.clone()
    };
    let record = support::acceptance_clone::source_record(
        id,
        &version.key,
        &inputs,
        PortAllocation {
            slot: "redis".into(),
            host_ip: "127.0.0.1".into(),
            host_port: port,
            container_port: 6379,
        },
        bind,
        storage_identity,
    );
    let fixture = Fixture {
        root,
        volume,
        image: image.into(),
        bind,
        record,
        port,
        password,
    };

    fs::create_dir(fixture.root.path().join("data")).unwrap();
    let storage = if bind {
        Storage::Bind(fixture.root.path().join("data").to_str().unwrap().into())
    } else {
        docker(&["volume", "create", &fixture.volume]);
        Storage::Volume {
            name: fixture.volume.clone(),
            presence: StoragePresence::Present,
        }
    };
    let ports = BTreeMap::from([("redis".into(), port)]);
    let storage = BTreeMap::from([("data".into(), storage)]);
    let model = generate(&ConfirmedCompose {
        snapshot,
        version: &version.key,
        instance_id: id,
        scope_id: "redis-acceptance",
        spec_revision: 1,
        inputs: &inputs,
        ports: &ports,
        storage: &storage,
        source_image: &version.definition.image,
        execution_image: image,
        platform,
    })
    .unwrap();
    fs::write(
        fixture.root.path().join("compose.yaml"),
        to_yaml(&model).unwrap(),
    )
    .unwrap();
    fixture.compose(&["config", "--quiet"]);
    drop(listener);
    fixture.up(false);
    fixture
}

fn check_container(fixture: &Fixture, platform: &str) {
    let id = fixture.container();
    let inspect: Value = serde_json::from_str(&docker(&["inspect", &id])).unwrap();
    let container = &inspect[0];
    assert_eq!(container["State"]["Health"]["Status"], "healthy");
    assert_eq!(container["Config"]["Image"], fixture.image);
    let image: Value =
        serde_json::from_str(&docker(&["image", "inspect", &fixture.image])).unwrap();
    assert_eq!(
        format!(
            "{}/{}",
            image[0]["Os"].as_str().unwrap(),
            image[0]["Architecture"].as_str().unwrap()
        ),
        platform
    );
    assert_eq!(container["Image"], image[0]["Id"]);
    let mounts = container["Mounts"].as_array().unwrap();
    assert_eq!(mounts.len(), 1, "Unexpected implicit or anonymous mount");
    assert_eq!(mounts[0]["Destination"], "/data");
    assert_eq!(mounts[0]["RW"], true);
    if fixture.bind {
        assert_eq!(mounts[0]["Type"], "bind");
        assert_eq!(
            mounts[0]["Source"],
            fixture.root.path().join("data").to_str().unwrap()
        );
    } else {
        assert_eq!(mounts[0]["Type"], "volume");
        assert_eq!(mounts[0]["Name"], fixture.volume);
    }
    assert_eq!(
        container["HostConfig"]["PortBindings"]["6379/tcp"],
        json!([{"HostIp": "127.0.0.1", "HostPort": fixture.port.to_string()}])
    );
    assert!(
        container["Config"]["Cmd"]
            == json!([
                "redis-server",
                "--appendonly",
                "yes",
                "--requirepass",
                fixture.password
            ]),
        "Command secret reference differs from the connection input"
    );
    assert!(
        container["Config"]["Env"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry.as_str() == Some(&format!("REDISCLI_AUTH={}", fixture.password))),
        "Environment secret reference differs from the connection input"
    );
    assert_eq!(
        container["Config"]["Healthcheck"]["Test"],
        json!(["CMD", "redis-cli", "-e", "ping"])
    );
    assert_eq!(docker(&["exec", &id, "redis-cli", "-e", "ping"]), "PONG");
    for auth in ["REDISCLI_AUTH=", "REDISCLI_AUTH=wrong-password"] {
        let output = Command::new("docker")
            .args(["exec", "--env", auth, &id, "redis-cli", "-e", "ping"])
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "Health command accepted invalid authentication"
        );
    }
    assert_eq!(fixture.query(&["PING"]), "PONG");
    for (password, expected) in [(None, "NOAUTH"), (Some("wrong-password"), "WRONGPASS")] {
        let output = redis_cli(fixture.port, password, &["PING"]);
        assert!(
            !output.status.success(),
            "TCP connection accepted invalid authentication"
        );
        let message = format!(
            "{}{}",
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap()
        );
        assert!(
            message.contains(expected),
            "TCP connection did not return the expected authentication error"
        );
    }
    assert!(
        fixture
            .query(&["INFO", "server"])
            .lines()
            .any(|line| line.starts_with("redis_version:8.2."))
    );
    assert!(
        fixture
            .query(&["INFO", "persistence"])
            .lines()
            .any(|line| line.trim_end() == "aof_enabled:1")
    );
    assert_eq!(
        docker(&[
            "exec",
            &id,
            "sh",
            "-c",
            "test -d /data/appendonlydir && test -s /data/appendonlydir/appendonly.aof.manifest"
        ]),
        ""
    );
    // Disable RDB snapshots so persistence after stop/recreate proves AOF replay.
    assert_eq!(fixture.query(&["CONFIG", "SET", "save", ""]), "OK");
    assert_eq!(
        docker(&["exec", &id, "sh", "-c", "test ! -e /data/dump.rdb"]),
        ""
    );
}

#[test]
#[ignore = "requires a local Docker Engine, Compose, and permission to pull redis:8.2"]
fn docker_redis_preserves_authenticated_aof_data_in_both_storage_modes() {
    let snapshot = snapshot();
    let info: Value = serde_json::from_str(&docker(&["info", "--format", "{{json .}}"])).unwrap();
    let arch = match info["Architecture"].as_str().unwrap() {
        "x86_64" | "amd64" => "amd64",
        "aarch64" | "arm64" => "arm64",
        _ => panic!("Unsupported test Engine architecture"),
    };
    let platform = format!("{}/{arch}", info["OSType"].as_str().unwrap());
    println!(
        "Host: {} {}; Engine: {} {}; Compose: {}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        info["ServerVersion"],
        platform,
        docker(&["compose", "version", "--short"])
    );
    let version = &snapshot.versions[0];
    assert!(version.definition.platforms.contains(&platform));
    docker(&["pull", "--platform", &platform, &version.definition.image]);
    let image = docker(&[
        "image",
        "inspect",
        &version.definition.image,
        "--format",
        "{{index .RepoDigests 0}}",
    ]);
    for bind in [true, false] {
        let fixture = create_fixture(&snapshot, version, &image, &platform, bind, None);
        check_container(&fixture, &platform);
        assert_eq!(fixture.query(&["SET", "acceptance", DATA]), "OK");
        fixture.check_data();
        let original = fixture.container();
        fixture.compose(&["stop", "--timeout", "30"]);
        let stopped: Value = serde_json::from_str(&docker(&["inspect", &original])).unwrap();
        assert_eq!(stopped[0]["State"]["Running"], false);
        fixture.up(false);
        assert_eq!(fixture.container(), original);
        check_container(&fixture, &platform);
        fixture.check_data();
        fixture.up(true);
        assert_ne!(fixture.container(), original);
        check_container(&fixture, &platform);
        fixture.check_data();
        for clone_bind in [true, false] {
            let clone = create_fixture(
                &snapshot,
                version,
                &image,
                &platform,
                clone_bind,
                Some(&fixture),
            );
            check_container(&clone, &platform);
            assert_eq!(clone.query(&["EXISTS", "acceptance"]), "0");
            assert_eq!(clone.query(&["SET", "acceptance", "clone_only"]), "OK");
            assert_eq!(clone.query(&["GET", "acceptance"]), "clone_only");
            fixture.check_data();
            assert_ne!(clone.record.project_name, fixture.record.project_name);
            println!(
                "PASS Redis 8.2 Clone {}->{}; source data absent and destination writes isolated",
                if bind { "bind" } else { "named" },
                if clone_bind { "bind" } else { "named" }
            );
        }
        println!(
            "PASS Redis 8.2 {} {platform}; digest={image}; connection secret, argv, environment, TCP authentication, health, AOF without RDB, mounts, stop/start and recreate",
            if bind { "bind" } else { "named" }
        );
    }
}
