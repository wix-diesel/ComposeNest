//! Restricted Schema 1 template documents.

mod structure;
mod validation;
mod yaml;

pub use structure::{TemplateManifest, VersionDefinition, parse_manifest, parse_version};
pub use yaml::{Node, Position, TemplateError, Value};
