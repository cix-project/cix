//! Bounded CIXB1 reference-backend envelope codecs.
// Native C++ backends are linked into every CLI, including when Cargo does
// not pull a Rust rlib member that carries cc's C++ runtime link metadata.
#[cfg(target_os = "linux")]
#[link(name = "stdc++")]
unsafe extern "C" {}
#[cfg(target_os = "macos")]
#[link(name = "c++")]
unsafe extern "C" {}

use flate2::{bufread::GzDecoder, write::ZlibEncoder, Compression, GzBuilder};
use sha2::{Digest, Sha256};
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::io::{Cursor, Read, Write};

struct CappedWriter {
    bytes: Vec<u8>,
    cap: usize,
}
impl Write for CappedWriter {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        if input.len() > self.cap.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("output cap"));
        }
        self.bytes
            .try_reserve_exact(input.len())
            .map_err(std::io::Error::other)?;
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// CIXB1 direct backends may call a native one-shot implementation.  The
// shared flag is observed before and after those calls; one-shot libraries
// cannot be preempted safely in the middle of a call.
#[inline]
fn check_cancel() -> Result<(), String> {
    crate::limits::check()
}

// Whole-input trials are bounded by a fixed safety ceiling and aggregate
// memory admission. u32 lengths are unchanged; existing archives remain valid.
// 128 MiB admits the verified 100 MB catalog input when resources permit.
pub const MAX_INPUT: usize = 128 * 1024 * 1024;
// Bzip2 permits n + n/100 + 601 output; 2 MiB safely covers
// that bound at the 128 MiB input ceiling, as well as wrapper slack.
pub const MAX_PAYLOAD: usize = MAX_INPUT + 2 * 1024 * 1024;
const HEADER: usize = 47;
/// bzip2, Brotli, zstd and libbsc use stable one-shot APIs in this bounded
/// envelope. Cancellation is observed immediately before and after each
/// call, so interruption latency can be one native call. XZ encoding is
/// incrementally polled per 64 KiB output chunk; its existing bounded decode
/// ABI remains one-shot. libbsc's BWT/QLFC block call is also coarse and can
/// delay SIGINT or a BEST deadline until a block of up to 128 MiB returns.
/// Replacing these calls with streaming state machines is a separate
/// compatibility/performance change and must be measured before it is
/// claimed as an improvement.
pub const ONE_SHOT_CANCELLATION_LIMITATION: &str =
    "bzip2/Brotli/zstd/libbsc and XZ decode use one-shot native calls with before/after cancellation checks; libbsc can delay cancellation for a block up to 128 MiB; XZ encode polls per 64 KiB output chunk; pinned ZPAQ polls cancellation in source reads and at most every 4096 interpreted ZPAQL instructions";

/// Called from the in-process ZPAQ C++ bridge. Keep this small and allocation
/// free so SIGINT/deadline cancellation is observed inside level-5 model work,
/// rather than only after a potentially long native call returns.
extern "C" fn zpaq_cancelled() -> bool {
    crate::limits::check().is_err()
}

#[link(name = "bz2")]
unsafe extern "C" {
    fn BZ2_bzBuffToBuffCompress(
        dest: *mut c_char,
        dest_len: *mut c_uint,
        source: *mut c_char,
        source_len: c_uint,
        block_size_100k: c_int,
        verbosity: c_int,
        work_factor: c_int,
    ) -> c_int;
    fn BZ2_bzDecompressInit(stream: *mut BzStream, verbosity: c_int, small: c_int) -> c_int;
    fn BZ2_bzDecompress(stream: *mut BzStream) -> c_int;
    fn BZ2_bzDecompressEnd(stream: *mut BzStream) -> c_int;
    fn BZ2_bzlibVersion() -> *const c_char;
}

#[link(name = "z")]
unsafe extern "C" {
    fn zlibVersion() -> *const c_char;
}
#[repr(C)]
struct BzStream {
    next_in: *mut c_char,
    avail_in: c_uint,
    total_in_lo32: c_uint,
    total_in_hi32: c_uint,
    next_out: *mut c_char,
    avail_out: c_uint,
    total_out_lo32: c_uint,
    total_out_hi32: c_uint,
    state: *mut c_void,
    bzalloc: Option<unsafe extern "C" fn(*mut c_void, c_int, c_int) -> *mut c_void>,
    bzfree: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    opaque: *mut c_void,
}

#[link(name = "lzma")]
unsafe extern "C" {
    fn lzma_stream_buffer_bound(uncompressed_size: usize) -> usize;
    fn lzma_easy_buffer_encode(
        preset: u32,
        check: u32,
        allocator: *const c_void,
        input: *const u8,
        input_size: usize,
        output: *mut u8,
        output_pos: *mut usize,
        output_size: usize,
    ) -> c_int;
    fn lzma_easy_encoder_memusage(preset: u32) -> u64;
    fn lzma_raw_encoder_memusage(filters: *const LzmaFilter) -> u64;
    fn lzma_easy_encoder(stream: *mut LzmaStream, preset: u32, check: u32) -> c_int;
    fn lzma_stream_encoder(stream: *mut LzmaStream, filters: *mut LzmaFilter, check: u32) -> c_int;
    fn lzma_lzma_preset(options: *mut LzmaOptionsLzma, preset: u32) -> c_int;
    fn lzma_code(stream: *mut LzmaStream, action: c_int) -> c_int;
    fn lzma_end(stream: *mut LzmaStream);
    fn lzma_stream_buffer_decode(
        memlimit: *mut u64,
        flags: u32,
        allocator: *const c_void,
        input: *const u8,
        input_pos: *mut usize,
        input_size: usize,
        output: *mut u8,
        output_pos: *mut usize,
        output_size: usize,
    ) -> u32;
    fn lzma_version_string() -> *const c_char;
}

// ABI layout from liblzma's lzma_stream. Streaming encode leaves the XZ block
// size unknown, matching Python's canonical liblzma stream output.
#[repr(C)]
struct LzmaStream {
    next_in: *const u8,
    avail_in: usize,
    total_in: u64,
    next_out: *mut u8,
    avail_out: usize,
    total_out: u64,
    allocator: *const c_void,
    internal: *mut c_void,
    reserved_ptr1: *mut c_void,
    reserved_ptr2: *mut c_void,
    reserved_ptr3: *mut c_void,
    reserved_ptr4: *mut c_void,
    seek_pos: u64,
    reserved_int2: u64,
    reserved_int3: usize,
    reserved_int4: usize,
    reserved_enum1: c_int,
    reserved_enum2: c_int,
}

impl Drop for LzmaStream {
    fn drop(&mut self) {
        // lzma_end accepts the initialized zero/null stream too. This also
        // releases native state on a cooperative cancellation or deadline.
        unsafe { lzma_end(self) };
    }
}

#[repr(C)]
struct LzmaOptionsLzma {
    dict_size: u32,
    preset_dict: *const u8,
    preset_dict_size: u32,
    lc: u32,
    lp: u32,
    pb: u32,
    mode: c_int,
    nice_len: u32,
    mf: c_int,
    depth: u32,
    ext_flags: u32,
    ext_size_low: u32,
    ext_size_high: u32,
    reserved_int4: u32,
    reserved_int5: u32,
    reserved_int6: u32,
    reserved_int7: u32,
    reserved_int8: u32,
    reserved_enum1: c_int,
    reserved_enum2: c_int,
    reserved_enum3: c_int,
    reserved_enum4: c_int,
    reserved_ptr1: *mut c_void,
    reserved_ptr2: *mut c_void,
}

#[repr(C)]
struct LzmaFilter {
    id: u64,
    options: *mut c_void,
}

#[link(name = "zstd")]
unsafe extern "C" {
    fn ZSTD_compressBound(src_size: usize) -> usize;
    fn ZSTD_compress(
        dst: *mut c_void,
        dst_capacity: usize,
        src: *const c_void,
        src_size: usize,
        level: c_int,
    ) -> usize;
    fn ZSTD_decompress(
        dst: *mut c_void,
        dst_capacity: usize,
        src: *const c_void,
        src_size: usize,
    ) -> usize;
    fn ZSTD_findFrameCompressedSize(src: *const c_void, src_size: usize) -> usize;
    fn ZSTD_isError(code: usize) -> u32;
    fn ZSTD_getErrorName(code: usize) -> *const c_char;
    // This is declared as a static-only API by zstd because callers that
    // dynamically load zstd cannot rely on it.  CIX links to zstd directly,
    // and the installed shared library exports it, so it is the appropriate
    // estimator for our one-shot `ZSTD_compress` call.
    fn ZSTD_estimateCCtxSize(compression_level: c_int) -> usize;
    fn ZSTD_versionString() -> *const c_char;
}

