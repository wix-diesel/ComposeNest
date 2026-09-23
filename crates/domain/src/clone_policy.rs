//! Evaluation of schema v1 Clone policies and built-in candidates.

use std::fmt;

use crate::identity::{InstanceId, SlotId};

/// A source of cryptographically secure random bytes supplied by an adapter.
pub trait RandomSource {
    /// Fills the entire buffer or reports failure without a weaker fallback.
    fn fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), RandomError>;
}

/// Failure to obtain secure random bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomError;

/// A Clone policy supported by schema version 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Copy,
    Regenerate,
    NextAvailable,
    Clear,
    Ask,
}

/// The type and role of an input or Core-owned field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    String,
    Integer,
    Boolean,
    Select,
    Secret,
    DisplayName,
    HostPort,
    ContainerPort,
    InstanceId,
    Project,
    Storage,
    StorageMethod,
    ServiceVersion,
    Image,
    ServiceName,
}

impl FieldKind {
    /// Returns the fixed or default policy for this schema v1 field kind.
    #[must_use]
    pub const fn default_policy(self) -> Policy {
        match self {
            Self::String
            | Self::Integer
            | Self::Boolean
            | Self::Select
            | Self::ContainerPort
            | Self::StorageMethod
            | Self::ServiceVersion
            | Self::Image
            | Self::ServiceName => Policy::Copy,
            Self::Secret | Self::InstanceId | Self::Project | Self::Storage => Policy::Regenerate,
            Self::HostPort => Policy::NextAvailable,
            Self::DisplayName => Policy::Ask,
        }
    }

    /// Checks whether an explicit policy may be declared for this field.
    #[must_use]
    pub const fn permits(self, policy: Policy) -> bool {
        match self {
            Self::String | Self::Integer | Self::Boolean | Self::Select => {
                matches!(policy, Policy::Copy | Policy::Clear | Policy::Ask)
            }
            Self::Secret => matches!(
                policy,
                Policy::Copy | Policy::Regenerate | Policy::Clear | Policy::Ask
            ),
            Self::DisplayName => matches!(policy, Policy::Ask),
            Self::HostPort => matches!(policy, Policy::NextAvailable),
            Self::ContainerPort | Self::ServiceName => matches!(policy, Policy::Copy),
            Self::StorageMethod | Self::ServiceVersion => {
                matches!(policy, Policy::Copy | Policy::Ask)
            }
            Self::InstanceId | Self::Project | Self::Storage => {
                matches!(policy, Policy::Regenerate)
            }
            Self::Image => false,
        }
    }
}

/// A typed input value. Secret debug output never contains its contents.
#[derive(Clone, PartialEq, Eq)]
pub enum Value {
    String(String),
    Integer(i64),
    Boolean(bool),
    Select(String),
    Secret(String),
}

impl fmt::Debug for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Secret(_) => formatter.write_str("Secret([redacted])"),
            Self::String(value) => formatter.debug_tuple("String").field(value).finish(),
            Self::Integer(value) => formatter.debug_tuple("Integer").field(value).finish(),
            Self::Boolean(value) => formatter.debug_tuple("Boolean").field(value).finish(),
            Self::Select(value) => formatter.debug_tuple("Select").field(value).finish(),
        }
    }
}

/// Length and character-set compatibility for secret-v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretRule {
    /// Minimum accepted number of characters.
    pub min_length: usize,
    /// Maximum accepted number of characters.
    pub max_length: usize,
    /// Whether all 64 generator characters are accepted without a pattern.
    pub accepts_generator_alphabet: bool,
}

impl SecretRule {
    /// Returns whether secret-v1's fixed output can satisfy this rule.
    #[must_use]
    pub const fn accepts_generator(self) -> bool {
        self.min_length <= 32 && self.max_length >= 32 && self.accepts_generator_alphabet
    }

    fn accepts_input(self, value: &str) -> bool {
        let length = value.chars().count();
        length >= self.min_length && length <= self.max_length
    }
}

