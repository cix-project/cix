//! Shared admission control for memory charged by concurrent CIX work.
//!
//! Every buffer, candidate, and queued result should acquire a permit before
//! allocating the memory it estimates it will need.  A permit is charged until
//! it is dropped, including while a worker is blocked on ordered output.  This
//! bounds the sum of reserved working-memory estimates across workers.
//! It is not an exact process RSS limit: executable/library mappings, allocator
//! bookkeeping and allocations outside this admission layer are not charged.
//! Qualification measures total memory independently with a cgroup hard limit.

use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Maximum time an admission waiter sleeps before rechecking cancellation.
const CANCELLATION_POLL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    /// The requested reservation can never fit in this budget.
    Impossible { requested: usize, capacity: usize },
    /// The caller cancelled while waiting for currently-held permits.
    Cancelled,
    /// The caller's admission deadline passed while waiting.
    DeadlineExceeded,
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Impossible {
                requested,
                capacity,
            } => write!(
                f,
                "memory reservation of {requested} bytes exceeds the {capacity}-byte budget"
            ),
            Self::Cancelled => f.write_str("memory reservation cancelled while waiting"),
            Self::DeadlineExceeded => f.write_str("memory reservation deadline expired"),
        }
    }
}

impl std::error::Error for MemoryError {}

/// A snapshot suitable for `--explain` and benchmark accounting.
///
/// `used_bytes` currently equals `reserved_bytes`: estimates are charged at
/// admission rather than after allocation.  Both fields are retained so an
/// allocator-aware caller can later report actual use without weakening the
/// hard reservation invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryStats {
    pub capacity_bytes: usize,
    pub reserved_bytes: usize,
    pub used_bytes: usize,
    pub peak_reserved_bytes: usize,
    pub peak_used_bytes: usize,
}

#[derive(Debug)]
struct State {
    reserved_bytes: usize,
    used_bytes: usize,
    peak_reserved_bytes: usize,
    peak_used_bytes: usize,
}

#[derive(Debug)]
struct Shared {
    capacity_bytes: usize,
    state: Mutex<State>,
    released: Condvar,
}

/// A clonable, shared hard budget for estimated scratch memory.
#[derive(Clone, Debug)]
pub struct MemoryBudget {
    shared: Arc<Shared>,
}

impl MemoryBudget {
    pub fn new(capacity_bytes: usize) -> Self {
        Self {
            shared: Arc::new(Shared {
                capacity_bytes,
                state: Mutex::new(State {
                    reserved_bytes: 0,
                    used_bytes: 0,
                    peak_reserved_bytes: 0,
                    peak_used_bytes: 0,
                }),
                released: Condvar::new(),
            }),
        }
    }

    pub fn capacity_bytes(&self) -> usize {
        self.shared.capacity_bytes
    }

    pub fn stats(&self) -> MemoryStats {
        let state = lock_unpoisoned(&self.shared.state);
        MemoryStats {
            capacity_bytes: self.shared.capacity_bytes,
            reserved_bytes: state.reserved_bytes,
            used_bytes: state.used_bytes,
            peak_reserved_bytes: state.peak_reserved_bytes,
            peak_used_bytes: state.peak_used_bytes,
        }
    }

    /// Attempts immediate admission without waiting.
    ///
    /// `Ok(None)` means another permit currently occupies enough capacity;
    /// `Err(Impossible)` means it cannot fit even after all permits release.
    pub fn try_acquire(&self, bytes: usize) -> Result<Option<MemoryPermit>, MemoryError> {
        self.check_request(bytes)?;
        let mut state = lock_unpoisoned(&self.shared.state);
        if !fits(&state, self.shared.capacity_bytes, bytes) {
            return Ok(None);
        }
        reserve(&mut state, bytes);
        Ok(Some(MemoryPermit {
            shared: Arc::clone(&self.shared),
            bytes,
        }))
    }