/// A legal, already implemented direct CIXB1 backend configuration.
///
/// These descriptors are deliberately configuration-level rather than just
/// backend-level.  For example, `xz-size-9e` and `xz-dict128-preset6` have
/// different encoder state and can produce different complete archives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CandidateDescriptor {
    pub id: &'static str,
    pub backend: &'static str,
    pub profile: &'static str,
    pub config: CandidateConfig,
}

/// Extra configuration carried by a CIXB1 candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateConfig {
    /// The canonical `encode(backend, profile, input)` configuration.
    Canonical,
    /// An existing XZ stream with a 128 MiB dictionary and an explicit preset.
    XzDictionary { dictionary_bytes: u32, preset: u32 },
    /// Brotli's generic encoder mode.  Quality/window affect encoding only;
    /// the emitted Brotli payload carries the decoder-relevant window and is
    /// enclosed in the regular CIXB1 backend/profile envelope.  No custom or
    /// source-specific dictionary is supplied.
    BrotliQuality {
        quality: u8,
        lgwin: u8,
        mode: BrotliMode,
    },
    /// libbsc BWT/QLFC settings. The libbsc payload records the decoder
    /// semantics; these fields make the encoder-side choice observable and
    /// keep future BSC candidates from sharing an ambiguous profile label.
    Bsc {
        lzp_hash_size: u8,
        lzp_min_len: u8,
        adaptive_coder: bool,
        fast_mode: bool,
    },
    /// Pinned libzpaq 7.15 built-in level. Its payload is self-describing.
    Zpaq { level: u8 },
}

/// Brotli's mode is typed so a future text/font setting cannot silently be
/// selected under a generic candidate identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrotliMode {
    Generic,
}

/// Why a complete CIXB1 candidate could not be admitted under the caller's
/// remaining budget.  It is structured so explain output never turns a
/// resource omission into a silent portfolio gap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateOmission {
    pub needed_bytes: usize,
    pub available_bytes: usize,
    pub reason: String,
}

const CIXB1_BEST_CANDIDATES: &[CandidateDescriptor] = &[
    CandidateDescriptor {
        id: "gzip-fast",
        backend: "gzip",
        profile: "fast",
        config: CandidateConfig::Canonical,
    },
    // gzip's default settings equal its size settings.  The `size` envelope
    // is retained as the single deterministic representative.
    CandidateDescriptor {
        id: "gzip-size",
        backend: "gzip",
        profile: "size",
        config: CandidateConfig::Canonical,
    },
    CandidateDescriptor {
        id: "bzip2-size",
        backend: "bzip2",
        profile: "size",
        config: CandidateConfig::Canonical,
    },
    CandidateDescriptor {
        id: "xz-default-6",
        backend: "xz",
        profile: "default",
        config: CandidateConfig::Canonical,
    },
    CandidateDescriptor {
        id: "xz-size-9e",
        backend: "xz",
        profile: "size",
        config: CandidateConfig::Canonical,
    },
    CandidateDescriptor {
        id: "zstd-fast",
        backend: "zstd",
        profile: "fast",
        config: CandidateConfig::Canonical,
    },
    CandidateDescriptor {
        id: "zstd-default-3",
        backend: "zstd",
        profile: "default",
        config: CandidateConfig::Canonical,
    },
    CandidateDescriptor {
        id: "zstd-size-22",
        backend: "zstd",
        profile: "size",
        config: CandidateConfig::Canonical,
    },
    CandidateDescriptor {
        id: "brotli-fast-0",
        backend: "brotli",
        profile: "fast",
        config: CandidateConfig::Canonical,
    },
    // The previous registry incorrectly treated its historical fast envelope
    // as Brotli coverage.  Quality 11 is a legal generic, self-describing
    // Brotli payload inside CIXB1's existing `brotli/size` envelope.  Its
    // configuration is explicit in this typed descriptor and explain output;
    // the decoder needs neither the quality nor a dictionary to restore it.
    CandidateDescriptor {
        id: "brotli-size-q11-generic-w22",
        backend: "brotli",
        profile: "size",
        config: CandidateConfig::BrotliQuality {
            quality: 11,
            lgwin: 22,
            mode: BrotliMode::Generic,
        },
    },
    CandidateDescriptor {
        id: "xz-dict128-size-9e",
        backend: "xz",
        profile: "size",
        config: CandidateConfig::XzDictionary {
            dictionary_bytes: 128 * 1024 * 1024,
            preset: (1 << 31) | 9,
        },
    },
    CandidateDescriptor {
        id: "xz-dict128-preset6",
        backend: "xz",
        profile: "size",
        config: CandidateConfig::XzDictionary {
            dictionary_bytes: 128 * 1024 * 1024,
            preset: 6,
        },
    },
    CandidateDescriptor {
        id: "bsc-v3.3.12-bwt-qlfc-static-lzp15-72-fast",
        backend: "bsc",
        profile: "size",
        config: CandidateConfig::Bsc {
            lzp_hash_size: 15,
            lzp_min_len: 72,
            adaptive_coder: false,
            fast_mode: true,
        },
    },
    CandidateDescriptor {
        id: "zpaq715-l5",
        backend: "zpaq",
        profile: "size",
        config: CandidateConfig::Zpaq { level: 5 },
    },
];

/// Every direct backend configuration automatically eligible to BEST.
///
/// `gzip-default` is intentionally omitted: it uses the same deflate level
/// as `gzip-size`, while the latter is the historical size-profile envelope.
pub fn best_candidates() -> &'static [CandidateDescriptor] {
    // Each descriptor produces a complete CIXB1 archive and has a
    // conservative resource admission calculation. BEST compares materialized
    // archives, including their wrapper, before selection. A caller that
    // cannot admit a large dictionary/table receives an explicit omission.
    CIXB1_BEST_CANDIDATES
}

/// Compatibility view for callers which previously inspected candidates that
/// were awaiting promotion. Every implemented direct CIXB1 configuration is
/// now eligible for BEST; an empty result is therefore expected.
pub fn experimental_candidates() -> &'static [CandidateDescriptor] {
    &[]
}

/// Runtime version of the native backend selected by a descriptor. Evidence
/// recorders can use this without guessing package-manager versions.
pub fn backend_version(backend: &str) -> Result<String, String> {
    unsafe {
        let version = match backend {
            "gzip" => CStr::from_ptr(zlibVersion()).to_string_lossy().into_owned(),
            "bzip2" => CStr::from_ptr(BZ2_bzlibVersion())
                .to_string_lossy()
                .into_owned(),
            "xz" => CStr::from_ptr(lzma_version_string())
                .to_string_lossy()
                .into_owned(),
            "zstd" => CStr::from_ptr(ZSTD_versionString())
                .to_string_lossy()
                .into_owned(),
            "brotli" => {
                let version = BrotliEncoderVersion();
                format!(
                    "{}.{}.{}",
                    version >> 24,
                    (version >> 12) & 0x0fff,
                    version & 0x0fff
                )
            }
            "zpaq" => CStr::from_ptr(cix_zpaq715_version())
                .to_string_lossy()
                .into_owned(),
            "bsc" => crate::bsc_ffi::LIBBSC_VERSION.to_string(),
            _ => return Err(format!("unsupported CIXB1 backend: {backend}")),
        };
        if version.is_empty() {
            Err(format!("native backend {backend} did not report a version"))
        } else {
            Ok(version)
        }
    }
}

