use composenest_adapters::{
    sqlite::DatabaseWorker,
    template_catalog_view::{catalog_view, reload_results},
};
use std::{fs, path::Path};

fn package(root: &Path, name: &str, id: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.join("versions")).unwrap();
    fs::write(path.join("template.yaml"), format!("schemaVersion: 1\nid: {id}\ntemplateVersion: \"1.0.0\"\nname: Test\ndescription: Test package\ndefaultVersion: \"2\"\nversions:\n  \"2\": versions/2.yaml\n  \"1\": versions/1.yaml\n")).unwrap();
    for key in ["1", "2"] {
        fs::write(path.join(format!("versions/{key}.yaml")), "image: example:1\nplatforms: [linux/amd64]\nservice:\n  storage:\n    data:\n      label: Data\n      container: /data\n  healthcheck:\n    command: [check]\n").unwrap();
    }
}

fn fixture() -> (
    tempfile::TempDir,
    DatabaseWorker,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let root = tempfile::tempdir().unwrap();
    let bundled = root.path().join("bundled");
    let local = root.path().join("templates/local");
    let management = root.path().join("management");
    for path in [
        &bundled,
        &local,
        &management.join("state"),
        &management.join("locks"),
    ] {
        fs::create_dir_all(path).unwrap();
    }
    #[cfg(unix)]
    for path in [
        &management,
        &management.join("state"),
        &management.join("locks"),
    ] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = DatabaseWorker::start(&management).unwrap();
    (root, store, bundled, local)
}

#[test]
fn projects_packages_and_retains_old_revision_without_counting_failed_reload() {
    let (_root, store, bundled, local) = fixture();
    let empty = catalog_view(
        &store,
        &local,
        reload_results(&store, &bundled, &local).unwrap(),
    )
    .unwrap();
    assert!(empty.templates.is_empty());
    assert!(empty.results.is_empty());
    package(&bundled, "shipped", "example.shipped");
    package(&local, "custom", "example.custom");
    let view = catalog_view(
        &store,
        &local,
        reload_results(&store, &bundled, &local).unwrap(),
    )
    .unwrap();
    assert_eq!(view.templates.len(), 2);
    assert_eq!(view.results.len(), 2);
    for card in &view.templates {
        assert_eq!(card.versions, ["2", "1"]);
        assert_eq!(card.storage_methods.len(), 2);
        assert!(card.loaded);
    }
    assert!(view.templates.iter().any(|card| card.origin == "bundled"));
    assert!(view.templates.iter().any(|card| card.origin == "local"));
    assert_eq!(view.local_root, local.to_string_lossy());
    let serialized = serde_json::to_string(&view).unwrap();
    assert!(!serialized.contains("canonicalJson"));
    assert!(!serialized.contains("image:"));

    fs::remove_file(local.join("custom/versions/1.yaml")).unwrap();
    let view = catalog_view(
        &store,
        &local,
        reload_results(&store, &bundled, &local).unwrap(),
    )
    .unwrap();
    assert_eq!(view.templates.len(), 2);
    assert_eq!(
        view.results
            .iter()
            .filter(|entry| entry.revision_id.is_some())
            .count(),
        1
    );
    assert!(
        !view
            .templates
            .iter()
            .find(|card| card.origin == "local")
            .unwrap()
            .loaded
    );
    let failure = view
        .results
        .iter()
        .find(|entry| entry.package == "custom")
        .unwrap();
    assert_eq!(failure.origin, "local");
    let reason = failure.error.as_ref().unwrap();
    for expected in ["Version 1", "versions/1.yaml", "$.versions.1"] {
        assert!(reason.contains(expected), "{reason}");
    }

    package(&local, "custom", "example.custom");
    fs::write(
        local.join("custom/versions/1.yaml"),
        "image: example:1\nplatforms: [linux/amd64]\nservice:\n  unknown: true\n",
    )
    .unwrap();
    let errors = reload_results(&store, &bundled, &local).unwrap();
    let reason = errors
        .iter()
        .find(|entry| entry.package == "custom")
        .unwrap()
        .error
        .as_ref()
        .unwrap();
    for expected in ["Version 1", "versions/1.yaml", "$.service.unknown"] {
        assert!(reason.contains(expected), "{reason}");
    }
    assert!(
        errors
            .iter()
            .any(|entry| entry.package == "shipped" && entry.error.is_none())
    );
    fs::remove_dir_all(&bundled).unwrap();
    assert!(reload_results(&store, &bundled, &local).is_err());
}

#[test]
fn semantic_warnings_and_discovery_notices_reach_successful_reload_results() {
    let (_root, store, bundled, local) = fixture();
    package(&local, "unused", "example.unused");
    let document = "image: example:1\nplatforms: [linux/amd64]\ninputs:\n  note:\n    label: Note\n    type: string\n    default: private-default-value\nservice:\n  healthcheck:\n    command: [check]\n";
    for version in ["1", "2", "unused"] {
        fs::write(
            local.join(format!("unused/versions/{version}.yaml")),
            document,
        )
        .unwrap();
    }
    // Repeat registration to ensure warnings do not disappear on an unchanged revision.
    for _ in 0..2 {
        let results = reload_results(&store, &bundled, &local).unwrap();
        let entry = &results[0];
        assert!(entry.revision_id.is_some());
        assert!(entry.error.is_none());
        assert_eq!(entry.warnings.len(), 3);
        assert!(entry.warnings[0].contains("versions/unused.yaml"));
        for (warning, version) in entry.warnings[1..].iter().zip(["2", "1"]) {
            for expected in [
                format!("Version {version}"),
                format!("versions/{version}.yaml"),
                "$.inputs.note".into(),
                "この値はコンテナ設定に反映されません".into(),
            ] {
                assert!(warning.contains(&expected), "{warning}");
            }
        }
        assert!(
            !serde_json::to_string(&entry)
                .unwrap()
                .contains("private-default-value")
        );
    }
}

#[test]
fn version_and_manifest_failures_show_japanese_corrections_with_source_positions() {
    let (_root, store, bundled, local) = fixture();
    package(&local, "invalid", "example.invalid");
    for (document, position, reason) in [
        (
            "image: example:1\nplatforms: [linux/amd64]\nservice:\n  unknown: true\n",
            "4行・12列",
            "未知の項目",
        ),
        (
            "image: example:1\nimage: example:2\nplatforms: [linux/amd64]\nservice:\n  healthcheck:\n    command: [check]\n",
            "2行・1列",
            "項目名が重複",
        ),
    ] {
        fs::write(local.join("invalid/versions/1.yaml"), document).unwrap();
        let results = reload_results(&store, &bundled, &local).unwrap();
        let error = results[0].error.as_ref().unwrap();
        for expected in ["Version 1", "versions/1.yaml", position, reason] {
            assert!(error.contains(expected), "{error}");
        }
        assert!(results[0].revision_id.is_none());
    }
    fs::write(
        local.join("invalid/template.yaml"),
        "schemaVersion: 1\nschemaVersion: 1\n",
    )
    .unwrap();
    let results = reload_results(&store, &bundled, &local).unwrap();
    let error = results[0].error.as_ref().unwrap();
    for expected in [
        "template.yaml",
        "2行・1列",
        "$.schemaVersion",
        "定義を1つにまとめて",
    ] {
        assert!(error.contains(expected), "{error}");
    }
}
