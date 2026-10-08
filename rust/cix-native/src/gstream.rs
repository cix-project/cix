//! Ordered independent-frame streaming. One bounded CPU pool and one shared
//! scratch-memory ledger serve all blocks; stateful CIXM6 remains sequential.
use super::*;
use std::collections::VecDeque;

struct Completed {
    input: Vec<u8>,
    route: u8,
    payload: Vec<u8>,
    hash: [u8; 32],
    stats: AutoSelectionStats,
}

pub(super) struct EncodeOptions<'a> {
    pub(super) block: u32,
    pub(super) level: u8,
    pub(super) forced: Option<&'a str>,
    pub(super) backend: &'a str,
    pub(super) backend_set: bool,
    pub(super) format: &'a str,
    pub(super) input_fd: i32,
    pub(super) flush_interval: Option<Duration>,
    pub(super) memory: usize,
    pub(super) workers: usize,
    pub(super) explain: bool,
    pub(super) verbose: bool,
}

struct EncodeContext {
    magic: &'static [u8; 5],
    resolved_format: &'static str,
    retained: usize,
    budget: resources::MemoryBudget,
}

struct WorkerOptions {
    forced_id: Option<u8>,
    backend: String,
    backend_set: bool,
    resolved_format: &'static str,
    level: u8,
    budget: resources::MemoryBudget,
    explain: bool,
    cancelled: std::sync::Arc<AtomicBool>,
}

struct StreamState {
    buffer: Vec<u8>,
    filled: usize,
    counts: [u32; 256],
    since: Option<Instant>,
    free: VecDeque<Vec<u8>>,
    whole: Sha256,
    source_bytes: u64,
    archive_bytes: u64,
    totals: AutoSelectionStats,
    eof: bool,
    peak_outstanding: usize,
}

impl StreamState {
    fn new(block: u32) -> Self {
        Self {
            buffer: vec![0; block as usize],
            filled: 0,
            counts: [0; 256],
            since: None,
            free: VecDeque::new(),
            whole: Sha256::new(),
            source_bytes: 0,
            archive_bytes: HEADER as u64,
            totals: AutoSelectionStats::default(),
            eof: false,
            peak_outstanding: 0,
        }
    }

    fn ready(&self, flush_interval: Option<Duration>) -> bool {
        let expired = self
            .since
            .zip(flush_interval)
            .is_some_and(|(start, interval)| start.elapsed() >= interval);
        self.filled == self.buffer.len() || self.filled != 0 && (expired || self.eof)
    }

    fn record_input(&mut self, count: usize) {
        for &byte in &self.buffer[self.filled..self.filled + count] {
            self.counts[byte as usize] += 1;
        }
        self.filled += count;
        self.since.get_or_insert_with(Instant::now);
    }

    fn input_timeout(
        &self,
        flush_interval: Option<Duration>,
        outstanding: usize,
    ) -> Option<Duration> {
        let mut timeout = self
            .since
            .zip(flush_interval)
            .map(|(start, interval)| interval.saturating_sub(start.elapsed()));
        if outstanding != 0 {
            timeout = Some(timeout.map_or(Duration::from_millis(50), |wait| {
                wait.min(Duration::from_millis(50))
            }));
        }
        timeout
    }

    fn submit(
        &mut self,
        pipeline: &mut parallel::OrderedPipeline<Vec<u8>, Completed>,
        block: u32,
    ) -> Result<(), String> {
        if self.counts.iter().map(|&n| n as usize).sum::<usize>() != self.filled {
            return Err("internal block symbol count mismatch".into());
        }
        self.buffer.truncate(self.filled);
        self.whole.update(&self.buffer);
        self.source_bytes = self
            .source_bytes
            .checked_add(self.filled as u64)
            .ok_or("input length overflow")?;
        match pipeline.try_submit(std::mem::take(&mut self.buffer)) {
            Ok(()) => {}
            Err(parallel::TrySubmitError::Cancelled(_)) => {
                return Err(pipeline
                    .try_recv_next()
                    .err()
                    .map(pipeline_error)
                    .unwrap_or_else(|| "block encoding cancelled".into()));
            }
            Err(_) => return Err("pipeline admission invariant failed".into()),
        }
        self.peak_outstanding = self.peak_outstanding.max(pipeline.outstanding());
        self.buffer = self.free.pop_front().unwrap_or_default();
        self.buffer.resize(block as usize, 0);
        self.filled = 0;
        self.counts.fill(0);
        self.since = None;
        Ok(())
    }
}

fn pipeline_error(error: parallel::PipelineError) -> String {
    match error {
        parallel::PipelineError::Worker(message) => message,
        parallel::PipelineError::WorkerPanic => "block encoder panicked".into(),
        parallel::PipelineError::Cancelled => "block encoding cancelled".into(),
    }
}