impl CandidateDescriptor {
    /// Materialise the complete CIXB1 archive, including its header, length
    /// fields and checksum.  Callers must compare this result directly.
    pub fn full_archive_encode(&self, input: &[u8]) -> Result<Vec<u8>, String> {
        match self.config {
            CandidateConfig::Canonical => encode(self.backend, self.profile, input),
            CandidateConfig::XzDictionary {
                dictionary_bytes,
                preset,
            } => encode_xz_dictionary_preset(self.profile, input, dictionary_bytes, preset),
            CandidateConfig::BrotliQuality {
                quality,
                lgwin,
                mode,
            } => {
                if self.backend != "brotli" {
                    return Err(format!(
                        "Brotli configuration used by non-Brotli candidate {}",
                        self.id
                    ));
                }
                encode_brotli_quality(self.profile, input, quality, lgwin, mode)
            }
            CandidateConfig::Zpaq { level } => {
                if self.backend != "zpaq" {
                    return Err(format!(
                        "ZPAQ configuration used by non-ZPAQ candidate {}",
                        self.id
                    ));
                }
                encode_zpaq(self.profile, input, level)
            }
            CandidateConfig::Bsc {
                lzp_hash_size,
                lzp_min_len,
                adaptive_coder,
                fast_mode,
            } => {
                if self.backend != "bsc" {
                    return Err(format!(
                        "BSC configuration used by non-BSC candidate {}",
                        self.id
                    ));
                }
                encode_bsc(
                    self.profile,
                    input,
                    crate::bsc_ffi::BscOptions {
                        lzp_hash_size,
                        lzp_min_len,
                        adaptive_coder,
                        fast_mode,
                    },
                )
            }
        }
    }

    /// Conservative additional encoder memory required after the caller has
    /// retained the source input and any incumbent archive.  This includes
    /// native codec state, payload storage and the separately allocated CIXB1
    /// wrapper.  It is intentionally an admission estimate, not an RSS claim.
    pub fn encoder_peak_bytes(&self, input_len: usize) -> Result<usize, String> {
        if input_len > MAX_INPUT {
            return Err("CIXB1 input exceeds 128 MiB limit".into());
        }
        let payload = input_len
            .checked_add(2 * 1024 * 1024)
            .ok_or_else(|| "CIXB1 payload estimate overflow".to_string())?;
        // All ordinary backends hold a payload vector and then construct a
        // complete wrapper vector.  The dictionary XZ encoder grows its
        // payload dynamically, so reserve one further payload bound for a
        // potential capacity growth before it becomes the wrapper.
        let copies = match self.config {
            CandidateConfig::Canonical => 2usize,
            CandidateConfig::XzDictionary { .. }
            | CandidateConfig::BrotliQuality { .. }
            | CandidateConfig::Zpaq { .. }
            | CandidateConfig::Bsc { .. } => 3usize,
        };
        let storage = payload
            .checked_mul(copies)
            .and_then(|v| v.checked_add(HEADER))
            .ok_or_else(|| "CIXB1 output estimate overflow".to_string())?;
        let native = self.native_encoder_bytes(input_len)?;
        storage
            .checked_add(native)
            .and_then(|v| v.checked_add(128 * 1024)) // stream chunks and ABI bookkeeping
            .ok_or_else(|| "CIXB1 encoder estimate overflow".to_string())
    }

    /// Admit this candidate against the budget remaining after source and
    /// incumbent ownership have been charged by the automatic selector.
    pub fn admit(
        &self,
        input_len: usize,
        available_bytes: usize,
    ) -> Result<usize, CandidateOmission> {
        let needed_bytes = match self.encoder_peak_bytes(input_len) {
            Ok(needed) => needed,
            Err(reason) => {
                return Err(CandidateOmission {
                    needed_bytes: usize::MAX,
                    available_bytes,
                    reason,
                });
            }
        };
        if needed_bytes > available_bytes {
            return Err(CandidateOmission {
                needed_bytes,
                available_bytes,
                reason: format!(
                    "need {needed_bytes} bytes for CIXB1 {} ({}), available {available_bytes}",
                    self.id, self.backend
                ),
            });
        }
        Ok(needed_bytes)
    }

    fn native_encoder_bytes(&self, input_len: usize) -> Result<usize, String> {
        let bytes = match self.config {
            CandidateConfig::XzDictionary {
                dictionary_bytes,
                preset,
            } => xz_dictionary_memory_usage(dictionary_bytes, preset)?,
            CandidateConfig::Canonical if self.backend == "xz" => {
                let preset = match self.profile {
                    "fast" => 0,
                    "default" | "current" => 6,
                    "size" => (1 << 31) | 9,
                    _ => return Err(format!("unsupported xz profile {}", self.profile)),
                };
                xz_easy_memory_usage(preset)
            }
            CandidateConfig::Canonical if self.backend == "zstd" => {
                let level = match self.profile {
                    "fast" => -131072,
                    "default" | "current" => 3,
                    "size" => 22,
                    _ => return Err(format!("unsupported zstd profile {}", self.profile)),
                };
                zstd_memory_usage(level)
            }
            CandidateConfig::Canonical if self.backend == "gzip" => 16 * 1024 * 1024,
            CandidateConfig::Canonical if self.backend == "bzip2" => 24 * 1024 * 1024,
            CandidateConfig::Canonical if self.backend == "brotli" => 16 * 1024 * 1024,
            // Brotli's documented maximum quality makes substantially larger
            // encoder tables than quality 0.  Keep an intentionally
            // conservative admission bound; this is not an RSS assertion.
            CandidateConfig::BrotliQuality {
                quality,
                lgwin,
                mode: BrotliMode::Generic,
            } if self.backend == "brotli" && quality <= 11 && (10..=24).contains(&lgwin) => {
                256 * 1024 * 1024
            }
            // Method 5 has fixed 16 MiB blocks plus adaptive tables whose
            // allocation is model-dependent. The conservative admission is
            // intentionally a resource guard rather than an RSS claim.
            CandidateConfig::Zpaq { level: 5 } if self.backend == "zpaq" => 1024 * 1024 * 1024,
            CandidateConfig::Bsc { .. } if self.backend == "bsc" => {
                // Upstream's CPU estimate is 16 MiB + 5 × block bytes. The
                // FFI wrapper additionally owns aligned input/output and a
                // copied payload, so reserve three further block copies.
                16 * 1024 * 1024 + input_len.saturating_mul(8)
            }
            _ => return Err(format!("unsupported CIXB1 candidate {}", self.id)),
        };
        Ok(bytes)
    }

    /// Conservative decoder admission for a complete CIXB1 candidate whose
    /// uncompressed length is known from its wrapper.  It counts a bounded
    /// maximum payload, reconstructed output and native decoder state.  The
    /// caller still passes the actual archive length to `decode`, which
    /// performs exact header and output limits before invoking native code.
    pub fn decoder_peak_bytes(&self, input_len: usize) -> Result<usize, String> {
        if input_len > MAX_INPUT {
            return Err("CIXB1 input exceeds 128 MiB limit".into());
        }
        let native = match self.config {
            CandidateConfig::XzDictionary {
                dictionary_bytes, ..
            } => dictionary_bytes as usize + 32 * 1024 * 1024,
            CandidateConfig::Canonical if self.backend == "xz" => 128 * 1024 * 1024,
            CandidateConfig::BrotliQuality { lgwin, .. } if self.backend == "brotli" => {
                // lgwin is payload-described to the decoder.  Account above
                // its declared window for decoder tables and allocator slack.
                (1usize << lgwin).saturating_add(32 * 1024 * 1024)
            }
            CandidateConfig::Canonical if self.backend == "brotli" => 48 * 1024 * 1024,
            CandidateConfig::Canonical if self.backend == "bzip2" => 32 * 1024 * 1024,
            CandidateConfig::Canonical if self.backend == "zstd" => 64 * 1024 * 1024,
            CandidateConfig::Canonical if self.backend == "gzip" => 8 * 1024 * 1024,
            CandidateConfig::Zpaq { level: 5 } if self.backend == "zpaq" => 1024 * 1024 * 1024,
            CandidateConfig::Bsc { .. } if self.backend == "bsc" => {
                16 * 1024 * 1024 + input_len.saturating_mul(8)
            }
            _ => return Err(format!("unsupported CIXB1 candidate {}", self.id)),
        };
        MAX_PAYLOAD
            .checked_add(input_len)
            .and_then(|v| v.checked_add(HEADER))
            .and_then(|v| v.checked_add(native))
            .ok_or_else(|| "CIXB1 decoder estimate overflow".into())
    }
}

pub fn xz_easy_memory_usage(preset: u32) -> usize {
    let reported = unsafe { lzma_easy_encoder_memusage(preset) };
    // LZMA_UINT64_MAX signals unsupported options.  The configured presets
    // are known legal; use a deliberately large fallback instead of admitting
    // an unknown native allocation on a small estimate.
    if reported == u64::MAX || reported > usize::MAX as u64 {
        2 * 1024 * 1024 * 1024usize
    } else {
        reported as usize
    }
}

