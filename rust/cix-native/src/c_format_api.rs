//! Stateless, process-free standard-format C ABI with caller-owned buffers.
use crate::c_api::*;
use crate::standard_formats::{self, StandardError, StandardFormat, StandardOptions};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    ptr, slice,
};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct cix_format_options_v1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub format: u32,
    pub level: u32,
    pub output_limit: u64,
    pub memory_limit: u64,
}

fn format(id: u32) -> Result<StandardFormat, i32> {
    Ok(match id {
        1 => StandardFormat::Gzip,
        2 => StandardFormat::Zlib,
        3 => StandardFormat::Deflate,
        4 => StandardFormat::Bzip2,
        5 => StandardFormat::Xz,
        6 => StandardFormat::Zstd,
        7 => StandardFormat::Brotli,
        8 => StandardFormat::Lz4Frame,
        9 => StandardFormat::SnappyFramed,
        _ => return Err(CIX_STATUS_INVALID_OPTIONS),
    })
}

fn range(p: usize, n: usize) -> Result<(usize, usize), i32> {
    if n > isize::MAX as usize || (n != 0 && p == 0) {
        return Err(CIX_STATUS_INVALID_ARGUMENT);
    }
    Ok((p, p.checked_add(n).ok_or(CIX_STATUS_INVALID_ARGUMENT)?))
}
fn overlaps(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0 < a.1 && b.0 < b.1 && a.0 < b.1 && b.0 < a.1
}
unsafe fn options(
    p: *const cix_format_options_v1,
) -> Result<(StandardFormat, StandardOptions), i32> {
    if p.is_null() {
        return Err(CIX_STATUS_INVALID_ARGUMENT);
    }
    // Caller must provide the readable eight-byte ABI prefix. Read it without
    // assuming full struct size or alignment, as in the native context ABI.
    let version = unsafe { ptr::read_unaligned(ptr::addr_of!((*p).abi_version)) };
    let size = unsafe { ptr::read_unaligned(ptr::addr_of!((*p).struct_size)) };
    if version != 1 || (size as usize) < std::mem::size_of::<cix_format_options_v1>() {
        return Err(CIX_STATUS_INVALID_OPTIONS);
    }
    let value = unsafe { ptr::read_unaligned(p) };
    let codec = format(value.format)?;
    if value.level > 9 {
        return Err(CIX_STATUS_INVALID_OPTIONS);
    }
    Ok((
        codec,
        StandardOptions {
            level: value.level as u8,
            output_limit: usize::try_from(value.output_limit)
                .map_err(|_| CIX_STATUS_RESOURCE_LIMIT)?,
            memory_limit: usize::try_from(value.memory_limit)
                .map_err(|_| CIX_STATUS_RESOURCE_LIMIT)?,
        },
    ))
}

unsafe fn run(
    opts: *const cix_format_options_v1,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_capacity: usize,
    written: *mut usize,
    decode: bool,
) -> i32 {
    // Validate all disjoint ranges before writing the result or borrowing any
    // buffer. Destruction/concurrent mutation of caller buffers is forbidden.
    if opts.is_null()
        || written.is_null()
        || !(written as usize).is_multiple_of(std::mem::align_of::<usize>())
    {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    let regions = [
        range(opts as usize, std::mem::size_of::<cix_format_options_v1>()),
        range(input as usize, input_len),
        range(output as usize, output_capacity),
        range(written as usize, std::mem::size_of::<usize>()),
    ];
    if regions.iter().any(Result::is_err) {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    let regions = regions.map(Result::unwrap);
    for i in 0..regions.len() {
        for j in i + 1..regions.len() {
            if overlaps(regions[i], regions[j]) {
                return CIX_STATUS_INVALID_ARGUMENT;
            }
        }
    }
    unsafe {
        *written = 0;
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<Vec<u8>, i32> {
        let (codec, mut options) = unsafe { options(opts) }?;
        // The standard codec counts input and its owned output. Reserve the
        // caller's destination as well while the temporary output is live.
        options.memory_limit = options
            .memory_limit
            .checked_sub(output_capacity)
            .ok_or(CIX_STATUS_RESOURCE_LIMIT)?;
        let input = if input_len == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(input, input_len) }
        };
        let _library = crate::limits::LibraryGuard::new();
        let encoded = if decode {
            standard_formats::decode(codec, input, &options)
        } else {
            standard_formats::encode(codec, input, &options)
        };
        encoded.map_err(|e| match e {
            StandardError::InvalidOptions(_) => CIX_STATUS_INVALID_OPTIONS,
            StandardError::OutputLimit => CIX_STATUS_RESOURCE_LIMIT,
            StandardError::Unavailable(_) | StandardError::Codec(_) => CIX_STATUS_CODEC_ERROR,
        })
    }));
    match result {
        Ok(Ok(bytes)) => {
            // Report the required capacity even when the caller's buffer is
            // too small; output is untouched in that case.
            unsafe {
                *written = bytes.len();
            }
            if bytes.len() > output_capacity {
                return CIX_STATUS_OUTPUT_TOO_SMALL;
            }
            if !bytes.is_empty() {
                unsafe {
                    ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len());
                }
            }
            CIX_STATUS_OK
        }
        Ok(Err(status)) => status,
        Err(_) => CIX_STATUS_PANIC,
    }
}

/// Encode a standard member. Pointers must name readable/writable disjoint
/// storage for the declared lengths; NULL is allowed only for zero buffers.
///
/// # Safety
///
/// `options` must point to a readable initialized options structure. `input`
/// must provide `input_len` readable bytes, `output` `capacity` writable bytes,
/// and `written` an aligned writable `usize`. All regions must be disjoint and
/// remain valid for the call.
#[no_mangle]
pub unsafe extern "C" fn cix_format_encode_v1(
    options: *const cix_format_options_v1,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    unsafe { run(options, input, input_len, output, capacity, written, false) }
}

/// Decode exactly one standard member under the same ownership contract.
///
/// # Safety
///
/// `options` must point to a readable initialized options structure. `input`
/// must provide `input_len` readable bytes, `output` `capacity` writable bytes,
/// and `written` an aligned writable `usize`. All regions must be disjoint and
/// remain valid for the call.
#[no_mangle]
pub unsafe extern "C" fn cix_format_decode_v1(
    options: *const cix_format_options_v1,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    unsafe { run(options, input, input_len, output, capacity, written, true) }
}
