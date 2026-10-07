//! Durable publication and verification of generated Compose artifacts.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set, TransactionTrait};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    entities::{artifact, artifact_file, operation},
    sqlite::{DatabaseError, DatabaseWorker},
};

mod external;
pub use external::{ArtifactDifference, ExternalArtifact};

const FILE_LIMIT: u64 = 2 * 1024 * 1024;

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

/// Publishes and checks artifacts under one protected root, reading at most 2 MiB per file.
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
        let recorded = self.database.orm_write(move |db| {
            let tx = db.begin()?;
            let operation_instance =
                operation::Entity::find_by_id(&publication_operation).one(&tx)?;
            if operation_instance
                .as_ref()
                .map(|operation| operation.instance_id.as_str())
                != Some(instance.as_str())
            {
                return Ok(None);
            }
            let current = artifact::Entity::find_by_id(&db_id).one(&tx)?;
            if let Some(current) = current {
                if current.instance_id != instance
                    || current.spec_revision != revision
                    || current.generator_version != generator
                    || current.manifest_hash != db_hash
                    || current.publication_operation_id.as_deref() != Some(&publication_operation)
                {
                    return Ok(None);
                }
                let recorded: BTreeMap<String, String> = artifact_file::Entity::find()
                    .filter(artifact_file::Column::ArtifactId.eq(&db_id))
                    .all(&tx)?
                    .into_iter()
                    .map(|file| (file.relative_path, file.sha256))
                    .collect();
                return Ok((recorded == db_expected).then_some(current.placement == "published"));
            }
            artifact::Entity::insert(artifact::ActiveModel {
                id: Set(db_id.clone()),
                instance_id: Set(instance),
                spec_revision: Set(revision),
                generator_version: Set(generator),
                manifest_hash: Set(db_hash),
                placement: Set("staged".into()),
                publication_operation_id: Set(Some(publication_operation)),
            })
            .exec(&tx)?;
            for (path, hash) in db_expected {
                artifact_file::Entity::insert(artifact_file::ActiveModel {
                    artifact_id: Set(db_id.clone()),
                    relative_path: Set(path),
                    sha256: Set(hash),
                })
                .exec(&tx)?;
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
            check_dir(&staging)?;
            let mut found = BTreeMap::new();
            collect_files(&staging, &staging, &mut found)?;
            if found.keys().any(|path| !expected.contains_key(path)) {
                return Err(ArtifactError::Conflict);
            }
            if found != expected {
                fs::remove_dir_all(&staging)?;
                create_private_dir(&staging)?;
                write_stage(&staging, &files)?;
            } else {
                flush_stage_files(&staging, &expected)?;
            }
        } else {
            create_private_dir(&staging)?;
            write_stage(&staging, &files)?;
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
        let artifact = self
            .database
            .orm_read(|db| Ok(artifact::Entity::find_by_id(id).one(db)?))?;
        if artifact
            .as_ref()
            .map(|artifact| artifact.placement.as_str())
            != Some("published")
        {
            return Err(ArtifactError::Unavailable);
        }
        Ok(path.join("compose.yaml"))
    }

    fn verify(&self, id: &str) -> Result<PathBuf, ArtifactError> {
        validate_id(id)?;
        let record = self.database.orm_read(|db| {
            let identity = artifact::Entity::find_by_id(id)
                .one(db)?
                .map(|artifact| (artifact.instance_id, artifact.manifest_hash));
            let files = artifact_file::Entity::find()
                .filter(artifact_file::Column::ArtifactId.eq(id))
                .all(db)?
                .into_iter()
                .map(|file| (file.relative_path, file.sha256))
                .collect::<BTreeMap<_, _>>();
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
        if !found.contains_key("compose.yaml") {
            return Err(ArtifactError::Unavailable);
        }
        if found != record.1 {
            return Err(ArtifactError::Modified);
        }
        Ok(path)
    }

    /// Reads bounded UTF-8 Compose bytes and verifies those exact bytes against SQLite.
    pub fn read_compose(&self, id: &str) -> Result<(PathBuf, String), ArtifactError> {
        let path = self.verified_compose_path(id)?;
        let file = open_bounded_file(&path)?;
        let mut bytes = Vec::new();
        file.take(FILE_LIMIT + 1).read_to_end(&mut bytes)?;
        let expected = self.database.read(|db| {
            Ok(db.query_row("SELECT sha256 FROM artifact_files WHERE artifact_id=?1 AND relative_path='compose.yaml'", [id], |row| row.get::<_, String>(0))?)
        })?;
        if bytes.len() as u64 > FILE_LIMIT {
            return Err(ArtifactError::InvalidInput);
        }
        if digest(&bytes) != expected {
            return Err(ArtifactError::Modified);
        }
        self.verified_compose_path(id)?;
        let text = String::from_utf8(bytes).map_err(|_| ArtifactError::InvalidInput)?;
        Ok((path, text))
    }

    fn mark_published(&self, id: &str) -> Result<(), ArtifactError> {
        let id = id.to_owned();
        let changed = self.database.orm_write(move |db| {
            let current = artifact::Entity::find_by_id(&id).one(db)?;
            let Some(current) = current
                .filter(|artifact| matches!(artifact.placement.as_str(), "staged" | "published"))
            else {
                return Ok(false);
            };
            artifact::ActiveModel {
                id: Set(current.id),
                placement: Set("published".into()),
                ..Default::default()
            }
            .update(db)?;
            Ok(true)
        })?;
        if !changed {
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

pub(crate) fn validate_id(id: &str) -> Result<(), ArtifactError> {
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

fn write_stage(staging: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<(), ArtifactError> {
    for (relative, bytes) in files {
        let path = staging.join(relative);
        if let Some(parent) = path.parent() {
            create_missing_dirs(staging, parent)?;
        }
        let mut file = private_file(&path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    Ok(())
}

fn flush_stage_files(
    staging: &Path,
    expected: &BTreeMap<String, String>,
) -> Result<(), ArtifactError> {
    for relative in expected.keys() {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(staging.join(relative))?
            .sync_all()?;
    }
    Ok(())
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
            found.insert(relative, bounded_digest(open_bounded_file(&path)?)?);
        } else {
            return Err(ArtifactError::UnsafePath(path));
        }
    }
    Ok(())
}

fn open_bounded_file(path: &Path) -> Result<File, ArtifactError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err(ArtifactError::UnsafePath(path.into()));
    }
    if metadata.len() > FILE_LIMIT {
        return Err(ArtifactError::InvalidInput);
    }
    Ok(file)
}

fn bounded_digest(reader: impl Read) -> Result<String, ArtifactError> {
    let mut reader = reader.take(FILE_LIMIT + 1);
    let mut hash = Sha256::new();
    if io::copy(&mut reader, &mut hash)? > FILE_LIMIT {
        return Err(ArtifactError::InvalidInput);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[test]
fn streaming_hash_stops_if_content_grows_past_the_limit() {
    assert!(matches!(
        bounded_digest(io::repeat(0)),
        Err(ArtifactError::InvalidInput)
    ));
    assert_eq!(bounded_digest(io::empty()).unwrap(), digest(&[]));
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
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let source: Vec<u16> = staging.as_os_str().encode_wide().chain([0]).collect();
    let destination: Vec<u16> = target.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: both NUL-terminated paths remain valid for this call.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } != 0
    {
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
