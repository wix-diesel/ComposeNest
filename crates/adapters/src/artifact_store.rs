//! Durable publication and verification of generated Compose artifacts.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::sqlite::{DatabaseError, DatabaseWorker};

/// A failure that leaves an artifact unavailable for execution.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    /// An artifact identifier, relative path, or file set is invalid.
    #[error("invalid artifact input")]
    InvalidInput,
    /// A path is a link, has an unexpected type, or escapes the management root.
    #[error("unsafe artifact path: {0}")]
    UnsafePath(PathBuf),
    /// The published artifact differs from the SQLite record.
    #[error("artifact contents differ from the recorded manifest")]
    Modified,
    /// The artifact is absent or has not been published.
    #[error("artifact is not published")]
    Unavailable,
    /// A different artifact already occupies the target path.
    #[error("artifact target already exists")]
    Conflict,
    /// SQLite could not record or confirm the artifact.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Filesystem publication or verification failed.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Immutable input captured before artifact publication.
pub struct ArtifactInput {
    /// Artifact identifier, also used as its directory name.
    pub id: String,
    /// Owning instance identifier.
    pub instance_id: String,
    /// Revision of the confirmed instance specification.
    pub spec_revision: u64,
    /// Version of the deterministic Compose generator.
    pub generator_version: String,
    /// Generated files, including `compose.yaml` but excluding `manifest.json`.
    pub files: BTreeMap<String, Vec<u8>>,
}

/// Publishes and checks artifacts under one protected management root.
pub struct ArtifactStore<'a> {
    root: PathBuf,
    database: &'a DatabaseWorker,
}

impl<'a> ArtifactStore<'a> {
    /// Binds publication to the protected root used by the database worker.
    pub fn new(root: impl Into<PathBuf>, database: &'a DatabaseWorker) -> Self {
        Self {
            root: root.into(),
            database,
        }
    }