/// A normalized field definition from the source template snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDefinition {
    /// The field's type and semantic role.
    pub kind: FieldKind,
    /// Whether a value is required to complete the plan.
    pub required: bool,
    /// Explicit policy, or the schema v1 default when absent.
    pub policy: Option<Policy>,
    /// Secret validation, required for a secret field.
    pub secret_rule: Option<SecretRule>,
}

impl FieldDefinition {
    /// Validates the declaration and returns its effective policy.
    pub fn effective_policy(&self) -> Result<Policy, PolicyError> {
        let policy = self.policy.unwrap_or(self.kind.default_policy());
        if self.policy.is_some() && !self.kind.permits(policy) {
            return Err(PolicyError::NotAllowed);
        }
        if self.kind == FieldKind::Secret {
            let rule = self.secret_rule.ok_or(PolicyError::InvalidDefinition)?;
            if rule.min_length > rule.max_length {
                return Err(PolicyError::InvalidDefinition);
            }
            if policy == Policy::Regenerate && !rule.accepts_generator() {
                return Err(PolicyError::GeneratorIncompatible);
            }
        } else if self.secret_rule.is_some() {
            return Err(PolicyError::InvalidDefinition);
        }
        Ok(policy)
    }

    fn accepts(&self, value: &Value) -> bool {
        match (self.kind, value) {
            (FieldKind::String, Value::String(_))
            | (FieldKind::Integer, Value::Integer(_))
            | (FieldKind::Boolean, Value::Boolean(_))
            | (FieldKind::Select, Value::Select(_)) => true,
            (FieldKind::Secret, Value::Secret(secret)) => self
                .secret_rule
                .is_some_and(|rule| rule.accepts_input(secret)),
            _ => false,
        }
    }
}

/// A stable reason for an invalid policy evaluation, without secret values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyError {
    NotAllowed,
    InvalidDefinition,
    GeneratorIncompatible,
    InvalidValue,
    InvalidAnswer,
    GenerationFailed,
    AvailabilityUnknown,
}

/// An explicit answer to an `ask` field or a permitted field edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Copy,
    Generate,
    Input(Value),
    Clear,
}

/// One evaluated field, retaining its candidate across unrelated plan edits.
#[derive(Clone, PartialEq, Eq)]
pub struct EvaluatedField {
    definition: FieldDefinition,
    source: Option<Value>,
    candidate: Option<Value>,
    answered: bool,
    inheritance_confirmed: bool,
}

impl fmt::Debug for EvaluatedField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EvaluatedField")
            .field("definition", &self.definition)
            .field("source", &self.source)
            .field("candidate", &self.candidate)
            .field("answered", &self.answered)
            .field("inheritance_confirmed", &self.inheritance_confirmed)
            .finish()
    }
}

impl EvaluatedField {
    /// Evaluates copy, clear or ask without applying new-instance defaults.
    pub fn evaluate(
        definition: FieldDefinition,
        source: Option<Value>,
        random: &mut impl RandomSource,
        other_generated: &[&str],
    ) -> Result<Self, PolicyError> {
        let policy = definition.effective_policy()?;
        if !matches!(
            definition.kind,
            FieldKind::String
                | FieldKind::Integer
                | FieldKind::Boolean
                | FieldKind::Select
                | FieldKind::Secret
        ) {
            return Err(PolicyError::NotAllowed);
        }
        let valid_source = source
            .as_ref()
            .is_none_or(|value| definition.accepts(value));
        let candidate = match policy {
            Policy::Copy if valid_source => source.clone(),
            Policy::Regenerate => {
                let previous = source.as_ref().and_then(secret_text);
                Some(Value::Secret(generate_secret(
                    random,
                    previous,
                    other_generated,
                )?))
            }
            _ => None,
        };
        Ok(Self {
            definition,
            source,
            candidate,
            answered: policy != Policy::Ask && (policy != Policy::Copy || valid_source),
            inheritance_confirmed: false,
        })
    }

    /// Returns the current value, including empty strings, zero and false.
    #[must_use]
    pub fn candidate(&self) -> Option<&Value> {
        self.candidate.as_ref()
    }

