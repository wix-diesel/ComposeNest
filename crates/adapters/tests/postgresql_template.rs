mod support;

use composenest_adapters::{
    template_catalog_view::{catalog_view, reload_results},
    template_package::read_packages,
};
use composenest_application::template_catalog::{TemplateOrigin, prepare_revision};
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
        .find(|package| package.name == "postgresql")
        .unwrap();
    let revision = prepare_revision(package.clone()).unwrap();
    assert_eq!(revision.files.len(), 3);
    let manifest = parse_manifest("postgresql", &package.files[0].contents).unwrap();
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
                parse_version("postgresql", key, path, &file.contents).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let snapshot = resolve_template(manifest, &definitions).unwrap();
    assert_eq!(snapshot.manifest.default_version, "18");
    assert_eq!(
        snapshot
            .versions
            .iter()
            .map(|version| version.key.as_str())
            .collect::<Vec<_>>(),
        ["18", "17"]
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
fn postgresql_package_contains_all_independent_versions() {
    let candidate = snapshot();
    let example_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/template-examples");
    let example = snapshot_at(&example_root, TemplateOrigin::Local);
    assert_eq!(
        candidate.versions, example.versions,
        "Acceptance definitions drifted from the specification examples"
    );
    assert_eq!(candidate.manifest.id, "composenest.postgresql");
}

#[test]
fn unaccepted_postgresql_candidate_is_not_shipped_or_registered_as_bundled() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let candidate = package_root().join("postgresql").canonicalize().unwrap();
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
            .any(|card| card.template_id == "composenest.postgresql")
    );

    // A deliberate local fixture import remains local, regardless of the author's ID.
    let package = local.join("postgresql");
    fs::create_dir_all(package.join("versions")).unwrap();
    for path in ["template.yaml", "versions/17.yaml", "versions/18.yaml"] {
        fs::copy(candidate.join(path), package.join(path)).unwrap();
    }
    let results = reload_results(&store, &bundled, &local).unwrap();
    let view = catalog_view(&store, &local, results).unwrap();
    let card = view
        .templates
        .iter()
        .find(|card| card.template_id == "composenest.postgresql")
        .unwrap();
    assert!(card.loaded);
    assert_eq!(card.origin, "local");
    assert_eq!(card.versions, ["18", "17"]);
    assert_eq!(card.storage_methods.len(), 2);
}

