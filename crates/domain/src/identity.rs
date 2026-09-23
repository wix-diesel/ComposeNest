//! Stable identifiers and validated names used by instances.

use unicode_normalization::UnicodeNormalization;

/// Why a display name cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayNameError {
    /// The normalized name contains no characters.
    Empty,
    /// The normalized name exceeds 100 Unicode scalar values.
    TooLong,
    /// The normalized name contains a control character.
    ControlCharacter,
}

/// A display name after Unicode whitespace trimming and NFC normalization.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DisplayName(String);

impl DisplayName {
    /// Validates a user supplied name and returns its canonical value.
    pub fn parse(input: &str) -> Result<Self, DisplayNameError> {
        if input.chars().any(char::is_control) {
            return Err(DisplayNameError::ControlCharacter);
        }
        let normalized: String = input.trim().nfc().collect();
        if normalized.is_empty() {
            return Err(DisplayNameError::Empty);
        }
        if normalized.chars().count() > 100 {
            return Err(DisplayNameError::TooLong);
        }
        if normalized.chars().any(char::is_control) {
            return Err(DisplayNameError::ControlCharacter);
        }
        Ok(Self(normalized))
    }

    /// Returns the canonical spelling used for equality and uniqueness checks.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An externally allocated, immutable instance identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstanceId(u128);

impl InstanceId {
    /// Wraps an identifier allocated by the application layer.
    #[must_use]
    pub const fn from_u128(value: u128) -> Self {
        Self(value)
    }

    /// Returns the numeric identifier for persistence or transport.
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0
    }

    /// Derives the stable Compose project name independently of the display name.
    #[must_use]
    pub fn compose_project_name(self) -> String {
        format!("cn-{:032x}", self.0)
    }
}

/// A stable template slot key, independent of its label or list position.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SlotId(String);

impl SlotId {
    /// Accepts schema v1 keys: 1–32 lowercase ASCII letters, digits or hyphens.
    pub fn parse(input: &str) -> Result<Self, SlotIdError> {
        let bytes = input.as_bytes();
        if bytes.is_empty() || bytes.len() > 32 || !bytes[0].is_ascii_lowercase() {
            return Err(SlotIdError);
        }
        if !bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        {
            return Err(SlotIdError);
        }
        Ok(Self(input.to_owned()))
    }

    /// Returns the stable key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A slot key violates the template schema v1 identifier rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotIdError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_names_without_folding_distinct_spelling() {
        assert_eq!(
            DisplayName::parse(" \u{65}\u{301} ").unwrap(),
            DisplayName::parse("é").unwrap()
        );
        assert_ne!(
            DisplayName::parse("Database").unwrap(),
            DisplayName::parse("database").unwrap()
        );
        assert_ne!(
            DisplayName::parse("Ａ").unwrap(),
            DisplayName::parse("A").unwrap()
        );
        assert_eq!(DisplayName::parse(" 日本語 ").unwrap().as_str(), "日本語");
    }

    #[test]
    fn checks_name_boundaries_after_normalization() {
        assert_eq!(
            DisplayName::parse(" \u{3000}"),
            Err(DisplayNameError::Empty)
        );
        assert!(DisplayName::parse(&"界".repeat(100)).is_ok());
        assert_eq!(
            DisplayName::parse(&"界".repeat(101)),
            Err(DisplayNameError::TooLong)
        );
        assert_eq!(
            DisplayName::parse("a\nb"),
            Err(DisplayNameError::ControlCharacter)
        );
        assert_eq!(
            DisplayName::parse("a\0b"),
            Err(DisplayNameError::ControlCharacter)
        );
        assert_eq!(
            DisplayName::parse("name\n"),
            Err(DisplayNameError::ControlCharacter)
        );
    }

    #[test]
    fn stable_identifiers_do_not_depend_on_labels() {
        let id = InstanceId::from_u128(42);
        assert_eq!(
            id.compose_project_name(),
            "cn-0000000000000000000000000000002a"
        );
        assert_eq!(id.as_u128(), 42);
        assert!(SlotId::parse("data-1").is_ok());
        for invalid in [
            "",
            "Data",
            "1data",
            "data_name",
            "データ",
            "a23456789012345678901234567890123",
        ] {
            assert_eq!(SlotId::parse(invalid), Err(SlotIdError));
        }
    }
}
