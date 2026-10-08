use composenest_domain::template::{parse_manifest, parse_version};

const MANIFEST: &str = include_str!("../../../docs/template-examples/redis/template.yaml");
const VERSION: &str = include_str!("../../../docs/template-examples/redis/versions/8.2.yaml");

#[test]
fn example_documents_keep_version_and_platform_order() {
    let manifest = parse_manifest("redis", MANIFEST.as_bytes()).unwrap();
    assert_eq!(
        manifest.versions,
        [("8.2".into(), "versions/8.2.yaml".into())]
    );
    let version = parse_version("redis", "8.2", "versions/8.2.yaml", VERSION.as_bytes()).unwrap();
    assert_eq!(version.platforms, ["linux/amd64", "linux/arm64"]);
    let postgres = parse_manifest(
        "postgresql",
        include_bytes!("../../../docs/template-examples/postgresql/template.yaml"),
    )
    .unwrap();
    assert_eq!(postgres.versions.len(), 2);
    for (version, path) in &postgres.versions {
        let contents = if version == "17" {
            include_bytes!("../../../docs/template-examples/postgresql/versions/17.yaml").as_slice()
        } else {
            include_bytes!("../../../docs/template-examples/postgresql/versions/18.yaml").as_slice()
        };
        parse_version("postgresql", version, path, contents).unwrap();
    }
}

#[test]
fn rejects_unsafe_yaml_before_structural_validation() {
    let cases = [
        ("schemaVersion: 1\nschemaVersion: 1\n", "duplicate key"),
        ("schemaVersion: &id 1\n", "anchors"),
        ("schemaVersion: *id\n", "aliases"),
        ("schemaVersion: !custom 1\n", "tags"),
        (
            "schemaVersion: 1\n---\nschemaVersion: 1\n",
            "one YAML document",
        ),
        ("schemaVersion: null\n", "null"),
    ];
    for (source, expected) in cases {
        let error = parse_manifest("test", source.as_bytes()).unwrap_err();
        assert!(error.message.contains(expected), "{source}: {error:?}");
        assert_eq!(error.file.as_ref(), "template.yaml");
    }
}

#[test]
fn rejects_numeric_version_and_unknown_manifest_key() {
    let error = parse_manifest(
        "redis",
        MANIFEST
            .replace("defaultVersion: \"8.2\"", "defaultVersion: 8.2")
            .as_bytes(),
    )
    .unwrap_err();
    assert_eq!(error.path.as_ref(), "$.defaultVersion");
    let error =
        parse_manifest("redis", format!("{MANIFEST}\ninputs: {{}}\n").as_bytes()).unwrap_err();
    assert_eq!(error.path.as_ref(), "$.inputs");
    let error = parse_manifest(
        "redis",
        MANIFEST
            .replace("versions/8.2.yaml", "versions/../8.2.yaml")
            .as_bytes(),
    )
    .unwrap_err();
    assert_eq!(error.path.as_ref(), "$.versions.8.2");
}

#[test]
fn rejects_unknown_nested_keys_and_version_metadata() {
    let error = parse_version(
        "redis",
        "8.2",
        "versions/8.2.yaml",
        format!("{VERSION}\nschemaVersion: 1\n").as_bytes(),
    )
    .unwrap_err();
    assert_eq!(error.path.as_ref(), "$.schemaVersion");
    let error = parse_version(
        "redis",
        "8.2",
        "versions/8.2.yaml",
        VERSION
            .replace(
                "container: /data",
                "container: /data\n      host: /tmp/data",
            )
            .as_bytes(),
    )
    .unwrap_err();
    assert_eq!(error.path.as_ref(), "$.service.storage.data.host");
    assert_eq!(error.version.as_deref(), Some("8.2"));
    assert!(error.position.is_some());
}

#[test]
fn enforces_file_and_depth_limits() {
    let oversized = vec![b' '; 256 * 1024 + 1];
    assert!(
        parse_manifest("test", &oversized)
            .unwrap_err()
            .message
            .contains("256 KiB")
    );
    let deeply_nested = format!("{}value{}", "[".repeat(17), "]".repeat(17));
    assert!(
        parse_manifest("test", deeply_nested.as_bytes())
            .unwrap_err()
            .message
            .contains("depth limit")
    );
}

#[test]
fn command_requires_a_nonempty_executable() {
    let source = "image: redis:8.2\nplatforms: [linux/amd64]\nservice:\n  healthcheck:\n    command: [\"\"]\n";
    let error = parse_version("redis", "8.2", "versions/8.2.yaml", source.as_bytes()).unwrap_err();
    assert_eq!(error.path.as_ref(), "$.service.healthcheck.command[0]");
}

#[test]
fn connection_inputs_must_be_unique() {
    let source = VERSION.replace("inputs: [password]", "inputs: [password, password]");
    let error = parse_version("redis", "8.2", "versions/8.2.yaml", source.as_bytes()).unwrap_err();
    assert_eq!(error.path.as_ref(), "$.connections.redis.inputs[1]");
}

#[test]
fn reports_the_failing_field_and_one_based_line() {
    let source = "image: redis:8.2\nplatforms: [linux/amd64]\nservice:\n  healthcheck:\n    command: [echo, { input: password, onMissing: omit }]\n";
    let error = parse_version("redis", "8.2", "versions/8.2.yaml", source.as_bytes()).unwrap_err();
    assert_eq!(
        error.path.as_ref(),
        "$.service.healthcheck.command[1].onMissing"
    );
    assert_eq!(error.position.unwrap().line, 5);
}

#[test]
fn rejects_host_privileges_inheritance_and_distributed_secrets() {
    let version = VERSION.replace("\r\n", "\n");
    let manifest = MANIFEST.replace("\r\n", "\n");
    for setting in [
        "privileged: true",
        "network_mode: host",
        "pid: host",
        "devices: []",
        "volumes: ['/var/run/docker.sock:/var/run/docker.sock']",
        "shell: sh",
        "entrypoint: [sh, -c, id]",
        "cap_add: [SYS_ADMIN]",
        "compose: {}",
    ] {
        let source = version.replace("service:\n", &format!("service:\n  {setting}\n"));
        assert!(
            parse_version("test", "1", "versions/1.yaml", source.as_bytes()).is_err(),
            "{setting}"
        );
    }
    for setting in ["host: /tmp/data", "volume: shared", "clone: copy"] {
        let source = version.replace(
            "container: /data",
            &format!("container: /data\n      {setting}"),
        );
        assert!(
            parse_version("test", "1", "versions/1.yaml", source.as_bytes()).is_err(),
            "{setting}"
        );
    }
    for setting in [
        "inputs: {}",
        "service: {}",
        "extends: other",
        "include: other.yaml",
    ] {
        assert!(
            parse_manifest("test", format!("{manifest}{setting}\n").as_bytes()).is_err(),
            "{setting}"
        );
        assert!(
            parse_version(
                "test",
                "1",
                "versions/1.yaml",
                format!("{version}{setting}\n").as_bytes()
            )
            .is_err(),
            "{setting}"
        );
    }
    let secret_default = version.replace("type: secret", "type: secret\n    default: fixed-secret");
    assert!(parse_version("test", "1", "versions/1.yaml", secret_default.as_bytes()).is_err());
    let legacy = manifest.replace("versions/8.2.yaml", "{ image: redis:8.2, service: {} }");
    assert!(parse_manifest("test", legacy.as_bytes()).is_err());
}
