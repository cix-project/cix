use cix_native::{adaptive, limits, ppm, rank};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

#[test]
fn expired_deadline_stops_costly_encoders() {
    let data = vec![b'a'; 32 * 1024];
    let _guard = limits::DeadlineGuard::new(Duration::ZERO);
    assert_eq!(
        adaptive::encode_general(&data, 1, 8, &[], 256).unwrap_err(),
        limits::DEADLINE_ERROR
    );
    assert_eq!(
        rank::encode_type_class(&data).unwrap_err(),
        limits::DEADLINE_ERROR
    );
    assert_eq!(ppm::encode(&data, 3).unwrap_err(), limits::DEADLINE_ERROR);
}

#[test]
fn deadline_guard_restores_the_outer_scope() {
    assert!(limits::check().is_ok());
    let outer = limits::DeadlineGuard::new(Duration::from_secs(1));
    assert!(limits::check().is_ok());
    {
        let _inner = limits::DeadlineGuard::new(Duration::ZERO);
        assert_eq!(limits::check().unwrap_err(), limits::DEADLINE_ERROR);
    }
    assert!(limits::check().is_ok());
    drop(outer);
    assert!(limits::check().is_ok());
}

#[test]
fn nested_guard_cannot_extend_outer_deadline() {
    let _outer = limits::DeadlineGuard::new(Duration::ZERO);
    let _inner = limits::DeadlineGuard::new(Duration::from_secs(60));
    assert_eq!(limits::check().unwrap_err(), limits::DEADLINE_ERROR);
}

#[test]
fn pipeline_cancellation_is_thread_local_and_restored() {
    limits::INTERRUPTED.store(false, Ordering::Relaxed);
    let outer = Arc::new(AtomicBool::new(false));
    let inner = Arc::new(AtomicBool::new(false));
    let guard = limits::CancellationGuard::new(outer.clone());
    assert!(limits::check().is_ok());
    {
        let _inner_guard = limits::CancellationGuard::new(inner.clone());
        inner.store(true, Ordering::Relaxed);
        assert_eq!(limits::check().unwrap_err(), limits::CANCELLED_ERROR);
    }
    assert!(limits::check().is_ok());
    outer.store(true, Ordering::Relaxed);
    assert_eq!(limits::check().unwrap_err(), limits::CANCELLED_ERROR);
    drop(guard);
    assert!(limits::check().is_ok());
}

#[test]
fn no_limit_preserves_roundtrip() {
    limits::INTERRUPTED.store(false, Ordering::Relaxed);
    let data: Vec<u8> = (0..8192).map(|n| ((n * 17) ^ (n >> 3)) as u8).collect();
    let adaptive_blob = adaptive::encode_general(&data, 2, 8, &[], 256).unwrap();
    assert_eq!(
        adaptive::decode_general(&adaptive_blob, data.len(), &[], 256).unwrap(),
        data
    );
    let rank_blob = rank::encode_type_class(&data).unwrap();
    let mut pos = 0;
    assert_eq!(
        rank::decode_type_class(&rank_blob, &mut pos, data.len()).unwrap(),
        data
    );
    assert_eq!(pos, rank_blob.len());
    let ppm_blob = ppm::encode(&data, 3).unwrap();
    assert_eq!(ppm::decode(&ppm_blob, data.len()).unwrap(), data);
}