fn docker(args: &[&str]) -> String {
    let output = Command::new("docker").args(args).output().unwrap();
    // Docker config and inspect can contain credentials; never print their output on failure.
    assert!(
        output.status.success(),
        "PostgreSQL Docker acceptance step failed: {}",
        args[0]
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

struct Fixture {
    root: tempfile::TempDir,
    volume: String,
    image: String,
    bind: bool,
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

    fn check_data(&self, port: u16, password: &str) {
        assert_eq!(
            query(
                self,
                port,
                password,
                "SELECT value FROM acceptance WHERE id = 1"
            ),
            "persistent_日本語"
        );
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
        // Only the unique fixture directory is re-owned for TempDir cleanup, never app data.
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

fn psql(fixture: &Fixture, port: u16, password: Option<&str>, query: &str) -> std::process::Output {
    let executable = std::env::var_os("COMPOSENEST_TEST_PSQL").unwrap_or_else(|| "psql".into());
    let mut command = Command::new(executable);
    command
        .args([
            "-X",
            "-w",
            "-q",
            "-t",
            "-A",
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            query,
        ])
        .env("PGHOST", "127.0.0.1")
        .env("PGPORT", port.to_string())
        .env("PGDATABASE", "acceptance_db")
        .env("PGUSER", "acceptance_user")
        .env("PGCONNECT_TIMEOUT", "5")
        .env("PGSSLMODE", "disable")
        .env("PGCLIENTENCODING", "UTF8")
        .env("LC_ALL", "C")
        .env("PGPASSFILE", fixture.root.path().join("absent.pgpass"))
        .env_remove("PGPASSWORD")
        .env_remove("PGSERVICE")
        .env_remove("PGOPTIONS");
    if let Some(password) = password {
        command.env("PGPASSWORD", password);
    }
    command.output().unwrap()
}

fn query(fixture: &Fixture, port: u16, password: &str, sql: &str) -> String {
    let output = psql(fixture, port, Some(password), sql);
    assert!(output.status.success(), "PostgreSQL TCP query failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn check_container(
    fixture: &Fixture,
    platform: &str,
    mount_target: &str,
    port: u16,
    password: &str,
) {
    let inspect: Value = serde_json::from_str(&docker(&["inspect", &fixture.container()])).unwrap();
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
    assert_eq!(mounts[0]["Destination"], mount_target);
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
        container["HostConfig"]["PortBindings"]["5432/tcp"],
        json!([{"HostIp":"127.0.0.1", "HostPort":port.to_string()}])
    );
    let environment = container["Config"]["Env"].as_array().unwrap();
    for expected in [
        "POSTGRES_DB=acceptance_db".to_owned(),
        "POSTGRES_USER=acceptance_user".to_owned(),
        format!("POSTGRES_PASSWORD={password}"),
    ] {
        assert!(
            environment
                .iter()
                .any(|entry| entry.as_str() == Some(&expected)),
            "Template input reference was not preserved"
        );
    }
    assert_eq!(
        container["Config"]["Healthcheck"]["Test"],
        json!([
            "CMD",
            "pg_isready",
            "-U",
            "acceptance_user",
            "-d",
            "acceptance_db"
        ])
    );
    assert_eq!(
        query(
            fixture,
            port,
            password,
            "SELECT current_database() || ':' || current_user"
        ),
        "acceptance_db:acceptance_user"
    );
    let data = query(fixture, port, password, "SHOW data_directory");
    assert!(
        data == mount_target || data.starts_with(&format!("{mount_target}/")),
        "Database writes outside its declared storage slot"
    );
    for (credential, expected) in [
        (None, "no password supplied"),
        (Some("wrong-password"), "password authentication failed"),
    ] {
        let output = psql(fixture, port, credential, "SELECT 1");
        assert!(!output.status.success());
        assert!(
            String::from_utf8(output.stderr).unwrap().contains(expected),
            "TCP connection did not reject invalid authentication"
        );
    }
}

const PASSWORD: &str = "$' \"日本語 acceptance";

fn create_fixture(
    snapshot: &ResolvedTemplate,
    version: &ResolvedVersion,
    image: &str,
    platform: &str,
    bind: bool,
) -> (Fixture, u16) {
    let root = tempfile::tempdir().unwrap();
    let id = InstanceId::from_u128(
        root.path()
            .to_string_lossy()
            .bytes()
            .fold(std::process::id() as u128, |value, byte| {
                value.wrapping_mul(31).wrapping_add(byte as u128)
            }),
    );
    let volume = format!("cn-{:032x}-data", id.as_u128());
    let fixture = Fixture {
        root,
        volume,
        image: image.to_owned(),
        bind,
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
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let inputs = BTreeMap::from([
        (
            "database".into(),
            InputValue::String("acceptance_db".into()),
        ),
        (
            "username".into(),
            InputValue::String("acceptance_user".into()),
        ),
        ("password".into(), InputValue::String(PASSWORD.into())),
    ]);
    let ports = BTreeMap::from([("database".into(), port)]);
    let storage = BTreeMap::from([("data".into(), storage)]);
    let model = generate(&ConfirmedCompose {
        snapshot,
        version: &version.key,
        instance_id: id,
        scope_id: "postgresql-acceptance",
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
    (fixture, port)
}

fn check_lifecycle(
    fixture: &Fixture,
    version: &str,
    platform: &str,
    target: &str,
    port: u16,
    password: &str,
) {
    check_container(fixture, platform, target, port, password);
    assert_eq!(
        query(
            fixture,
            port,
            password,
            "SELECT current_setting('server_version_num')::int / 10000"
        ),
        version
    );
    query(
        fixture,
        port,
        password,
        "CREATE TABLE acceptance (id integer PRIMARY KEY, value text); INSERT INTO acceptance VALUES (1, 'persistent_日本語')",
    );
    let original = fixture.container();
    fixture.compose(&["stop", "--timeout", "30"]);
    let stopped: Value = serde_json::from_str(&docker(&["inspect", &original])).unwrap();
    assert_eq!(stopped[0]["State"]["Running"], false);
    fixture.up(false);
    assert_eq!(fixture.container(), original);
    check_container(fixture, platform, target, port, password);
    fixture.check_data(port, password);
    fixture.up(true);
    assert_ne!(fixture.container(), original);
    check_container(fixture, platform, target, port, password);
    fixture.check_data(port, password);
}

#[test]
#[ignore = "requires a local Docker Engine, Compose, psql, and permission to pull postgres:17/18"]
fn docker_postgresql_versions_preserve_authenticated_data_in_both_storage_modes() {
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
    for version in &snapshot.versions {
        assert!(version.definition.platforms.contains(&platform));
        let tag = &version.definition.image;
        docker(&["pull", "--platform", &platform, tag]);
        let image = docker(&[
            "image",
            "inspect",
            tag,
            "--format",
            "{{index .RepoDigests 0}}",
        ]);
        let target = if version.key == "17" {
            "/var/lib/postgresql/data"
        } else {
            "/var/lib/postgresql"
        };
        for bind in [true, false] {
            let (fixture, port) = create_fixture(&snapshot, version, &image, &platform, bind);
            check_lifecycle(&fixture, &version.key, &platform, target, port, PASSWORD);
            println!(
                "PASS PostgreSQL {} {} {}; digest={image}; all mounts, inputs, TCP authentication, health, stop/start, recreate and persistence",
                version.key,
                if bind { "bind" } else { "named" },
                platform
            );
        }
    }
}
