use composenest_application::state_store::TemplateFile;
use composenest_application::template_catalog::{
    CatalogError, TemplateOrigin, TemplatePackage, prepare_revision,
};

const MANIFEST: &str = "schemaVersion: 1\nid: example.test\ntemplateVersion: \"1.0.0\"\nname: Test\ndescription: Test template\ndefaultVersion: \"2\"\nversions:\n  \"2\": versions/2.yaml\n  \"1\": versions/1.yaml\n";
const VERSION: &str = "image: example:1\nplatforms: [linux/amd64]\ninputs:\n  first:\n    label: First\n    type: string\n    default: value\n  second:\n    label: Second\n    type: boolean\n    default: true\nservice:\n  healthcheck:\n    command: [check]\n";

fn package(manifest: &str, version: &str) -> TemplatePackage {
    TemplatePackage {
        name: "test".into(),
        origin: TemplateOrigin::Local,
        files: vec![
            TemplateFile {
                relative_path: "template.yaml".into(),
                contents: manifest.as_bytes().to_vec(),
            },
            TemplateFile {
                relative_path: "versions/2.yaml".into(),
                contents: version.as_bytes().to_vec(),
            },
            TemplateFile {
                relative_path: "versions/1.yaml".into(),
                contents: version.as_bytes().to_vec(),
            },
        ],
    }
}

#[test]
fn complete_package_preserves_all_versions_and_original_bytes() {
    let revision = prepare_revision(package(MANIFEST, VERSION)).unwrap();
    assert_eq!(revision.normalization, "template-normalization-v1");
    assert_eq!(revision.files.len(), 3);
    assert_eq!(revision.files[0].contents, MANIFEST.as_bytes());
    assert_eq!(revision.origin, "local");
    let value: serde_json::Value = serde_json::from_str(&revision.canonical_json).unwrap();
    assert_eq!(value["manifest"]["versions"], serde_json::json!(["2", "1"]));
    assert_eq!(value["versions"][0]["key"], "2");
    assert_eq!(value["versions"][1]["key"], "1");
    assert_eq!(
        value["versions"][0]["inputPolicies"][0],
        serde_json::json!(["first", "copy"])
    );
    assert_eq!(value["rules"]["secret"], "secret-v1");
}

#[test]
fn comments_whitespace_and_file_names_do_not_change_meaning() {
    let original = prepare_revision(package(MANIFEST, VERSION)).unwrap();
    let relocated = MANIFEST.replace("versions/2.yaml", "versions/new.yaml");
    let mut changed = package(
        &format!("# Comment\n{relocated}"),
        &format!("# Comment\n{VERSION}"),
    );
    changed.files[1].relative_path = "versions/new.yaml".into();
    let revision = prepare_revision(changed).unwrap();
    assert_eq!(revision.semantic_hash, original.semantic_hash);
    assert_eq!(revision.id, original.id);
    assert_ne!(revision.files[0].contents, original.files[0].contents);
    assert_ne!(
        revision.files[1].relative_path,
        original.files[1].relative_path
    );
}

#[test]
fn display_order_and_effective_fields_change_meaning() {
    let original = prepare_revision(package(MANIFEST, VERSION)).unwrap();
    let reordered = MANIFEST.replace(
        "  \"2\": versions/2.yaml\n  \"1\": versions/1.yaml",
        "  \"1\": versions/1.yaml\n  \"2\": versions/2.yaml",
    );
    let reordered = prepare_revision(package(&reordered, VERSION)).unwrap();
    assert_ne!(original.semantic_hash, reordered.semantic_hash);

    let swapped_inputs = VERSION.replace(
        "  first:\n    label: First\n    type: string\n    default: value\n  second:\n    label: Second\n    type: boolean\n    default: true",
        "  second:\n    label: Second\n    type: boolean\n    default: true\n  first:\n    label: First\n    type: string\n    default: value",
    );
    let swapped = prepare_revision(package(MANIFEST, &swapped_inputs)).unwrap();
    assert_ne!(original.semantic_hash, swapped.semantic_hash);
    let changed_label =
        prepare_revision(package(MANIFEST, &VERSION.replace("First", "Changed"))).unwrap();
    assert_ne!(original.semantic_hash, changed_label.semantic_hash);
}

#[test]
fn missing_invalid_or_unlisted_non_default_version_rejects_whole_package() {
    let mut missing = package(MANIFEST, VERSION);
    missing.files.pop();
    assert!(matches!(
        prepare_revision(missing),
        Err(CatalogError::InvalidPackage(_))
    ));

    let mut invalid = package(MANIFEST, VERSION);
    invalid.files[2].contents = b"image: invalid:1\n".to_vec();
    assert!(matches!(
        prepare_revision(invalid),
        Err(CatalogError::Template(_))
    ));

    let mut unlisted = package(MANIFEST, VERSION);
    unlisted.files.push(TemplateFile {
        relative_path: "versions/extra.yaml".into(),
        contents: VERSION.as_bytes().to_vec(),
    });
    assert!(matches!(
        prepare_revision(unlisted),
        Err(CatalogError::InvalidPackage(_))
    ));
}
