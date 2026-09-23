use composenest_domain::template::{parse_manifest, parse_version, resolve_template};

const MANIFEST: &str = "schemaVersion: 1\nid: example.test\ntemplateVersion: \"1.0.0\"\nname: Test\ndescription: Test template\ndefaultVersion: \"1\"\nversions:\n  \"1\": versions/1.yaml\n  \"2\": versions/2.yaml\n";
const VERSION: &str = "image: example:1\nplatforms: [linux/amd64]\ninputs:\n  name:\n    label: Name\n    type: string\n    default: app\n  password:\n    label: Password\n    type: secret\nservice:\n  environment:\n    NAME: { input: name }\n    PASSWORD: { input: password }\n  ports:\n    api:\n      label: API\n      container: 9000\n  storage:\n    data:\n      label: Data\n      container: /data\n  healthcheck:\n    command: [check, { input: name }]\nconnections:\n  api:\n    label: API\n    port: api\n    inputs: [name, password]\n";

fn resolve(
    first: &str,
    second: &str,
) -> Result<
    composenest_domain::template::ResolvedTemplate,
    composenest_domain::template::TemplateError,
> {
    let manifest = parse_manifest("test", MANIFEST.as_bytes()).unwrap();
    let definitions = [
        (
            "1".to_owned(),
            parse_version("test", "1", "versions/1.yaml", first.as_bytes()).unwrap(),
        ),
        (
            "2".to_owned(),
            parse_version("test", "2", "versions/2.yaml", second.as_bytes()).unwrap(),
        ),
    ];
    resolve_template(manifest, &definitions)
}

