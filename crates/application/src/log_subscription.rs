//! Scoped pull-based log IPC; no command, path, endpoint or secret is accepted from the UI.
use crate::RequestContext;
use serde::{Deserialize, Serialize};

/// Starts logs for the committed configuration currently displayed by the caller.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubscribeLogsRequest {
    /// Correlation identity also used as the subscription ID for lost-response cleanup.
    pub context: RequestContext,
    /// Instance resolved inside the trusted management scope.
    pub instance_id: String,
    /// Saved committed spec displayed by the caller.
    pub expected_spec_revision: u64,
}
/// Reads or releases exactly one subscription owned by the requesting window.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogSubscriptionRequest {
    /// API version and response correlation.
    pub context: RequestContext,
    /// Identity assigned by the subscribe request, never a Docker resource ID.
    pub subscription_id: String,
}
/// Bounded masked memory snapshot; service log content is never persisted.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogsView {
    /// Subscription owned by this window.
    pub subscription_id: String,
    /// Scoped saved instance identity.
    pub instance_id: String,
    /// Saved committed spec revision.
    pub spec_revision: u64,
    /// Masked complete lines, bounded by both display budgets.
    pub lines: Vec<String>,
    /// Lines evicted from the buffer; earlier Docker history is outside the requested tail.
    pub dropped_lines: u64,
    /// Lines shortened to the per-line budget.
    pub truncated_lines: u64,
    /// Whether the log CLI finished; this does not establish container state.
    pub finished: bool,
    /// Safe read failure indicator without raw CLI output.
    pub failed: bool,
}