fn emit<W: Write>(
    dst: &mut W,
    mut item: Completed,
    totals: &mut AutoSelectionStats,
    archive_bytes: &mut u64,
    free: &mut VecDeque<Vec<u8>>,
) -> Result<(), String> {
    check_interrupted()?;
    put_frame(
        dst,
        item.route,
        item.input.len() as u32,
        u32::try_from(item.payload.len()).map_err(|_| "frame payload length overflow")?,
        &item.hash,
    )
    .map_err(ioerr)?;
    dst.write_all(&item.payload).map_err(ioerr)?;
    dst.flush().map_err(ioerr)?;
    *archive_bytes = archive_bytes
        .checked_add(41 + item.payload.len() as u64)
        .ok_or("archive length overflow")?;
    totals.blocks += item.stats.blocks;
    totals.profile_seconds += item.stats.profile_seconds;
    totals.search_seconds += item.stats.search_seconds;
    totals.candidate_seconds += item.stats.candidate_seconds;
    totals.candidate_attempts += item.stats.candidate_attempts;
    totals.memory_skips += item.stats.memory_skips;
    totals.budget_skips += item.stats.budget_skips;
    for (total, bytes) in totals
        .selected_source_bytes
        .iter_mut()
        .zip(item.stats.selected_source_bytes)
    {
        *total += bytes;
    }
    for explanation in item.stats.explanations.drain(..) {
        eprintln!("cix: region={} {}", totals.blocks, explanation);
    }
    item.input.clear();
    free.push_back(item.input);
    Ok(())
}

fn wait_one<W: Write>(
    pipeline: &mut parallel::OrderedPipeline<Vec<u8>, Completed>,
    dst: &mut W,
    totals: &mut AutoSelectionStats,
    archive_bytes: &mut u64,
    free: &mut VecDeque<Vec<u8>>,
) -> Result<(), String> {
    loop {
        check_interrupted()?;
        match pipeline
            .recv_next_timeout(Duration::from_millis(50))
            .map_err(pipeline_error)?
        {
            parallel::Poll::Item(item) => return emit(dst, item, totals, archive_bytes, free),
            parallel::Poll::Pending => {}
            parallel::Poll::Finished => {
                return Err("pipeline finished before expected frame".into())
            }
        }
    }
}

fn drain_ready<W: Write>(
    pipeline: &mut parallel::OrderedPipeline<Vec<u8>, Completed>,
    dst: &mut W,
    state: &mut StreamState,
) -> Result<(), String> {
    while let parallel::Poll::Item(item) = pipeline.try_recv_next().map_err(pipeline_error)? {
        emit(
            dst,
            item,
            &mut state.totals,
            &mut state.archive_bytes,
            &mut state.free,
        )?;
    }
    Ok(())
}

fn wait_for_capacity<W: Write>(
    pipeline: &mut parallel::OrderedPipeline<Vec<u8>, Completed>,
    dst: &mut W,
    state: &mut StreamState,
) -> Result<(), String> {
    while pipeline.outstanding() == pipeline.capacity() {
        wait_one(
            pipeline,
            dst,
            &mut state.totals,
            &mut state.archive_bytes,
            &mut state.free,
        )?;
    }
    Ok(())
}

fn encode_block(input: Vec<u8>, options: &WorkerOptions) -> Result<Completed, String> {
    let _cancel = crate::limits::CancellationGuard::new(options.cancelled.clone());
    check_interrupted()?;
    let hash = Sha256::digest(&input).into();
    let mut stats = AutoSelectionStats::default();
    let override_backend = options.backend_set.then_some(options.backend.as_str());
    let (route, payload) = if let Some(id) = options.forced_id {
        select_cixg_fixed_route(CixgFixedRouteRequest {
            route: id,
            data: &input,
            backend_override: override_backend,
            format: options.resolved_format,
            effort_name: selector_effort_name(options.level),
            budget: &options.budget,
            workers: 1,
            stats: &mut stats,
            explain: options.explain,
        })?
    } else {
        select_cixg_auto(CixgAutoRequest {
            data: &input,
            level: options.level,
            backend_override: override_backend,
            format: options.resolved_format,
            budget: &options.budget,
            workers: 1,
            stats: &mut stats,
            explain: options.explain,
        })?
    };
    Ok(Completed {
        input,
        route,
        payload,
        hash,
        stats,
    })
}

