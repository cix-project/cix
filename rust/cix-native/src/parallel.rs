//! A bounded, ordered worker pipeline for independently encodable blocks.
//!
//! The caller owns input and output: it reads a block once, submits its owned
//! buffer here, and writes values returned by [`OrderedPipeline::recv_next`].
//! A block occupies one of `capacity` slots from submission until the caller
//! consumes its ordered result.  Consequently both queued input and completed
//! out-of-order results are bounded.  This module deliberately has no nested
//! worker pools; the supplied encoder runs once per worker job.

use super::limits;
use std::collections::{BTreeMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// A result from a non-blocking or timed ordered receive.
#[derive(Debug, PartialEq, Eq)]
pub enum Poll<R> {
    /// The next input block completed and may be written.
    Item(R),
    /// Work exists, but the next input block has not completed yet.
    Pending,
    /// Input was closed and every submitted block was delivered.
    Finished,
}

/// A failed worker makes the pipeline unusable.  Worker errors deliberately
/// use strings so an encoder can expose its existing diagnostic without
/// imposing an error trait on the surrounding CLI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineError {
    Worker(String),
    WorkerPanic,
    Cancelled,
}

/// Why an owned job could not be accepted by [`OrderedPipeline::try_submit`].
#[derive(Debug, PartialEq, Eq)]
pub enum TrySubmitError<T> {
    /// All slots are occupied, including completed results waiting for order.
    Full(T),
    /// The caller closed input with [`OrderedPipeline::close`].
    Closed(T),
    /// A worker failed or the caller cancelled the pipeline.
    Cancelled(T),
}

struct State<T, R> {
    queue: VecDeque<(u64, T)>,
    results: BTreeMap<u64, R>,
    /// Submitted values not yet returned to the caller.  This is the memory
    /// admission counter, rather than merely the queue length.
    outstanding: usize,
    active: usize,
    input_closed: bool,
    cancelled: bool,
    failure: Option<PipelineError>,
}

struct Shared<T, R> {
    state: Mutex<State<T, R>>,
    wake: Condvar,
    capacity: usize,
    cancellation: Arc<AtomicBool>,
}

/// Persistent bounded workers which return completed blocks in input order.
///
/// `capacity` is a count of whole owned jobs/results.  Size based admission is
/// intentionally left to the caller, which knows its chosen block size and
/// total memory budget.  The invariant is therefore suitable for a CIX stream
/// reader: choose `capacity * block_size` within its budget, then hold no
/// additional unbounded reorder queue.
pub struct OrderedPipeline<T: Send + 'static, R: Send + 'static> {
    shared: Arc<Shared<T, R>>,
    workers: Vec<JoinHandle<()>>,
    next_submit: u64,
    next_receive: u64,
}

impl<T: Send + 'static, R: Send + 'static> OrderedPipeline<T, R> {
    /// Starts persistent workers.  `workers` and `capacity` must both be
    /// non-zero.  The encoder is called on exactly one worker for each job.
    pub fn new<F>(workers: usize, capacity: usize, encode: F) -> Result<Self, &'static str>
    where
        F: Fn(T) -> Result<R, String> + Send + Sync + 'static,
    {
        Self::new_with_cancellation(workers, capacity, Arc::new(AtomicBool::new(false)), encode)
    }

