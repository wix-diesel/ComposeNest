use std::fs;
use std::path::Path;

use composenest_adapters::sqlite::DatabaseWorker;
use composenest_adapters::template_package::{ReloadEntry, read_packages, reload_catalog};
use composenest_application::state_store::StateStore;
use composenest_application::template_catalog::{CatalogError, TemplateOrigin};

const MANIFEST: &str = "schemaVersion: 1\nid: example.test\ntemplateVersion: \"1.0.0\"\nname: Test\ndescription: Test template\ndefaultVersion: \"2\"\nversions:\n  \"2\": versions/2.yaml\n  \"1\": versions/1.yaml\n";
const VERSION: &str =
    "image: example:1\nplatforms: [linux/amd64]\nservice:\n  healthcheck:\n    command: [check]\n";

fn write_package(root: &Path, name: &str) {
    let package = root.join(name);
    fs::create_dir_all(package.join("versions")).unwrap();
    fs::write(package.join("template.yaml"), MANIFEST).unwrap();
    for version in ["1", "2"] {
        fs::write(package.join(format!("versions/{version}.yaml")), VERSION).unwrap();
    }
}

#[test]
fn scans_only_direct_packages_and_isolates_missing_versions() {
    let root = tempfile::tempdir().unwrap();
    write_package(root.path(), "good");
    write_package(root.path(), "broken");
    fs::remove_file(root.path().join("broken/versions/1.yaml")).unwrap();
    fs::write(root.path().join("standalone.template.yaml"), MANIFEST).unwrap();
    fs::write(root.path().join("versions.yaml"), VERSION).unwrap();
    fs::write(root.path().join("good/versions/unused.yaml"), VERSION).unwrap();

    let results = read_packages(root.path(), TemplateOrigin::Local).unwrap();
    assert_eq!(results.len(), 2);
    let good = results
        .iter()
        .find(|result| result.as_ref().is_ok_and(|package| package.name == "good"))
        .unwrap();
    assert_eq!(good.as_ref().unwrap().files.len(), 3);
    assert_eq!(good.as_ref().unwrap().warnings.len(), 1);
    assert!(good.as_ref().unwrap().warnings[0].contains("versions/unused.yaml"));
    assert!(results.iter().any(|result| {
        result
            .as_ref()
            .err()
            .is_some_and(|error| error.package == "broken")
    }));
}

#[cfg(windows)]
#[test]
fn rejects_directory_and_file_symlinks_when_windows_can_create_them() {
    use std::os::windows::fs::{symlink_dir, symlink_file};

    let root = tempfile::tempdir().unwrap();
    write_package(root.path(), "real");
    if symlink_dir(root.path().join("real"), root.path().join("linked")).is_ok() {
        let results = read_packages(root.path(), TemplateOrigin::Local).unwrap();
        assert!(results.iter().any(|result| {
            result
                .as_ref()
                .err()
                .is_some_and(|error| error.package == "linked")
        }));
    }
    fs::remove_file(root.path().join("real/versions/1.yaml")).unwrap();
    if symlink_file(
        root.path().join("real/versions/2.yaml"),
        root.path().join("real/versions/1.yaml"),
    )
    .is_ok()
    {
        let results = read_packages(root.path(), TemplateOrigin::Local).unwrap();
        assert!(results.iter().any(|result| {
            result
                .as_ref()
                .err()
                .is_some_and(|error| error.package == "real")
        }));
    }
}

#[test]
fn rejects_unsafe_references_and_duplicate_physical_files() {
    let root = tempfile::tempdir().unwrap();
    write_package(root.path(), "unsafe");
    for reference in [
        "../outside.yaml",
        "/absolute.yaml",
        "C:/absolute.yaml",
        "https://example.com/version.yaml",
        "versions/CON.yaml",
        "versions/nested/1.yaml",
        "versions/2.yaml",
    ] {
        let manifest = MANIFEST.replace("versions/1.yaml", reference);
        fs::write(root.path().join("unsafe/template.yaml"), manifest).unwrap();
        let results = read_packages(root.path(), TemplateOrigin::Local).unwrap();
        assert!(
            results[0].is_err(),
            "accepted unsafe reference: {reference}"
        );
    }

    fs::write(root.path().join("unsafe/template.yaml"), MANIFEST).unwrap();
    fs::remove_file(root.path().join("unsafe/versions/1.yaml")).unwrap();
    fs::hard_link(
        root.path().join("unsafe/versions/2.yaml"),
        root.path().join("unsafe/versions/1.yaml"),
    )
    .unwrap();
    let results = read_packages(root.path(), TemplateOrigin::Local).unwrap();
    assert!(
        results[0]
            .as_ref()
            .err()
            .unwrap()
            .reason
            .contains("same physical file")
    );
}

