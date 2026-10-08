//! Versioned, same-executable full-engine job protocol.
//!
//! Control records travel on a small bounded binary wire. Input/archive bytes
//! never share that wire: the embedding supplies `Read`/`Write` transports for
//! data, so this library never opens paths or executes arbitrary commands.
//! Cancellation is cooperative: it is checked between protocol stages; an
//! in-process codec call cannot be force-preempted.

use super::{
    dispatch::{FullEngine, OperationLimits},
    io::{self, DecodePath},
    selection::{
        self, CandidateExecutor, SelectionLimits, SelectionProfile, TrialDetails, TrialStatus,
    },
    window_stream::{self, EncodeLimits, StreamSummary},
};
use std::{
    io::{self as stdio, Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// Version two adds the independent CIXW1 window-selection operation. Readers
/// retain v1 request decoding so an existing whole-input client remains a
/// valid one-shot client.
pub const JOB_PROTOCOL_VERSION: u16 = 2;
const JOB_PROTOCOL_V1: u16 = 1;
pub const MAX_CONTROL_RECORD_BYTES: usize = 8 << 20;
pub const MAX_CONTROL_TEXT_BYTES: usize = 16 << 10;
pub const MAX_PROGRESS_STAGES: usize = 16;
const MAGIC: [u8; 4] = *b"CJB1";
const HEADER: usize = 4 + 2 + 1 + 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobOperation {
    SelectWholeInput,
    /// Read bounded independent windows and publish one CIXW1 archive.
    /// Every window starts a fresh selector and codec context.
    SelectIndependentWindows,
    DecodeArchive,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobProfile {
    Fast,
    Default,
    Best,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobStage {
    Accepted,
    InputRead,
    SelectionStarted,
    SelectionFinished,
    DecodeStarted,
    DecodeFinished,
    OutputWritten,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobLimits {
    pub input_bytes: usize,
    pub archive_bytes: usize,
    pub output_bytes: usize,
    pub memory_bytes: usize,
    pub intermediate_bytes: usize,
    pub temporary_bytes: usize,
    pub workers: usize,
    pub deadline_millis: Option<u64>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobWindowReport {
    pub window_bytes: usize,
    pub windows: u64,
    pub input_bytes: u64,
    pub archive_bytes: u64,
    pub independent_windows: u64,
    /// These full-file capabilities deliberately do not cross a CIXW1 reset.
    pub whole_only_omissions: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobRequest {
    pub request_id: u64,
    pub operation: JobOperation,
    pub profile: JobProfile,
    pub limits: JobLimits,
    /// Required only for `SelectIndependentWindows`; it is the maximum source
    /// bytes retained for one independently selected CIXW1 record.
    pub window_bytes: Option<usize>,
    pub diagnostics: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobCapability {
    pub protocol_version: u16,
    pub select_whole_input: bool,
    pub decode_archive: bool,
    pub select_independent_windows: bool,
    pub data_transport: String,
    pub cancellation: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobProgress {
    pub request_id: u64,
    pub stage: JobStage,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobError {
    pub request_id: u64,
    pub code: JobErrorCode,
    pub message: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobErrorCode {
    InvalidRequest,
    ResourceLimit,
    Cancelled,
    Unsupported,
    Io,
    Execution,
    Protocol,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobTrial {
    pub id: String,
    pub priority: usize,
    pub status: String,
    pub reason: Option<String>,
    pub archive_bytes: Option<usize>,
    pub nested: Vec<JobDiagnostic>,
    pub nested_omitted: usize,
    pub provider_omissions: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobDiagnostic {
    pub kind: String,
    pub id: String,
    pub status: String,
    pub region: Option<usize>,
    pub backend: Option<String>,
    /// `archive` for whole candidates; `payload` for mixed region choices.
    pub bytes_unit: String,
    pub bytes: Option<usize>,
    pub detail: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobDiscovery {
    pub family: String,
    pub admitted: bool,
    pub available: bool,
    pub reason: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobSelectionReport {
    pub profile: JobProfile,
    pub input_bytes: usize,
    pub discovery: Vec<JobDiscovery>,
    pub trials: Vec<JobTrial>,
    pub peak_reserved_memory: usize,
    pub peak_reserved_workers: usize,
    pub peak_reserved_temporary: usize,
    pub selected: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobResult {
    pub request_id: u64,
    pub operation: JobOperation,
    pub output_bytes: usize,
    pub selection: Option<JobSelectionReport>,
    pub windows: Option<JobWindowReport>,
    pub decode_path: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlRecord {
    Request(JobRequest),
    Capability(JobCapability),
    Progress(JobProgress),
    Result(JobResult),
    Error(JobError),
    Cancel { request_id: u64 },
}

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);
impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub fn as_arc(&self) -> Arc<AtomicBool> {
        self.0.clone()
    }
}

pub fn capabilities() -> JobCapability {
    JobCapability {
        protocol_version: JOB_PROTOCOL_VERSION,
        select_whole_input: true,
        decode_archive: true,
        select_independent_windows: true,
        data_transport:
            "caller-provided Read/Write; no paths; whole-input selection up to 128 MiB; bounded independent CIXW1 window selection and incremental CIXW1 archive decode"
                .into(),
        cancellation: "cooperative stage-boundary cancellation; no in-process force-preemption"
            .into(),
    }
}

#[derive(Default)]
struct Buf {
    bytes: Vec<u8>,
    failed: bool,
}
impl Buf {
    fn add(&mut self, value: &[u8]) {
        if self.failed {
            return;
        }
        let Some(next) = self.bytes.len().checked_add(value.len()) else {
            self.failed = true;
            return;
        };
        if next > MAX_CONTROL_RECORD_BYTES || self.bytes.try_reserve_exact(value.len()).is_err() {
            self.failed = true;
            return;
        }
        self.bytes.extend_from_slice(value);
    }
    fn u8(&mut self, n: u8) {
        self.add(&[n])
    }
    fn u16(&mut self, n: u16) {
        self.add(&n.to_be_bytes())
    }
    fn u32(&mut self, n: u32) {
        self.add(&n.to_be_bytes())
    }
    fn u64(&mut self, n: u64) {
        self.add(&n.to_be_bytes())
    }
    fn usize(&mut self, n: usize) -> Result<(), String> {
        self.u64(u64::try_from(n).map_err(|_| "control integer overflow")?);
        Ok(())
    }
    fn text(&mut self, s: &str) -> Result<(), String> {
        if s.len() > MAX_CONTROL_TEXT_BYTES || !s.is_char_boundary(s.len()) {
            return Err("control text exceeds limit".into());
        }
        self.u32(u32::try_from(s.len()).map_err(|_| "control text overflow")?);
        self.add(s.as_bytes());
        Ok(())
    }
    fn opt_usize(&mut self, n: Option<usize>) -> Result<(), String> {
        self.u8(u8::from(n.is_some()));
        if let Some(n) = n {
            self.usize(n)?
        }
        Ok(())
    }
    fn opt_text(&mut self, s: &Option<String>) -> Result<(), String> {
        self.u8(u8::from(s.is_some()));
        if let Some(s) = s {
            self.text(s)?
        }
        Ok(())
    }
}
struct Take<'a> {
    b: &'a [u8],
    p: usize,
}
impl<'a> Take<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.p.checked_add(n).ok_or("control length overflow")?;
        let out = self.b.get(self.p..end).ok_or("truncated control record")?;
        self.p = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn usize(&mut self) -> Result<usize, String> {
        usize::try_from(self.u64()?).map_err(|_| "control integer overflow".into())
    }
    fn text(&mut self) -> Result<String, String> {
        let n = usize::try_from(self.u32()?).map_err(|_| "control text overflow")?;
        if n > MAX_CONTROL_TEXT_BYTES {
            return Err("control text exceeds limit".into());
        }
        let b = self.take(n)?;
        let s = std::str::from_utf8(b).map_err(|_| "invalid control UTF-8")?;
        Ok(s.into())
    }
    fn opt_usize(&mut self) -> Result<Option<usize>, String> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.usize()?)),
            _ => Err("invalid control option".into()),
        }
    }
    fn opt_text(&mut self) -> Result<Option<String>, String> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.text()?)),
            _ => Err("invalid control option".into()),
        }
    }
    fn end(&self) -> Result<(), String> {
        if self.p == self.b.len() {
            Ok(())
        } else {
            Err("trailing control bytes".into())
        }
    }
}
fn op_tag(v: JobOperation) -> u8 {
    match v {
        JobOperation::SelectWholeInput => 1,
        JobOperation::SelectIndependentWindows => 2,
        JobOperation::DecodeArchive => 3,
    }
}
fn op(v: u8) -> Result<JobOperation, String> {
    match v {
        1 => Ok(JobOperation::SelectWholeInput),
        2 => Ok(JobOperation::SelectIndependentWindows),
        3 => Ok(JobOperation::DecodeArchive),
        _ => Err("unknown job operation".into()),
    }
}
fn profile_tag(v: JobProfile) -> u8 {
    match v {
        JobProfile::Fast => 1,
        JobProfile::Default => 2,
        JobProfile::Best => 3,
    }
}
fn profile(v: u8) -> Result<JobProfile, String> {
    match v {
        1 => Ok(JobProfile::Fast),
        2 => Ok(JobProfile::Default),
        3 => Ok(JobProfile::Best),
        _ => Err("unknown job profile".into()),
    }
}
fn put_limits(b: &mut Buf, l: &JobLimits) -> Result<(), String> {
    for n in [
        l.input_bytes,
        l.archive_bytes,
        l.output_bytes,
        l.memory_bytes,
        l.intermediate_bytes,
        l.temporary_bytes,
        l.workers,
    ] {
        b.usize(n)?
    }
    b.u8(u8::from(l.deadline_millis.is_some()));
    if let Some(n) = l.deadline_millis {
        b.u64(n)
    }
    Ok(())
}
fn get_limits(t: &mut Take<'_>) -> Result<JobLimits, String> {
    let x = JobLimits {
        input_bytes: t.usize()?,
        archive_bytes: t.usize()?,
        output_bytes: t.usize()?,
        memory_bytes: t.usize()?,
        intermediate_bytes: t.usize()?,
        temporary_bytes: t.usize()?,
        workers: t.usize()?,
        deadline_millis: match t.u8()? {
            0 => None,
            1 => Some(t.u64()?),
            _ => return Err("invalid deadline option".into()),
        },
    };
    if x.input_bytes == 0
        || x.archive_bytes == 0
        || x.output_bytes == 0
        || x.memory_bytes == 0
        || x.intermediate_bytes == 0
        || x.temporary_bytes == 0
        || x.workers == 0
    {
        return Err("zero job limit".into());
    }
    Ok(x)
}

/// Write one bounded control record. The data transport is intentionally not
/// touched by this function.
pub fn write_control<W: Write>(out: &mut W, record: &ControlRecord) -> Result<(), String> {
    let mut p = Buf::default();
    let kind = match record {
        ControlRecord::Request(r) => {
            p.u64(r.request_id);
            p.u8(op_tag(r.operation));
            p.u8(profile_tag(r.profile));
            put_limits(&mut p, &r.limits)?;
            p.opt_usize(r.window_bytes)?;
            p.u8(u8::from(r.diagnostics));
            1
        }
        ControlRecord::Capability(c) => {
            p.u16(c.protocol_version);
            p.u8(u8::from(c.select_whole_input));
            p.u8(u8::from(c.decode_archive));
            p.u8(u8::from(c.select_independent_windows));
            p.text(&c.data_transport)?;
            p.text(&c.cancellation)?;
            2
        }
        ControlRecord::Progress(x) => {
            p.u64(x.request_id);
            p.u8(match x.stage {
                JobStage::Accepted => 1,
                JobStage::InputRead => 2,
                JobStage::SelectionStarted => 3,
                JobStage::SelectionFinished => 4,
                JobStage::DecodeStarted => 5,
                JobStage::DecodeFinished => 6,
                JobStage::OutputWritten => 7,
            });
            3
        }
        ControlRecord::Result(r) => {
            p.u64(r.request_id);
            p.u8(op_tag(r.operation));
            p.usize(r.output_bytes)?;
            p.u8(u8::from(r.selection.is_some()));
            if let Some(s) = &r.selection {
                put_report(&mut p, s)?
            }
            p.opt_text(&r.decode_path)?;
            put_window_report(&mut p, &r.windows)?;
            4
        }
        ControlRecord::Error(e) => {
            p.u64(e.request_id);
            p.u8(e.code as u8);
            p.text(&e.message)?;
            5
        }
        ControlRecord::Cancel { request_id } => {
            p.u64(*request_id);
            6
        }
    };
    if p.failed || p.bytes.len() > MAX_CONTROL_RECORD_BYTES {
        return Err("control record exceeds limit".into());
    }
    out.write_all(&MAGIC).map_err(|e| e.to_string())?;
    out.write_all(&JOB_PROTOCOL_VERSION.to_be_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(&[kind]).map_err(|e| e.to_string())?;
    out.write_all(
        &u32::try_from(p.bytes.len())
            .map_err(|_| "control record overflow")?
            .to_be_bytes(),
    )
    .map_err(|e| e.to_string())?;
    out.write_all(&p.bytes).map_err(|e| e.to_string())?;
    Ok(())
}

/// Read exactly one control record, rejecting unknown versions, oversized
/// length fields, truncated input and non-UTF-8 text.
pub fn read_control<R: Read>(input: &mut R) -> Result<ControlRecord, String> {
    let mut h = [0u8; HEADER];
    input.read_exact(&mut h).map_err(|e| e.to_string())?;
    if h[..4] != MAGIC {
        return Err("invalid control magic".into());
    }
    let version = u16::from_be_bytes([h[4], h[5]]);
    if version != JOB_PROTOCOL_V1 && version != JOB_PROTOCOL_VERSION {
        return Err("unsupported control version".into());
    }
    let n = usize::try_from(u32::from_be_bytes(h[7..11].try_into().unwrap()))
        .map_err(|_| "control length overflow")?;
    if n > MAX_CONTROL_RECORD_BYTES {
        return Err("control record exceeds limit".into());
    }
    if !(1..=6).contains(&h[6]) {
        return Err("unknown control record kind".into());
    }
    let mut b = Vec::new();
    b.try_reserve_exact(n)
        .map_err(|_| "control allocation failed")?;
    b.resize(n, 0);
    input.read_exact(&mut b).map_err(|e| e.to_string())?;
    let mut t = Take { b: &b, p: 0 };
    let record = match h[6] {
        1 => {
            let request_id = t.u64()?;
            let operation = if version == JOB_PROTOCOL_V1 {
                match t.u8()? {
                    1 => JobOperation::SelectWholeInput,
                    2 => JobOperation::DecodeArchive,
                    _ => return Err("unknown v1 job operation".into()),
                }
            } else {
                op(t.u8()?)?
            };
            let profile = profile(t.u8()?)?;
            let limits = get_limits(&mut t)?;
            let window_bytes = if version == JOB_PROTOCOL_V1 {
                None
            } else {
                t.opt_usize()?
            };
            let diagnostics = match t.u8()? {
                0 => false,
                1 => true,
                _ => return Err("invalid diagnostic flag".into()),
            };
            ControlRecord::Request(JobRequest {
                request_id,
                operation,
                profile,
                limits,
                window_bytes,
                diagnostics,
            })
        }
        2 => {
            let capability_version = t.u16()?;
            if capability_version != version {
                return Err("capability version does not match control header".into());
            }
            let a = match t.u8()? {
                0 => false,
                1 => true,
                _ => return Err("invalid capability flag".into()),
            };
            let d = match t.u8()? {
                0 => false,
                1 => true,
                _ => return Err("invalid capability flag".into()),
            };
            let select_independent_windows = if version == JOB_PROTOCOL_V1 {
                false
            } else {
                match t.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err("invalid capability flag".into()),
                }
            };
            ControlRecord::Capability(JobCapability {
                protocol_version: capability_version,
                select_whole_input: a,
                decode_archive: d,
                select_independent_windows,
                data_transport: t.text()?,
                cancellation: t.text()?,
            })
        }
        3 => ControlRecord::Progress(JobProgress {
            request_id: t.u64()?,
            stage: match t.u8()? {
                1 => JobStage::Accepted,
                2 => JobStage::InputRead,
                3 => JobStage::SelectionStarted,
                4 => JobStage::SelectionFinished,
                5 => JobStage::DecodeStarted,
                6 => JobStage::DecodeFinished,
                7 => JobStage::OutputWritten,
                _ => return Err("unknown progress stage".into()),
            },
        }),
        4 => get_result(&mut t, version)?,
        5 => ControlRecord::Error(JobError {
            request_id: t.u64()?,
            code: match t.u8()? {
                0 => JobErrorCode::InvalidRequest,
                1 => JobErrorCode::ResourceLimit,
                2 => JobErrorCode::Cancelled,
                3 => JobErrorCode::Unsupported,
                4 => JobErrorCode::Io,
                5 => JobErrorCode::Execution,
                6 => JobErrorCode::Protocol,
                _ => return Err("unknown job error code".into()),
            },
            message: t.text()?,
        }),
        6 => ControlRecord::Cancel {
            request_id: t.u64()?,
        },
        _ => return Err("unknown control record kind".into()),
    };
    t.end()?;
    Ok(record)
}
fn put_report(b: &mut Buf, r: &JobSelectionReport) -> Result<(), String> {
    b.u8(profile_tag(r.profile));
    b.usize(r.input_bytes)?;
    b.usize(r.peak_reserved_memory)?;
    b.usize(r.peak_reserved_workers)?;
    b.usize(r.peak_reserved_temporary)?;
    b.text(&r.selected)?;
    b.u32(u32::try_from(r.discovery.len()).map_err(|_| "too many discovery diagnostics")?);
    for d in &r.discovery {
        b.text(&d.family)?;
        b.u8(u8::from(d.admitted));
        b.u8(u8::from(d.available));
        b.text(&d.reason)?;
    }
    b.u32(u32::try_from(r.trials.len()).map_err(|_| "too many trial diagnostics")?);
    for x in &r.trials {
        b.text(&x.id)?;
        b.usize(x.priority)?;
        b.text(&x.status)?;
        b.opt_text(&x.reason)?;
        b.opt_usize(x.archive_bytes)?;
        b.usize(x.nested_omitted)?;
        b.u32(
            u32::try_from(x.provider_omissions.len()).map_err(|_| "too many provider omissions")?,
        );
        for omission in &x.provider_omissions {
            b.text(omission)?;
        }
        b.u32(u32::try_from(x.nested.len()).map_err(|_| "too many nested diagnostics")?);
        for n in &x.nested {
            b.text(&n.kind)?;
            b.text(&n.id)?;
            b.text(&n.status)?;
            b.opt_usize(n.region)?;
            b.opt_text(&n.backend)?;
            b.text(&n.bytes_unit)?;
            b.opt_usize(n.bytes)?;
            b.opt_text(&n.detail)?;
        }
    }
    Ok(())
}
fn get_report(t: &mut Take<'_>) -> Result<JobSelectionReport, String> {
    let profile = profile(t.u8()?)?;
    let input_bytes = t.usize()?;
    let peak_reserved_memory = t.usize()?;
    let peak_reserved_workers = t.usize()?;
    let peak_reserved_temporary = t.usize()?;
    let selected = t.text()?;
    let discovery_count = usize::try_from(t.u32()?).map_err(|_| "discovery count overflow")?;
    if discovery_count > t.b.len().saturating_sub(t.p) / 10 {
        return Err("too many discovery diagnostics".into());
    }
    let mut discovery = Vec::new();
    discovery
        .try_reserve_exact(discovery_count)
        .map_err(|_| "diagnostic allocation failed")?;
    for _ in 0..discovery_count {
        let family = t.text()?;
        let admitted = match t.u8()? {
            0 => false,
            1 => true,
            _ => return Err("invalid control boolean".into()),
        };
        let available = match t.u8()? {
            0 => false,
            1 => true,
            _ => return Err("invalid control boolean".into()),
        };
        let reason = t.text()?;
        discovery.push(JobDiscovery {
            family,
            admitted,
            available,
            reason,
        });
    }
    let count = usize::try_from(t.u32()?).map_err(|_| "trial count overflow")?;
    if count > MAX_CONTROL_RECORD_BYTES / 8 || count > t.b.len().saturating_sub(t.p) / 34 {
        return Err("too many trial diagnostics".into());
    }
    let mut trials = Vec::new();
    trials
        .try_reserve_exact(count)
        .map_err(|_| "diagnostic allocation failed")?;
    for _ in 0..count {
        let id = t.text()?;
        let priority = t.usize()?;
        let status = t.text()?;
        let reason = t.opt_text()?;
        let archive_bytes = t.opt_usize()?;
        let nested_omitted = t.usize()?;
        let omissions =
            usize::try_from(t.u32()?).map_err(|_| "provider omission count overflow")?;
        if omissions > t.b.len().saturating_sub(t.p) / 4 {
            return Err("too many provider omissions".into());
        }
        let mut provider_omissions = Vec::new();
        provider_omissions
            .try_reserve_exact(omissions)
            .map_err(|_| "diagnostic allocation failed")?;
        for _ in 0..omissions {
            provider_omissions.push(t.text()?);
        }
        let nested_count = usize::try_from(t.u32()?).map_err(|_| "nested count overflow")?;
        if nested_count > t.b.len().saturating_sub(t.p) / 20 {
            return Err("too many nested diagnostics".into());
        }
        let mut nested = Vec::new();
        nested
            .try_reserve_exact(nested_count)
            .map_err(|_| "diagnostic allocation failed")?;
        for _ in 0..nested_count {
            nested.push(JobDiagnostic {
                kind: t.text()?,
                id: t.text()?,
                status: t.text()?,
                region: t.opt_usize()?,
                backend: t.opt_text()?,
                bytes_unit: t.text()?,
                bytes: t.opt_usize()?,
                detail: t.opt_text()?,
            });
        }
        trials.push(JobTrial {
            id,
            priority,
            status,
            reason,
            archive_bytes,
            nested,
            nested_omitted,
            provider_omissions,
        });
    }
    Ok(JobSelectionReport {
        profile,
        input_bytes,
        discovery,
        trials,
        peak_reserved_memory,
        peak_reserved_workers,
        peak_reserved_temporary,
        selected,
    })
}
fn get_result(t: &mut Take<'_>, version: u16) -> Result<ControlRecord, String> {
    let request_id = t.u64()?;
    let operation = if version == JOB_PROTOCOL_V1 {
        match t.u8()? {
            1 => JobOperation::SelectWholeInput,
            2 => JobOperation::DecodeArchive,
            _ => return Err("unknown v1 job operation".into()),
        }
    } else {
        op(t.u8()?)?
    };
    let output_bytes = t.usize()?;
    let selection = match t.u8()? {
        0 => None,
        1 => Some(get_report(t)?),
        _ => return Err("invalid selection report option".into()),
    };
    let decode_path = t.opt_text()?;
    let windows = if version == JOB_PROTOCOL_V1 {
        None
    } else {
        get_window_report(t)?
    };
    Ok(ControlRecord::Result(JobResult {
        request_id,
        operation,
        output_bytes,
        selection,
        decode_path,
        windows,
    }))
}
fn put_window_report(b: &mut Buf, report: &Option<JobWindowReport>) -> Result<(), String> {
    b.u8(u8::from(report.is_some()));
    let Some(report) = report else { return Ok(()) };
    b.usize(report.window_bytes)?;
    b.u64(report.windows);
    b.u64(report.input_bytes);
    b.u64(report.archive_bytes);
    b.u64(report.independent_windows);
    b.u32(
        u32::try_from(report.whole_only_omissions.len())
            .map_err(|_| "too many window omissions")?,
    );
    for omission in &report.whole_only_omissions {
        b.text(omission)?;
    }
    Ok(())
}
fn get_window_report(t: &mut Take<'_>) -> Result<Option<JobWindowReport>, String> {
    match t.u8()? {
        0 => return Ok(None),
        1 => {}
        _ => return Err("invalid window report option".into()),
    }
    let window_bytes = t.usize()?;
    let windows = t.u64()?;
    let input_bytes = t.u64()?;
    let archive_bytes = t.u64()?;
    let independent_windows = t.u64()?;
    let count = usize::try_from(t.u32()?).map_err(|_| "window omission count overflow")?;
    if count > t.b.len().saturating_sub(t.p) / 4 {
        return Err("too many window omissions".into());
    }
    let mut whole_only_omissions = Vec::new();
    whole_only_omissions
        .try_reserve_exact(count)
        .map_err(|_| "diagnostic allocation failed")?;
    for _ in 0..count {
        whole_only_omissions.push(t.text()?);
    }
    Ok(Some(JobWindowReport {
        window_bytes,
        windows,
        input_bytes,
        archive_bytes,
        independent_windows,
        whole_only_omissions,
    }))
}
fn simple_status(v: TrialStatus) -> String {
    match v {
        TrialStatus::Completed => "completed",
        TrialStatus::BudgetOmitted => "budget-omitted",
        TrialStatus::ProfileOmitted => "profile-omitted",
        TrialStatus::Failed => "failed",
        TrialStatus::Selected => "selected",
    }
    .into()
}
fn report_of(r: selection::SelectionReport) -> JobSelectionReport {
    let discovery = r
        .discovery
        .into_iter()
        .map(|d| JobDiscovery {
            family: d.family.into(),
            admitted: d.admitted,
            available: d.available,
            reason: d.reason.into(),
        })
        .collect();
    let trials = r
        .trials
        .into_iter()
        .map(|x| {
            let (nested, nested_omitted, provider_omissions) = match x.details {
                TrialDetails::None => (Vec::new(), 0, Vec::new()),
                TrialDetails::Native(v) => {
                    let rows = v
                        .into_iter()
                        .map(|n| JobDiagnostic {
                            kind: "native".into(),
                            id: n.id,
                            status: format!("{:?}", n.status),
                            region: None,
                            backend: None,
                            bytes_unit: "archive".into(),
                            bytes: n.archive_bytes,
                            detail: n.reason,
                        })
                        .collect();
                    (rows, 0, Vec::new())
                }
                TrialDetails::Mixed(v) => {
                    let omitted = v.omitted_entries;
                    let provider_omissions = v.provider_omissions;
                    let rows = v
                        .entries
                        .into_iter()
                        .map(|n| JobDiagnostic {
                            kind: format!("mixed:{}", n.region_kind.name()),
                            id: n.candidate,
                            status: format!("{:?}", n.status),
                            region: Some(n.region),
                            backend: n.backend.map(|value| format!("{:?}", value)),
                            bytes_unit: "payload".into(),
                            bytes: n.payload_bytes,
                            detail: n.detail,
                        })
                        .collect();
                    (rows, omitted, provider_omissions)
                }
            };
            JobTrial {
                id: x.id,
                priority: x.priority,
                status: simple_status(x.status),
                reason: x.reason,
                archive_bytes: x.archive_bytes,
                nested,
                nested_omitted,
                provider_omissions,
            }
        })
        .collect();
    JobSelectionReport {
        profile: match r.profile {
            SelectionProfile::Fast => JobProfile::Fast,
            SelectionProfile::Default => JobProfile::Default,
            SelectionProfile::Best => JobProfile::Best,
        },
        input_bytes: r.input_bytes,
        discovery,
        trials,
        peak_reserved_memory: r.peak_reserved_memory,
        peak_reserved_workers: r.peak_reserved_workers,
        peak_reserved_temporary: r.peak_reserved_temporary,
        selected: r.selected,
    }
}
fn stage<F: FnMut(JobProgress)>(
    request_id: u64,
    s: JobStage,
    emit: &mut F,
    c: &CancelToken,
) -> Result<(), String> {
    if c.is_cancelled() {
        return Err("job cancelled before stage".into());
    }
    emit(JobProgress {
        request_id,
        stage: s,
    });
    Ok(())
}
fn validate_limits(request: &JobRequest) -> Result<Option<Instant>, String> {
    let l = &request.limits;
    if l.input_bytes == 0
        || l.archive_bytes == 0
        || l.output_bytes == 0
        || l.memory_bytes == 0
        || l.intermediate_bytes == 0
        || l.temporary_bytes == 0
        || l.workers == 0
    {
        return Err("zero job limit".into());
    }
    if request.operation == JobOperation::SelectWholeInput
        && l.input_bytes > selection::MAX_WHOLE_INPUT
    {
        return Err("whole-input job exceeds maximum input".into());
    }
    match request.operation {
        JobOperation::SelectIndependentWindows => {
            let window = request
                .window_bytes
                .ok_or("windowed job requires window_bytes")?;
            if window == 0 || window > selection::MAX_WHOLE_INPUT || window > l.memory_bytes {
                return Err("invalid independent window size".into());
            }
        }
        _ if request.window_bytes.is_some() => {
            return Err("window_bytes is only valid for windowed jobs".into())
        }
        _ => {}
    }
    l.deadline_millis
        .map(|m| {
            Instant::now()
                .checked_add(Duration::from_millis(m))
                .ok_or_else(|| "job deadline overflow".into())
        })
        .transpose()
}
fn read_data<R: Read>(
    input: &mut R,
    cap: usize,
    memory: usize,
    deadline: Option<Instant>,
    c: &CancelToken,
) -> Result<Vec<u8>, String> {
    const SCRATCH: usize = 64 << 10;
    if cap.checked_add(SCRATCH).is_none_or(|need| need > memory) {
        return Err("job input and scratch exceed memory limit".into());
    }
    let mut out = Vec::new();
    out.try_reserve_exact(cap)
        .map_err(|_| "job input allocation failed")?;
    if out
        .capacity()
        .checked_add(SCRATCH)
        .is_none_or(|need| need > memory)
    {
        return Err("job input allocation exceeds memory limit".into());
    }
    let mut chunk = [0u8; SCRATCH];
    loop {
        if c.is_cancelled() || deadline.is_some_and(|d| Instant::now() >= d) {
            return Err("job cancelled while reading input".into());
        }
        let n = input.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        let next = out.len().checked_add(n).ok_or("job input size overflow")?;
        if next > cap {
            return Err("job input exceeds declared cap".into());
        }
        out.extend_from_slice(&chunk[..n]);
    }
    Ok(out)
}

struct CountingWriter<W> {
    inner: W,
    count: usize,
    cap: usize,
}

struct CappedReader<R> {
    inner: R,
    count: usize,
    cap: usize,
}
impl<R: Read> Read for CappedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> stdio::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.count == self.cap {
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(stdio::Error::other("job input exceeds declared cap")),
            };
        }
        let limit = (self.cap - self.count).min(buffer.len());
        let read = self.inner.read(&mut buffer[..limit])?;
        self.count = self
            .count
            .checked_add(read)
            .ok_or_else(|| stdio::Error::other("job input overflow"))?;
        Ok(read)
    }
}

fn window_omissions() -> Vec<String> {
    vec![
        "whole-file competition across windows and CIXM6 context retained across windows".into(),
        "PAQ and other codec history crossing a window boundary".into(),
        "CIXH1 regions or external CIXB1 payloads spanning multiple windows".into(),
        "recognition or region selection requiring cross-window bytes".into(),
        "whole-archive comparison; each CIXW1 record resets independently".into(),
    ]
}

/// Run bounded, independent full-engine selections and emit one CIXW1 carrier.
/// Its result intentionally reports the reset cost instead of claiming parity
/// with a whole-input selection.
pub fn execute_select_independent_windows<
    E: CandidateExecutor + Clone,
    R: Read,
    W: Write,
    F: FnMut(JobProgress),
>(
    request: &JobRequest,
    source: R,
    destination: W,
    executor: E,
    cancel: CancelToken,
    mut emit: F,
) -> Result<JobResult, String> {
    if request.operation != JobOperation::SelectIndependentWindows {
        return Err("wrong job operation".into());
    }
    let deadline = validate_limits(request)?;
    let window_bytes = request.window_bytes.expect("validated window size");
    stage(request.request_id, JobStage::Accepted, &mut emit, &cancel)?;
    stage(
        request.request_id,
        JobStage::SelectionStarted,
        &mut emit,
        &cancel,
    )?;
    let archive_cap = request
        .limits
        .archive_bytes
        .min(request.limits.output_bytes);
    let mut output = CountingWriter {
        inner: destination,
        count: 0,
        cap: archive_cap,
    };
    let mut input = CappedReader {
        inner: source,
        count: 0,
        cap: request.limits.input_bytes,
    };
    let summary: StreamSummary = window_stream::encode(
        &mut input,
        &mut output,
        &EncodeLimits {
            window_bytes,
            memory_bytes: request.limits.memory_bytes,
            output_bytes: archive_cap,
            deadline,
            cancellation: Some(cancel.as_arc()),
        },
        |window, selection_memory| {
            let per_window_output = window
                .len()
                .checked_add(2 << 20)
                .ok_or("window selection output bound overflow")?
                .min(archive_cap);
            let required = selection::minimum_whole_input_memory(window.len(), per_window_output)?;
            if selection_memory < required {
                return Err("window cannot admit bounded local selection".into());
            }
            let selected = selection::select_whole_input(
                window,
                match request.profile {
                    JobProfile::Fast => SelectionProfile::Fast,
                    JobProfile::Default => SelectionProfile::Default,
                    JobProfile::Best => SelectionProfile::Best,
                },
                SelectionLimits {
                    memory_bytes: selection_memory,
                    workers: request.limits.workers,
                    temporary_bytes: request.limits.temporary_bytes,
                    output_bytes: per_window_output,
                    intermediate_bytes: request.limits.intermediate_bytes,
                    deadline,
                    cancellation: Some(cancel.as_arc()),
                },
                executor.clone(),
            )?;
            Ok(selected.archive)
        },
    )?;
    stage(request.request_id, JobStage::InputRead, &mut emit, &cancel)?;
    stage(
        request.request_id,
        JobStage::SelectionFinished,
        &mut emit,
        &cancel,
    )?;
    stage(
        request.request_id,
        JobStage::OutputWritten,
        &mut emit,
        &cancel,
    )?;
    Ok(JobResult {
        request_id: request.request_id,
        operation: request.operation,
        output_bytes: output.count,
        selection: None,
        decode_path: None,
        windows: Some(JobWindowReport {
            window_bytes,
            windows: summary.windows,
            input_bytes: summary.input_bytes,
            archive_bytes: summary.archive_bytes,
            independent_windows: summary.independent_windows,
            whole_only_omissions: window_omissions(),
        }),
    })
}
impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, data: &[u8]) -> stdio::Result<usize> {
        let next = self
            .count
            .checked_add(data.len())
            .ok_or_else(|| stdio::Error::other("job output overflow"))?;
        if next > self.cap {
            return Err(stdio::Error::other("job output exceeds declared cap"));
        }
        let n = self.inner.write(data)?;
        self.count += n;
        Ok(n)
    }
    fn flush(&mut self) -> stdio::Result<()> {
        self.inner.flush()
    }
}