    /// Starts workers using `cancellation` as a shared stop signal for the
    /// caller's codec loops and memory-admission guards. The pipeline sets it
    /// before clearing work on cancellation, a worker error, or a panic.
    pub fn new_with_cancellation<F>(
        workers: usize,
        capacity: usize,
        cancellation: Arc<AtomicBool>,
        encode: F,
    ) -> Result<Self, &'static str>
    where
        F: Fn(T) -> Result<R, String> + Send + Sync + 'static,
    {
        if workers == 0 {
            return Err("worker count must be non-zero");
        }
        if capacity == 0 {
            return Err("pipeline capacity must be non-zero");
        }

        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                results: BTreeMap::new(),
                outstanding: 0,
                active: 0,
                input_closed: false,
                cancelled: false,
                failure: None,
            }),
            wake: Condvar::new(),
            capacity,
            cancellation,
        });
        let encode = Arc::new(encode);
        let context = limits::capture_context();
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let worker_shared = Arc::clone(&shared);
            let worker_encode = Arc::clone(&encode);
            let worker_context = context.clone();
            match thread::Builder::new().spawn(move || {
                let _scope = limits::install_context(&worker_context);
                worker_loop(worker_shared, worker_encode)
            }) {
                Ok(handle) => handles.push(handle),
                Err(_) => {
                    {
                        let mut state = shared.state.lock().expect("pipeline mutex poisoned");
                        shared.cancellation.store(true, Ordering::Release);
                        cancel_locked(&mut state);
                        shared.wake.notify_all();
                    }
                    for worker in handles {
                        let _ = worker.join();
                    }
                    return Err("unable to start pipeline worker");
                }
            }
        }
        Ok(Self {
            shared,
            workers: handles,
            next_submit: 0,
            next_receive: 0,
        })
    }

    /// Maximum number of submitted blocks, including reordered completed
    /// results, that may be retained at once.
    pub fn capacity(&self) -> usize {
        self.shared.capacity
    }

    /// Number of submitted blocks which still occupy a bounded slot.
    pub fn outstanding(&self) -> usize {
        self.shared
            .state
            .lock()
            .expect("pipeline mutex poisoned")
            .outstanding
    }

    /// Returns the terminal worker failure, or cancellation when the shared
    /// token was raised by the caller. It never consumes the diagnostic.
    pub fn failure(&self) -> Option<PipelineError> {
        let state = self.shared.state.lock().expect("pipeline mutex poisoned");
        state.failure.clone().or_else(|| {
            self.shared
                .cancellation
                .load(Ordering::Acquire)
                .then_some(PipelineError::Cancelled)
        })
    }

    /// Attempts to submit without waiting.  The returned job remains owned by
    /// the caller when capacity or state prevents submission.
    pub fn try_submit(&mut self, job: T) -> Result<(), TrySubmitError<T>> {
        let mut state = self.shared.state.lock().expect("pipeline mutex poisoned");
        cancel_if_requested_locked(&mut state, &self.shared.cancellation);
        if state.cancelled || state.failure.is_some() {
            return Err(TrySubmitError::Cancelled(job));
        }
        if state.input_closed {
            return Err(TrySubmitError::Closed(job));
        }
        if state.outstanding == self.shared.capacity {
            return Err(TrySubmitError::Full(job));
        }
        let seq = self.next_submit;
        self.next_submit = self
            .next_submit
            .checked_add(1)
            .expect("block sequence overflow");
        state.queue.push_back((seq, job));
        state.outstanding += 1;
        self.shared.wake.notify_one();
        Ok(())
    }

    /// Stops accepting new input.  Existing jobs continue and their results
    /// remain available in order.
    pub fn close(&mut self) {
        let mut state = self.shared.state.lock().expect("pipeline mutex poisoned");
        cancel_if_requested_locked(&mut state, &self.shared.cancellation);
        state.input_closed = true;
        self.shared.wake.notify_all();
    }

    /// Returns the next block if it is ready, without blocking.
    pub fn try_recv_next(&mut self) -> Result<Poll<R>, PipelineError> {
        let shared = Arc::clone(&self.shared);
        let mut state = shared.state.lock().expect("pipeline mutex poisoned");
        poll_locked(
            &mut state,
            &mut self.next_receive,
            &shared.wake,
            &shared.cancellation,
        )
    }

    /// Waits at most `timeout` for the next ordered block.  This lets a stream
    /// caller service signals or an idle flush clock without creating another
    /// queue or reader thread.
    pub fn recv_next_timeout(&mut self, timeout: Duration) -> Result<Poll<R>, PipelineError> {
        let deadline = Instant::now().checked_add(timeout);
        let shared = Arc::clone(&self.shared);
        let mut state = shared.state.lock().expect("pipeline mutex poisoned");
        loop {
            match poll_locked(
                &mut state,
                &mut self.next_receive,
                &shared.wake,
                &shared.cancellation,
            )? {
                Poll::Pending => {}
                result => return Ok(result),
            }
            let remaining = deadline
                .and_then(|end| end.checked_duration_since(Instant::now()))
                .unwrap_or(Duration::ZERO);
            if remaining.is_zero() {
                return Ok(Poll::Pending);
            }
            let (new_state, waited) = shared
                .wake
                .wait_timeout(state, remaining)
                .expect("pipeline mutex poisoned");
            state = new_state;
            if waited.timed_out() {
                return poll_locked(
                    &mut state,
                    &mut self.next_receive,
                    &shared.wake,
                    &shared.cancellation,
                );
            }
        }
    }

    /// Blocks until the next input-order result is ready, completion occurs,
    /// or a worker error is reported.
    pub fn recv_next(&mut self) -> Result<Option<R>, PipelineError> {
        let shared = Arc::clone(&self.shared);
        let mut state = shared.state.lock().expect("pipeline mutex poisoned");
        loop {
            match poll_locked(
                &mut state,
                &mut self.next_receive,
                &shared.wake,
                &shared.cancellation,
            )? {
                Poll::Item(value) => return Ok(Some(value)),
                Poll::Finished => return Ok(None),
                Poll::Pending => state = shared.wake.wait(state).expect("pipeline mutex poisoned"),
            }
        }
    }

    /// Cancels queued jobs, waits for currently executing encodes, and joins
    /// persistent workers.  A running user encoder cannot be forcibly stopped,
    /// so it must itself have a finite cancellation-aware budget.
    pub fn cancel(&mut self) {
        {
            let mut state = self.shared.state.lock().expect("pipeline mutex poisoned");
            self.shared.cancellation.store(true, Ordering::Release);
            cancel_locked(&mut state);
            self.shared.wake.notify_all();
        }
        self.join_workers();
    }

    /// Closes input and joins workers after all accepted encodes have finished.
    /// Results are retained for subsequent ordered receives.  It reports any
    /// worker failure even when results preceded the failing block.
    pub fn finish(&mut self) -> Result<(), PipelineError> {
        self.close();
        let mut state = self.shared.state.lock().expect("pipeline mutex poisoned");
        cancel_if_requested_locked(&mut state, &self.shared.cancellation);
        while state.active != 0 || !state.queue.is_empty() {
            state = self
                .shared
                .wake
                .wait(state)
                .expect("pipeline mutex poisoned");
        }
        if let Some(error) = state.failure.clone() {
            drop(state);
            self.join_workers();
            return Err(error);
        }
        drop(state);
        self.join_workers();
        Ok(())
    }

    fn join_workers(&mut self) {
        for worker in std::mem::take(&mut self.workers) {
            let _ = worker.join();
        }
    }
}

