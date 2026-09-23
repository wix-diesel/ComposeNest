//! Restricted Schema 1 template documents.

mod semantics;
mod structure;
mod validation;
mod yaml;

pub use semantics::{ResolvedTemplate, ResolvedVersion, TemplateWarning, resolve_template};
pub use structure::{TemplateManifest, VersionDefinition, parse_manifest, parse_version};
pub use yaml::{Node, Position, TemplateError, Value};
