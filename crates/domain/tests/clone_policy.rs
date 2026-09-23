use std::collections::VecDeque;

use composenest_domain::clone_policy::{
    Answer, EvaluatedField, FieldDefinition, FieldKind, Policy, PolicyError, RandomError,
    RandomSource, SecretRule, Value, generate_identity, generate_secret, named_volume,
};
use composenest_domain::identity::{InstanceId, SlotId};

struct FixedRandom {
    chunks: VecDeque<Vec<u8>>,
}

impl FixedRandom {
    fn new(chunks: impl IntoIterator<Item = Vec<u8>>) -> Self {
        Self {
            chunks: chunks.into_iter().collect(),
        }
    }
}

impl RandomSource for FixedRandom {
    fn fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), RandomError> {
        let chunk = self.chunks.pop_front().ok_or(RandomError)?;
        if chunk.len() != bytes.len() {
            return Err(RandomError);
        }
        bytes.copy_from_slice(&chunk);
        Ok(())
    }
}

fn definition(kind: FieldKind, required: bool, policy: Policy) -> FieldDefinition {
    FieldDefinition {
        kind,
        required,
        policy: Some(policy),
        secret_rule: (kind == FieldKind::Secret).then_some(SecretRule {
            min_length: 1,
            max_length: 4096,
            accepts_generator_alphabet: true,
        }),
    }
}

fn evaluate(definition: FieldDefinition, source: Option<Value>) -> EvaluatedField {
    EvaluatedField::evaluate(definition, source, &mut FixedRandom::new([]), &[]).unwrap()
}

#[test]
fn cp_t01_copy_preserves_empty_string_zero_and_false() {
    for (kind, value) in [
        (FieldKind::String, Value::String(String::new())),
        (FieldKind::Integer, Value::Integer(0)),
        (FieldKind::Boolean, Value::Boolean(false)),
    ] {
        let field = evaluate(definition(kind, true, Policy::Copy), Some(value.clone()));
        assert_eq!(field.candidate(), Some(&value));
        assert!(field.is_ready());
    }
}

#[test]
fn cp_t02_t03_clear_and_absent_copy_never_apply_a_default() {
    let mut required = evaluate(
        definition(FieldKind::String, true, Policy::Clear),
        Some(Value::String("source".into())),
    );
    assert_eq!(required.candidate(), None);
    assert!(!required.is_ready());
    required
        .answer(
            Answer::Input(Value::String("new".into())),
            &mut FixedRandom::new([]),
            &[],
        )
        .unwrap();
    assert!(required.is_ready());
    let optional = evaluate(definition(FieldKind::String, false, Policy::Copy), None);
    assert_eq!(optional.candidate(), None);
    assert!(optional.is_ready());
}

#[test]
fn cp_t04_ask_requires_an_explicit_answer() {
    let mut field = evaluate(
        definition(FieldKind::Boolean, true, Policy::Ask),
        Some(Value::Boolean(false)),
    );
    assert_eq!(field.candidate(), None);
    assert!(!field.is_ready());
    field
        .answer(Answer::Copy, &mut FixedRandom::new([]), &[])
        .unwrap();
    assert_eq!(field.candidate(), Some(&Value::Boolean(false)));
    assert!(field.is_ready());
}

#[test]
fn cp_t05_t06_reject_disallowed_policies() {
    assert_eq!(
        definition(FieldKind::Storage, true, Policy::Copy).effective_policy(),
        Err(PolicyError::NotAllowed)
    );
    assert_eq!(
        definition(FieldKind::Integer, true, Policy::Regenerate).effective_policy(),
        Err(PolicyError::NotAllowed)
    );
}

#[test]
fn cp_t07_t08_inherited_secret_requires_confirmation_even_if_typed() {
    let original = Value::Secret("same-secret".into());
    let mut field = evaluate(
        definition(FieldKind::Secret, true, Policy::Copy),
        Some(original.clone()),
    );
    assert!(field.needs_secret_confirmation());
    assert!(!field.is_ready());
    assert!(!format!("{field:?}").contains("same-secret"));
    field.confirm_secret_inheritance().unwrap();
    assert!(field.is_ready());
    field
        .answer(Answer::Input(original), &mut FixedRandom::new([]), &[])
        .unwrap();
    assert!(field.needs_secret_confirmation());
    assert!(!field.is_ready());
}

#[test]
fn cp_t09_secret_has_fixed_alphabet_and_retries_equality() {
    let repeated = "A".repeat(32);
    let secret = generate_secret(
        &mut FixedRandom::new([vec![0; 32], vec![63; 32]]),
        Some(&repeated),
        &[],
    )
    .unwrap();
    assert_eq!(secret, "-".repeat(32));
    assert_eq!(secret.len(), 32);
    assert!(
        secret
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    );
    assert_eq!(
        generate_secret(
            &mut FixedRandom::new([vec![0; 32], vec![1; 32]]),
            None,
            &[repeated.as_str()]
        )
        .unwrap(),
        "B".repeat(32)
    );
}

#[test]
fn cp_t10_generation_failure_has_no_fallback() {
    assert_eq!(
        generate_secret(&mut FixedRandom::new([]), None, &[]),
        Err(PolicyError::GenerationFailed)
    );
    assert_eq!(
        generate_secret(
            &mut FixedRandom::new(vec![vec![0; 32]; 8]),
            Some(&"A".repeat(32)),
            &[]
        ),
        Err(PolicyError::GenerationFailed)
    );
    assert_eq!(
        generate_identity(&mut FixedRandom::new(vec![vec![0; 16]; 8]), |_| Ok(false)),
        Err(PolicyError::GenerationFailed)
    );
    assert_eq!(
        generate_identity(&mut FixedRandom::new(vec![vec![0; 16]; 8]), |_| Err(
            PolicyError::AvailabilityUnknown
        )),
        Err(PolicyError::AvailabilityUnknown)
    );
}

#[test]
fn cp_t11_unrelated_plan_changes_leave_secret_candidate_stable() {
    let mut random = FixedRandom::new([vec![1; 32]]);
    let field = EvaluatedField::evaluate(
        definition(FieldKind::Secret, true, Policy::Regenerate),
        Some(Value::Secret("old".into())),
        &mut random,
        &[],
    )
    .unwrap();
    let candidate = field.candidate().cloned();
    let _other_field = evaluate(definition(FieldKind::String, true, Policy::Ask), None);
    assert_eq!(field.candidate(), candidate.as_ref());
    assert!(field.is_ready());
    assert!(random.chunks.is_empty());
}

#[test]
fn identity_and_storage_names_derive_from_the_same_candidate() {
    let mut random = FixedRandom::new([vec![0; 16], vec![1; 16]]);
    let id = generate_identity(&mut random, |id| Ok(id != InstanceId::from_u128(0))).unwrap();
    assert_eq!(id, InstanceId::from_u128(u128::from_be_bytes([1; 16])));
    assert_eq!(
        id.compose_project_name(),
        "cn-01010101010101010101010101010101"
    );
    assert_eq!(
        named_volume(id, &SlotId::parse("data").unwrap()),
        "cn-01010101010101010101010101010101-data"
    );
}

#[test]
fn incompatible_secret_generator_is_rejected() {
    let mut incompatible = definition(FieldKind::Secret, true, Policy::Regenerate);
    incompatible.secret_rule.as_mut().unwrap().max_length = 31;
    assert_eq!(
        incompatible.effective_policy(),
        Err(PolicyError::GeneratorIncompatible)
    );
}
