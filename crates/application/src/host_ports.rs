//! Deterministic, bounded host TCP port planning without persistent reservations.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant, SystemTime},
};

/// One host port slot in a clone or new instance plan.
#[derive(Debug, Clone)]
pub struct PortSlot {
    /// Stable ASCII slot key.
    pub key: String,
    /// Confirmed source port, if this is a clone.
    pub source: Option<u16>,
    /// Snapshot recommendation when the source has no port.
    pub recommended: Option<u16>,
    /// User-entered decimal text, checked before automatic candidates.
    pub explicit: Option<String>,
    /// Template lower bound, inclusive.
    pub min: u16,
    /// Template upper bound, inclusive.
    pub max: u16,
}

/// Why a port cannot be assigned or checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortReason {
    /// An active application reservation owns the port, even if stopped.
    Reserved,
    /// Another slot in this plan owns the port.
    Planned,
    /// Docker has published the port.
    Docker,
    /// The host OS cannot bind the port.
    Host,
    /// An external check could not be completed reliably.
    Unavailable,
    /// Input is outside the allowed range or is not decimal.
    InvalidInput,
    /// Two explicit inputs request the same port.
    DuplicateInput,
}

/// Result of a check performed at a specific instant.
#[derive(Debug, Clone)]
pub struct PortCheck {
    /// Availability within the checks performed, not a reservation.
    pub result: Result<(), PortReason>,
    /// Time at which this observation was made.
    pub observed_at: SystemTime,
}

/// Checks the active ledger, Docker publishing and host OS in the same collision scope.
pub trait PortInspector {
    /// Returns a checked port result; an unavailable backend must never report free.
    fn inspect(&self, port: u16) -> PortCheck;
}

/// State needed to resume a bounded scan in the same slot order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortCursor {
    /// Next unchecked candidate by automatic slot.
    pub next: BTreeMap<String, u32>,
    /// Earlier automatic selections; callers must revalidate on confirmation.
    pub selected: BTreeMap<String, u16>,
}

/// Outcome of one planning request.
#[derive(Debug, Clone)]
pub enum PortPlan {
    /// Every slot has a candidate, subject to rechecking on confirmation.
    Complete(BTreeMap<String, u16>),
    /// The budget expired; resume with this cursor and the same input.
    Incomplete(PortCursor),
    /// A slot has no automatic starting point or the allowed range is exhausted.
    NeedsInput(String),
    /// Explicit input is invalid or a required check is inconclusive.
    Rejected {
        /// Slot that needs correction or could not be checked.
        slot: String,
        /// Other explicit slot when both inputs requested the same port.
        conflicting_slot: Option<String>,
        /// Why the candidate could not be accepted.
        reason: PortReason,
        /// Observation with time when an external check was performed.
        check: Option<PortCheck>,
    },
}

/// Evaluates all explicit inputs first, then automatic slots in ASCII key order.
/// A preview does not bind or reserve a port. Recheck all slots before committing.
pub fn plan_ports(
    slots: &[PortSlot],
    inspector: &impl PortInspector,
    cursor: PortCursor,
) -> PortPlan {
    plan_ports_bounded(slots, inspector, cursor, Duration::from_secs(2))
}

/// Applies a remaining time budget after external observations have completed.
pub fn plan_ports_bounded(
    slots: &[PortSlot],
    inspector: &impl PortInspector,
    cursor: PortCursor,
    deadline: Duration,
) -> PortPlan {
    plan_with_budget(slots, inspector, cursor, 256, deadline)
}