    /// Records the expected bytes, flushes staging, then publishes without replacing a target.
    /// Repeating the same request reconciles an earlier interrupted publication.
    pub fn publish(
        &self,
        operation_id: &str,
        input: ArtifactInput,
    ) -> Result<PathBuf, ArtifactError> {
        validate_id(operation_id)?;
        validate_id(&input.id)?;
        validate_id(&input.instance_id)?;
        if input.spec_revision == 0
            || input.generator_version.is_empty()
            || !input.files.contains_key("compose.yaml")
            || input.files.contains_key("manifest.json")
            || input.files.keys().any(|path| !valid_relative(path))
        {
            return Err(ArtifactError::InvalidInput);
        }
        let revision =
            i64::try_from(input.spec_revision).map_err(|_| ArtifactError::InvalidInput)?;
        let mut files = input.files;
        let hashes: BTreeMap<_, _> = files
            .iter()
            .map(|(path, bytes)| (path.clone(), digest(bytes)))
            .collect();
        let manifest = serde_json::to_vec(&json!({
            "artifactId": input.id,
            "instanceId": input.instance_id,
            "specRevision": input.spec_revision,
            "generatorVersion": input.generator_version,
            "files": hashes,
        }))
        .map_err(|_| ArtifactError::InvalidInput)?;
        let manifest_hash = digest(&manifest);
        files.insert("manifest.json".into(), manifest);
        let expected: BTreeMap<_, _> = files
            .iter()
            .map(|(path, bytes)| (path.clone(), digest(bytes)))
            .collect();
        let (id, instance, generator) = (input.id, input.instance_id, input.generator_version);
        let path_instance = instance.clone();
        let db_id = id.clone();
        let publication_operation = operation_id.to_owned();
        let db_expected = expected.clone();
        let db_hash = manifest_hash.clone();
        let recorded = self.database.write(move |db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let operation_instance: Option<String> = tx.query_row(
                "SELECT instance_id FROM operations WHERE id = ?1", [&publication_operation], |row| row.get(0),
            ).optional()?;
            if operation_instance.as_deref() != Some(instance.as_str()) { return Ok(None); }
            let current: Option<(String, i64, String, String, String, String)> = tx.query_row(
                "SELECT instance_id, spec_revision, generator_version, manifest_hash, publication_operation_id, placement FROM artifacts WHERE id = ?1",
                [&db_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            ).optional()?;
            if let Some(current) = current {
                if (current.0, current.1, current.2, current.3, current.4) != (instance, revision, generator, db_hash, publication_operation) {
                    return Ok(None);
                }
                let mut statement = tx.prepare("SELECT relative_path, sha256 FROM artifact_files WHERE artifact_id = ?1")?;
                let recorded: BTreeMap<String, String> = statement.query_map([&db_id], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                return Ok((recorded == db_expected).then_some(current.5 == "published"));
            }
            tx.execute("INSERT INTO artifacts (id, instance_id, spec_revision, generator_version, manifest_hash, placement, publication_operation_id) VALUES (?1, ?2, ?3, ?4, ?5, 'staged', ?6)",
                params![db_id, instance, revision, generator, db_hash, publication_operation])?;
            for (path, hash) in db_expected {
                tx.execute("INSERT INTO artifact_files (artifact_id, relative_path, sha256) VALUES (?1, ?2, ?3)",
                    params![db_id, path, hash])?;
            }
            tx.commit()?;
            Ok(Some(false))
        })?;
        let was_published = recorded.ok_or(ArtifactError::Conflict)?;

        let artifacts = self.artifacts_dir(&path_instance)?;
        let target = artifacts.join(&id);
        if exists(&target)? {
            self.verify(&id)?;
            self.mark_published(&id)?;
            return Ok(target);
        }
        if was_published {
            return Err(ArtifactError::Unavailable);
        }
        let staging = self.staging_dir()?.join(operation_id);
        if exists(&staging)? {
            return Err(ArtifactError::Conflict);
        }
        create_private_dir(&staging)?;
        for (relative, bytes) in &files {
            let path = staging.join(relative);
            if let Some(parent) = path.parent() {
                create_missing_dirs(&staging, parent)?;
            }
            let mut file = private_file(&path)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        sync_tree_dirs(&staging)?;
        if exists(&target)? {
            return Err(ArtifactError::Conflict);
        }
        publish_directory(&staging, &target)?;
        sync_dir(&artifacts)?;
        self.verify(&id)?;
        self.mark_published(&id)?;
        Ok(target)
    }

    /// Reconciles a published directory left behind before SQLite placement changed.
    pub fn reconcile(&self, id: &str) -> Result<PathBuf, ArtifactError> {
        let path = self.verify(id)?;
        self.mark_published(id)?;
        Ok(path)
    }

    /// Returns a fixed Compose path only after a full check against SQLite hashes.
    pub fn verified_compose_path(&self, id: &str) -> Result<PathBuf, ArtifactError> {
        let path = self.verify(id)?;
        let placement = self.database.read(|db| {
            Ok(db
                .query_row(
                    "SELECT placement FROM artifacts WHERE id = ?1",
                    [id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?)
        })?;
        if placement.as_deref() != Some("published") {
            return Err(ArtifactError::Unavailable);
        }
        Ok(path.join("compose.yaml"))
    }

    fn verify(&self, id: &str) -> Result<PathBuf, ArtifactError> {
        validate_id(id)?;
        let record = self.database.read(|db| {
            let identity: Option<(String, String)> = db
                .query_row(
                    "SELECT instance_id, manifest_hash FROM artifacts WHERE id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let mut statement = db.prepare(
                "SELECT relative_path, sha256 FROM artifact_files WHERE artifact_id = ?1",
            )?;
            let files = statement
                .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<BTreeMap<String, String>>>()?;
            Ok((identity, files))
        })?;
        let (instance, manifest_hash) = record.0.ok_or(ArtifactError::Unavailable)?;
        validate_id(&instance)?;
        if record.1.is_empty()
            || !record.1.contains_key("compose.yaml")
            || record.1.get("manifest.json") != Some(&manifest_hash)
            || record
                .1
                .keys()
                .any(|path| path != "manifest.json" && !valid_relative(path))
        {
            return Err(ArtifactError::Modified);
        }
        check_dir(&self.root)?;
        let instances = self.root.join("instances");
        let instance_dir = instances.join(&instance);
        let artifacts = instance_dir.join("artifacts");
        for directory in [&instances, &instance_dir, &artifacts] {
            if !exists(directory)? {
                return Err(ArtifactError::Unavailable);
            }
            check_dir(directory)?;
        }
        let path = artifacts.join(id);
        if !exists(&path)? {
            return Err(ArtifactError::Unavailable);
        }
        check_dir(&path)?;
        let mut found = BTreeMap::new();
        collect_files(&path, &path, &mut found)?;
        if found != record.1 {
            return Err(ArtifactError::Modified);
        }
        Ok(path)
    }

    fn mark_published(&self, id: &str) -> Result<(), ArtifactError> {
        let id = id.to_owned();
        let changed = self.database.write(move |db| {
            Ok(db.execute("UPDATE artifacts SET placement = 'published' WHERE id = ?1 AND placement IN ('staged', 'published')", [&id])?)
        })?;
        if changed != 1 {
            return Err(ArtifactError::Unavailable);
        }
        Ok(())
    }

    fn artifacts_dir(&self, instance: &str) -> Result<PathBuf, ArtifactError> {
        check_dir(&self.root)?;
        let instances = self.root.join("instances");
        create_or_check_dir(&instances)?;
        let instance_dir = instances.join(instance);
        create_or_check_dir(&instance_dir)?;
        let artifacts = instance_dir.join("artifacts");
        create_or_check_dir(&artifacts)?;
        Ok(artifacts)
    }

    fn staging_dir(&self) -> Result<PathBuf, ArtifactError> {
        check_dir(&self.root)?;
        let path = self.root.join("staging");
        create_or_check_dir(&path)?;
        Ok(path)
    }
}

fn validate_id(id: &str) -> Result<(), ArtifactError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(ArtifactError::InvalidInput);
    }
    Ok(())
}

fn valid_relative(path: &str) -> bool {
    !path.is_empty()
        && path != "manifest.json"
        && !path.contains('\\')
        && !path.contains(':')
        && !path.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.ends_with(['.', ' '])
        })
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn exists(path: &Path) -> Result<bool, io::Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn check_dir(path: &Path) -> Result<(), ArtifactError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(ArtifactError::UnsafePath(path.into()));
    }
    Ok(())
}

