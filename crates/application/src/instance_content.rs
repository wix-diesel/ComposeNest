//! Explicit, scoped reads of sensitive instance content.

use crate::RequestContext;
use serde::{Deserialize, Serialize};

/// Requests one saved connection secret without accepting a value or filesystem path.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstanceSecretRequest {
    /// API version and response correlation.
    pub context: RequestContext,
    /// Target instance in the trusted backend scope.
    pub instance_id: String,
    /// Committed spec displayed by the caller.
    pub expected_spec_revision: u64,
    /// Secret input slot from a saved connection definition.
    pub slot: String,
}

/// Sensitive result returned only for an explicit reveal or copy request.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceSecretView {
    /// Validated target identity.
    pub instance_id: String,
    /// Validated committed spec revision.
    pub spec_revision: u64,
    /// Validated secret input slot.
    pub slot: String,
    /// Saved actual value, never a Compose-escaped value.
    pub value: String,
}

/// Requests the selected published Compose artifact, masked unless explicitly revealed.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstanceComposeRequest {
    /// API version and response correlation.
    pub context: RequestContext,
    /// Target instance in the trusted backend scope.
    pub instance_id: String,
    /// Committed spec displayed by the caller.
    pub expected_spec_revision: u64,
    /// Explicit permission to return the unmodified file contents.
    pub reveal: bool,
}

/// Actual artifact content and OS-resolved location; raw content is sensitive.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceComposeView {
    /// Validated target identity.
    pub instance_id: String,
    /// Validated committed spec revision.
    pub spec_revision: u64,
    /// Whether known secrets have been removed.
    pub masked: bool,
    /// Exact file text, with only known-secret substitutions when masked.
    pub content: String,
    /// Backend-resolved path for display, not an executable frontend path.
    pub path: String,
}
