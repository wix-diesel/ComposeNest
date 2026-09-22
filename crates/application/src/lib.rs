//! Application use cases and their dependency ports.

use composenest_domain::application_title;

/// Provides the current Unix time to application use cases.
pub trait Clock {
    /// Returns the current Unix time in whole seconds.
    fn unix_seconds(&self) -> u64;
}

/// Provides the data required by the initial desktop screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bootstrap {
    /// The product name to display.
    pub application_title: &'static str,
    /// The time at which the initial state was created.
    pub started_at_unix_seconds: u64,
}

/// Loads the initial application state without depending on a transport.
pub struct BootstrapService<C> {
    clock: C,
}

impl<C> BootstrapService<C>
where
    C: Clock,
{
    /// Creates the service with the supplied system clock port.
    #[must_use]
    pub const fn new(clock: C) -> Self {
        Self { clock }
    }

    /// Returns the initial state for a supported client.
    #[must_use]
    pub fn bootstrap(&self) -> Bootstrap {
        Bootstrap {
            application_title: application_title(),
            started_at_unix_seconds: self.clock.unix_seconds(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Bootstrap, BootstrapService, Clock};

    struct FixedClock;

    impl Clock for FixedClock {
        fn unix_seconds(&self) -> u64 {
            0
        }
    }

    #[test]
    fn returns_the_initial_application_state() {
        let service = BootstrapService::new(FixedClock);

        assert_eq!(
            service.bootstrap(),
            Bootstrap {
                application_title: "ComposeNest",
                started_at_unix_seconds: 0,
            }
        );
    }
}
