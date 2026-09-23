//! Bounded, package-scoped file capture for Template registration.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use composenest_application::state_store::StateStore;
use composenest_application::template_catalog::{CatalogEntry, CatalogError, register_packages};
use composenest_application::template_catalog::{TemplateOrigin, TemplatePackage};
use composenest_domain::template::parse_manifest;

#[cfg(unix)]
#[path = "template_package/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "template_package/windows.rs"]
mod platform;

const FILE_LIMIT: usize = 256 * 1024;
const PACKAGE_LIMIT: usize = 8 * 1024 * 1024;

/// A package-specific read error; other packages can still be loaded.
#[derive(Debug)]
pub struct PackageReadFailure {
    /// Immediate package directory name.
    pub package: String,
    /// Safe diagnostic without document contents.
    pub reason: String,
    identity: Option<(String, String)>,
}

/// Outcome of reading and registering a catalog package.
#[derive(Debug)]
pub enum ReloadEntry {
    /// Filesystem access failed for this package; no revision was registered.
    ReadFailure(PackageReadFailure),
    /// Fixed bytes were validated and registered or produced a package-specific error.
    Registration(CatalogEntry),
}

/// Reloads bundled and local packages together so cross-source duplicates never win by order.
pub fn reload_catalog(
    store: &impl StateStore,
    bundled_root: &Path,
    local_root: &Path,
) -> io::Result<Vec<ReloadEntry>> {
    let mut packages = Vec::new();
    let mut failures = Vec::new();
    let mut failed_identities = HashSet::new();
    for (root, origin) in [
        (bundled_root, TemplateOrigin::Bundled),
        (local_root, TemplateOrigin::Local),
    ] {
        for result in read_packages(root, origin)? {
            match result {
                Ok(package) => packages.push(package),
                Err(error) => {
                    if let Some(identity) = &error.identity {
                        failed_identities.insert(identity.clone());
                    }
                    failures.push(ReloadEntry::ReadFailure(error));
                }
            }
        }
    }
    let mut unambiguous = Vec::new();
    for package in packages {
        let Some(manifest_file) = package.files.first() else {
            failures.push(ReloadEntry::Registration(CatalogEntry {
                package: package.name,
                warnings: package.warnings,
                result: Err(CatalogError::InvalidPackage("manifest is missing".into())),
            }));
            continue;
        };
        let manifest = match parse_manifest(&package.name, &manifest_file.contents) {
            Ok(manifest) => manifest,
            Err(error) => {
                failures.push(ReloadEntry::Registration(CatalogEntry {
                    package: package.name,
                    warnings: package.warnings,
                    result: Err(CatalogError::Template(error)),
                }));
                continue;
            }
        };
        if failed_identities.contains(&(manifest.id, manifest.template_version)) {
            failures.push(ReloadEntry::Registration(CatalogEntry {
                package: package.name,
                warnings: package.warnings,
                result: Err(CatalogError::AmbiguousRevision),
            }));
        } else {
            unambiguous.push(package);
        }
    }
    failures.extend(
        register_packages(store, unambiguous)
            .into_iter()
            .map(ReloadEntry::Registration),
    );
    Ok(failures)
}

/// Reads direct child packages of a trusted resource or `templates/local` directory.
///
/// Only `template.yaml` is an entry point. An invalid package yields an individual
/// error and does not prevent other packages from being captured.
pub fn read_packages(
    root: &Path,
    origin: TemplateOrigin,
) -> io::Result<Vec<Result<TemplatePackage, PackageReadFailure>>> {
    let mut results = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let display = name.to_string_lossy().into_owned();
        let metadata = match std::fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) => {
                results.push(Err(PackageReadFailure {
                    package: display,
                    reason: error.to_string(),
                    identity: None,
                }));
                continue;
            }
        };
        if !metadata.is_dir() && !platform::is_link(&metadata) {
            continue;
        }
        let result =
            read_package(root, &name, origin).map_err(|(error, identity)| PackageReadFailure {
                package: display,
                reason: error.to_string(),
                identity,
            });
        results.push(result);
    }
    Ok(results)
}

type ReadError = (io::Error, Option<(String, String)>);

fn read_package(
    root: &Path,
    name: &OsStr,
    origin: TemplateOrigin,
) -> Result<TemplatePackage, ReadError> {
    let display = name.to_string_lossy().into_owned();
    let mut handle = platform::SafePackage::open(root, name).map_err(|error| (error, None))?;
    let mut total = 0;
    let manifest_file = handle
        .read("template.yaml", &mut total)
        .map_err(|error| (error, None))?;
    let manifest = parse_manifest(&display, &manifest_file.contents)
        .map_err(|error| (invalid(&error.to_string()), None))?;
    let identity = (manifest.id.clone(), manifest.template_version.clone());
    let mut files = Vec::with_capacity(manifest.versions.len() + 1);
    files.push(manifest_file);
    for (_, relative) in manifest.versions {
        files.push(
            handle
                .read(&relative, &mut total)
                .map_err(|error| (error, Some(identity.clone())))?,
        );
    }
    let listed: HashSet<&str> = files
        .iter()
        .map(|file| file.relative_path.as_str())
        .collect();
    let mut warnings = Vec::new();
    for entry in std::fs::read_dir(root.join(name).join("versions"))
        .map_err(|error| (error, Some(identity.clone())))?
    {
        let entry = entry.map_err(|error| (error, Some(identity.clone())))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = format!("versions/{name}");
        if name.ends_with(".yaml") && !listed.contains(relative.as_str()) {
            warnings.push(format!("unlisted version file was ignored: {relative}"));
        }
    }
    warnings.sort();
    handle.finish().map_err(|error| (error, Some(identity)))?;
    Ok(TemplatePackage {
        name: display,
        origin,
        files,
        warnings,
    })
}

fn read_capped(file: &mut File, total: &mut usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take((FILE_LIMIT + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > FILE_LIMIT {
        return Err(invalid("document exceeds the 256 KiB file limit"));
    }
    if total.saturating_add(bytes.len()) > PACKAGE_LIMIT {
        return Err(invalid("package exceeds the 8 MiB total limit"));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut verification = Vec::new();
    file.take((FILE_LIMIT + 1) as u64)
        .read_to_end(&mut verification)?;
    if bytes != verification {
        return Err(invalid("file changed while being read; reload the package"));
    }
    *total += bytes.len();
    Ok(bytes)
}

fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discards_collection_when_a_file_changes_before_final_verification() {
        let root = tempfile::tempdir().unwrap();
        let package_path = root.path().join("package");
        std::fs::create_dir(&package_path).unwrap();
        std::fs::write(package_path.join("template.yaml"), b"before").unwrap();
        let mut package = platform::SafePackage::open(root.path(), OsStr::new("package")).unwrap();
        package.read("template.yaml", &mut 0).unwrap();
        std::fs::write(package_path.join("template.yaml"), b"after this write").unwrap();
        assert!(package.finish().is_err());
    }
}
