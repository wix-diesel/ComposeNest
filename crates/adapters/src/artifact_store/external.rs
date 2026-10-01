//! Hash-only external changes and protected, non-executable recovery files.

use super::*;

/// One changed file; contents and secret values never leave the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDifference {
    /// Validated relative name of the changed file.
    pub path: String,
    /// Hash of the generated file, or None for an added file.
    pub recorded_hash: Option<String>,
    /// Hash of the external file, or None for a removed file.
    pub observed_hash: Option<String>,
}

/// Non-sensitive confirmation of an entire externally edited artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalArtifact {
    /// Artifact whose generated contents differ from disk.
    pub artifact_id: String,
    /// Hash binding the artifact identity and the complete observed file set.
    pub confirmation_hash: String,
    /// Added, removed and modified files, without raw values.
    pub differences: Vec<ArtifactDifference>,
}

impl ArtifactStore<'_> {
    /// Returns file-level changes and a confirmation hash without exposing contents.
    pub fn inspect_external(&self, id: &str) -> Result<ExternalArtifact, ArtifactError> {
        let (path, expected) = self.external_record(id)?;
        let found = scan(&path)?;
        let mut names: Vec<_> = expected.keys().chain(found.keys()).cloned().collect();
        names.sort();
        names.dedup();
        let differences = names
            .into_iter()
            .filter_map(|path| {
                let recorded_hash = expected.get(&path).cloned();
                let observed_hash = found.get(&path).cloned();
                (recorded_hash != observed_hash).then_some(ArtifactDifference {
                    path,
                    recorded_hash,
                    observed_hash,
                })
            })
            .collect();
        Ok(ExternalArtifact {
            artifact_id: id.into(),
            confirmation_hash: confirmation(id, &found)?,
            differences,
        })
    }

    /// Rechecks confirmation, protects all files, then moves them without overwriting.
    /// A retry checks archived bytes after an interrupted move; archives are never executable.
    pub fn archive_external(
        &self,
        operation_id: &str,
        id: &str,
        confirmed_hash: &str,
    ) -> Result<PathBuf, ArtifactError> {
        validate_id(operation_id)?;
        let (source, _) = self.external_record(id)?;
        let allowed = self.database.orm_read(|db| {
            let saved = artifact::Entity::find_by_id(id).one(db)?;
            let op = operation::Entity::find_by_id(operation_id).one(db)?;
            Ok(saved.zip(op).is_some_and(|(saved, op)| {
                saved.instance_id == op.instance_id
                    && op.kind == "recover"
                    && !matches!(op.status.as_str(), "Succeeded" | "Abandoned")
            }))
        })?;
        if !allowed {
            return Err(ArtifactError::InvalidInput);
        }
        let recovery = self.root.join("recovery");
        create_or_check_dir(&recovery)?;
        let operation_dir = recovery.join(operation_id);
        create_or_check_dir(&operation_dir)?;
        let target = operation_dir.join(id);
        if exists(&target)? {
            // Two copies are ambiguous; never discard either as a guessed duplicate.
            if exists(&source)? {
                return Err(ArtifactError::Conflict);
            }
            require_confirmation(id, &target, confirmed_hash)?;
            protect_tree(&target)?;
        } else {
            require_confirmation(id, &source, confirmed_hash)?;
            protect_tree(&source)?;
            require_confirmation(id, &source, confirmed_hash)?;
            publish_directory(&source, &target)?;
            sync_dir(source.parent().ok_or(ArtifactError::InvalidInput)?)?;
            sync_dir(&operation_dir)?;
            require_confirmation(id, &target, confirmed_hash)?;
        }
        let id = id.to_owned();
        self.database.orm_write(move |db| {
            artifact::ActiveModel {
                id: Set(id),
                placement: Set("retained".into()),
                ..Default::default()
            }
            .update(db)?;
            Ok(())
        })?;
        Ok(target)
    }

    fn external_record(
        &self,
        id: &str,
    ) -> Result<(PathBuf, BTreeMap<String, String>), ArtifactError> {
        validate_id(id)?;
        let record = self.database.orm_read(|db| {
            let saved = artifact::Entity::find_by_id(id).one(db)?;
            let files = artifact_file::Entity::find()
                .filter(artifact_file::Column::ArtifactId.eq(id))
                .all(db)?
                .into_iter()
                .map(|f| (f.relative_path, f.sha256))
                .collect();
            Ok((saved, files))
        })?;
        let saved = record.0.ok_or(ArtifactError::Unavailable)?;
        validate_id(&saved.instance_id)?;
        if !matches!(saved.placement.as_str(), "published" | "retained") {
            return Err(ArtifactError::Unavailable);
        }
        let parent = self.artifacts_dir(&saved.instance_id)?;
        Ok((parent.join(id), record.1))
    }
}

fn scan(path: &Path) -> Result<BTreeMap<String, String>, ArtifactError> {
    check_dir(path)?;
    let mut found = BTreeMap::new();
    collect_files(path, path, &mut found)?;
    Ok(found)
}

fn confirmation(id: &str, hashes: &BTreeMap<String, String>) -> Result<String, ArtifactError> {
    let bytes = serde_json::to_vec(&(id, hashes)).map_err(|_| ArtifactError::InvalidInput)?;
    Ok(digest(&bytes))
}

fn require_confirmation(id: &str, path: &Path, hash: &str) -> Result<(), ArtifactError> {
    if confirmation(id, &scan(path)?)? != hash {
        return Err(ArtifactError::Modified);
    }
    Ok(())
}

fn protect_tree(path: &Path) -> Result<(), ArtifactError> {
    let metadata = fs::symlink_metadata(path)?;
    if is_link(&metadata) {
        return Err(ArtifactError::UnsafePath(path.into()));
    }
    #[cfg(windows)]
    {
        // Reset explicit grants to the protected management parent's inherited ACL.
        let system = std::env::var_os("SystemRoot").ok_or(ArtifactError::InvalidInput)?;
        let status = std::process::Command::new(PathBuf::from(system).join("System32/icacls.exe"))
            .arg(path)
            .args(["/reset", "/Q"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if !status.success() {
            return Err(ArtifactError::UnsafePath(path.into()));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // Changing a hard link's permissions would also affect the other owner.
        if metadata.is_file() && metadata.nlink() != 1 {
            return Err(ArtifactError::UnsafePath(path.into()));
        }
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if metadata.is_dir() { 0o700 } else { 0o600 }),
        )?;
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            protect_tree(&entry?.path())?;
        }
        sync_dir(path)?;
    } else if metadata.is_file() {
        OpenOptions::new().read(true).write(true).open(path)?.sync_all()?;
    } else {
        return Err(ArtifactError::UnsafePath(path.into()));
    }
    Ok(())
}
