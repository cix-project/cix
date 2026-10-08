//! Small cooperative cancellation/deadline facade required by the shared
//! native arithmetic sources.  The portable container installs it around each
//! bounded block operation; it is not a preemptive execution or RSS limit.

use std::cell::Cell;
use std::time::{Duration, Instant};

pub const DEADLINE_ERROR: &str = "portable codec deadline exceeded";

thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

pub fn check() -> Result<(), String> {
    if DEADLINE.with(|slot| slot.get().is_some_and(|at| Instant::now() >= at)) {
        Err(DEADLINE_ERROR.into())
    } else {
        Ok(())
    }
}

/// Compatibility guard for the shared pure-Rust codec tests and bounded calls.
#[must_use]
pub struct DeadlineGuard {
    previous: Option<Instant>,
}

impl DeadlineGuard {
    pub fn new(duration: Duration) -> Self {
        let previous = DEADLINE.with(|slot| {
            let old = slot.get();
            let requested = Instant::now().checked_add(duration);
            slot.set(match (old, requested) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, b) => b,
            });
            old
        });
        Self { previous }
    }
}

impl Drop for DeadlineGuard {
    fn drop(&mut self) {
        DEADLINE.with(|slot| slot.set(self.previous));
    }
}
