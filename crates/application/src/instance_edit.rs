//! Masked desktop contracts for immutable settings and stopped-only port changes.
use crate::{RequestContext, instance_actions::InstanceActionView};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Changes only the published host ports of every existing slot.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditInstancePortsRequest {
    /// Stable context reused until acceptance is known.
    pub context: RequestContext,
    /// Target in the trusted local scope.
    pub instance_id: String,
    /// Instance revision explicitly confirmed by the user.
    pub expected_revision: u64,
    /// Committed spec revision explicitly confirmed by the user.
    pub expected_spec_revision: u64,
    /// Complete slot-to-host-port mapping; other connection properties are immutable.
    pub ports: BTreeMap<String, u16>,
}

/// Immutable initialization value with secrets masked at the persistence boundary.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditSettingView {
    /// Input slot from the private snapshot.
    pub slot: String,
    /// Whether the saved value is confidential.
    pub secret: bool,
    /// Non-secret saved value; secret values are never included.
    pub value: Option<Value>,
}

/// Original and proposed bindings with their durable reservation status.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditPortView {
    /// Stable template slot.
    pub slot: String,
    /// Immutable published address.
    pub host_ip: String,
    /// Immutable container port.
    pub container_port: u16,
    /// Port before the most recent port edit, or the current port.
    pub old_port: u16,
    /// Committed host port used by connection information.
    pub committed_port: u16,
    /// Candidate from the most recent edit, if any.
    pub candidate_port: Option<u16>,
    /// Actual old reservation status, including released.
    pub old_reservation: String,
    /// Actual candidate reservation status, if any.
    pub candidate_reservation: Option<String>,
}

/// Saved edit state; reading this view never claims a fresh Docker inspection.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceEditView {
    /// Core state and separately available name/lifecycle actions.
    pub state: InstanceActionView,
    /// Immutable template identity.
    pub template_id: String,
    /// Immutable selected service version.
    pub selected_version: String,
    /// Immutable storage method.
    pub storage_method: String,
    /// Committed configuration revision.
    pub spec_revision: u64,
    /// Last verified applied revision; this does not prove current external consistency.
    pub applied_spec_revision: Option<u64>,
    /// Immutable, masked initialization values.
    pub inputs: Vec<EditSettingView>,
    /// Original, committed and candidate ports with reservations.
    pub ports: Vec<EditPortView>,
    /// Whether saved state permits proposing a port edit; execution inspects Docker again.
    pub can_edit_ports: bool,
}
