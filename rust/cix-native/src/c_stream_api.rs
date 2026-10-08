//! C ABI for the native independent-block incremental stream only.
use crate::c_api::{
    cix_options_v1, native_options_from_ptr, CIX_STATUS_CODEC_ERROR, CIX_STATUS_INVALID_ARGUMENT,
    CIX_STATUS_INVALID_OPTIONS, CIX_STATUS_OK, CIX_STATUS_PANIC, CIX_STATUS_RESOURCE_LIMIT,
};
use crate::core::incremental::{
    IncrementalDecoder, IncrementalEncoder, IncrementalProgress, IncrementalState,
};
use crate::core::{NativeError, NativeOptions};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    ptr, slice,
    sync::Mutex,
};

pub const CIX_STREAM_ABI_VERSION_1: u32 = 1;
pub const CIX_STREAM_NEEDS_INPUT: u32 = 1;
pub const CIX_STREAM_NEEDS_OUTPUT: u32 = 2;
pub const CIX_STREAM_FINISHED: u32 = 3;

#[repr(C)]
pub struct cix_stream_result_v1 {
    pub consumed: usize,
    pub produced: usize,
    pub state: u32,
}
pub struct CixStreamEncoder {
    inner: Mutex<IncrementalEncoder>,
}
// C ABI compatibility name; the public header intentionally uses this spelling.
#[allow(non_camel_case_types)]
pub type cix_stream_encoder = CixStreamEncoder;
pub struct CixStreamDecoder {
    inner: Mutex<IncrementalDecoder>,
}
// C ABI compatibility name; the public header intentionally uses this spelling.
#[allow(non_camel_case_types)]
pub type cix_stream_decoder = CixStreamDecoder;

