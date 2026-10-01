//! Concrete adapters used when assembling the desktop application.

#[cfg(test)]
extern crate self as composenest_adapters;

use std::time::{SystemTime, UNIX_EPOCH};

use composenest_application::Clock;
use composenest_domain::clone_policy::{RandomError, RandomSource};

#[cfg(windows)]
pub mod windows_management_root;

#[cfg(target_os = "macos")]
pub mod macos_management_root;

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

pub mod artifact_store;
pub mod create_projection;
pub mod create_stages;
pub mod create_state;
pub mod delete_stages;
pub mod delete_state;
pub mod docker_cli;
pub mod docker_create;
pub mod docker_delete;
pub mod docker_observation;
pub mod docker_target;
mod entities;
pub mod external_recovery_state;
pub mod host_ports;
pub mod image_resolution;
pub mod image_store;
pub mod lifecycle_stages;
pub mod lifecycle_state;
pub mod named_volumes;
pub mod operation_journal;
pub mod port_edit_stages;
pub mod port_edit_state;
pub mod port_recovery;
pub mod query_service;
pub mod retained_storage;
pub mod sqlite;
pub mod state_store;
pub mod storage;
pub mod template_package;

