//! Docker image pull and inspection bound to a fixed Engine and saved digest.

use std::{ffi::OsString, time::Duration};

use composenest_application::image_resolution::{
    ImageError, ImageResolution, ImageResolutionStore, artifact_image, valid_hash, validate_image,
};
use serde_json::Value;

use crate::{docker_cli::CliOutcome, docker_target::BoundDocker};

const PULL_DEADLINE: Duration = Duration::from_secs(15 * 60);
const INSPECT_FORMAT: &str = r#"{"Id":{{json .Id}},"RepoDigests":{{json .RepoDigests}},"Os":{{json .Os}},"Architecture":{{json .Architecture}},"Volumes":{{json .Config.Volumes}}}"#;

/// Confirmed spec and operation identity for one image resolution attempt.
pub struct ImageRequest<'a> {
    /// Instance whose spec contains the image reference.
    pub instance_id: &'a str,
    /// Confirmed spec revision.
    pub revision: u64,
    /// Image reference from the selected template version.
    pub requested: &'a str,
    /// Registered Engine platform.
    pub platform: &'a str,
    /// Container paths of every declared template storage slot.
    pub storage_destinations: &'a [String],
    /// Operation with a durable ResolveImage step.
    pub operation_id: &'a str,
}

/// Resolves a requested image once and verifies a previously saved digest on later attempts.
/// The caller must journal the ResolveImage step before calling this function.
pub async fn ensure_image<S: ImageResolutionStore>(
    docker: &BoundDocker,
    store: &S,
    request: ImageRequest<'_>,
) -> Result<ImageResolution, ImageError> {
    let ImageRequest {
        instance_id,
        revision,
        requested,
        platform,
        storage_destinations,
        operation_id,
    } = request;
    let saved = store
        .image_resolution(instance_id, revision)
        .map_err(|_| ImageError::Unavailable)?;
    if let Some(saved) = saved {
        if saved.requested != requested || saved.platform != platform {
            return Err(ImageError::ImageChanged);
        }
        let pinned = artifact_image(Some(&saved))?;
        let inspected = match inspect(docker, pinned).await {
            Ok(Some(image)) => image,
            Ok(None) => {
                pull(docker, pinned, platform).await?;
                inspect(docker, pinned)
                    .await?
                    .ok_or(ImageError::Unresolved)?
            }
            Err(error) => return Err(error),
        };
        validate_inspection(&inspected, platform, storage_destinations)?;
        if inspected.id != saved.image_id {
            return Err(ImageError::ImageChanged);
        }
        return Ok(saved);
    }
    pull(docker, requested, platform).await?;
    let inspected = inspect(docker, requested)
        .await?
        .ok_or(ImageError::Unresolved)?;
    validate_inspection(&inspected, platform, storage_destinations)?;
    let repository = requested.split('@').next().unwrap_or(requested);
    let repository = repository
        .rsplit_once(':')
        .filter(|(_, tag)| !tag.contains('/'))
        .map_or(repository, |(name, _)| name);
    let digest =
        select_digest(repository, &inspected.repo_digests).ok_or(ImageError::Unresolved)?;
    // A concurrently moved tag cannot substitute a different image for the selected digest.
    let pinned = inspect(docker, &digest)
        .await?
        .ok_or(ImageError::Unresolved)?;
    validate_inspection(&pinned, platform, storage_destinations)?;
    if pinned.id != inspected.id {
        return Err(ImageError::ImageChanged);
    }
    let resolved = ImageResolution {
        instance_id: instance_id.into(),
        spec_revision: revision,
        requested: requested.into(),
        digest,
        image_id: pinned.id,
        platform: platform.into(),
        operation_id: operation_id.into(),
    };
    store
        .record_image_resolution(&resolved)
        .map_err(|_| ImageError::Unavailable)?;
    Ok(resolved)
}

struct Inspected {
    id: String,
    platform: String,
    repo_digests: Vec<String>,
    volumes: Vec<String>,
}

fn validate_inspection(
    image: &Inspected,
    platform: &str,
    slots: &[String],
) -> Result<(), ImageError> {
    validate_image(&image.id, &image.platform, platform, &image.volumes, slots)
}

fn select_digest(repository: &str, digests: &[String]) -> Option<String> {
    digests
        .iter()
        .find(|candidate| {
            candidate.rsplit_once('@').is_some_and(|(name, hash)| {
                valid_hash(hash)
                    && (name == repository
                        || name == format!("docker.io/{repository}")
                        || (repository.find('/').is_none()
                            && name == format!("docker.io/library/{repository}")))
            })
        })
        .cloned()
}

async fn pull(docker: &BoundDocker, reference: &str, platform: &str) -> Result<(), ImageError> {
    let args: [OsString; 4] = [
        "pull".into(),
        "--platform".into(),
        platform.into(),
        reference.into(),
    ];
    let outcome = docker
        .change(&args, PULL_DEADLINE)
        .await
        .map_err(|_| ImageError::Unavailable)?;
    if outcome.outcome_unknown {
        return Err(ImageError::OutcomeUnknown);
    }
    if !complete(&outcome) {
        return Err(ImageError::Unavailable);
    }
    Ok(())
}