fn plan_with_budget(
    slots: &[PortSlot],
    inspector: &impl PortInspector,
    mut cursor: PortCursor,
    limit: usize,
    deadline: Duration,
) -> PortPlan {
    let started = Instant::now();
    let mut ordered: Vec<_> = slots.iter().collect();
    ordered.sort_by(|a, b| a.key.cmp(&b.key));
    let mut planned = BTreeMap::new();
    let mut explicit_ports = BTreeMap::<u16, String>::new();
    for slot in &ordered {
        if !valid_slot(slot) || planned.contains_key(&slot.key) {
            return rejected(&slot.key, PortReason::InvalidInput, None);
        }
        planned.insert(slot.key.clone(), None);
        if let Some(input) = &slot.explicit {
            let Ok(port) = input.parse::<u16>() else {
                return rejected(&slot.key, PortReason::InvalidInput, None);
            };
            if port < slot.min || port > slot.max {
                return rejected(&slot.key, PortReason::InvalidInput, None);
            }
            if let Some(other) = explicit_ports.insert(port, slot.key.clone()) {
                return PortPlan::Rejected {
                    slot: slot.key.clone(),
                    conflicting_slot: Some(other),
                    reason: PortReason::DuplicateInput,
                    check: None,
                };
            }
            planned.insert(slot.key.clone(), Some(port));
        }
    }
    let explicit: BTreeSet<_> = explicit_ports.keys().copied().collect();
    // A resumed scan must not trust earlier selections if the plan has changed.
    cursor.selected.retain(|key, port| {
        ordered.iter().any(|slot| {
            slot.key == *key
                && slot.explicit.is_none()
                && *port >= slot.min
                && *port <= slot.max
                && slot.source.is_none_or(|source| *port > source)
        }) && !explicit.contains(port)
    });
    for slot in &ordered {
        if let Some(port) = planned[&slot.key] {
            if started.elapsed() >= deadline {
                return PortPlan::Incomplete(cursor);
            }
            let check = inspector.inspect(port);
            if let Err(reason) = check.result {
                return rejected(&slot.key, reason, Some(check));
            }
        }
    }
    let mut used = explicit;
    for (key, port) in cursor.selected.clone() {
        if started.elapsed() >= deadline {
            return PortPlan::Incomplete(cursor);
        }
        let check = inspector.inspect(port);
        match check.result {
            Ok(()) => {}
            Err(PortReason::Unavailable) => {
                return rejected(&key, PortReason::Unavailable, Some(check));
            }
            Err(_) => {
                cursor.selected.remove(&key);
                cursor
                    .next
                    .entry(key)
                    .and_modify(|next| *next = (*next).max(u32::from(port) + 1))
                    .or_insert(u32::from(port) + 1);
            }
        }
    }
    for (key, port) in &cursor.selected {
        if !used.insert(*port) {
            return rejected(key, PortReason::Planned, None);
        }
    }
    let mut attempts = 0;
    for slot in ordered {
        if slot.explicit.is_some() {
            continue;
        }
        if cursor.selected.contains_key(&slot.key) {
            continue;
        }
        let start = slot
            .source
            .map(|port| u32::from(port) + 1)
            .or_else(|| slot.recommended.map(u32::from));
        let Some(start) = start else {
            return PortPlan::NeedsInput(slot.key.clone());
        };
        let mut candidate = (*cursor.next.entry(slot.key.clone()).or_insert(start)).max(start);
        // Source + 1 may be beyond the allowed range; never wrap around.
        while candidate <= u32::from(slot.max) {
            if attempts >= limit || started.elapsed() >= deadline {
                cursor.next.insert(slot.key.clone(), candidate);
                return PortPlan::Incomplete(cursor);
            }
            let port = candidate as u16;
            candidate += 1;
            cursor.next.insert(slot.key.clone(), candidate);
            attempts += 1;
            if port < slot.min || used.contains(&port) {
                continue;
            }
            let check = inspector.inspect(port);
            match check.result {
                Ok(()) => {
                    used.insert(port);
                    cursor.selected.insert(slot.key.clone(), port);
                    break;
                }
                Err(PortReason::Unavailable) => {
                    return rejected(&slot.key, PortReason::Unavailable, Some(check));
                }
                Err(_) => continue,
            }
        }
        if !cursor.selected.contains_key(&slot.key) {
            return PortPlan::NeedsInput(slot.key.clone());
        }
    }
    for (key, port) in cursor.selected {
        planned.insert(key, Some(port));
    }
    if let Some((key, _)) = planned.iter().find(|(_, port)| port.is_none()) {
        return PortPlan::NeedsInput(key.clone());
    }
    PortPlan::Complete(
        planned
            .into_iter()
            .filter_map(|(key, port)| port.map(|port| (key, port)))
            .collect(),
    )
}

fn valid_slot(slot: &PortSlot) -> bool {
    let mut chars = slot.key.bytes();
    slot.key.len() <= 32
        && matches!(chars.next(), Some(b'a'..=b'z'))
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && slot.min >= 1024
        && slot.min <= slot.max
}

