//! Cooperative cancellation for candidate work.
//!
//! The flag is process-wide because Ctrl-C is process-wide.  Candidate
//! deadlines are deliberately thread-local: an expensive worker must not
//! cancel unrelated decoding or output work on another thread.
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Set by the CLI's interruption handler.  Workers only observe it at safe
/// codec polling points and return an error before publishing a partial frame.
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub const INTERRUPTED_ERROR: &str = "CIX interrupted";
pub const CANCELLED_ERROR: &str = "CIX candidate cancelled";
pub const DEADLINE_ERROR: &str = "CIX candidate deadline exceeded";
pub fn is_library_scope() -> bool {
    LIBRARY_DEPTH.with(|slot| slot.get() > 0)
}
pub fn cli_interrupted() -> bool {
    !is_library_scope() && INTERRUPTED.load(Ordering::Relaxed)
}

thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
    static CANCELLATION: RefCell<Vec<Arc<AtomicBool>>> = const { RefCell::new(Vec::new()) };
    static LIBRARY_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Capturable per-context state for worker-thread propagation.  CLI signal
/// state is deliberately absent: library contexts never inherit it.
#[derive(Clone)]
pub struct Context {
    cancellation: Vec<Arc<AtomicBool>>,
    deadline: Option<Instant>,
    library: bool,
}
pub fn capture_context() -> Context {
    Context {
        cancellation: CANCELLATION.with(|s| s.borrow().clone()),
        deadline: DEADLINE.with(Cell::get),
        library: is_library_scope(),
    }
}
pub struct ContextGuard {
    cancellation: Vec<Arc<AtomicBool>>,
    deadline: Option<Instant>,
    depth: u32,
}
pub fn install_context(context: &Context) -> ContextGuard {
    let cancellation = CANCELLATION.with(|s| s.replace(context.cancellation.clone()));
    let deadline = DEADLINE.with(|s| {
        let old = s.get();
        s.set(context.deadline);
        old
    });
    let depth = LIBRARY_DEPTH.with(|s| {
        let old = s.get();
        if context.library {
            s.set(old.saturating_add(1));
        }
        old
    });
    ContextGuard {
        cancellation,
        deadline,
        depth,
    }
}
impl Drop for ContextGuard {
    fn drop(&mut self) {
        CANCELLATION.with(|s| {
            s.replace(std::mem::take(&mut self.cancellation));
        });
        DEADLINE.with(|s| s.set(self.deadline));
        LIBRARY_DEPTH.with(|s| s.set(self.depth));
    }
}
pub struct LibraryGuard {
    previous: u32,
}
impl Default for LibraryGuard {
    fn default() -> Self {
        Self::new()
    }
}
impl LibraryGuard {
    pub fn new() -> Self {
        let previous = LIBRARY_DEPTH.with(|s| {
            let old = s.get();
            s.set(old.saturating_add(1));
            old
        });
        Self { previous }
    }
}
impl Drop for LibraryGuard {
    fn drop(&mut self) {
        LIBRARY_DEPTH.with(|s| s.set(self.previous));
    }
}

/// Installs a pipeline-local cancellation token for this thread.  A worker
/// failure can set that token to stop its peers without pretending that the
/// user interrupted the whole process.
#[must_use]
pub struct CancellationGuard {
    // Keeps the installed token alive even if the caller drops its Arc while
    // the guarded operation is still running.
    _current: Arc<AtomicBool>,
    previous: Vec<Arc<AtomicBool>>,
}

impl CancellationGuard {
    pub fn new(token: Arc<AtomicBool>) -> Self {
        let previous = CANCELLATION.with(|slot| {
            let old = slot.borrow().clone();
            slot.borrow_mut().push(token.clone());
            old
        });
        Self {
            _current: token,
            previous,
        }
    }
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        CANCELLATION.with(|slot| {
            slot.replace(std::mem::take(&mut self.previous));
        });
    }
}

/// Installs a deadline for the current thread and restores the prior one when
/// dropped.  Nested candidate searches therefore cannot leak their budget to
/// their caller.
#[must_use]
pub struct DeadlineGuard {
    previous: Option<Instant>,
}

impl DeadlineGuard {
    pub fn new(duration: Duration) -> Self {
        let previous = DEADLINE.with(|slot| {
            let old = slot.get();
            // A nested scope may narrow a budget but cannot evade an outer
            // deadline by asking for more time.
            let requested = Instant::now().checked_add(duration);
            let deadline = match (old, requested) {
                (Some(outer), Some(inner)) => Some(outer.min(inner)),
                (Some(outer), None) => Some(outer),
                (None, requested) => requested,
            };
            slot.set(deadline);
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

/// Returns a stable error for cancellation or expiry.  Cancellation has
/// priority so Ctrl-C is never reported as an ordinary candidate timeout.
#[inline]
pub fn check() -> Result<(), String> {
    if cli_interrupted() {
        return Err(INTERRUPTED_ERROR.into());
    }
    let cancelled = CANCELLATION.with(|slot| {
        slot.borrow()
            .iter()
            .any(|token| token.load(Ordering::Relaxed))
    });
    if cancelled {
        return Err(CANCELLED_ERROR.into());
    }
    let expired = DEADLINE.with(|slot| slot.get().is_some_and(|at| Instant::now() >= at));
    if expired {
        Err(DEADLINE_ERROR.into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn library_scope_nests_and_restores_without_global_mutation() {
        assert!(check().is_ok());
        {
            let _one = LibraryGuard::new();
            assert!(check().is_ok());
            {
                let _two = LibraryGuard::new();
                assert!(check().is_ok());
            }
            assert!(check().is_ok());
        }
        assert!(check().is_ok());
    }
    #[test]
    fn contexts_are_independent_and_nested_scope_restores() {
        let cancelled = Arc::new(AtomicBool::new(true));
        let context = Context {
            cancellation: vec![cancelled],
            deadline: None,
            library: true,
        };
        assert!(check().is_ok());
        {
            let _one = install_context(&context);
            assert!(check().is_err());
            {
                let _two = LibraryGuard::new();
                assert!(check().is_err());
            }
        }
        assert!(check().is_ok());
    }
}
