use cix_native::resources::{MemoryBudget, MemoryError};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn concurrent_permits_never_exceed_budget_and_record_peak() {
    let budget = Arc::new(MemoryBudget::new(12));
    let start = Arc::new(Barrier::new(7));
    let active = Arc::new(AtomicUsize::new(0));
    let peak_active = Arc::new(AtomicUsize::new(0));
    let mut workers = Vec::new();

    for _ in 0..6 {
        let budget = Arc::clone(&budget);
        let start = Arc::clone(&start);
        let active = Arc::clone(&active);
        let peak_active = Arc::clone(&peak_active);
        workers.push(thread::spawn(move || {
            start.wait();
            for _ in 0..3 {
                let _permit = budget.acquire_uninterruptibly(3).unwrap();
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak_active.fetch_max(now, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(5));
                active.fetch_sub(1, Ordering::SeqCst);
            }
        }));
    }
    start.wait();
    for worker in workers {
        worker.join().unwrap();
    }

    assert!(peak_active.load(Ordering::SeqCst) <= 4);
    let stats = budget.stats();
    assert_eq!(stats.reserved_bytes, 0);
    assert_eq!(stats.used_bytes, 0);
    assert!(stats.peak_reserved_bytes <= 12);
    assert_eq!(stats.peak_reserved_bytes, stats.peak_used_bytes);
}

#[test]
fn waiting_admission_observes_cancellation_without_notification() {
    let budget = Arc::new(MemoryBudget::new(8));
    assert!(matches!(
        budget.acquire(1, None, || true),
        Err(MemoryError::Cancelled)
    ));
    let held = budget.acquire_uninterruptibly(8).unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let started = Arc::new(Barrier::new(2));
    let child_budget = Arc::clone(&budget);
    let child_cancelled = Arc::clone(&cancelled);
    let child_started = Arc::clone(&started);
    let waiter = thread::spawn(move || {
        child_started.wait();
        child_budget.acquire(1, None, || child_cancelled.load(Ordering::SeqCst))
    });

    started.wait();
    thread::sleep(Duration::from_millis(10));
    cancelled.store(true, Ordering::SeqCst);
    assert!(matches!(
        waiter.join().unwrap(),
        Err(MemoryError::Cancelled)
    ));
    drop(held);
}

#[test]
fn oversized_request_fails_without_waiting_or_charging() {
    let budget = MemoryBudget::new(7);
    assert!(matches!(
        budget.acquire_uninterruptibly(8),
        Err(MemoryError::Impossible {
            requested: 8,
            capacity: 7,
        })
    ));
    assert_eq!(budget.stats().reserved_bytes, 0);
    assert!(matches!(
        budget.try_acquire(8),
        Err(MemoryError::Impossible {
            requested: 8,
            capacity: 7,
        })
    ));
}

#[test]
fn permit_is_released_during_unwind() {
    let budget = MemoryBudget::new(10);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _permit = budget.acquire_uninterruptibly(7).unwrap();
        panic!("intentional unwind");
    }));
    assert!(result.is_err());
    assert_eq!(budget.stats().reserved_bytes, 0);
    let permit = budget.acquire_uninterruptibly(10).unwrap();
    assert_eq!(permit.bytes(), 10);
}

#[test]
fn deadline_expires_when_capacity_stays_held() {
    let budget = MemoryBudget::new(1);
    let _held = budget.acquire_uninterruptibly(1).unwrap();
    assert!(matches!(
        budget.acquire(1, Some(Instant::now() + Duration::from_millis(5)), || false),
        Err(MemoryError::DeadlineExceeded)
    ));
}
