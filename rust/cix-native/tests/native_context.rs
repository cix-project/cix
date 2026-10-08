use cix_native::core::{decode_buffer, encode_buffer, NativeError, NativeOptions, NativeProfile};
use cix_native::limits;
use std::sync::{atomic::AtomicBool, Arc};

fn options() -> NativeOptions {
    NativeOptions {
        profile: NativeProfile::Fast,
        workers: 2,
        ..NativeOptions::default()
    }
}

#[test]
fn buffer_api_preserves_bytes_and_enforces_output_limits() {
    let source: Vec<u8> = (0..131_100).map(|n| (n % 251) as u8).collect();
    let archive = encode_buffer(&source, &options()).unwrap();
    assert_eq!(decode_buffer(&archive, &options()).unwrap(), source);
    let mut limited = options();
    limited.output_limit = source.len() - 1;
    assert!(matches!(
        decode_buffer(&archive, &limited),
        Err(NativeError::OutputLimit)
    ));
    limited.output_limit = 1;
    assert!(matches!(
        encode_buffer(&source, &limited),
        Err(NativeError::OutputLimit)
    ));
}

#[test]
fn independent_calls_do_not_share_cancellation() {
    let cancelled = NativeOptions {
        cancellation: Some(Arc::new(AtomicBool::new(true))),
        ..options()
    };
    std::thread::scope(|scope| {
        let failing = scope.spawn(|| encode_buffer(b"cancelled operation", &cancelled));
        let working = scope.spawn(|| {
            let archive = encode_buffer(b"independent operation", &options()).unwrap();
            decode_buffer(&archive, &options()).unwrap()
        });
        assert!(failing.join().unwrap().is_err());
        assert_eq!(working.join().unwrap(), b"independent operation");
    });
    assert!(limits::check().is_ok());
}

#[test]
fn nested_internal_token_cannot_mask_callers_cancellation() {
    let _scope = limits::LibraryGuard::new();
    let _caller = limits::CancellationGuard::new(Arc::new(AtomicBool::new(true)));
    let _internal = limits::CancellationGuard::new(Arc::new(AtomicBool::new(false)));
    assert_eq!(limits::check().unwrap_err(), limits::CANCELLED_ERROR);
}

#[test]
fn pipeline_propagates_the_callers_context_to_worker_polls() {
    let _scope = limits::LibraryGuard::new();
    let _caller = limits::CancellationGuard::new(Arc::new(AtomicBool::new(true)));
    let mut pipeline =
        cix_native::parallel::OrderedPipeline::new(1, 1, |_: ()| limits::check()).unwrap();
    pipeline.try_submit(()).unwrap();
    pipeline.close();
    assert!(pipeline.recv_next().is_err());
}