async fn inspect(docker: &BoundDocker, reference: &str) -> Result<Option<Inspected>, ImageError> {
    let args: [OsString; 5] = [
        "image".into(),
        "inspect".into(),
        "--format".into(),
        INSPECT_FORMAT.into(),
        reference.into(),
    ];
    let outcome = docker
        .read_inspection(&args)
        .await
        .map_err(|_| ImageError::Unavailable)?;
    if outcome.outcome_unknown || outcome.stdout.truncated || outcome.stderr.truncated {
        return Err(ImageError::OutcomeUnknown);
    }
    if !outcome.status.is_some_and(|status| status.success()) {
        return Ok(None);
    }
    let value: Value =
        serde_json::from_slice(&outcome.stdout.bytes).map_err(|_| ImageError::Unresolved)?;
    let id = value["Id"]
        .as_str()
        .ok_or(ImageError::Unresolved)?
        .to_owned();
    let os = value["Os"].as_str().ok_or(ImageError::Unresolved)?;
    let arch = value["Architecture"]
        .as_str()
        .ok_or(ImageError::Unresolved)?;
    let repo_digests = value["RepoDigests"]
        .as_array()
        .ok_or(ImageError::Unresolved)?
        .iter()
        .map(|v| v.as_str().map(str::to_owned).ok_or(ImageError::Unresolved))
        .collect::<Result<Vec<_>, _>>()?;
    let volumes = match &value["Volumes"] {
        Value::Null => Vec::new(),
        Value::Object(map) => map.keys().cloned().collect(),
        _ => return Err(ImageError::Unresolved),
    };
    Ok(Some(Inspected {
        id,
        platform: format!("{os}/{arch}"),
        repo_digests,
        volumes,
    }))
}

fn complete(outcome: &CliOutcome) -> bool {
    outcome.status.is_some_and(|status| status.success())
        && !outcome.stdout.truncated
        && !outcome.stderr.truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_selects_digest_for_requested_repository() {
        let hash = format!("sha256:{}", "a".repeat(64));
        let values = vec![
            format!("other@{hash}"),
            format!("docker.io/library/redis@{hash}"),
        ];
        assert_eq!(select_digest("redis", &values), Some(values[1].clone()));
        assert_eq!(select_digest("postgres", &values), None);
    }
}

#[cfg(all(test, unix))]
mod docker_tests {
    use super::*;
    use crate::docker_target::DockerProbe;
    use composenest_application::state_store::{RuntimeTarget, StoreConflict};
    use std::{fs, os::unix::fs::PermissionsExt, sync::Mutex};

    #[derive(Default)]
    struct MemoryStore(Mutex<Option<ImageResolution>>);
    impl ImageResolutionStore for MemoryStore {
        fn image_resolution(
            &self,
            _: &str,
            _: u64,
        ) -> Result<Option<ImageResolution>, StoreConflict> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn record_image_resolution(&self, value: &ImageResolution) -> Result<(), StoreConflict> {
            *self.0.lock().unwrap() = Some(value.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn pins_digest_and_repulls_only_that_digest_when_missing() {
        let root = tempfile::tempdir().unwrap();
        let docker_path = root.path().join("docker-mock");
        let hash = format!("sha256:{}", "a".repeat(64));
        let script = format!(
            r#"#!/bin/sh
if [ "$1" != '--host' ]; then exit 1; fi
shift 2
case "$1 $2" in
  'info --format') printf '{{"ID":"engine"}}\n' ;;
  'pull --platform')
    printf '%s\n' "$4" >> pulls
    /usr/bin/touch present ;;
  'image inspect')
    if [ "$5" != 'redis:8' ] && [ ! -f present ]; then exit 1; fi
    if [ -f wrong-platform ]; then arch=arm64; else arch=amd64; fi
    if [ -f extra-volume ]; then volume=',"/unknown":{{}}'; else volume=''; fi
    printf '{{"Id":"{hash}","RepoDigests":["docker.io/library/redis@{hash}"],"Os":"linux","Architecture":"%s","Volumes":{{"/data":{{}}%s}}}}\n' "$arch" "$volume" ;;
  *) exit 1 ;;
esac
"#
        );
        fs::write(&docker_path, script).unwrap();
        fs::set_permissions(&docker_path, fs::Permissions::from_mode(0o700)).unwrap();
        let probe = DockerProbe {
            executable: docker_path,
            directory: root.path().into(),
            config_directory: root.path().into(),
        };
        let docker = probe
            .bind(RuntimeTarget {
                id: "target".into(),
                scope_id: "scope".into(),
                endpoint: "unix:///tmp/mock.sock".into(),
                engine_id: "engine".into(),
                platform: "linux/amd64".into(),
            })
            .unwrap();
        let store = MemoryStore::default();
        let slots = vec!["/data".into()];
        let resolve = || {
            ensure_image(
                &docker,
                &store,
                ImageRequest {
                    instance_id: "instance",
                    revision: 1,
                    requested: "redis:8",
                    platform: "linux/amd64",
                    storage_destinations: &slots,
                    operation_id: "op",
                },
            )
        };
        let first = resolve().await.unwrap();
        assert_eq!(first.digest, format!("docker.io/library/redis@{hash}"));
        fs::remove_file(root.path().join("present")).unwrap();
        assert_eq!(resolve().await.unwrap(), first);
        let pulls = fs::read_to_string(root.path().join("pulls")).unwrap();
        assert_eq!(
            pulls.lines().collect::<Vec<_>>(),
            vec!["redis:8", first.digest.as_str()]
        );
        fs::write(root.path().join("wrong-platform"), "").unwrap();
        assert_eq!(resolve().await.err(), Some(ImageError::PlatformMismatch));
        fs::remove_file(root.path().join("wrong-platform")).unwrap();
        fs::write(root.path().join("extra-volume"), "").unwrap();
        assert_eq!(resolve().await.err(), Some(ImageError::UncoveredVolume));
    }
}
