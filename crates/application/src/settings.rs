//! Non-sensitive desktop settings contracts; persistence stays in the scoped store.

use crate::{RequestContext, state_store::StorageMethod};
use serde::{Deserialize, Serialize};

/// Persisted default and the management root resolved by the host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsView {
    /// Default used only when preparing new environments.
    pub storage_method: StorageMethod,
    /// Fixed host path, never supplied or changed by the WebView.
    pub management_root: String,
}

/// Changes only the default for future new environments.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveSettingsRequest {
    /// Version and correlation metadata.
    pub context: RequestContext,
    /// Requested default; existing environments and clones are unaffected.
    pub storage_method: StorageMethod,
}
