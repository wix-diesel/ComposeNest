//! Concrete adapters used when assembling the desktop application.

use std::time::{SystemTime, UNIX_EPOCH};

use composenest_application::Clock;

/// Reads the current time from the operating system.
pub struct SystemClock;

impl Clock for SystemClock {
    fn unix_seconds(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs())
    }
}
