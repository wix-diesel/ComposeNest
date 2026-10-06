//! Non-sensitive prerequisite observations for the desktop diagnosis screen.

use serde::Serialize;

/// One independently observed prerequisite with a stable status code.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCheck {
    /// Stable prerequisite identifier.
    pub name: String,
    /// Ready, missing, unavailable, unsupported, changed, or permission_denied.
    pub status: String,
    /// Parsed observed component version, when available.
    pub version: Option<String>,
}

/// A completed observation; current context names never prove target identity.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDiagnosis {
    /// Completion time in Unix seconds, independent of the browser clock.
    pub observed_at: u64,
    /// CLI, Compose, endpoint, Engine, Linux, architecture, and root observations.
    pub checks: Vec<DiagnosticCheck>,
    /// Fixed local endpoint used for Engine observation.
    pub endpoint: Option<String>,
    /// Current context name, shown only as informational metadata.
    pub context_name: Option<String>,
    /// Observed Engine OS and canonical architecture.
    pub platform: Option<String>,
    /// Observed stable Engine ID.
    pub engine_id: Option<String>,
    /// Persisted Engine ID, if a target is registered.
    pub registered_engine_id: Option<String>,
    /// Verified, changed, unverified, or unregistered target identity.
    pub target_status: String,
    /// Fixed management root; never accepts a frontend path.
    pub management_root: String,
}