/// Run the real bounded whole-input selector with external data transports.
/// `emit` receives only stage transitions; no made-up byte-progress is emitted.
pub fn execute_select_whole_input<
    E: CandidateExecutor,
    R: Read,
    W: Write,
    F: FnMut(JobProgress),
>(
    request: &JobRequest,
    source: &mut R,
    destination: &mut W,
    executor: E,
    cancel: CancelToken,
    mut emit: F,
) -> Result<JobResult, String> {
    if request.operation != JobOperation::SelectWholeInput {
        return Err("wrong job operation".into());
    }
    let deadline = validate_limits(request)?;
    stage(request.request_id, JobStage::Accepted, &mut emit, &cancel)?;
    let data = read_data(
        source,
        request.limits.input_bytes,
        request.limits.memory_bytes,
        deadline,
        &cancel,
    )?;
    stage(request.request_id, JobStage::InputRead, &mut emit, &cancel)?;
    stage(
        request.request_id,
        JobStage::SelectionStarted,
        &mut emit,
        &cancel,
    )?;
    let capacity_slack = data.capacity().saturating_sub(data.len());
    let selection_memory = request
        .limits
        .memory_bytes
        .checked_sub(capacity_slack)
        .ok_or("job input capacity exceeds memory limit")?;
    let r = selection::select_whole_input(
        &data,
        match request.profile {
            JobProfile::Fast => SelectionProfile::Fast,
            JobProfile::Default => SelectionProfile::Default,
            JobProfile::Best => SelectionProfile::Best,
        },
        SelectionLimits {
            memory_bytes: selection_memory,
            workers: request.limits.workers,
            temporary_bytes: request.limits.temporary_bytes,
            output_bytes: request.limits.output_bytes,
            intermediate_bytes: request.limits.intermediate_bytes,
            deadline,
            cancellation: Some(cancel.as_arc()),
        },
        executor,
    )?;
    stage(
        request.request_id,
        JobStage::SelectionFinished,
        &mut emit,
        &cancel,
    )?;
    if r.archive.len() > request.limits.archive_bytes
        || r.archive.capacity() > request.limits.archive_bytes
    {
        return Err("selected archive exceeds declared cap".into());
    }
    destination
        .write_all(&r.archive)
        .map_err(|e| e.to_string())?;
    stage(
        request.request_id,
        JobStage::OutputWritten,
        &mut emit,
        &cancel,
    )?;
    Ok(JobResult {
        request_id: request.request_id,
        operation: request.operation,
        output_bytes: r.archive.len(),
        selection: request.diagnostics.then(|| report_of(r.report)),
        decode_path: None,
        windows: None,
    })
}

