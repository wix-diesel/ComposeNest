//! Non-sensitive recovery choices supplied by Core, never inferred by the UI.
use crate::{RequestContext, query_service::PortView};
use serde::{Deserialize, Serialize};

/// Fixed operation target for a fresh read-only recovery inspection.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryRequest {
    /// Validated transport context.
    pub context: RequestContext,
    /// Expected instance, checked against scope and operation.
    pub instance_id: String,
    /// Existing operation; rechecking never creates a replacement operation.
    pub operation_id: String,
}

/// Hash-only file change; file contents and credentials are never returned.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryFileDiff {
    /// Validated relative filename.
    pub path: String,
    /// Generated hash, absent for added files.
    pub recorded_hash: Option<String>,
    /// Current hash, absent for removed files.
    pub observed_hash: Option<String>,
}

/// Fresh evidence and permitted choices for one immutable operation identity.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryView {
    /// Stable target identity.
    pub instance_id: String,
    /// Existing operation identity.
    pub operation_id: String,
    /// Attempt observed by this inspection.
    pub attempt: u64,
    /// Current instance revision, checked on confirmation.
    pub instance_revision: u64,
    /// Current immutable candidate revision, checked on confirmation.
    pub candidate_revision: u64,
    /// Historical result; current runtime is a separate value.
    pub previous_status: String,
    /// Freshly verified runtime or unknown, without claiming operation success.
    pub current_runtime: String,
    /// Safe Core action identifiers.
    pub actions: Vec<String>,
    /// Non-sensitive reason codes explaining a hold.
    pub hold_reasons: Vec<String>,
    /// Complete currently confirmed bindings.
    pub ports: Vec<PortView>,
    /// Original bindings for an explicit old-port restoration.
    pub original_ports: Vec<PortView>,
    /// Proposed complete bindings, populated only by a separate proposal call.
    pub proposed_ports: Vec<PortView>,
    /// Externally modified generated artifact, if independently inspected.
    pub artifact_id: Option<String>,
    /// Exact observed file set hash required for explicit restoration.
    pub confirmation_hash: Option<String>,
    /// Hash-only file differences shown before restoration.
    pub files: Vec<RecoveryFileDiff>,
}