fn xz_dictionary_memory_usage(dictionary_bytes: u32, preset: u32) -> Result<usize, String> {
    let mut options: LzmaOptionsLzma = unsafe { std::mem::zeroed() };
    if unsafe { lzma_lzma_preset(&mut options, preset) } != 0 {
        return Err("liblzma rejected the requested preset".into());
    }
    options.dict_size = dictionary_bytes;
    let filters = [
        LzmaFilter {
            id: 0x21,
            options: (&mut options as *mut LzmaOptionsLzma).cast(),
        },
        LzmaFilter {
            id: u64::MAX,
            options: std::ptr::null_mut(),
        },
    ];
    let reported = unsafe { lzma_raw_encoder_memusage(filters.as_ptr()) };
    if reported == u64::MAX || reported > usize::MAX as u64 {
        Ok(2 * 1024 * 1024 * 1024usize)
    } else {
        Ok(reported as usize)
    }
}

fn zstd_memory_usage(level: c_int) -> usize {
    let reported = unsafe { ZSTD_estimateCCtxSize(level) };
    // ZSTD_isError accepts a size_t result.  The static fallback deliberately
    // exceeds a level-22 window/table budget, avoiding a false small
    // admission if an old library rejects the estimator query.
    if unsafe { ZSTD_isError(reported) } != 0 || reported == 0 {
        2 * 1024 * 1024 * 1024usize
    } else {
        reported
    }
}

#[link(name = "brotlienc")]
unsafe extern "C" {
    fn BrotliEncoderMaxCompressedSize(input_size: usize) -> usize;
    fn BrotliEncoderCompress(
        quality: c_int,
        lgwin: c_int,
        mode: c_int,
        input_size: usize,
        input: *const u8,
        encoded_size: *mut usize,
        encoded: *mut u8,
    ) -> c_int;
    fn BrotliEncoderVersion() -> u32;
}
#[link(name = "brotlidec")]
unsafe extern "C" {
    fn BrotliDecoderCreateInstance(
        alloc_func: *const c_void,
        free_func: *const c_void,
        opaque: *mut c_void,
    ) -> *mut BrotliDecoderState;
    fn BrotliDecoderDestroyInstance(state: *mut BrotliDecoderState);
    fn BrotliDecoderDecompressStream(
        state: *mut BrotliDecoderState,
        available_in: *mut usize,
        next_in: *mut *const u8,
        available_out: *mut usize,
        next_out: *mut *mut u8,
        total_out: *mut usize,
    ) -> c_int;
}

#[repr(C)]
struct BrotliDecoderState {
    _opaque: [u8; 0],
}

struct BrotliDecoder(*mut BrotliDecoderState);

impl Drop for BrotliDecoder {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { BrotliDecoderDestroyInstance(self.0) };
        }
    }
}

/// Decode exactly one Brotli stream. The one-shot Brotli API reports success
/// after a valid stream even when bytes remain in its input buffer. CIXB1
/// treats such bytes as a forged trailing payload, so use the streaming API
/// and require that the successful decode consumes every wrapper payload byte.
fn brotli_decode_exact(payload: &[u8], output: &mut [u8]) -> Result<usize, String> {
    let state = unsafe {
        BrotliDecoderCreateInstance(std::ptr::null(), std::ptr::null(), std::ptr::null_mut())
    };
    if state.is_null() {
        return Err("brotli decoder allocation failed".into());
    }
    let _state = BrotliDecoder(state);
    let mut available_in = payload.len();
    let mut next_in = payload.as_ptr();
    let mut available_out = output.len();
    let mut next_out = output.as_mut_ptr();
    let mut total_out = 0usize;
    let status = unsafe {
        BrotliDecoderDecompressStream(
            state,
            &mut available_in,
            &mut next_in,
            &mut available_out,
            &mut next_out,
            &mut total_out,
        )
    };
    // BROTLI_DECODER_RESULT_SUCCESS is 1. NEEDS_MORE_INPUT (2) means a
    // truncated payload; NEEDS_MORE_OUTPUT (3) exceeds the bounded output.
    if status != 1 {
        return Err("brotli truncated, invalid or oversized data".into());
    }
    if available_in != 0 {
        return Err("brotli trailing data".into());
    }
    if total_out > output.len() {
        return Err("brotli decoder returned invalid output length".into());
    }
    Ok(total_out)
}

#[link(name = "cix_zpaq715", kind = "static")]
unsafe extern "C" {
    fn cix_zpaq715_compress_l5(
        input: *const u8,
        input_size: usize,
        output_limit: usize,
        output: *mut *mut u8,
        output_size: *mut usize,
        error: *mut c_char,
        error_capacity: usize,
        cancellation: extern "C" fn() -> bool,
    ) -> c_int;
    fn cix_zpaq715_decompress(
        input: *const u8,
        input_size: usize,
        output: *mut u8,
        output_capacity: usize,
        native_memory_limit: usize,
        output_size: *mut usize,
        error: *mut c_char,
        error_capacity: usize,
        cancellation: extern "C" fn() -> bool,
    ) -> c_int;
    fn cix_zpaq715_free(pointer: *mut c_void);
    fn cix_zpaq715_version() -> *const c_char;
}

fn backend_id(name: &str) -> Result<u8, String> {
    match name {
        "gzip" => Ok(1),
        "bzip2" => Ok(2),
        "xz" => Ok(3),
        "zstd" => Ok(4),
        "brotli" => Ok(5),
        "zpaq" => Ok(6),
        "bsc" => Ok(7),
        _ => Err(format!("unsupported CIXB1 backend: {name}")),
    }
}
fn profile_id(name: &str) -> Result<u8, String> {
    match name {
        "fast" => Ok(1),
        "default" | "current" => Ok(2),
        "size" => Ok(3),
        _ => Err(format!("unsupported CIXB1 profile: {name}")),
    }
}
fn check_eligible(backend: &str, profile: &str) -> Result<(), String> {
    let valid = match profile {
        "fast" => matches!(backend, "gzip" | "zstd" | "brotli"),
        "default" | "current" | "size" => {
            matches!(backend, "gzip" | "bzip2" | "xz" | "zstd" | "zpaq" | "bsc")
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "CIXB1 backend {backend} is not eligible for profile {profile}"
        ))
    }
}
fn zstd_error(code: usize) -> String {
    unsafe {
        CStr::from_ptr(ZSTD_getErrorName(code))
            .to_string_lossy()
            .into_owned()
    }
}

fn zpaq_error(prefix: &str, status: c_int, error: &[c_char]) -> String {
    let detail = unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy();
    if detail.is_empty() {
        format!("{prefix} failed (status {status})")
    } else {
        format!("{prefix}: {detail}")
    }
}

fn zpaq_payload(input: &[u8], level: u8) -> Result<Vec<u8>, String> {
    if level != 5 {
        return Err(format!("unsupported pinned ZPAQ level {level}"));
    }
    check_cancel()?;
    let limit = input
        .len()
        .checked_add(1024 * 1024)
        .ok_or("ZPAQ payload bound overflow")?;
    let mut result = std::ptr::null_mut();
    let mut result_size = 0usize;
    let mut error = [0 as c_char; 512];
    let status = unsafe {
        cix_zpaq715_compress_l5(
            input.as_ptr(),
            input.len(),
            limit,
            &mut result,
            &mut result_size,
            error.as_mut_ptr(),
            error.len(),
            zpaq_cancelled,
        )
    };
    if status != 0 {
        return Err(zpaq_error("ZPAQ 7.15 level-5 encode", status, &error));
    }
    if result_size > limit || (result.is_null() && result_size != 0) {
        if !result.is_null() {
            unsafe { cix_zpaq715_free(result.cast()) };
        }
        return Err("ZPAQ encoder returned an invalid bounded payload".into());
    }
    let payload = if result_size == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(result, result_size) }.to_vec()
    };
    if !result.is_null() {
        unsafe { cix_zpaq715_free(result.cast()) };
    }
    check_cancel()?;
    Ok(payload)
}

