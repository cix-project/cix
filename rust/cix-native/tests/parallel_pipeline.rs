use cix_native::parallel::{OrderedPipeline, PipelineError, Poll, TrySubmitError};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn completes_in_input_order_even_when_workers_finish_out_of_order() {
    let mut pipe = OrderedPipeline::new(3, 6, |input: u32| {
        thread::sleep(Duration::from_millis((5 - input % 5) as u64 * 3));
        Ok(input * 10)
    })
    .unwrap();
    for value in 0..6 {
        pipe.try_submit(value).unwrap();
    }
    pipe.close();
    let mut got = Vec::new();
    while let Some(value) = pipe.recv_next().unwrap() {
        got.push(value);
    }
    pipe.finish().unwrap();
    assert_eq!(got, vec![0, 10, 20, 30, 40, 50]);
}

#[test]
fn completed_reordered_results_still_consume_capacity() {
    let release = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicBool::new(false));
    let worker_release = Arc::clone(&release);
    let worker_started = Arc::clone(&started);
    let mut pipe = OrderedPipeline::new(1, 2, move |input| {
        worker_started.store(true, Ordering::Release);
        while !worker_release.load(Ordering::Acquire) {
            thread::yield_now();
        }
        Ok(input)
    })
    .unwrap();

    pipe.try_submit(1u8).unwrap();
    pipe.try_submit(2u8).unwrap();
    let until = Instant::now() + Duration::from_secs(1);
    while !started.load(Ordering::Acquire) && Instant::now() < until {
        thread::yield_now();
    }
    assert!(started.load(Ordering::Acquire));
    assert!(matches!(pipe.try_submit(3), Err(TrySubmitError::Full(3))));

    release.store(true, Ordering::Release);
    let until = Instant::now() + Duration::from_secs(1);
    while pipe.try_recv_next().unwrap() == Poll::Pending && Instant::now() < until {
        thread::yield_now();
    }
    // The first receive releases one slot; the second completed value remains.
    assert_eq!(pipe.outstanding(), 1);
    pipe.try_submit(3).unwrap();
    pipe.close();
    assert_eq!(pipe.recv_next().unwrap(), Some(2));
    assert_eq!(pipe.recv_next().unwrap(), Some(3));
    assert_eq!(pipe.recv_next().unwrap(), None);
}

#[test]
fn capacity_one_keeps_a_completed_result_admitted_until_it_is_received() {
    let completed = Arc::new(AtomicBool::new(false));
    let worker_completed = Arc::clone(&completed);
    let mut pipe = OrderedPipeline::new(1, 1, move |input: u8| {
        worker_completed.store(true, Ordering::Release);
        Ok(input)
    })
    .unwrap();
    assert_eq!(pipe.capacity(), 1);
    pipe.try_submit(7).unwrap();
    let until = Instant::now() + Duration::from_secs(1);
    while !completed.load(Ordering::Acquire) && Instant::now() < until {
        thread::yield_now();
    }
    assert!(completed.load(Ordering::Acquire));
    // The worker completed. Its result must still consume the only slot until
    // the caller receives it.
    assert!(matches!(pipe.try_submit(8), Err(TrySubmitError::Full(8))));
    assert_eq!(pipe.recv_next().unwrap(), Some(7));
    pipe.try_submit(8).unwrap();
    pipe.close();
    assert_eq!(pipe.recv_next().unwrap(), Some(8));
    assert_eq!(pipe.recv_next().unwrap(), None);
}

#[test]
fn timeout_does_not_require_a_reader_thread() {
    let mut pipe = OrderedPipeline::new(1, 1, |_| {
        thread::sleep(Duration::from_millis(80));
        Ok(9u8)
    })
    .unwrap();
    pipe.try_submit(()).unwrap();
    assert_eq!(
        pipe.recv_next_timeout(Duration::from_millis(1)).unwrap(),
        Poll::Pending
    );
    pipe.close();
    assert_eq!(pipe.recv_next().unwrap(), Some(9));
    assert_eq!(pipe.recv_next().unwrap(), None);
}

#[test]
fn worker_error_and_panic_cancel_pending_work_without_deadlock() {
    let mut error_pipe = OrderedPipeline::new(2, 4, |input: u8| {
        if input == 1 {
            Err("intentional error".into())
        } else {
            Ok(input)
        }
    })
    .unwrap();
    error_pipe.try_submit(1).unwrap();
    error_pipe.try_submit(2).unwrap();
    assert_eq!(
        error_pipe.recv_next_timeout(Duration::from_secs(1)),
        Err(PipelineError::Worker("intentional error".into()))
    );
    assert!(matches!(
        error_pipe.try_submit(3),
        Err(TrySubmitError::Cancelled(3))
    ));
    assert_eq!(
        error_pipe.finish(),
        Err(PipelineError::Worker("intentional error".into()))
    );

    let mut panic_pipe = OrderedPipeline::new(1, 2, |_| -> Result<(), String> {
        panic!("intentional panic")
    })
    .unwrap();
    panic_pipe.try_submit(()).unwrap();
    assert_eq!(
        panic_pipe.recv_next_timeout(Duration::from_secs(1)),
        Err(PipelineError::WorkerPanic)
    );
    // Drop joins a worker after a caught panic and cannot leave a waiter.
    drop(panic_pipe);
}

fn wait_for_flag(flag: &AtomicBool, message: &str) -> Result<(), String> {
    let until = Instant::now() + Duration::from_secs(1);
    while !flag.load(Ordering::Acquire) && Instant::now() < until {
        thread::yield_now();
    }
    if !flag.load(Ordering::Acquire) {
        return Err(message.into());
    }
    Ok(())
}

#[test]
fn worker_failure_signals_a_running_sibling_through_the_shared_token() {
    let token = Arc::new(AtomicBool::new(false));
    let sibling_started = Arc::new(AtomicBool::new(false));
    let sibling_saw_cancel = Arc::new(AtomicBool::new(false));
    let encoder_token = Arc::clone(&token);
    let encoder_started = Arc::clone(&sibling_started);
    let encoder_saw_cancel = Arc::clone(&sibling_saw_cancel);
    let mut pipe = OrderedPipeline::new_with_cancellation(2, 2, Arc::clone(&token), move |job| {
        if job == 0 {
            wait_for_flag(&encoder_started, "sibling never began")?;
            Err("intentional error".into())
        } else {
            encoder_started.store(true, Ordering::Release);
            wait_for_flag(&encoder_token, "cancellation token was not raised")?;
            encoder_saw_cancel.store(true, Ordering::Release);
            Ok(())
        }
    })
    .unwrap();
    pipe.try_submit(0).unwrap();
    pipe.try_submit(1).unwrap();
    assert_eq!(
        pipe.recv_next_timeout(Duration::from_secs(1)),
        Err(PipelineError::Worker("intentional error".into()))
    );
    assert!(token.load(Ordering::Acquire));
    assert_eq!(
        pipe.failure(),
        Some(PipelineError::Worker("intentional error".into()))
    );
    assert_eq!(
        pipe.finish(),
        Err(PipelineError::Worker("intentional error".into()))
    );
    assert!(sibling_saw_cancel.load(Ordering::Acquire));
}

#[test]
fn cancellation_drops_queued_jobs_and_joins_workers() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let mut pipe = OrderedPipeline::new(2, 8, move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(20));
        Ok(())
    })
    .unwrap();
    for _ in 0..8 {
        pipe.try_submit(()).unwrap();
    }
    pipe.cancel();
    assert!(calls.load(Ordering::SeqCst) <= 2);
    assert_eq!(pipe.try_recv_next(), Err(PipelineError::Cancelled));
}
