//! Immutable image selection for one confirmed instance spec.

use std::collections::BTreeSet;

use crate::state_store::StoreConflict;

/// Image evidence committed separately from the requested spec reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageResolution {
    /// Confirmed instance ID.
    pub instance_id: String,
    /// Spec revision containing the original requested reference.
    pub spec_revision: u64,
    /// Original image reference, never rewritten to the resolved digest.
    pub requested: String,
    /// Repository-qualified digest used by generated artifacts.
    pub digest: String,
    /// Docker image ID observed for that digest.
    pub image_id: String,
    /// Linux platform observed for that digest.
    pub platform: String,
    /// Operation that first resolved this image.
    pub operation_id: String,
}

/// Durable image records used to prevent changing an already resolved tag.
pub trait ImageResolutionStore {
    /// Reads a committed resolution, if any.
    fn image_resolution(
        &self,
        instance_id: &str,
        revision: u64,
    ) -> Result<Option<ImageResolution>, StoreConflict>;
    /// Commits evidence only when the spec, target and operation match; never replaces it.
    fn record_image_resolution(&self, resolution: &ImageResolution) -> Result<(), StoreConflict>;
}

/// A rejected image or an indeterminate Docker operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    /// No trustworthy digest or image ID was observed.
    Unresolved,
    /// The image does not match the registered runtime platform.
    PlatformMismatch,
    /// An image-declared writable path is not covered by a template storage slot.
    UncoveredVolume,
    /// A saved digest now resolves to a different image ID.
    ImageChanged,
    /// Docker did not confirm completion within the allowed deadline.
    OutcomeUnknown,
    /// Docker or persistence rejected the operation.
    Unavailable,
}

/// Checks image metadata against the registered platform and complete storage destinations.
pub fn validate_image(
    image_id: &str,
    platform: &str,
    target_platform: &str,
    volumes: &[String],
    storage_destinations: &[String],
) -> Result<(), ImageError> {
    if !valid_hash(image_id) {
        return Err(ImageError::Unresolved);
    }
    if platform != target_platform {
        return Err(ImageError::PlatformMismatch);
    }
    let slots: BTreeSet<_> = storage_destinations.iter().collect();
    for volume in volumes {
        if !volume.starts_with('/')
            || volume == "/"
            || volume.split('/').any(|part| part == ".." || part == ".")
            || !slots.contains(volume)
        {
            return Err(ImageError::UncoveredVolume);
        }
    }
    Ok(())
}

/// Accepts only a full lowercase SHA-256 identifier.
pub fn valid_hash(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    })
}

/// Returns the pinned image reference only after a durable resolution exists.
pub fn artifact_image(resolution: Option<&ImageResolution>) -> Result<&str, ImageError> {
    let value = resolution.ok_or(ImageError::Unresolved)?;
    let (_, hash) = value
        .digest
        .rsplit_once('@')
        .ok_or(ImageError::Unresolved)?;
    if !valid_hash(hash) || !valid_hash(&value.image_id) {
        return Err(ImageError::Unresolved);
    }
    Ok(&value.digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_unresolved_image_and_uncovered_or_mismatched_volumes() {
        assert_eq!(artifact_image(None), Err(ImageError::Unresolved));
        let id = format!("sha256:{}", "a".repeat(64));
        assert_eq!(
            validate_image(&id, "linux/arm64", "linux/amd64", &[], &[]),
            Err(ImageError::PlatformMismatch)
        );
        assert_eq!(
            validate_image(
                &id,
                "linux/amd64",
                "linux/amd64",
                &["/var/data".into()],
                &[]
            ),
            Err(ImageError::UncoveredVolume)
        );
        assert!(
            validate_image(
                &id,
                "linux/amd64",
                "linux/amd64",
                &["/var/data".into()],
                &["/var/data".into()]
            )
            .is_ok()
        );
    }
}