/// Wrap the pinned libzpaq 7.15 built-in level-5 stream in a versioned CIXB1
/// envelope. The CIXB1 backend ID identifies libzpaq semantics and its fixed
/// embedded level, while the ZPAQ payload itself carries the decoder model.
pub fn encode_zpaq(profile: &str, input: &[u8], level: u8) -> Result<Vec<u8>, String> {
    if input.is_empty() {
        return Err("ZPAQ candidate rejected: empty input emits no framed libzpaq block".into());
    }
    if !matches!(profile, "default" | "current" | "size") {
        return Err(format!(
            "CIXB1 ZPAQ level-5 is not eligible for profile {profile}"
        ));
    }
    if input.len() > MAX_INPUT {
        return Err("CIXB1 input exceeds 128 MiB limit".into());
    }
    let payload = zpaq_payload(input, level)?;
    if payload.len() > MAX_PAYLOAD {
        return Err("CIXB1 payload exceeds bounded limit".into());
    }
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(b"CIXB1");
    out.push(backend_id("zpaq")?);
    out.push(profile_id(profile)?);
    out.extend_from_slice(&(input.len() as u32).to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&Sha256::digest(input));
    out.extend_from_slice(&payload);
    Ok(out)
}

fn bsc_payload(input: &[u8], options: crate::bsc_ffi::BscOptions) -> Result<Vec<u8>, String> {
    check_cancel()?;
    match crate::bsc_ffi::compress(input, options) {
        Ok(payload) => {
            check_cancel()?;
            Ok(payload)
        }
        // BSC deliberately declines blocks where its transformed payload is
        // not smaller. In automatic BEST this is a normal rejected candidate;
        // the native CIX raw candidate remains present and charged.
        Err(crate::bsc_ffi::BscError::Compress(-3)) | Err(crate::bsc_ffi::BscError::EmptyBlock) => {
            Err("BSC candidate rejected: libbsc reported not compressible".into())
        }
        Err(error) => Err(format!("libbsc encode: {error}")),
    }
}

/// Encode an explicitly configured libbsc payload in a complete CIXB1
/// envelope. The payload itself includes BSC's transform/coder settings;
/// CIXB1 records backend 7 and retains the generic profile only as an effort
/// label. No input-derived external dictionary is supplied.
pub fn encode_bsc(
    profile: &str,
    input: &[u8],
    options: crate::bsc_ffi::BscOptions,
) -> Result<Vec<u8>, String> {
    if !matches!(profile, "default" | "current" | "size") {
        return Err(format!("CIXB1 BSC is not eligible for profile {profile}"));
    }
    if input.len() > MAX_INPUT {
        return Err("CIXB1 input exceeds 128 MiB limit".into());
    }
    let payload = bsc_payload(input, options)?;
    if payload.len() > MAX_PAYLOAD {
        return Err("CIXB1 payload exceeds bounded limit".into());
    }
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(b"CIXB1");
    out.push(backend_id("bsc")?);
    out.push(profile_id(profile)?);
    out.extend_from_slice(&(input.len() as u32).to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&Sha256::digest(input));
    out.extend_from_slice(&payload);
    Ok(out)
}

pub fn encode(backend: &str, profile: &str, input: &[u8]) -> Result<Vec<u8>, String> {
    check_eligible(backend, profile)?;
    check_cancel()?;
    if input.len() > MAX_INPUT {
        return Err("CIXB1 input exceeds 128 MiB limit".into());
    }
    let payload = encode_payload(backend, profile, input)?;
    if payload.len() > MAX_PAYLOAD {
        return Err("CIXB1 payload exceeds bounded limit".into());
    }
    wrap_archive(backend, profile, input, payload)
}

fn wrap_archive(
    backend: &str,
    profile: &str,
    input: &[u8],
    payload: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(b"CIXB1");
    out.push(backend_id(backend)?);
    out.push(profile_id(profile)?);
    out.extend_from_slice(&(input.len() as u32).to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&Sha256::digest(input));
    out.extend_from_slice(&payload);
    Ok(out)
}

pub(crate) fn encode_payload(
    backend: &str,
    profile: &str,
    input: &[u8],
) -> Result<Vec<u8>, String> {
    match backend {
        "gzip" => encode_gzip_payload(profile, input),
        "bzip2" => encode_bzip2_payload(profile, input),
        "xz" => encode_xz_payload(profile, input),
        "zstd" => encode_zstd_payload(profile, input),
        "brotli" => brotli_payload(input, 0, 22, BrotliMode::Generic),
        "zpaq" => zpaq_payload(input, 5),
        "bsc" => bsc_payload(input, crate::bsc_ffi::BscOptions::default()),
        _ => Err(format!("unsupported CIXB1 backend: {backend}")),
    }
}

/// Raw bounded payload adapters for the full-engine catalogue.  They do not
/// add CIXB1 framing; callers must carry backend identity separately.
pub fn raw_zlib_encode(input: &[u8], level: u32, output_limit: usize) -> Result<Vec<u8>, String> {
    if input.len() > MAX_INPUT || !(0..=9).contains(&level) {
        return Err("invalid raw zlib request".into());
    }
    check_cancel()?;
    let mut encoder = ZlibEncoder::new(
        CappedWriter {
            bytes: Vec::new(),
            cap: output_limit,
        },
        Compression::new(level),
    );
    encoder.write_all(input).map_err(|e| e.to_string())?;
    let out = encoder.finish().map_err(|e| e.to_string())?.bytes;
    check_cancel()?;
    Ok(out)
}

pub fn raw_zlib_decode(
    payload: &[u8],
    expected: usize,
    output_limit: usize,
) -> Result<Vec<u8>, String> {
    if expected > output_limit {
        return Err("raw zlib output exceeds limit".into());
    }
    let output = raw_zlib_decode_bounded(payload, expected)?;
    if output.len() != expected {
        return Err("raw zlib decoded length mismatch".into());
    }
    Ok(output)
}
pub fn raw_zlib_decode_bounded(payload: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    check_cancel()?;
    let cap = limit
        .checked_add(1)
        .filter(|n| *n <= isize::MAX as usize)
        .ok_or("raw zlib allocation overflow")?;
    let mut out = Vec::new();
    out.try_reserve_exact(cap)
        .map_err(|_| "raw zlib allocation failed")?;
    out.resize(cap, 0);
    let mut decoder = flate2::Decompress::new(true);
    let status = decoder
        .decompress(payload, &mut out, flate2::FlushDecompress::Finish)
        .map_err(|e| e.to_string())?;
    if status != flate2::Status::StreamEnd
        || decoder.total_in() != payload.len() as u64
        || decoder.total_out() > limit as u64
    {
        return Err("raw zlib truncated, trailing, or output exceeds limit".into());
    }
    out.truncate(decoder.total_out() as usize);
    check_cancel()?;
    Ok(out)
}

pub fn raw_xz_encode(input: &[u8], profile: &str, output_limit: usize) -> Result<Vec<u8>, String> {
    let preset = match profile {
        "fast" => 0,
        "default" => 6,
        "size" => 9,
        _ => return Err("invalid raw XZ preset".into()),
    };
    raw_xz_encode_preset(input, preset, output_limit)
}

/// Encode a standard XZ member with an explicit liblzma easy preset.
pub fn raw_xz_encode_preset(
    input: &[u8],
    preset: u32,
    output_limit: usize,
) -> Result<Vec<u8>, String> {
    check_cancel()?;
    if preset > 9 {
        return Err("invalid raw XZ preset".into());
    }
    let bound = unsafe { lzma_stream_buffer_bound(input.len()) };
    let cap = bound.min(output_limit);
    let mut out = Vec::new();
    out.try_reserve_exact(cap)
        .map_err(|_| "raw XZ allocation failed")?;
    out.resize(cap, 0);
    let mut used = 0;
    let rc = unsafe {
        lzma_easy_buffer_encode(
            preset,
            4,
            std::ptr::null(),
            input.as_ptr(),
            input.len(),
            out.as_mut_ptr(),
            &mut used,
            cap,
        )
    };
    if rc != 0 {
        return Err("raw XZ output exceeds limit or encoding failed".into());
    };
    out.truncate(used);
    check_cancel()?;
    Ok(out)
}