#[test]
fn resolves_each_complete_version_and_preserves_order_and_presence() {
    let second = VERSION
        .replace("  password:\n    label: Password\n    type: secret\n", "")
        .replace("    PASSWORD: { input: password }\n", "")
        .replace("    inputs: [name, password]", "    inputs: [name]")
        .replace("image: example:1", "image: example:2");
    let template = resolve(VERSION, &second).unwrap();
    assert_eq!(
        template
            .versions
            .iter()
            .map(|v| v.key.as_str())
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
    assert_eq!(
        template.versions[0].input_policies,
        [
            ("name".into(), "copy".into()),
            ("password".into(), "regenerate".into())
        ]
    );
    assert_eq!(
        template.versions[1].input_policies,
        [("name".into(), "copy".into())]
    );
    assert_eq!(template.version_clone, "copy");
    assert_eq!(template.storage_clone, "copy");
    assert_eq!(template.versions[1].definition.image, "example:2");
    assert!(matches!(
        template.versions[0].definition.document.value,
        composenest_domain::template::Value::Map(_)
    ));
}

#[test]
fn rejects_missing_and_non_default_invalid_versions() {
    let manifest = parse_manifest("test", MANIFEST.as_bytes()).unwrap();
    let one = parse_version("test", "1", "versions/1.yaml", VERSION.as_bytes()).unwrap();
    let error = resolve_template(manifest, &[("1".into(), one)]).unwrap_err();
    assert_eq!(error.version.as_deref(), Some("2"));

    let second = VERSION.replace("{ input: name }", "{ input: removed }");
    let error = resolve(VERSION, &second).unwrap_err();
    assert_eq!(error.version.as_deref(), Some("2"));
    assert_eq!(error.path.as_ref(), "$.service.environment.NAME.input");
}

#[test]
fn rejects_changed_input_type_across_versions() {
    let second = VERSION
        .replace("type: string", "type: integer")
        .replace("default: app", "default: 1");
    let error = resolve(VERSION, &second).unwrap_err();
    assert_eq!(error.path.as_ref(), "$.inputs.name.type");
}

#[test]
fn rejects_optional_command_and_requires_omit_in_environment() {
    let optional = VERSION.replace("    default: app\n", "    required: false\n");
    let error = resolve(&optional, &optional).unwrap_err();
    assert_eq!(error.path.as_ref(), "$.service.environment.NAME");
    let with_omit = optional.replace(
        "NAME: { input: name }",
        "NAME: { input: name, onMissing: omit }",
    );
    let error = resolve(&with_omit, &with_omit).unwrap_err();
    assert_eq!(error.path.as_ref(), "$.service.healthcheck.command[1]");
    let safe = with_omit.replace("command: [check, { input: name }]", "command: [check]");
    resolve(&safe, &safe).unwrap();
}

#[test]
fn validates_defaults_options_and_secret_generator() {
    let cases = [
        (
            VERSION.replace("default: app", "default: bad").replace(
                "    default: bad",
                "    default: bad\n    validation: { pattern: 'app' }",
            ),
            "$.inputs.name.default",
        ),
        (
            VERSION.replace(
                "    default: app",
                "    default: app\n    validation: { minLength: 5 }",
            ),
            "$.inputs.name.default",
        ),
        (
            VERSION.replace(
                "    default: app",
                "    default: app\n    validation: { minLength: 5, maxLength: 2 }",
            ),
            "$.inputs.name.validation",
        ),
        (
            VERSION.replace(
                "    type: secret",
                "    type: secret\n    validation: { minLength: 33 }",
            ),
            "$.inputs.password",
        ),
        (
            VERSION.replace(
                "    type: secret",
                "    type: secret\n    validation: { pattern: '[a-z]+' }",
            ),
            "$.inputs.password",
        ),
    ];
    for (source, path) in cases {
        let error = resolve(&source, VERSION).unwrap_err();
        assert_eq!(error.path.as_ref(), path);
    }
    let ask = VERSION.replace(
        "    type: secret",
        "    type: secret\n    initial: ask\n    clone: ask\n    validation: { pattern: '[a-z]+' }",
    );
    resolve(&ask, &ask).unwrap();
}

#[test]
fn select_options_and_false_zero_defaults_keep_their_values() {
    let select = VERSION.replace(
        "    type: string\n    default: app",
        "    type: select\n    default: app\n    options:\n      - { value: app, label: App }",
    );
    resolve(&select, &select).unwrap();
    let duplicate = select.replace(
        "      - { value: app, label: App }",
        "      - { value: app, label: App }\n      - { value: app, label: Duplicate }",
    );
    assert_eq!(
        resolve(&duplicate, VERSION).unwrap_err().path.as_ref(),
        "$.inputs.name.options[1].value"
    );
    let bad_default = select.replace("default: app", "default: missing");
    assert_eq!(
        resolve(&bad_default, VERSION).unwrap_err().path.as_ref(),
        "$.inputs.name.default"
    );
    for (kind, default) in [("integer", "0"), ("boolean", "false"), ("string", "\"\"")] {
        let source = VERSION.replace(
            "type: string\n    default: app",
            &format!("type: {kind}\n    default: {default}"),
        );
        resolve(&source, &source).unwrap();
    }
}

#[test]
fn validates_ports_storage_and_connections() {
    let cases = [
        (VERSION.replace("      container: 9000", "      container: 9000\n    other:\n      label: Other\n      container: 9000"), "$.service.ports.other.container"),
        (VERSION.replace("      container: /data", "      container: /data\n    nested:\n      label: Nested\n      container: /data/logs"), "$.service.storage.nested.container"),
        (VERSION.replace("      container: /data", "      container: ../data"), "$.service.storage.data.container"),
        (VERSION.replace("    port: api", "    port: missing"), "$.connections.api.port"),
        (VERSION.replace("inputs: [name, password]", "inputs: [name, removed]"), "$.connections.api.inputs[1]"),
    ];
    for (source, path) in cases {
        let error = resolve(&source, VERSION).unwrap_err();
        assert_eq!(error.path.as_ref(), path);
    }
}

#[test]
fn warns_about_inputs_unused_by_service() {
    let source = VERSION.replace(
        "  password:\n",
        "  unused:\n    label: Unused\n    type: boolean\n  password:\n",
    );
    let result = resolve(&source, &source).unwrap();
    assert_eq!(result.versions[0].warnings[0].path, "$.inputs.unused");
}

#[test]
fn retains_omitted_and_explicitly_empty_connections() {
    let omitted = VERSION.split("connections:\n").next().unwrap();
    let empty = format!("{omitted}connections: {{}}\n");
    let result = resolve(omitted, &empty).unwrap();
    let first = &result.versions[0].definition.document;
    let second = &result.versions[1].definition.document;
    let has_connections = |document: &composenest_domain::template::Node| match &document.value {
        composenest_domain::template::Value::Map(entries) => {
            entries.iter().any(|(key, _)| key == "connections")
        }
        _ => false,
    };
    assert!(!has_connections(first));
    assert!(has_connections(second));
}

#[test]
fn published_examples_resolve_across_every_version() {
    let postgres = parse_manifest(
        "postgresql",
        include_bytes!("../../../docs/template-examples/postgresql/template.yaml"),
    )
    .unwrap();
    let versions = [
        (
            "18".into(),
            parse_version(
                "postgresql",
                "18",
                "versions/18.yaml",
                include_bytes!("../../../docs/template-examples/postgresql/versions/18.yaml"),
            )
            .unwrap(),
        ),
        (
            "17".into(),
            parse_version(
                "postgresql",
                "17",
                "versions/17.yaml",
                include_bytes!("../../../docs/template-examples/postgresql/versions/17.yaml"),
            )
            .unwrap(),
        ),
    ];
    assert_eq!(
        resolve_template(postgres, &versions)
            .unwrap()
            .versions
            .len(),
        2
    );
    let redis = parse_manifest(
        "redis",
        include_bytes!("../../../docs/template-examples/redis/template.yaml"),
    )
    .unwrap();
    let redis_version = parse_version(
        "redis",
        "8.2",
        "versions/8.2.yaml",
        include_bytes!("../../../docs/template-examples/redis/versions/8.2.yaml"),
    )
    .unwrap();
    resolve_template(redis, &[("8.2".into(), redis_version)]).unwrap();
}