fn write_archive<R: Read, W: Write>(
    src: &mut R,
    dst: &mut W,
    pipeline: &mut parallel::OrderedPipeline<Vec<u8>, Completed>,
    options: &EncodeOptions<'_>,
    context: &EncodeContext,
) -> Result<(), String> {
    dst.write_all(context.magic).map_err(ioerr)?;
    dst.write_all(&options.block.to_le_bytes()).map_err(ioerr)?;
    dst.write_all(&0u32.to_le_bytes()).map_err(ioerr)?;
    let mut state = StreamState::new(options.block);
    loop {
        check_interrupted()?;
        drain_ready(pipeline, dst, &mut state)?;
        // Do not read ahead while the writer's ordered output is backlogged.
        wait_for_capacity(pipeline, dst, &mut state)?;
        if state.ready(options.flush_interval) {
            state.submit(pipeline, options.block)?;
            continue;
        }
        if state.eof {
            break;
        }
        let timeout = state.input_timeout(options.flush_interval, pipeline.outstanding());
        match read_poll(
            src,
            &mut state.buffer[state.filled..],
            options.input_fd,
            timeout,
        )? {
            Some(0) => state.eof = true,
            Some(count) => state.record_input(count),
            None => {}
        }
    }
    pipeline.close();
    while pipeline.outstanding() != 0 {
        wait_one(
            pipeline,
            dst,
            &mut state.totals,
            &mut state.archive_bytes,
            &mut state.free,
        )?;
    }
    pipeline.finish().map_err(pipeline_error)?;
    check_interrupted()?;
    put_frame(dst, END, 0, 0, &state.whole.finalize().into()).map_err(ioerr)?;
    state.archive_bytes = state
        .archive_bytes
        .checked_add(41)
        .ok_or("archive length overflow")?;
    dst.flush().map_err(ioerr)?;
    if options.verbose || options.explain {
        let routes = state
            .totals
            .selected_source_bytes
            .iter()
            .enumerate()
            .filter(|(_, n)| **n != 0)
            .map(|(id, n)| format!("{}:{n}", selection::ROUTES[id]))
            .collect::<Vec<_>>()
            .join(",");
        eprintln!("cix: effort={} format={} parallelism=blocks workers={} queue_capacity={} peak_outstanding={} reserved_buffers={} peak_scratch_reserved={} source_bytes={} blocks={} candidates={} memory_skips={} budget_skips={} analyse_seconds={:.3} candidate_seconds={:.3} search_seconds={:.3} archive_bytes={} selected_source_bytes=[{}]",
        effort_display_name(options.level), context.resolved_format, options.workers, options.workers, state.peak_outstanding, context.retained,
        context.budget.stats().peak_reserved_bytes, state.source_bytes, state.totals.blocks, state.totals.candidate_attempts,
        state.totals.memory_skips, state.totals.budget_skips, state.totals.profile_seconds, state.totals.candidate_seconds,
        state.totals.search_seconds, state.archive_bytes, routes);
    }
    Ok(())
}

pub(super) fn encode<R: Read, W: Write>(
    mut src: R,
    mut dst: W,
    options: EncodeOptions<'_>,
) -> Result<(), String> {
    let magic = native_magic(
        options.format,
        options.level,
        options.backend,
        options.backend_set,
    );
    let resolved_format = if magic == MAGIC_V2 { "cixg2" } else { "cixg1" };
    // Two retained buffers per outstanding job (source and winning payload),
    // one reader block, thread stacks, and bounded descriptor/stat overhead.
    let retained = parallel_retained(options.block, options.workers);
    if retained >= options.memory {
        return Err(
            "parallel block buffers and thread stacks exceed --memory; reduce --threads".into(),
        );
    }
    let context = EncodeContext {
        magic,
        resolved_format,
        retained,
        budget: resources::MemoryBudget::new(options.memory - retained),
    };
    let cancelled = std::sync::Arc::new(AtomicBool::new(false));
    let worker_options = WorkerOptions {
        forced_id: options.forced.map(route_id).transpose()?,
        backend: options.backend.to_owned(),
        backend_set: options.backend_set,
        resolved_format,
        level: options.level,
        budget: context.budget.clone(),
        explain: options.explain,
        cancelled: cancelled.clone(),
    };
    // Output polling and input reads must observe a failed sibling too.
    let _output_cancel = crate::limits::CancellationGuard::new(cancelled.clone());
    let mut pipeline = parallel::OrderedPipeline::new_with_cancellation(
        options.workers,
        options.workers,
        cancelled,
        move |input| encode_block(input, &worker_options),
    )
    .map_err(str::to_owned)?;

    let result = write_archive(&mut src, &mut dst, &mut pipeline, &options, &context);
    // Cooperative I/O cancellation is secondary to the original worker error.
    result.map_err(|error| pipeline.failure().map(pipeline_error).unwrap_or(error))
}
