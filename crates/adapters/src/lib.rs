//! Concrete adapters used when assembling the desktop application.

use std::time::{SystemTime, UNIX_EPOCH};

use composenest_application::Clock;
use composenest_domain::clone_policy::{RandomError, RandomSource};

#[cfg(windows)]
pub mod windows_management_root;

/// Reads the current time from the operating system.
pub struct SystemClock;

impl Clock for SystemClock {
    fn unix_seconds(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs())
    }
}

/// Supplies cryptographically secure bytes from the operating system.
pub struct SystemRandom;

impl RandomSource for SystemRandom {
    fn fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), RandomError> {
        getrandom::fill(bytes).map_err(|_| RandomError)
    }
}

pub mod sqlite;