fn create_or_check_dir(path: &Path) -> Result<(), ArtifactError> {
    match create_private_dir(path) {
        Ok(()) => Ok(()),
        Err(ArtifactError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
            check_dir(path)
        }
        Err(error) => Err(error),
    }
}

fn create_missing_dirs(root: &Path, parent: &Path) -> Result<(), ArtifactError> {
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| ArtifactError::InvalidInput)?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        create_or_check_dir(&current)?;
    }
    Ok(())
}

fn create_private_dir(path: &Path) -> Result<(), ArtifactError> {
    #[cfg(unix)]
    let mut builder = fs::DirBuilder::new();
    #[cfg(not(unix))]
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}

fn private_file(path: &Path) -> Result<File, ArtifactError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

fn collect_files(
    root: &Path,
    dir: &Path,
    found: &mut BTreeMap<String, String>,
) -> Result<(), ArtifactError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if is_link(&metadata) {
            return Err(ArtifactError::UnsafePath(path));
        }
        if metadata.is_dir() {
            collect_files(root, &path, found)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| ArtifactError::Modified)?
                .to_str()
                .ok_or(ArtifactError::Modified)?
                .replace('\\', "/");
            if relative != "manifest.json" && !valid_relative(&relative) {
                return Err(ArtifactError::Modified);
            }
            found.insert(relative, digest(&fs::read(path)?));
        } else {
            return Err(ArtifactError::UnsafePath(path));
        }
    }
    Ok(())
}

fn sync_tree_dirs(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree_dirs(&entry.path())?;
        }
    }
    sync_dir(dir)
}

#[cfg(unix)]
fn publish_directory(staging: &Path, target: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let source = std::ffi::CString::new(staging.as_os_str().as_bytes())?;
    let destination = std::ffi::CString::new(target.as_os_str().as_bytes())?;
    #[cfg(target_os = "linux")]
    // SAFETY: both NUL-terminated paths remain valid for this call.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    // SAFETY: both NUL-terminated paths remain valid for this call.
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn publish_directory(staging: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileW;
    let source: Vec<u16> = staging.as_os_str().encode_wide().chain([0]).collect();
    let destination: Vec<u16> = target.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: both NUL-terminated paths remain valid for this call.
    if unsafe { MoveFileW(source.as_ptr(), destination.as_ptr()) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn is_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}
#[cfg(not(windows))]
fn is_link(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(unix)]
fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}
#[cfg(windows)]
fn sync_dir(_: &Path) -> io::Result<()> {
    Ok(())
}