pub fn raw_brotli_encode(
    input: &[u8],
    quality: u32,
    lgwin: u32,
    output_limit: usize,
) -> Result<Vec<u8>, String> {
    let cap = unsafe { BrotliEncoderMaxCompressedSize(input.len()) }.min(output_limit);
    let mut out = Vec::new();
    out.try_reserve_exact(cap)
        .map_err(|_| "raw Brotli allocation failed")?;
    out.resize(cap, 0);
    let mut used = cap;
    let ok = unsafe {
        BrotliEncoderCompress(
            quality as c_int,
            lgwin as c_int,
            BrotliMode::Generic as c_int,
            input.len(),
            input.as_ptr(),
            &mut used,
            out.as_mut_ptr(),
        )
    };
    if ok == 0 {
        return Err("raw Brotli output exceeds limit or encoding failed".into());
    };
    out.truncate(used);
    check_cancel()?;
    Ok(out)
}
pub fn raw_xz_decode(
    payload: &[u8],
    expected: usize,
    memory: usize,
    output_limit: usize,
) -> Result<Vec<u8>, String> {
    if expected == usize::MAX || expected > output_limit || expected >= memory {
        return Err("raw XZ resource limit".into());
    }
    let mut out = vec![0; expected + 1];
    let native_memory = memory
        .checked_sub(payload.len())
        .and_then(|n| n.checked_sub(out.len()))
        .ok_or("raw XZ live-buffer limit")?;
    let written = decode_xz_payload(payload, &mut out, native_memory)?;
    if written != expected {
        return Err("raw XZ wrong length".into());
    }
    out.truncate(written);
    Ok(out)
}
pub fn raw_brotli_decode(
    payload: &[u8],
    expected: usize,
    output_limit: usize,
) -> Result<Vec<u8>, String> {
    if expected == usize::MAX || expected > output_limit {
        return Err("raw Brotli output exceeds limit".into());
    }
    let mut out = vec![0; expected + 1];
    let written = decode_brotli_payload(payload, &mut out)?;
    if written != expected {
        return Err("raw Brotli wrong length".into());
    }
    out.truncate(written);
    Ok(out)
}
pub fn raw_xz_decode_bounded(
    payload: &[u8],
    limit: usize,
    memory: usize,
) -> Result<Vec<u8>, String> {
    if limit >= memory {
        return Err("raw XZ resource limit".into());
    }
    let mut out = Vec::new();
    out.try_reserve_exact(limit.saturating_add(1))
        .map_err(|_| "raw XZ allocation failed")?;
    out.resize(limit.saturating_add(1), 0);
    let native_memory = memory
        .checked_sub(payload.len())
        .and_then(|n| n.checked_sub(out.len()))
        .ok_or("raw XZ live-buffer limit")?;
    let written = decode_xz_payload(payload, &mut out, native_memory)?;
    if written > limit {
        return Err("raw XZ output exceeds limit".into());
    }
    out.truncate(written);
    check_cancel()?;
    Ok(out)
}
pub fn raw_brotli_decode_bounded(payload: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.try_reserve_exact(limit.saturating_add(1))
        .map_err(|_| "raw Brotli allocation failed")?;
    out.resize(limit.saturating_add(1), 0);
    let written = decode_brotli_payload(payload, &mut out)?;
    if written > limit {
        return Err("raw Brotli output exceeds limit".into());
    }
    out.truncate(written);
    check_cancel()?;
    Ok(out)
}

fn encode_gzip_payload(profile: &str, input: &[u8]) -> Result<Vec<u8>, String> {
    let level = if profile == "fast" { 1 } else { 9 };
    let mut encoder = GzBuilder::new().mtime(0).write(
        Vec::with_capacity(input.len() + 1024 * 1024),
        Compression::new(level),
    );
    encoder
        .write_all(input)
        .map_err(|error| error.to_string())?;
    let payload = encoder.finish().map_err(|error| error.to_string())?;
    check_cancel()?;
    Ok(payload)
}

fn encode_bzip2_payload(profile: &str, input: &[u8]) -> Result<Vec<u8>, String> {
    let level = if profile == "fast" { 1 } else { 9 };
    let cap = input
        .len()
        .saturating_add(input.len() / 100)
        .saturating_add(601)
        .max(601);
    let mut out = vec![0u8; cap];
    let mut n = u32::try_from(cap).map_err(|_| "bzip2 output bound overflow")?;
    let srcn = u32::try_from(input.len()).map_err(|_| "bzip2 source too large")?;
    let rc = unsafe {
        BZ2_bzBuffToBuffCompress(
            out.as_mut_ptr().cast(),
            &mut n,
            input.as_ptr().cast_mut().cast(),
            srcn,
            level,
            0,
            30,
        )
    };
    if rc != 0 {
        return Err(format!("bzip2 compressor returned {rc}"));
    }
    check_cancel()?;
    out.truncate(n as usize);
    Ok(out)
}

fn encode_xz_payload(profile: &str, input: &[u8]) -> Result<Vec<u8>, String> {
    let base = match profile {
        "fast" => 0,
        "default" | "current" => 6,
        _ => 9,
    };
    let preset = base | if profile == "size" { 1u32 << 31 } else { 0 };
    let cap = unsafe { lzma_stream_buffer_bound(input.len()) };
    let mut stream: LzmaStream = unsafe { std::mem::zeroed() };
    stream.next_in = input.as_ptr();
    stream.avail_in = input.len();
    let init = unsafe { lzma_easy_encoder(&mut stream, preset, 4) };
    if init != 0 {
        return Err(format!("xz encoder initialization returned {init}"));
    }
    let mut out = Vec::with_capacity(cap);
    let mut chunk = [0u8; 65536];
    loop {
        check_cancel()?;
        stream.next_out = chunk.as_mut_ptr();
        stream.avail_out = chunk.len();
        let before_in = stream.avail_in;
        let rc = unsafe { lzma_code(&mut stream, 3) };
        let written = chunk.len() - stream.avail_out;
        out.extend_from_slice(&chunk[..written]);
        if rc != 0 && rc != 1 {
            return Err(format!("xz encoder returned {rc}"));
        }
        check_cancel()?;
        if rc == 1 {
            break;
        }
        if written == 0 && stream.avail_in == before_in {
            return Err("xz encoder made no progress".into());
        }
    }
    Ok(out)
}

fn encode_zstd_payload(profile: &str, input: &[u8]) -> Result<Vec<u8>, String> {
    let level = match profile {
        "fast" => -131072,
        "default" | "current" => 3,
        _ => 22,
    };
    let cap = unsafe { ZSTD_compressBound(input.len()) };
    let mut out = vec![0u8; cap];
    let n = unsafe {
        ZSTD_compress(
            out.as_mut_ptr().cast(),
            out.len(),
            input.as_ptr().cast(),
            input.len(),
            level,
        )
    };
    if unsafe { ZSTD_isError(n) } != 0 {
        return Err(format!("zstd encode: {}", zstd_error(n)));
    }
    check_cancel()?;
    out.truncate(n);
    Ok(out)
}

fn brotli_payload(
    input: &[u8],
    quality: u8,
    lgwin: u8,
    mode: BrotliMode,
) -> Result<Vec<u8>, String> {
    if quality > 11 || !(10..=24).contains(&lgwin) {
        return Err("unsupported Brotli quality/window configuration".into());
    }
    let mode = match mode {
        BrotliMode::Generic => 0,
    };
    check_cancel()?;
    let cap = unsafe { BrotliEncoderMaxCompressedSize(input.len()) };
    let mut out = vec![0u8; cap];
    let mut n = cap;
    let ok = unsafe {
        BrotliEncoderCompress(
            quality as c_int,
            lgwin as c_int,
            mode,
            input.len(),
            input.as_ptr(),
            &mut n,
            out.as_mut_ptr(),
        )
    };
    // Native failure takes precedence over a concurrently noticed interrupt.
    if ok == 0 {
        return Err("brotli encoder failed".into());
    }
    check_cancel()?;
    out.truncate(n);
    Ok(out)
}