#[test]
fn rejects_oversized_documents_before_parsing() {
    let root = tempfile::tempdir().unwrap();
    write_package(root.path(), "large");
    let oversized = format!("{MANIFEST}{}", " ".repeat(256 * 1024));
    fs::write(root.path().join("large/template.yaml"), oversized).unwrap();
    let results = read_packages(root.path(), TemplateOrigin::Local).unwrap();
    assert_eq!(
        results[0].as_ref().err().unwrap().reason,
        "document exceeds the 256 KiB file limit"
    );
}

#[test]
fn rejects_package_when_cumulative_size_exceeds_eight_mebibytes() {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("cumulative");
    fs::create_dir_all(package.join("versions")).unwrap();
    let mut manifest = "schemaVersion: 1\nid: example.test\ntemplateVersion: \"1.0.0\"\nname: Test\ndescription: Test template\ndefaultVersion: \"0\"\nversions:\n".to_owned();
    let version = format!("{VERSION}{}", " ".repeat(256 * 1024 - VERSION.len()));
    for index in 0..32 {
        manifest.push_str(&format!("  \"{index}\": versions/{index}.yaml\n"));
        fs::write(package.join(format!("versions/{index}.yaml")), &version).unwrap();
    }
    fs::write(package.join("template.yaml"), manifest).unwrap();
    let results = read_packages(root.path(), TemplateOrigin::Local).unwrap();
    assert_eq!(
        results[0].as_ref().err().unwrap().reason,
        "package exceeds the 8 MiB total limit"
    );
}

#[test]
fn reload_does_not_choose_between_bundled_and_local_duplicate() {
    let root = tempfile::tempdir().unwrap();
    let bundled = root.path().join("bundled");
    let local = root.path().join("local");
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
    write_package(&bundled, "first");
    write_package(&local, "second");
    let worker = DatabaseWorker::start(&management).unwrap();
    let results = reload_catalog(&worker, &bundled, &local).unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|entry| matches!(
        entry,
        ReloadEntry::Registration(item) if matches!(item.result, Err(CatalogError::AmbiguousRevision))
    )));
    assert!(worker.list_templates().unwrap().is_empty());

    fs::remove_file(local.join("second/versions/1.yaml")).unwrap();
    let results = reload_catalog(&worker, &bundled, &local).unwrap();
    assert!(
        results
            .iter()
            .any(|entry| matches!(entry, ReloadEntry::ReadFailure(_)))
    );
    assert!(results.iter().any(|entry| matches!(
        entry,
        ReloadEntry::Registration(item) if matches!(item.result, Err(CatalogError::AmbiguousRevision))
    )));
    assert!(worker.list_templates().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_package_and_version_file() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    write_package(root.path(), "real");
    symlink(root.path().join("real"), root.path().join("linked")).unwrap();
    let results = read_packages(root.path(), TemplateOrigin::Local).unwrap();
    assert!(results.iter().any(|result| {
        result
            .as_ref()
            .err()
            .is_some_and(|error| error.package == "linked")
    }));

    fs::remove_file(root.path().join("real/versions/1.yaml")).unwrap();
    symlink(
        root.path().join("real/versions/2.yaml"),
        root.path().join("real/versions/1.yaml"),
    )
    .unwrap();
    let results = read_packages(root.path(), TemplateOrigin::Local).unwrap();
    assert!(results.iter().any(|result| {
        result
            .as_ref()
            .err()
            .is_some_and(|error| error.package == "real")
    }));
}