impl<T: Send + 'static, R: Send + 'static> Drop for OrderedPipeline<T, R> {
    fn drop(&mut self) {
        if !self.workers.is_empty() {
            self.cancel();
        }
    }
}

fn poll_locked<T, R>(
    state: &mut State<T, R>,
    next_receive: &mut u64,
    wake: &Condvar,
    cancellation: &AtomicBool,
) -> Result<Poll<R>, PipelineError> {
    cancel_if_requested_locked(state, cancellation);
    if let Some(error) = state.failure.clone() {
        return Err(error);
    }
    if state.cancelled {
        return Err(PipelineError::Cancelled);
    }
    if let Some(value) = state.results.remove(next_receive) {
        *next_receive = next_receive
            .checked_add(1)
            .expect("block sequence overflow");
        state.outstanding = state
            .outstanding
            .checked_sub(1)
            .expect("outstanding underflow");
        wake.notify_all();
        return Ok(Poll::Item(value));
    }
    if state.input_closed && state.outstanding == 0 {
        return Ok(Poll::Finished);
    }
    Ok(Poll::Pending)
}

fn worker_loop<T, R, F>(shared: Arc<Shared<T, R>>, encode: Arc<F>)
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> Result<R, String> + Send + Sync + 'static,
{
    loop {
        let (seq, job) = {
            let mut state = shared.state.lock().expect("pipeline mutex poisoned");
            cancel_if_requested_locked(&mut state, &shared.cancellation);
            while state.queue.is_empty() && !state.input_closed && !state.cancelled {
                state = shared.wake.wait(state).expect("pipeline mutex poisoned");
                cancel_if_requested_locked(&mut state, &shared.cancellation);
            }
            if state.cancelled || (state.input_closed && state.queue.is_empty()) {
                return;
            }
            let job = state.queue.pop_front().expect("non-empty queue");
            state.active += 1;
            job
        };

        let outcome = catch_unwind(AssertUnwindSafe(|| encode(job)));
        let mut state = shared.state.lock().expect("pipeline mutex poisoned");
        state.active = state.active.checked_sub(1).expect("active underflow");
        cancel_if_requested_locked(&mut state, &shared.cancellation);
        if !state.cancelled {
            match outcome {
                Ok(Ok(value)) => {
                    state.results.insert(seq, value);
                }
                Ok(Err(error)) => fail_locked(
                    &mut state,
                    PipelineError::Worker(error),
                    &shared.cancellation,
                ),
                Err(_) => fail_locked(&mut state, PipelineError::WorkerPanic, &shared.cancellation),
            }
        }
        shared.wake.notify_all();
    }
}

fn cancel_if_requested_locked<T, R>(state: &mut State<T, R>, cancellation: &AtomicBool) {
    if cancellation.load(Ordering::Acquire) && !state.cancelled {
        cancel_locked(state);
    }
}

fn cancel_locked<T, R>(state: &mut State<T, R>) {
    state.cancelled = true;
    state.input_closed = true;
    state.queue.clear();
    state.results.clear();
    state.outstanding = 0;
}

fn fail_locked<T, R>(state: &mut State<T, R>, error: PipelineError, cancellation: &AtomicBool) {
    if state.failure.is_none() {
        state.failure = Some(error);
    }
    cancellation.store(true, Ordering::Release);
    cancel_locked(state);
}