/// Run the bounded full-engine decoder against supplied streams. Constructing
/// `FullEngine` and selecting installed providers remains the caller's job.
pub fn execute_decode<R: Read, W: Write, F: FnMut(JobProgress)>(
    request: &JobRequest,
    engine: &FullEngine,
    source: R,
    destination: W,
    cancel: CancelToken,
    mut emit: F,
) -> Result<JobResult, String> {
    if request.operation != JobOperation::DecodeArchive {
        return Err("wrong job operation".into());
    }
    let deadline = validate_limits(request)?;
    stage(request.request_id, JobStage::Accepted, &mut emit, &cancel)?;
    stage(
        request.request_id,
        JobStage::DecodeStarted,
        &mut emit,
        &cancel,
    )?;
    let mut counted = CountingWriter {
        inner: destination,
        count: 0,
        cap: request.limits.output_bytes,
    };
    let path = io::decode(
        engine,
        source,
        &mut counted,
        request.limits.archive_bytes,
        &OperationLimits {
            memory_bytes: request.limits.memory_bytes,
            output_bytes: request.limits.output_bytes,
            intermediate_bytes: request.limits.intermediate_bytes,
            deadline,
            cancellation: Some(cancel.as_arc()),
        },
    )?;
    stage(
        request.request_id,
        JobStage::DecodeFinished,
        &mut emit,
        &cancel,
    )?;
    stage(
        request.request_id,
        JobStage::OutputWritten,
        &mut emit,
        &cancel,
    )?;
    Ok(JobResult {
        request_id: request.request_id,
        operation: request.operation,
        output_bytes: counted.count,
        selection: None,
        decode_path: Some(
            match path {
                DecodePath::NativeIncremental => "native-incremental",
                DecodePath::NativeCoreBuffered => "native-core-buffered",
                DecodePath::FullFrameBuffered => "full-frame-buffered",
                DecodePath::WindowIncremental => "window-incremental",
            }
            .into(),
        ),
        windows: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{self, NativeOptions},
        full_engine::backend_provider::{BackendProvider, TrustedBackendPaths},
    };
    use std::io::Cursor;
    #[derive(Clone)]
    struct Fake;
    impl CandidateExecutor for Fake {
        fn run(
            &self,
            _: &selection::Plan,
            _: &[u8],
            _: &SelectionLimits,
        ) -> Result<Vec<u8>, String> {
            Ok(b"CJ".to_vec())
        }
    }
    fn request() -> JobRequest {
        JobRequest {
            request_id: 7,
            operation: JobOperation::SelectWholeInput,
            profile: JobProfile::Fast,
            limits: JobLimits {
                input_bytes: 9,
                archive_bytes: 9,
                output_bytes: 9,
                memory_bytes: 9,
                intermediate_bytes: 9,
                temporary_bytes: 9,
                workers: 1,
                deadline_millis: None,
            },
            window_bytes: None,
            diagnostics: true,
        }
    }
    #[test]
    fn unknown_control_kind_rejects_before_body_read() {
        struct HeaderOnlyReader {
            header: Cursor<Vec<u8>>,
        }
        impl Read for HeaderOnlyReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                assert!(self.header.position() < 11, "control body requested");
                self.header.read(buf)
            }
        }
        for version in [JOB_PROTOCOL_V1, JOB_PROTOCOL_VERSION] {
            let mut header = Vec::from(MAGIC);
            header.extend_from_slice(&version.to_be_bytes());
            header.push(7);
            header.extend_from_slice(&(MAX_CONTROL_RECORD_BYTES as u32).to_be_bytes());
            let mut reader = HeaderOnlyReader {
                header: Cursor::new(header),
            };
            assert_eq!(
                read_control(&mut reader).unwrap_err(),
                "unknown control record kind"
            );
            assert_eq!(reader.header.position(), 11);
        }
    }
    fn compact_report() -> JobSelectionReport {
        JobSelectionReport {
            profile: JobProfile::Fast,
            input_bytes: 0,
            discovery: Vec::new(),
            trials: Vec::new(),
            peak_reserved_memory: 0,
            peak_reserved_workers: 0,
            peak_reserved_temporary: 0,
            selected: String::new(),
        }
    }

    fn report_prefix() -> Buf {
        let mut b = Buf::default();
        b.u8(profile_tag(JobProfile::Fast));
        for _ in 0..4 {
            b.usize(0).unwrap();
        }
        b.text("").unwrap();
        b
    }

    #[test]
    fn report_rejects_impossible_discovery_count_before_record_read() {
        // Nine remaining bytes passed the old seven-byte bound, but even
        // empty family/reason strings and two booleans require ten bytes.
        let mut b = report_prefix();
        b.u32(1);
        let record_start = b.bytes.len();
        b.add(&[0; 9]);
        let mut t = Take {
            b: &b.bytes,
            p: 0,
        };
        assert_eq!(
            get_report(&mut t).unwrap_err(),
            "too many discovery diagnostics"
        );
        assert_eq!(t.p, record_start);
    }

    #[test]
    fn report_rejects_impossible_trial_count_before_record_read() {
        // Empty strings, absent options and empty child lists still need
        // 34 bytes; the old 23-byte bound admitted this truncated record.
        let mut b = report_prefix();
        b.u32(0);
        b.u32(1);
        let record_start = b.bytes.len();
        b.add(&[0; 33]);
        let mut t = Take {
            b: &b.bytes,
            p: 0,
        };
        assert_eq!(
            get_report(&mut t).unwrap_err(),
            "too many trial diagnostics"
        );
        assert_eq!(t.p, record_start);
    }

    #[test]
    fn report_rejects_impossible_nested_count_before_record_read() {
        let mut b = report_prefix();
        b.u32(0);
        b.u32(1);
        b.text("").unwrap();
        b.usize(0).unwrap();
        b.text("").unwrap();
        b.opt_text(&None).unwrap();
        b.opt_usize(None).unwrap();
        b.usize(0).unwrap();
        b.u32(0);
        b.u32(1);
        let record_start = b.bytes.len();
        // Four empty strings and four absent options require twenty bytes,
        // rather than the old twelve-byte minimum.
        b.add(&[0; 19]);
        let mut t = Take {
            b: &b.bytes,
            p: 0,
        };
        assert_eq!(
            get_report(&mut t).unwrap_err(),
            "too many nested diagnostics"
        );
        assert_eq!(t.p, record_start);
    }

    #[test]
    fn compact_and_empty_selection_reports_remain_compatible_with_v1_and_v2() {
        let empty = compact_report();
        let mut compact = empty.clone();
        compact.discovery.push(JobDiscovery {
            family: String::new(),
            admitted: false,
            available: false,
            reason: String::new(),
        });
        compact.trials.push(JobTrial {
            id: String::new(),
            priority: 0,
            status: String::new(),
            reason: None,
            archive_bytes: None,
            nested: vec![JobDiagnostic {
                kind: String::new(),
                id: String::new(),
                status: String::new(),
                region: None,
                backend: None,
                bytes_unit: String::new(),
                bytes: None,
                detail: None,
            }],
            nested_omitted: 0,
            provider_omissions: Vec::new(),
        });
        for report in [empty, compact] {
            // Also decode without result trailers so the final nested record
            // exercises its exact twenty-byte minimum at the end of input.
            let mut encoded = Buf::default();
            put_report(&mut encoded, &report).unwrap();
            let mut t = Take {
                b: &encoded.bytes,
                p: 0,
            };
            assert_eq!(get_report(&mut t).unwrap(), report);
            t.end().unwrap();
            for version in [JOB_PROTOCOL_V1, JOB_PROTOCOL_VERSION] {
                let expected = ControlRecord::Result(JobResult {
                    request_id: 7,
                    operation: JobOperation::SelectWholeInput,
                    output_bytes: 0,
                    selection: Some(report.clone()),
                    windows: None,
                    decode_path: None,
                });
                let mut payload = Buf::default();
                payload.u64(7);
                payload.u8(1); // Whole-input selection in both wire versions.
                payload.usize(0).unwrap();
                payload.u8(1);
                put_report(&mut payload, &report).unwrap();
                payload.opt_text(&None).unwrap();
                if version == JOB_PROTOCOL_VERSION {
                    put_window_report(&mut payload, &None).unwrap();
                }
                let mut frame = Vec::from(MAGIC);
                frame.extend_from_slice(&version.to_be_bytes());
                frame.push(4);
                frame.extend_from_slice(&(payload.bytes.len() as u32).to_be_bytes());
                frame.extend_from_slice(&payload.bytes);
                let mut reader = Cursor::new(frame);
                assert_eq!(read_control(&mut reader).unwrap(), expected);
                assert_eq!(reader.position() as usize, reader.get_ref().len());
            }
        }
    }

    #[test]
    fn control_round_trip() {
        let r = ControlRecord::Request(request());
        let mut b = Vec::new();
        write_control(&mut b, &r).unwrap();
        assert_eq!(read_control(&mut Cursor::new(b)).unwrap(), r)
    }
    #[test]
    fn capabilities_describe_bounded_whole_selection_and_cixw1_decode() {
        let capability = capabilities();
        assert!(capability.select_whole_input && capability.decode_archive);
        assert!(capability.select_independent_windows);
        assert!(capability.data_transport.contains("128 MiB"));
        assert!(capability.data_transport.contains("CIXW1"));
    }
    #[test]
    fn rejects_unknown_version_oversize_truncated_and_utf8() {
        let mut b = Vec::new();
        write_control(&mut b, &ControlRecord::Request(request())).unwrap();
        b[4] = 0;
        b[5] = 3;
        assert!(read_control(&mut Cursor::new(b)).is_err());
        let mut h = Vec::from(MAGIC);
        h.extend_from_slice(&JOB_PROTOCOL_VERSION.to_be_bytes());
        h.push(1);
        h.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(read_control(&mut Cursor::new(h)).is_err());
        assert!(read_control(&mut Cursor::new(vec![0; 3])).is_err());
        let mut p = Buf::default();
        p.u64(1);
        p.u8(0);
        p.text("x").unwrap();
        let mut raw = Vec::from(MAGIC);
        raw.extend_from_slice(&JOB_PROTOCOL_VERSION.to_be_bytes());
        raw.push(5);
        raw.extend_from_slice(&(p.bytes.len() as u32).to_be_bytes());
        raw.extend_from_slice(&p.bytes);
        let at = HEADER + 9;
        raw[at] = 0xff;
        assert!(read_control(&mut Cursor::new(raw)).is_err());
    }
    #[test]
    fn direct_selection_entry_applies_limits_cancel_and_reports_actual_output() {
        let mut r = request();
        r.limits.input_bytes = 3;
        r.limits.archive_bytes = 16;
        r.limits.output_bytes = 16;
        r.limits.memory_bytes = 64 << 20;
        r.limits.intermediate_bytes = 1 << 16;
        r.limits.temporary_bytes = 1 << 16;
        let mut source = Cursor::new(b"abc".to_vec());
        let mut out = Vec::new();
        let mut stages = Vec::new();
        let result =
            execute_select_whole_input(&r, &mut source, &mut out, Fake, CancelToken::new(), |p| {
                stages.push(p.stage)
            })
            .unwrap();
        assert_eq!(result.output_bytes, out.len());
        assert!(result.selection.is_some());
        assert_eq!(stages.last(), Some(&JobStage::OutputWritten));
        let cancelled = CancelToken::new();
        cancelled.cancel();
        assert!(execute_select_whole_input(
            &r,
            &mut Cursor::new(b"a".to_vec()),
            &mut Vec::new(),
            Fake,
            cancelled,
            |_| {}
        )
        .is_err());
    }
    fn window_request(input_cap: usize) -> JobRequest {
        JobRequest {
            request_id: 8,
            operation: JobOperation::SelectIndependentWindows,
            profile: JobProfile::Fast,
            limits: JobLimits {
                input_bytes: input_cap,
                archive_bytes: 4096,
                output_bytes: 4096,
                memory_bytes: 64 << 20,
                intermediate_bytes: 1 << 20,
                temporary_bytes: 1 << 20,
                workers: 1,
                deadline_millis: None,
            },
            window_bytes: Some(3),
            diagnostics: true,
        }
    }
    #[test]
    fn window_selection_handles_empty_and_multiple_independent_records() {
        let mut archive = Vec::new();
        let empty = execute_select_independent_windows(
            &window_request(1),
            Cursor::new(Vec::new()),
            &mut archive,
            Fake,
            CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert!(archive.starts_with(window_stream::MAGIC));
        let report = empty.windows.expect("window report");
        assert_eq!(report.windows, 0);
        assert_eq!(report.input_bytes, 0);
        assert_eq!(report.independent_windows, 0);
        assert!(report
            .whole_only_omissions
            .iter()
            .any(|x| x.contains("resets")));

        let mut archive = Vec::new();
        let result = execute_select_independent_windows(
            &window_request(7),
            Cursor::new(b"abcdefg".to_vec()),
            &mut archive,
            Fake,
            CancelToken::new(),
            |_| {},
        )
        .unwrap();
        let report = result.windows.expect("window report");
        assert_eq!(
            (
                report.windows,
                report.input_bytes,
                report.independent_windows
            ),
            (3, 7, 3)
        );
        assert_eq!(result.output_bytes, archive.len());
    }
    #[test]
    fn window_selection_enforces_cumulative_cap_and_cancel_before_writing() {
        let mut archive = Vec::new();
        let error = execute_select_independent_windows(
            &window_request(2),
            Cursor::new(b"abc".to_vec()),
            &mut archive,
            Fake,
            CancelToken::new(),
            |_| {},
        )
        .unwrap_err();
        assert!(error.contains("declared cap"));
        let cancelled = CancelToken::new();
        cancelled.cancel();
        let mut archive = Vec::new();
        assert!(execute_select_independent_windows(
            &window_request(3),
            Cursor::new(b"abc".to_vec()),
            &mut archive,
            Fake,
            cancelled,
            |_| {},
        )
        .is_err());
        assert!(archive.is_empty());

        let mut expired = window_request(3);
        expired.limits.deadline_millis = Some(0);
        let mut archive = Vec::new();
        assert!(execute_select_independent_windows(
            &expired,
            Cursor::new(b"abc".to_vec()),
            &mut archive,
            Fake,
            CancelToken::new(),
            |_| {},
        )
        .is_err());
        assert!(archive.is_empty());
    }
    #[test]
    fn v1_whole_request_remains_a_valid_one_shot_request() {
        let r = request();
        let mut payload = Buf::default();
        payload.u64(r.request_id);
        payload.u8(1); // v1 whole-input operation
        payload.u8(profile_tag(r.profile));
        put_limits(&mut payload, &r.limits).unwrap();
        payload.u8(u8::from(r.diagnostics));
        let mut wire = Vec::from(MAGIC);
        wire.extend_from_slice(&JOB_PROTOCOL_V1.to_be_bytes());
        wire.push(1);
        wire.extend_from_slice(&(payload.bytes.len() as u32).to_be_bytes());
        wire.extend_from_slice(&payload.bytes);
        let ControlRecord::Request(decoded) = read_control(&mut Cursor::new(wire)).unwrap() else {
            panic!("expected request")
        };
        assert_eq!(decoded.operation, JobOperation::SelectWholeInput);
        assert_eq!(decoded.window_bytes, None);
        assert_eq!(decoded.request_id, r.request_id);

        let mut decode_payload = Buf::default();
        decode_payload.u64(r.request_id);
        decode_payload.u8(2); // v1 decode operation
        decode_payload.u8(profile_tag(r.profile));
        put_limits(&mut decode_payload, &r.limits).unwrap();
        decode_payload.u8(u8::from(r.diagnostics));
        let ControlRecord::Request(decoded) =
            read_control(&mut Cursor::new(v1_wire(1, decode_payload))).unwrap()
        else {
            panic!("expected request")
        };
        assert_eq!(decoded.operation, JobOperation::DecodeArchive);
        assert_eq!(decoded.window_bytes, None);
    }
    fn v1_wire(kind: u8, payload: Buf) -> Vec<u8> {
        let mut wire = Vec::from(MAGIC);
        wire.extend_from_slice(&JOB_PROTOCOL_V1.to_be_bytes());
        wire.push(kind);
        wire.extend_from_slice(&(payload.bytes.len() as u32).to_be_bytes());
        wire.extend_from_slice(&payload.bytes);
        wire
    }
    #[test]
    fn v1_capability_and_result_records_keep_their_original_layout() {
        let mut capability = Buf::default();
        capability.u16(JOB_PROTOCOL_V1);
        capability.u8(1);
        capability.u8(1);
        capability.text("files").unwrap();
        capability.text("cooperative").unwrap();
        let ControlRecord::Capability(capability) =
            read_control(&mut Cursor::new(v1_wire(2, capability))).unwrap()
        else {
            panic!("expected capability")
        };
        assert_eq!(capability.protocol_version, JOB_PROTOCOL_V1);
        assert!(!capability.select_independent_windows);

        for (tag, expected) in [
            (1, JobOperation::SelectWholeInput),
            (2, JobOperation::DecodeArchive),
        ] {
            let mut result = Buf::default();
            result.u64(19);
            result.u8(tag);
            result.usize(23).unwrap();
            result.u8(0);
            result.opt_text(&None).unwrap();
            let ControlRecord::Result(result) =
                read_control(&mut Cursor::new(v1_wire(4, result))).unwrap()
            else {
                panic!("expected result")
            };
            assert_eq!(result.operation, expected);
            assert_eq!(result.windows, None);
            assert_eq!(result.output_bytes, 23);
        }
    }
    #[test]
    fn v1_progress_error_and_cancel_records_remain_accepted() {
        let mut progress = Buf::default();
        progress.u64(31);
        progress.u8(4);
        assert_eq!(
            read_control(&mut Cursor::new(v1_wire(3, progress))).unwrap(),
            ControlRecord::Progress(JobProgress {
                request_id: 31,
                stage: JobStage::SelectionFinished
            })
        );
        let mut error = Buf::default();
        error.u64(32);
        error.u8(JobErrorCode::Execution as u8);
        error.text("legacy error").unwrap();
        assert_eq!(
            read_control(&mut Cursor::new(v1_wire(5, error))).unwrap(),
            ControlRecord::Error(JobError {
                request_id: 32,
                code: JobErrorCode::Execution,
                message: "legacy error".into()
            })
        );
        let mut cancel = Buf::default();
        cancel.u64(33);
        assert_eq!(
            read_control(&mut Cursor::new(v1_wire(6, cancel))).unwrap(),
            ControlRecord::Cancel { request_id: 33 }
        );
    }
    #[test]
    fn rejects_mismatched_capability_version_and_invalid_v2_window_flag() {
        let mut capability = Buf::default();
        capability.u16(JOB_PROTOCOL_V1);
        capability.u8(1);
        capability.u8(1);
        capability.u8(0);
        capability.text("files").unwrap();
        capability.text("cooperative").unwrap();
        let mut mismatch = Vec::from(MAGIC);
        mismatch.extend_from_slice(&JOB_PROTOCOL_VERSION.to_be_bytes());
        mismatch.push(2);
        mismatch.extend_from_slice(&(capability.bytes.len() as u32).to_be_bytes());
        mismatch.extend_from_slice(&capability.bytes);
        assert!(read_control(&mut Cursor::new(mismatch)).is_err());

        let mut result = Buf::default();
        result.u64(41);
        result.u8(2);
        result.usize(0).unwrap();
        result.u8(0);
        result.opt_text(&None).unwrap();
        result.u8(2);
        let mut malformed = Vec::from(MAGIC);
        malformed.extend_from_slice(&JOB_PROTOCOL_VERSION.to_be_bytes());
        malformed.push(4);
        malformed.extend_from_slice(&(result.bytes.len() as u32).to_be_bytes());
        malformed.extend_from_slice(&result.bytes);
        assert!(read_control(&mut Cursor::new(malformed)).is_err());
    }
    #[test]
    fn decode_entry_counts_actual_native_output_and_finishes_stage() {
        let mut archive = Vec::new();
        core::encode(
            Cursor::new(b"job".to_vec()),
            &mut archive,
            &NativeOptions {
                output_limit: 1024,
                memory_limit: 8 << 20,
                ..NativeOptions::default()
            },
        )
        .unwrap();
        let engine = FullEngine {
            backends: BackendProvider {
                paths: TrustedBackendPaths {
                    cix: "/not-used/cix".into(),
                    paq_libraries: "/not-used/lib".into(),
                    temporary_root: "/not-used/tmp".into(),
                },
            },
            images: None,
            spatial: None,
        };
        let mut r = request();
        r.operation = JobOperation::DecodeArchive;
        r.limits.archive_bytes = 1024;
        r.limits.output_bytes = 32;
        r.limits.memory_bytes = 64 << 20;
        r.limits.intermediate_bytes = 1 << 20;
        let mut out = Vec::new();
        let mut stages = Vec::new();
        let result = execute_decode(
            &r,
            &engine,
            Cursor::new(archive),
            &mut out,
            CancelToken::new(),
            |p| stages.push(p.stage),
        )
        .unwrap();
        assert_eq!(result.output_bytes, out.len());
        assert_eq!(out, b"job");
        assert_eq!(stages.last(), Some(&JobStage::OutputWritten));
    }
    #[test]
    fn decode_entry_reports_incremental_window_path() {
        use crate::full_engine::window_stream::{self, EncodeLimits};

        let source: Vec<u8> = (0u8..129)
            .map(|index| index.wrapping_mul(37).wrapping_add(11))
            .collect();
        assert_eq!(source.len(), 129);
        let mut encoded_window_lengths = Vec::new();
        let mut archive = Vec::new();
        window_stream::encode(
            Cursor::new(source.clone()),
            &mut archive,
            &EncodeLimits {
                window_bytes: 64,
                memory_bytes: 8 << 20,
                output_bytes: 1024,
                deadline: None,
                cancellation: None,
            },
            |window, memory| {
                encoded_window_lengths.push(window.len());
                core::encode_buffer(
                    window,
                    &NativeOptions {
                        output_limit: 512,
                        memory_limit: memory,
                        ..NativeOptions::default()
                    },
                )
                .map_err(|error| error.to_string())
            },
        )
        .unwrap();
        assert_eq!(encoded_window_lengths, [64, 64, 1]);
        let engine = FullEngine {
            backends: BackendProvider {
                paths: TrustedBackendPaths {
                    cix: "/not-used/cix".into(),
                    paq_libraries: "/not-used/lib".into(),
                    temporary_root: "/not-used/tmp".into(),
                },
            },
            images: None,
            spatial: None,
        };
        let mut request = request();
        request.operation = JobOperation::DecodeArchive;
        request.limits.archive_bytes = 1024;
        request.limits.output_bytes = source.len();
        request.limits.memory_bytes = 64 << 20;
        request.limits.intermediate_bytes = 1 << 20;
        let mut output = Vec::new();
        let mut stages = Vec::new();
        let result = execute_decode(
            &request,
            &engine,
            Cursor::new(archive),
            &mut output,
            CancelToken::new(),
            |progress| stages.push(progress.stage),
        )
        .unwrap();
        assert_eq!(output, source);
        assert_eq!(result.operation, JobOperation::DecodeArchive);
        assert_eq!(result.output_bytes, 129);
        assert_eq!(result.output_bytes, output.len());
        assert_eq!(result.decode_path.as_deref(), Some("window-incremental"));
        assert_eq!(stages.last(), Some(&JobStage::OutputWritten));
    }
}