    /// Returns whether this field can be used in a confirmed plan.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.answered
            && (!self.definition.required || self.candidate.is_some())
            && (!self.inherits_secret() || self.inheritance_confirmed)
    }

    /// Returns whether the source secret is being reused without confirmation.
    #[must_use]
    pub fn needs_secret_confirmation(&self) -> bool {
        self.inherits_secret() && !self.inheritance_confirmed
    }

    /// Applies a user answer; unrelated fields and candidates are not modified.
    pub fn answer(
        &mut self,
        answer: Answer,
        random: &mut impl RandomSource,
        other_generated: &[&str],
    ) -> Result<(), PolicyError> {
        let candidate = match answer {
            Answer::Copy
                if self
                    .source
                    .as_ref()
                    .is_some_and(|value| self.definition.accepts(value)) =>
            {
                self.source.clone()
            }
            Answer::Generate
                if self.definition.kind == FieldKind::Secret
                    && self
                        .definition
                        .secret_rule
                        .is_some_and(SecretRule::accepts_generator) =>
            {
                let previous = self.source.as_ref().and_then(secret_text);
                Some(Value::Secret(generate_secret(
                    random,
                    previous,
                    other_generated,
                )?))
            }
            Answer::Input(value) if self.definition.accepts(&value) => Some(value),
            Answer::Clear if !self.definition.required => None,
            _ => return Err(PolicyError::InvalidAnswer),
        };
        self.candidate = candidate;
        self.answered = true;
        self.inheritance_confirmed = false;
        Ok(())
    }

    /// Records explicit confirmation for the current inherited secret value.
    pub fn confirm_secret_inheritance(&mut self) -> Result<(), PolicyError> {
        if !self.inherits_secret() {
            return Err(PolicyError::InvalidAnswer);
        }
        self.inheritance_confirmed = true;
        Ok(())
    }

    fn inherits_secret(&self) -> bool {
        self.definition.kind == FieldKind::Secret
            && self.candidate.is_some()
            && self.candidate == self.source
    }
}

fn secret_text(value: &Value) -> Option<&str> {
    match value {
        Value::Secret(value) => Some(value),
        _ => None,
    }
}

const SECRET_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";
const MAX_CANDIDATES: usize = 8;

/// Generates a 32-character secret, retrying only accidental equality.
pub fn generate_secret(
    random: &mut impl RandomSource,
    source: Option<&str>,
    other_generated: &[&str],
) -> Result<String, PolicyError> {
    for _ in 0..MAX_CANDIDATES {
        let mut bytes = [0_u8; 32];
        random
            .fill_bytes(&mut bytes)
            .map_err(|RandomError| PolicyError::GenerationFailed)?;
        let candidate: String = bytes
            .iter()
            .map(|byte| SECRET_ALPHABET[(byte & 63) as usize] as char)
            .collect();
        if source != Some(candidate.as_str()) && !other_generated.contains(&candidate.as_str()) {
            return Ok(candidate);
        }
    }
    Err(PolicyError::GenerationFailed)
}

/// Generates a 128-bit identity after the caller checks ID, project and storage availability.
/// An unknown availability result stops evaluation instead of being treated as a collision.
pub fn generate_identity(
    random: &mut impl RandomSource,
    mut is_available: impl FnMut(InstanceId) -> Result<bool, PolicyError>,
) -> Result<InstanceId, PolicyError> {
    for _ in 0..MAX_CANDIDATES {
        let mut bytes = [0_u8; 16];
        random
            .fill_bytes(&mut bytes)
            .map_err(|RandomError| PolicyError::GenerationFailed)?;
        let candidate = InstanceId::from_u128(u128::from_be_bytes(bytes));
        if is_available(candidate)? {
            return Ok(candidate);
        }
    }
    Err(PolicyError::GenerationFailed)
}

/// Derives a named volume candidate from the new instance and stable slot ID.
#[must_use]
pub fn named_volume(id: InstanceId, slot: &SlotId) -> String {
    format!("{}-{}", id.compose_project_name(), slot.as_str())
}
