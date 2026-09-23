//! macOS management root discovery for the host adapter.

use std::path::PathBuf;

/// Returns the fixed system-wide management root used on macOS.
///
/// Initial setup must create this directory for the selected non-administrator
/// user before the normal GUI creates or opens managed files.
#[must_use]
pub fn management_root() -> PathBuf {
    PathBuf::from("/Library/Application Support").join("ComposeNest")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::management_root;

    #[test]
    fn root_is_under_system_application_support() {
        assert_eq!(
            management_root(),
            Path::new("/Library/Application Support/ComposeNest")
        );
    }
}