/// Encode a generic Brotli payload at an explicit quality/window.  The CIXB1
/// `brotli/size` profile identifies the size-search envelope; Brotli itself
/// serializes the decoder-relevant window in its payload.  Quality is an
/// encoder effort setting and is retained in the candidate descriptor/evidence
/// rather than requiring a decoder-side source dictionary.
pub fn encode_brotli_quality(
    profile: &str,
    input: &[u8],
    quality: u8,
    lgwin: u8,
    mode: BrotliMode,
) -> Result<Vec<u8>, String> {
    if !matches!(profile, "fast" | "default" | "current" | "size") {
        return Err(format!("unsupported CIXB1 profile: {profile}"));
    }
    if input.len() > MAX_INPUT {
        return Err("CIXB1 input exceeds 128 MiB limit".into());
    }
    let payload = brotli_payload(input, quality, lgwin, mode)?;
    if payload.len() > MAX_PAYLOAD {
        return Err("CIXB1 payload exceeds bounded limit".into());
    }
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(b"CIXB1");
    out.push(backend_id("brotli")?);
    out.push(profile_id(profile)?);
    out.extend_from_slice(&(input.len() as u32).to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&Sha256::digest(input));
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Encode an xz preset with a larger self-describing dictionary. The Python
/// preset remains available through `encode`; this is an additional measured
/// size candidate whose settings are carried inside the XZ stream itself.
pub fn encode_xz_dictionary(
    profile: &str,
    input: &[u8],
    dictionary_bytes: u32,
) -> Result<Vec<u8>, String> {
    check_eligible("xz", profile)?;
    if input.len() > MAX_INPUT {
        return Err("CIXB1 input exceeds 128 MiB limit".into());
    }
    let base = match profile {
        "fast" => 0,
        "default" | "current" => 6,
        _ => 9,
    };
    let preset = base | if profile == "size" { 1u32 << 31 } else { 0 };
    encode_xz_dictionary_preset(profile, input, dictionary_bytes, preset)
}

pub fn encode_xz_dictionary_preset(
    profile: &str,
    input: &[u8],
    dictionary_bytes: u32,
    preset: u32,
) -> Result<Vec<u8>, String> {
    check_eligible("xz", profile)?;
    check_cancel()?;
    if input.len() > MAX_INPUT {
        return Err("CIXB1 input exceeds 128 MiB limit".into());
    }
    let mut options: LzmaOptionsLzma = unsafe { std::mem::zeroed() };
    if unsafe { lzma_lzma_preset(&mut options, preset) } != 0 {
        return Err("liblzma rejected the requested preset".into());
    }
    options.dict_size = dictionary_bytes;
    let mut filters = [
        LzmaFilter {
            id: 0x21,
            options: (&mut options as *mut LzmaOptionsLzma).cast(),
        },
        LzmaFilter {
            id: u64::MAX,
            options: std::ptr::null_mut(),
        },
    ];
    let mut stream: LzmaStream = unsafe { std::mem::zeroed() };
    stream.next_in = input.as_ptr();
    stream.avail_in = input.len();
    let init = unsafe { lzma_stream_encoder(&mut stream, filters.as_mut_ptr(), 4) };
    if init != 0 {
        return Err(format!("xz encoder initialization returned {init}"));
    }
    let mut payload = Vec::with_capacity(input.len() + 1024 * 1024);
    let mut chunk = [0u8; 65536];
    loop {
        check_cancel()?;
        stream.next_out = chunk.as_mut_ptr();
        stream.avail_out = chunk.len();
        let before_in = stream.avail_in;
        let rc = unsafe { lzma_code(&mut stream, 3) };
        let written = chunk.len() - stream.avail_out;
        payload.extend_from_slice(&chunk[..written]);
        if rc == 1 {
            break;
        }
        if rc != 0 {
            return Err(format!("xz encoder returned {rc}"));
        }
        check_cancel()?;
        if written == 0 && stream.avail_in == before_in {
            return Err("xz encoder made no progress".into());
        }
    }
    if payload.len() > MAX_PAYLOAD {
        return Err("CIXB1 payload exceeds bounded limit".into());
    }
    let mut archive = Vec::with_capacity(HEADER + payload.len());
    archive.extend_from_slice(b"CIXB1");
    archive.push(backend_id("xz")?);
    archive.push(profile_id(profile)?);
    archive.extend_from_slice(&(input.len() as u32).to_le_bytes());
    archive.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    archive.extend_from_slice(&Sha256::digest(input));
    archive.extend_from_slice(&payload);
    Ok(archive)
}

pub fn decode(archive: &[u8], memory_limit: usize) -> Result<Vec<u8>, String> {
    check_cancel()?;
    if archive.len() < HEADER || &archive[..5] != b"CIXB1" {
        return Err("truncated or invalid CIXB1 header".into());
    }
    let bid = archive[5];
    let profile = archive[6];
    if !(1..=7).contains(&bid) || !(1..=3).contains(&profile) {
        return Err("unsupported CIXB1 backend or profile ID".into());
    }
    let expected = u32::from_le_bytes(archive[7..11].try_into().unwrap()) as usize;
    let size = u32::from_le_bytes(archive[11..15].try_into().unwrap()) as usize;
    // libbsc has no empty memory-block representation. Reject this before
    // native dispatch so a forged CIXB1 header cannot represent empty data
    // through backend 7. Empty CIX data uses a regular raw/native frame.
    if bid == 7 && expected == 0 {
        return Err("CIXB1 BSC backend does not encode empty input".into());
    }
    if expected > MAX_INPUT
        || size > MAX_PAYLOAD
        || expected.saturating_add(size).saturating_add(HEADER) > memory_limit
    {
        return Err("CIXB1 memory/resource limit exceeded".into());
    }
    if archive.len() != HEADER + size {
        return Err("CIXB1 payload length mismatch".into());
    }
    let payload = &archive[HEADER..];
    let fixed = archive
        .len()
        .checked_add(expected + 1)
        .ok_or("CIXB1 memory/resource limit exceeded")?;
    let native_memory = memory_limit
        .checked_sub(fixed)
        .ok_or("CIXB1 memory/resource limit exceeded")?;
    let native_reserve = match bid {
        1 => 8 * 1024 * 1024,
        2 => 32 * 1024 * 1024,
        3 => 0, // liblzma enforces the exact remainder below.
        4 => 64 * 1024 * 1024,
        5 => 48 * 1024 * 1024,
        6 => 1024 * 1024 * 1024,
        // libbsc documents 16 MiB + 5 × block bytes. The aligned FFI decoder
        // owns two more block copies before returning its final Vec.
        7 => 16 * 1024 * 1024 + expected.saturating_mul(7),
        _ => unreachable!(),
    };
    if native_memory < native_reserve {
        return Err("CIXB1 decoder memory/resource limit exceeded".into());
    }
    let mut output = vec![0u8; expected.saturating_add(1)];
    let written = decode_payload(bid, payload, &mut output, expected, native_memory)?;
    if written != expected {
        return Err(format!(
            "CIXB1 decoded length mismatch: expected {expected}, got {written}"
        ));
    }
    output.truncate(written);
    if Sha256::digest(&output).as_slice() != &archive[15..47] {
        return Err("CIXB1 SHA-256 checksum mismatch".into());
    }
    Ok(output)
}

pub(crate) fn decode_payload(
    backend_id: u8,
    payload: &[u8],
    output: &mut Vec<u8>,
    expected: usize,
    native_memory: usize,
) -> Result<usize, String> {
    match backend_id {
        1 => decode_gzip_payload(payload, output, expected),
        2 => decode_bzip2_payload(payload, output),
        3 => decode_xz_payload(payload, output, native_memory),
        4 => decode_zstd_payload(payload, output),
        5 => decode_brotli_payload(payload, output),
        6 => decode_zpaq_payload(payload, output, native_memory),
        7 => decode_bsc_payload(payload, output, expected),
        _ => Err("unknown CIXB1 backend".into()),
    }
}

fn decode_gzip_payload(
    payload: &[u8],
    output: &mut Vec<u8>,
    expected: usize,
) -> Result<usize, String> {
    output.clear();
    let mut decoder = GzDecoder::new(Cursor::new(payload));
    std::io::Read::by_ref(&mut decoder)
        .take((expected + 1) as u64)
        .read_to_end(output)
        .map_err(|error| format!("gzip decode: {error}"))?;
    check_cancel()?;
    if decoder.get_ref().position() as usize != payload.len() {
        return Err("gzip trailing or concatenated data".into());
    }
    Ok(output.len())
}

fn decode_bzip2_payload(payload: &[u8], output: &mut [u8]) -> Result<usize, String> {
    let mut stream = BzStream {
        next_in: payload.as_ptr().cast_mut().cast(),
        avail_in: u32::try_from(payload.len()).map_err(|_| "bzip2 input too large")?,
        total_in_lo32: 0,
        total_in_hi32: 0,
        next_out: output.as_mut_ptr().cast(),
        avail_out: u32::try_from(output.len()).map_err(|_| "bzip2 output too large")?,
        total_out_lo32: 0,
        total_out_hi32: 0,
        state: std::ptr::null_mut(),
        bzalloc: None,
        bzfree: None,
        opaque: std::ptr::null_mut(),
    };
    let init = unsafe { BZ2_bzDecompressInit(&mut stream, 0, 0) };
    if init != 0 {
        return Err(format!("bzip2 init returned {init}"));
    }
    let mut rc;
    loop {
        rc = unsafe { BZ2_bzDecompress(&mut stream) };
        if rc == 4 || rc < 0 || rc == 0 && stream.avail_in == 0 || stream.avail_out == 0 {
            break;
        }
        if let Err(error) = check_cancel() {
            unsafe { BZ2_bzDecompressEnd(&mut stream) };
            return Err(error);
        }
    }
    let _ = unsafe { BZ2_bzDecompressEnd(&mut stream) };
    if rc != 4 || stream.avail_in != 0 {
        return Err("bzip2 truncated, trailing or invalid data".into());
    }
    check_cancel()?;
    Ok(stream.total_out_lo32 as usize | ((stream.total_out_hi32 as usize) << 32))
}

fn decode_xz_payload(
    payload: &[u8],
    output: &mut [u8],
    native_memory: usize,
) -> Result<usize, String> {
    let mut inpos = 0usize;
    let mut outpos = 0usize;
    let mut memlimit = native_memory as u64;
    let rc = unsafe {
        lzma_stream_buffer_decode(
            &mut memlimit,
            0,
            std::ptr::null(),
            payload.as_ptr(),
            &mut inpos,
            payload.len(),
            output.as_mut_ptr(),
            &mut outpos,
            output.len(),
        )
    };
    if rc != 0 || inpos != payload.len() {
        return Err(format!("xz invalid/trailing data (status {rc})"));
    }
    check_cancel()?;
    Ok(outpos)
}

fn decode_zstd_payload(payload: &[u8], output: &mut [u8]) -> Result<usize, String> {
    let frame = unsafe { ZSTD_findFrameCompressedSize(payload.as_ptr().cast(), payload.len()) };
    if unsafe { ZSTD_isError(frame) } != 0 || frame != payload.len() {
        return Err("zstd invalid or trailing data".into());
    }
    let written = unsafe {
        ZSTD_decompress(
            output.as_mut_ptr().cast(),
            output.len(),
            payload.as_ptr().cast(),
            payload.len(),
        )
    };
    if unsafe { ZSTD_isError(written) } != 0 {
        return Err(format!("zstd decode: {}", zstd_error(written)));
    }
    check_cancel()?;
    Ok(written)
}

fn decode_brotli_payload(payload: &[u8], output: &mut [u8]) -> Result<usize, String> {
    let written = brotli_decode_exact(payload, output)?;
    check_cancel()?;
    Ok(written)
}

fn decode_zpaq_payload(
    payload: &[u8],
    output: &mut [u8],
    native_memory: usize,
) -> Result<usize, String> {
    let mut decoded_size = 0usize;
    let mut error = [0 as c_char; 512];
    let status = unsafe {
        cix_zpaq715_decompress(
            payload.as_ptr(),
            payload.len(),
            output.as_mut_ptr(),
            output.len(),
            native_memory,
            &mut decoded_size,
            error.as_mut_ptr(),
            error.len(),
            zpaq_cancelled,
        )
    };
    if status != 0 {
        return Err(zpaq_error("ZPAQ 7.15 decode", status, &error));
    }
    if decoded_size > output.len() {
        return Err("ZPAQ decoder returned invalid output length".into());
    }
    check_cancel()?;
    Ok(decoded_size)
}

fn decode_bsc_payload(payload: &[u8], output: &mut [u8], expected: usize) -> Result<usize, String> {
    let decoded = crate::bsc_ffi::decompress(payload, expected)
        .map_err(|error| format!("libbsc decode: {error}"))?;
    check_cancel()?;
    output[..expected].copy_from_slice(&decoded);
    Ok(expected)
}

#[cfg(test)]
mod tests {
    use super::{
        decode, encode_bsc, encode_xz_dictionary, encode_zpaq, raw_zlib_decode_bounded,
        raw_zlib_encode, HEADER,
    };
    use sha2::{Digest, Sha256};

    #[test]
    fn xz_larger_dictionary_archive_round_trips() {
        let input = b"CIX xz dictionary fixture: abcdef0123456789 ".repeat(8192);
        let archive = encode_xz_dictionary("size", &input, 128 * 1024 * 1024).unwrap();
        assert_eq!(decode(&archive, 1024 * 1024 * 1024).unwrap(), input);
    }

    #[test]
    fn pinned_zpaq715_level5_cixb1_round_trips_and_rejects_corruption() {
        let input = b"CIX ZPAQ 7.15 adaptive context fixture\n".repeat(4096);
        let archive = encode_zpaq("size", &input, 5).unwrap();
        assert_eq!(&archive[..5], b"CIXB1");
        assert_eq!(archive[5], 6);
        assert_eq!(decode(&archive, 2 * 1024 * 1024 * 1024).unwrap(), input);

        let mut corrupt = archive;
        let last = corrupt.len() - 1;
        corrupt[last] ^= 0x80;
        assert!(decode(&corrupt, 2 * 1024 * 1024 * 1024).is_err());

        let mut trailing = encode_zpaq("size", &input, 5).unwrap();
        trailing.push(0);
        let payload = u32::try_from(trailing.len() - HEADER).unwrap();
        trailing[11..15].copy_from_slice(&payload.to_le_bytes());
        assert!(decode(&trailing, 2 * 1024 * 1024 * 1024).is_err());

        assert!(encode_zpaq("size", &[], 5)
            .unwrap_err()
            .starts_with("ZPAQ candidate rejected:"));
        let mut missing_payload = vec![0u8; HEADER];
        missing_payload[..5].copy_from_slice(b"CIXB1");
        missing_payload[5] = 6;
        missing_payload[6] = 3;
        missing_payload[15..47].copy_from_slice(&Sha256::digest([]));
        missing_payload.truncate(HEADER);
        missing_payload[11..15].copy_from_slice(&0u32.to_le_bytes());
        assert!(decode(&missing_payload, 2 * 1024 * 1024 * 1024).is_err());
    }

    #[test]
    fn bsc_cixb1_round_trips_from_an_unaligned_envelope_payload() {
        let input = b"CIX BSC native envelope alignment fixture. ".repeat(4096);
        let archive = encode_bsc("size", &input, crate::bsc_ffi::BscOptions::default()).unwrap();
        assert_eq!(&archive[..5], b"CIXB1");
        assert_eq!(archive[5], 7);
        assert_eq!(decode(&archive, 64 * 1024 * 1024).unwrap(), input);

        let mut corrupt = archive;
        let last = corrupt.len() - 1;
        corrupt[last] ^= 0x40;
        assert!(decode(&corrupt, 64 * 1024 * 1024).is_err());
    }

    #[test]
    fn bsc_rejects_a_forged_stored_empty_archive() {
        // CIXB1 itself can describe an empty archive, but backend 7 cannot:
        // libbsc rejects empty memory blocks at encode time. Keep that rule
        // equally strict for an attacker-controlled decoder header, even if
        // the outer SHA-256 is the valid empty digest.
        let mut archive = Vec::with_capacity(HEADER);
        archive.extend_from_slice(b"CIXB1");
        archive.push(7); // native libbsc backend
        archive.push(3); // size effort profile
        archive.extend_from_slice(&0u32.to_le_bytes());
        archive.extend_from_slice(&0u32.to_le_bytes());
        archive.extend_from_slice(&Sha256::digest([]));
        assert_eq!(archive.len(), HEADER);
        assert!(decode(&archive, 64 * 1024 * 1024)
            .unwrap_err()
            .contains("BSC backend does not encode empty"));
    }

    #[test]
    fn raw_zlib_bounded_roundtrip_cap_and_trailing() {
        let input = b"raw provider zlib fixture".repeat(64);
        let payload = raw_zlib_encode(&input, 6, 1 << 20).unwrap();
        assert_eq!(
            raw_zlib_decode_bounded(&payload, input.len()).unwrap(),
            input
        );
        assert!(raw_zlib_decode_bounded(&payload, input.len() - 1).is_err());
        let mut trailing = payload;
        trailing.push(0);
        assert!(raw_zlib_decode_bounded(&trailing, input.len()).is_err());
    }
}