fn options(raw: *const cix_options_v1) -> Result<NativeOptions, &'static str> {
    unsafe { native_options_from_ptr(raw) }
}
unsafe fn buffers<'a>(
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> Result<(&'a [u8], &'a mut [u8]), &'static str> {
    if input_len != 0 && input.is_null() || output_len != 0 && output.is_null() {
        return Err("null stream buffer with non-zero length");
    }
    if input_len > isize::MAX as usize
        || output_len > isize::MAX as usize
        || (input as usize).checked_add(input_len).is_none()
        || (output as usize).checked_add(output_len).is_none()
    {
        return Err("invalid stream buffer range");
    }
    if input_len != 0 && output_len != 0 {
        let a = input as usize;
        let b = output as usize;
        if a < b.saturating_add(output_len) && b < a.saturating_add(input_len) {
            return Err("stream input and output overlap");
        }
    }
    let input = if input_len == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(input, input_len) }
    };
    let output = if output_len == 0 {
        &mut []
    } else {
        unsafe { slice::from_raw_parts_mut(output, output_len) }
    };
    Ok((input, output))
}
fn valid_result(
    result: *mut cix_stream_result_v1,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> bool {
    if result.is_null()
        || !(result as usize).is_multiple_of(std::mem::align_of::<cix_stream_result_v1>())
    {
        return false;
    }
    let start = result as usize;
    let Some(end) = start.checked_add(std::mem::size_of::<cix_stream_result_v1>()) else {
        return false;
    };
    let overlaps = |other: usize, length: usize| match other.checked_add(length) {
        Some(other_end) => length != 0 && start < other_end && other < end,
        None => true,
    };
    !overlaps(input as usize, input_len) && !overlaps(output as usize, output_len)
}
fn valid_context<T>(context: *mut T) -> bool {
    !context.is_null() && (context as usize).is_multiple_of(std::mem::align_of::<T>())
}
fn valid_create_out<T>(out: *mut *mut T, options: *const cix_options_v1) -> bool {
    if out.is_null()
        || !(out as usize).is_multiple_of(std::mem::align_of::<*mut T>())
        || (!options.is_null()
            && !(options as usize).is_multiple_of(std::mem::align_of::<cix_options_v1>()))
    {
        return false;
    }
    let Some(out_end) = (out as usize).checked_add(std::mem::size_of::<*mut T>()) else {
        return false;
    };
    if options.is_null() {
        return true;
    }
    let start = options as usize;
    let Some(options_end) = start.checked_add(std::mem::size_of::<cix_options_v1>()) else {
        return false;
    };
    out_end <= start || options_end <= out as usize
}
fn map(error: NativeError) -> i32 {
    match error {
        NativeError::InvalidOptions(_) => CIX_STATUS_INVALID_OPTIONS,
        NativeError::OutputLimit => CIX_STATUS_RESOURCE_LIMIT,
        NativeError::Codec(_) => CIX_STATUS_CODEC_ERROR,
    }
}
fn state(state: IncrementalState) -> u32 {
    match state {
        IncrementalState::NeedsInput => CIX_STREAM_NEEDS_INPUT,
        IncrementalState::NeedsOutput => CIX_STREAM_NEEDS_OUTPUT,
        IncrementalState::Finished => CIX_STREAM_FINISHED,
    }
}
fn write(result: *mut cix_stream_result_v1, progress: IncrementalProgress) -> i32 {
    if result.is_null()
        || !(result as usize).is_multiple_of(std::mem::align_of::<cix_stream_result_v1>())
    {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    unsafe {
        ptr::write(
            result,
            cix_stream_result_v1 {
                consumed: progress.consumed,
                produced: progress.produced,
                state: state(progress.state),
            },
        );
    }
    CIX_STATUS_OK
}

macro_rules! create {
    ($name:ident, $ty:ident) => {
        /// Creates an owned incremental stream context.
        ///
        /// # Safety
        ///
        /// `options_ptr` must be null or point to a readable initialized v1
        /// options structure. `out` must be a writable, aligned pointer slot
        /// that does not overlap the options. Both locations must remain live
        /// for the call. A successful result is owned by the caller and must
        /// be destroyed exactly once.
        #[no_mangle]
        pub unsafe extern "C" fn $name(
            options_ptr: *const cix_options_v1,
            out: *mut *mut $ty,
        ) -> i32 {
            catch_unwind(AssertUnwindSafe(|| {
                if !valid_create_out::<$ty>(out, options_ptr) {
                    return CIX_STATUS_INVALID_ARGUMENT;
                }
                // Preserve the established failure contract once the caller's
                // output slot itself has passed alignment/range/overlap checks.
                unsafe { ptr::write(out, ptr::null_mut()) };
                let options = match options(options_ptr) {
                    Ok(v) => v,
                    Err(_) => return CIX_STATUS_INVALID_OPTIONS,
                };
                match $ty::new(options) {
                    Ok(v) => {
                        unsafe { ptr::write(out, Box::into_raw(Box::new(v))) };
                        CIX_STATUS_OK
                    }
                    Err(e) => map(e),
                }
            }))
            .unwrap_or(CIX_STATUS_PANIC)
        }
    };
}
impl CixStreamEncoder {
    fn new(options: NativeOptions) -> Result<Self, NativeError> {
        Ok(Self {
            inner: Mutex::new(IncrementalEncoder::new(options)?),
        })
    }
}
impl CixStreamDecoder {
    fn new(options: NativeOptions) -> Result<Self, NativeError> {
        Ok(Self {
            inner: Mutex::new(IncrementalDecoder::new(options)?),
        })
    }
}
create!(cix_stream_encoder_create, cix_stream_encoder);
create!(cix_stream_decoder_create, cix_stream_decoder);
/// Destroys an encoder context. Passing null is a no-op.
///
/// # Safety
///
/// A non-null `value` must be a live encoder returned by this API and may be
/// destroyed only once. No concurrent call may access or destroy it.
#[no_mangle]
pub unsafe extern "C" fn cix_stream_encoder_destroy(value: *mut cix_stream_encoder) {
    if valid_context(value) {
        drop(unsafe { Box::from_raw(value) })
    }
}
/// Destroys a decoder context. Passing null is a no-op.
///
/// # Safety
///
/// A non-null `value` must be a live decoder returned by this API and may be
/// destroyed only once. No concurrent call may access or destroy it.
#[no_mangle]
pub unsafe extern "C" fn cix_stream_decoder_destroy(value: *mut cix_stream_decoder) {
    if valid_context(value) {
        drop(unsafe { Box::from_raw(value) })
    }
}

/// Supplies encoder input and accepts available archive output.
///
/// # Safety
///
/// `context` must be live and not concurrently destroyed. `input` must provide
/// `input_len` readable bytes, `output` `output_len` writable bytes, and
/// `result` writable aligned result storage. All three regions must be
/// disjoint and remain valid for the call.
#[no_mangle]
pub unsafe extern "C" fn cix_stream_encoder_process(
    context: *mut cix_stream_encoder,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
    result: *mut cix_stream_result_v1,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if !valid_context(context) || !valid_result(result, input, input_len, output, output_len) {
            return CIX_STATUS_INVALID_ARGUMENT;
        }
        let (input, output) = match unsafe { buffers(input, input_len, output, output_len) } {
            Ok(v) => v,
            Err(_) => return CIX_STATUS_INVALID_ARGUMENT,
        };
        let mut stream = match unsafe { &*context }.inner.lock() {
            Ok(v) => v,
            Err(_) => return CIX_STATUS_PANIC,
        };
        match stream.process(input, output) {
            Ok(p) => write(result, p),
            Err(e) => map(e),
        }
    }))
    .unwrap_or(CIX_STATUS_PANIC)
}
/// Supplies decoder input and accepts available restored output.
///
/// # Safety
///
/// `context` must be live and not concurrently destroyed. `input` must provide
/// `input_len` readable bytes, `output` `output_len` writable bytes, and
/// `result` writable aligned result storage. All three regions must be
/// disjoint and remain valid for the call.
#[no_mangle]
pub unsafe extern "C" fn cix_stream_decoder_process(
    context: *mut cix_stream_decoder,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
    result: *mut cix_stream_result_v1,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if !valid_context(context) || !valid_result(result, input, input_len, output, output_len) {
            return CIX_STATUS_INVALID_ARGUMENT;
        }
        let (input, output) = match unsafe { buffers(input, input_len, output, output_len) } {
            Ok(v) => v,
            Err(_) => return CIX_STATUS_INVALID_ARGUMENT,
        };
        let mut stream = match unsafe { &*context }.inner.lock() {
            Ok(v) => v,
            Err(_) => return CIX_STATUS_PANIC,
        };
        match stream.process(input, output) {
            Ok(p) => write(result, p),
            Err(e) => map(e),
        }
    }))
    .unwrap_or(CIX_STATUS_PANIC)
}
/// Flushes pending encoder output without ending the stream.
///
/// # Safety
///
/// `context` must be live and not concurrently destroyed. `output` must
/// designate `output_len` writable bytes and `result` writable aligned result
/// storage; they must not overlap and must remain valid for the call.
#[no_mangle]
pub unsafe extern "C" fn cix_stream_encoder_flush(
    context: *mut cix_stream_encoder,
    output: *mut u8,
    output_len: usize,
    result: *mut cix_stream_result_v1,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if !valid_context(context) || !valid_result(result, ptr::null(), 0, output, output_len) {
            return CIX_STATUS_INVALID_ARGUMENT;
        }
        let output = match unsafe { buffers(ptr::null(), 0, output, output_len) } {
            Ok((_, v)) => v,
            Err(_) => return CIX_STATUS_INVALID_ARGUMENT,
        };
        let mut stream = match unsafe { &*context }.inner.lock() {
            Ok(v) => v,
            Err(_) => return CIX_STATUS_PANIC,
        };
        match stream.flush(output) {
            Ok(p) => write(result, p),
            Err(e) => map(e),
        }
    }))
    .unwrap_or(CIX_STATUS_PANIC)
}
/// Finishes an encoder stream and emits its terminal bytes.
///
/// # Safety
///
/// `context` must be live and not concurrently destroyed. `output` must
/// designate `output_len` writable bytes and `result` writable aligned result
/// storage; they must not overlap and must remain valid for the call.
#[no_mangle]
pub unsafe extern "C" fn cix_stream_encoder_finish(
    context: *mut cix_stream_encoder,
    output: *mut u8,
    output_len: usize,
    result: *mut cix_stream_result_v1,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if !valid_context(context) || !valid_result(result, ptr::null(), 0, output, output_len) {
            return CIX_STATUS_INVALID_ARGUMENT;
        }
        let output = match unsafe { buffers(ptr::null(), 0, output, output_len) } {
            Ok((_, v)) => v,
            Err(_) => return CIX_STATUS_INVALID_ARGUMENT,
        };
        let mut stream = match unsafe { &*context }.inner.lock() {
            Ok(v) => v,
            Err(_) => return CIX_STATUS_PANIC,
        };
        match stream.finish(output) {
            Ok(p) => write(result, p),
            Err(e) => map(e),
        }
    }))
    .unwrap_or(CIX_STATUS_PANIC)
}
/// Finishes a decoder stream after all archive input has been supplied.
///
/// # Safety
///
/// `context` must be live and not concurrently destroyed. `output` must
/// designate `output_len` writable bytes and `result` writable aligned result
/// storage; they must not overlap and must remain valid for the call.
#[no_mangle]
pub unsafe extern "C" fn cix_stream_decoder_finish(
    context: *mut cix_stream_decoder,
    output: *mut u8,
    output_len: usize,
    result: *mut cix_stream_result_v1,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if !valid_context(context) || !valid_result(result, ptr::null(), 0, output, output_len) {
            return CIX_STATUS_INVALID_ARGUMENT;
        }
        let output = match unsafe { buffers(ptr::null(), 0, output, output_len) } {
            Ok((_, v)) => v,
            Err(_) => return CIX_STATUS_INVALID_ARGUMENT,
        };
        let mut stream = match unsafe { &*context }.inner.lock() {
            Ok(v) => v,
            Err(_) => return CIX_STATUS_PANIC,
        };
        match stream.finish(output) {
            Ok(p) => write(result, p),
            Err(e) => map(e),
        }
    }))
    .unwrap_or(CIX_STATUS_PANIC)
}
/// Resets an encoder context for a new archive.
///
/// # Safety
///
/// `context` must be live and not concurrently destroyed for the entire call.
#[no_mangle]
pub unsafe extern "C" fn cix_stream_encoder_reset(context: *mut cix_stream_encoder) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if !valid_context(context) {
            return CIX_STATUS_INVALID_ARGUMENT;
        };
        let mut stream = match unsafe { &*context }.inner.lock() {
            Ok(v) => v,
            Err(_) => return CIX_STATUS_PANIC,
        };
        match stream.reset() {
            Ok(()) => CIX_STATUS_OK,
            Err(e) => map(e),
        }
    }))
    .unwrap_or(CIX_STATUS_PANIC)
}
/// Resets a decoder context for a new archive.
///
/// # Safety
///
/// `context` must be live and not concurrently destroyed for the entire call.
#[no_mangle]
pub unsafe extern "C" fn cix_stream_decoder_reset(context: *mut cix_stream_decoder) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if !valid_context(context) {
            return CIX_STATUS_INVALID_ARGUMENT;
        };
        let mut stream = match unsafe { &*context }.inner.lock() {
            Ok(v) => v,
            Err(_) => return CIX_STATUS_PANIC,
        };
        match stream.reset() {
            Ok(()) => CIX_STATUS_OK,
            Err(e) => map(e),
        }
    }))
    .unwrap_or(CIX_STATUS_PANIC)
}