    /// Waits for admission, checking `cancelled` at least every 50 ms.
    ///
    /// A deadline is an upper bound on the wait for a permit.  The closure is
    /// evaluated before every wait and after every timed wake-up, so callers
    /// need not arrange a condvar notification in order to interrupt a wait.
    pub fn acquire<F>(
        &self,
        bytes: usize,
        deadline: Option<Instant>,
        mut cancelled: F,
    ) -> Result<MemoryPermit, MemoryError>
    where
        F: FnMut() -> bool,
    {
        self.check_request(bytes)?;
        let mut state = lock_unpoisoned(&self.shared.state);
        loop {
            if cancelled() {
                return Err(MemoryError::Cancelled);
            }
            let wait_for = match deadline {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(MemoryError::DeadlineExceeded);
                    }
                    CANCELLATION_POLL.min(deadline.saturating_duration_since(now))
                }
                None => CANCELLATION_POLL,
            };
            if fits(&state, self.shared.capacity_bytes, bytes) {
                reserve(&mut state, bytes);
                return Ok(MemoryPermit {
                    shared: Arc::clone(&self.shared),
                    bytes,
                });
            }
            let (new_state, _) = wait_timeout_unpoisoned(&self.shared.released, state, wait_for);
            state = new_state;
        }
    }

    /// Waits for admission with no deadline or cancellation source.
    pub fn acquire_uninterruptibly(&self, bytes: usize) -> Result<MemoryPermit, MemoryError> {
        self.acquire(bytes, None, || false)
    }

    fn check_request(&self, bytes: usize) -> Result<(), MemoryError> {
        if bytes > self.shared.capacity_bytes {
            Err(MemoryError::Impossible {
                requested: bytes,
                capacity: self.shared.capacity_bytes,
            })
        } else {
            Ok(())
        }
    }
}

/// An RAII reservation. Dropping it always releases its bytes and wakes waiters.
#[must_use = "a memory permit must be retained for the lifetime of its allocation"]
#[derive(Debug)]
pub struct MemoryPermit {
    shared: Arc<Shared>,
    bytes: usize,
}

impl MemoryPermit {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for MemoryPermit {
    fn drop(&mut self) {
        let mut state = lock_unpoisoned(&self.shared.state);
        // These invariants are maintained exclusively by admission and this
        // destructor. Saturating arithmetic would conceal a double-release.
        state.reserved_bytes = state
            .reserved_bytes
            .checked_sub(self.bytes)
            .expect("memory permit reservation underflow");
        state.used_bytes = state
            .used_bytes
            .checked_sub(self.bytes)
            .expect("memory permit usage underflow");
        debug_assert_eq!(state.reserved_bytes, state.used_bytes);
        drop(state);
        self.shared.released.notify_all();
    }
}

fn fits(state: &State, capacity: usize, bytes: usize) -> bool {
    bytes <= capacity.saturating_sub(state.reserved_bytes)
}

fn reserve(state: &mut State, bytes: usize) {
    state.reserved_bytes = state
        .reserved_bytes
        .checked_add(bytes)
        .expect("memory permit reservation overflow");
    state.used_bytes = state
        .used_bytes
        .checked_add(bytes)
        .expect("memory permit usage overflow");
    state.peak_reserved_bytes = state.peak_reserved_bytes.max(state.reserved_bytes);
    state.peak_used_bytes = state.peak_used_bytes.max(state.used_bytes);
    debug_assert_eq!(state.reserved_bytes, state.used_bytes);
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn wait_timeout_unpoisoned<'a, T>(
    condvar: &Condvar,
    guard: std::sync::MutexGuard<'a, T>,
    timeout: Duration,
) -> (std::sync::MutexGuard<'a, T>, std::sync::WaitTimeoutResult) {
    match condvar.wait_timeout(guard, timeout) {
        Ok(result) => result,
        Err(poisoned) => poisoned.into_inner(),
    }
}