fn rejected(slot: &str, reason: PortReason, check: Option<PortCheck>) -> PortPlan {
    PortPlan::Rejected {
        slot: slot.to_owned(),
        conflicting_slot: None,
        reason,
        check,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        reserved: BTreeSet<u16>,
        unknown: bool,
    }
    impl PortInspector for Fake {
        fn inspect(&self, port: u16) -> PortCheck {
            PortCheck {
                result: if self.unknown {
                    Err(PortReason::Unavailable)
                } else if self.reserved.contains(&port) {
                    Err(PortReason::Reserved)
                } else {
                    Ok(())
                },
                observed_at: SystemTime::now(),
            }
        }
    }
    fn slot(key: &str, source: Option<u16>, explicit: Option<&str>) -> PortSlot {
        PortSlot {
            key: key.into(),
            source,
            recommended: Some(5432),
            explicit: explicit.map(str::to_owned),
            min: 1024,
            max: 65535,
        }
    }
    fn free() -> Fake {
        Fake {
            reserved: BTreeSet::new(),
            unknown: false,
        }
    }
    fn complete(plan: PortPlan) -> BTreeMap<String, u16> {
        let PortPlan::Complete(ports) = plan else {
            panic!("expected complete plan")
        };
        ports
    }

    #[test]
    fn stopped_source_reservation_and_external_conflict_advance_without_wrap() {
        let inspector = Fake {
            reserved: BTreeSet::from([5432, 5433]),
            unknown: false,
        };
        assert_eq!(
            complete(plan_ports(
                &[slot("db", Some(5432), None)],
                &inspector,
                PortCursor::default()
            ))["db"],
            5434
        );
        assert!(matches!(
            plan_ports(
                &[slot("db", Some(65535), None)],
                &free(),
                PortCursor::default()
            ),
            PortPlan::NeedsInput(_)
        ));
    }

    #[test]
    fn invalid_and_boundary_explicit_inputs() {
        for input in ["1023", "65536", "foo", "5432.0"] {
            assert!(matches!(
                plan_ports(
                    &[slot("db", None, Some(input))],
                    &free(),
                    PortCursor::default()
                ),
                PortPlan::Rejected {
                    reason: PortReason::InvalidInput,
                    ..
                }
            ));
        }
        for input in ["1024", "65535"] {
            assert_eq!(
                complete(plan_ports(
                    &[slot("db", None, Some(input))],
                    &free(),
                    PortCursor::default()
                ))["db"]
                    .to_string(),
                input
            );
        }
    }

    #[test]
    fn explicit_first_then_ascii_slots_and_duplicate_inputs() {
        let slots = [
            slot("z", None, None),
            slot("a", None, None),
            slot("manual", None, Some("5432")),
        ];
        let ports = complete(plan_ports(&slots, &free(), PortCursor::default()));
        assert_eq!(
            (ports["manual"], ports["a"], ports["z"]),
            (5432, 5433, 5434)
        );
        assert!(matches!(
            plan_ports(
                &[slot("a", None, Some("5432")), slot("z", None, Some("5432"))],
                &free(),
                PortCursor::default()
            ),
            PortPlan::Rejected {
                reason: PortReason::DuplicateInput,
                ..
            }
        ));
    }

    #[test]
    fn unknown_does_not_count_as_occupied() {
        let inspector = Fake {
            reserved: BTreeSet::new(),
            unknown: true,
        };
        assert!(matches!(
            plan_ports(&[slot("db", None, None)], &inspector, PortCursor::default()),
            PortPlan::Rejected {
                reason: PortReason::Unavailable,
                check: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn bounded_scan_resumes_at_exact_next_candidate() {
        let inspector = Fake {
            reserved: (5432..=5434).collect(),
            unknown: false,
        };
        let first = plan_with_budget(
            &[slot("db", None, None)],
            &inspector,
            PortCursor::default(),
            2,
            Duration::from_secs(2),
        );
        let PortPlan::Incomplete(cursor) = first else {
            panic!("expected incomplete scan")
        };
        assert_eq!(cursor.next["db"], 5434);
        assert_eq!(
            complete(plan_with_budget(
                &[slot("db", None, None)],
                &inspector,
                cursor,
                2,
                Duration::from_secs(2)
            ))["db"],
            5435
        );
    }

    #[test]
    fn preview_becomes_invalid_when_another_instance_reserves_explicit_port() {
        let initial = complete(plan_ports(
            &[slot("db", None, Some("5432"))],
            &free(),
            PortCursor::default(),
        ));
        assert_eq!(initial["db"], 5432);
        let changed = Fake {
            reserved: BTreeSet::from([5432]),
            unknown: false,
        };
        assert!(matches!(
            plan_ports(
                &[slot("db", None, Some("5432"))],
                &changed,
                PortCursor::default()
            ),
            PortPlan::Rejected {
                reason: PortReason::Reserved,
                ..
            }
        ));
    }

    #[test]
    fn newly_reserved_automatic_preview_is_replanned_before_confirmation() {
        let slots = [slot("db", Some(5432), None)];
        assert_eq!(
            complete(plan_ports(&slots, &free(), PortCursor::default()))["db"],
            5433
        );
        let changed = Fake {
            reserved: BTreeSet::from([5432, 5433]),
            unknown: false,
        };
        assert_eq!(
            complete(plan_ports(&slots, &changed, PortCursor::default()))["db"],
            5434
        );
    }
}
