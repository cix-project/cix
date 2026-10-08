//! Allocation-explicit ABI for browser/Node wrappers.
//!
//! Symbols are exported only for `wasm32`. On another target these are still
//! Rust-callable unsafe FFI functions, not a supported native C ABI. A caller
//! must pass live, correctly aligned, non-overlapping ranges it owns; no Rust
//! function can prove allocation provenance from a raw pointer alone.

use crate::{
    decode, encode, DecodeConfig, EncodeConfig, StreamDecoder, StreamEncoder, StreamState,
};
use std::panic::{catch_unwind, AssertUnwindSafe};

const ABI_ERROR: i32 = -1;
const ABI_ARGUMENT: i32 = -2;
const ABI_PANIC: i32 = -3;

/// Opaque state allocated by `cix_portable_encoder_new`.
pub struct WasmEncoder {
    inner: StreamEncoder,
}
/// Opaque state allocated by `cix_portable_decoder_new`.
pub struct WasmDecoder {
    inner: StreamDecoder,
}

#[derive(Clone, Copy)]
struct ByteRange {
    start: usize,
    end: usize,
}
fn checked_range(ptr: *const u8, len: usize, alignment: usize) -> Result<Option<ByteRange>, i32> {
    if len == 0 {
        return Ok(None);
    }
    if ptr.is_null() || (ptr as usize) % alignment != 0 || len > isize::MAX as usize {
        return Err(ABI_ARGUMENT);
    }
    let start = ptr as usize;
    let end = start.checked_add(len).ok_or(ABI_ARGUMENT)?;
    Ok(Some(ByteRange { start, end }))
}
fn overlap(a: Option<ByteRange>, b: Option<ByteRange>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a.start < b.end && b.start < a.end)
}
fn checked_buffers(
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
    counts: Option<(*mut u32, *mut u32)>,
    state: Option<ByteRange>,
) -> Result<(), i32> {
    let input = checked_range(input, input_len, 1)?;
    let output = checked_range(output.cast_const(), output_len, 1)?;
    if overlap(input, output) || overlap(input, state) || overlap(output, state) {
        return Err(ABI_ARGUMENT);
    }
    if let Some((consumed, produced)) = counts {
        let consumed = checked_range(
            consumed.cast(),
            std::mem::size_of::<u32>(),
            std::mem::align_of::<u32>(),
        )?;
        let produced = checked_range(
            produced.cast(),
            std::mem::size_of::<u32>(),
            std::mem::align_of::<u32>(),
        )?;
        if overlap(consumed, produced)
            || overlap(input, consumed)
            || overlap(input, produced)
            || overlap(output, consumed)
            || overlap(output, produced)
            || overlap(state, consumed)
            || overlap(state, produced)
        {
            return Err(ABI_ARGUMENT);
        }
    }
    Ok(())
}
fn checked_handle<T>(ptr: *mut T) -> Result<ByteRange, i32> {
    checked_range(
        ptr.cast(),
        std::mem::size_of::<T>(),
        std::mem::align_of::<T>(),
    )?
    .ok_or(ABI_ARGUMENT)
}
unsafe fn input<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}
unsafe fn output<'a>(ptr: *mut u8, len: usize) -> &'a mut [u8] {
    if len == 0 {
        unsafe { std::slice::from_raw_parts_mut(std::ptr::NonNull::<u8>::dangling().as_ptr(), 0) }
    } else {
        unsafe { std::slice::from_raw_parts_mut(ptr, len) }
    }
}
unsafe fn handle<T>(ptr: *mut T) -> Result<&'static mut T, i32> {
    if ptr.is_null() || (ptr as usize) % std::mem::align_of::<T>() != 0 {
        return Err(ABI_ARGUMENT);
    }
    // Validity and exclusive ownership are the unsafe FFI caller contract.
    Ok(unsafe { &mut *ptr })
}
unsafe fn put_counts(
    consumed: *mut u32,
    produced: *mut u32,
    input: usize,
    output: usize,
) -> Result<(), i32> {
    unsafe {
        *consumed = u32::try_from(input).map_err(|_| ABI_ARGUMENT)?;
        *produced = u32::try_from(output).map_err(|_| ABI_ARGUMENT)?;
    }
    Ok(())
}
fn state_code(state: StreamState) -> i32 {
    state as i32
}
fn ffi(call: impl FnOnce() -> Result<i32, i32>) -> i32 {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => error,
        Err(_) => ABI_PANIC,
    }
}

#[cfg_attr(target_arch = "wasm32", no_mangle)]
pub extern "C" fn cix_portable_alloc(len: u32) -> *mut u8 {
    match catch_unwind(AssertUnwindSafe(|| {
        let mut bytes = Vec::<u8>::new();
        bytes.try_reserve_exact(len as usize).map_err(|_| ())?;
        bytes.resize(len as usize, 0);
        Ok::<*mut u8, ()>(Box::into_raw(bytes.into_boxed_slice()) as *mut u8)
    })) {
        Ok(Ok(ptr)) => ptr,
        _ => std::ptr::null_mut(),
    }
}
#[cfg_attr(target_arch = "wasm32", no_mangle)]
pub unsafe extern "C" fn cix_portable_free(ptr: *mut u8, len: u32) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !ptr.is_null() {
            // `alloc` created a boxed slice of exactly this length.
            unsafe {
                drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                    ptr,
                    len as usize,
                )));
            }
        }
    }));
}

#[cfg_attr(target_arch = "wasm32", no_mangle)]
pub unsafe extern "C" fn cix_portable_encode(
    input_ptr: *const u8,
    input_len: u32,
    output_ptr: *mut u8,
    output_cap: u32,
) -> i32 {
    ffi(|| unsafe {
        checked_buffers(
            input_ptr,
            input_len as usize,
            output_ptr,
            output_cap as usize,
            None,
            None,
        )?;
        let input = input(input_ptr, input_len as usize);
        let output = output(output_ptr, output_cap as usize);
        let archive = encode(
            input,
            EncodeConfig {
                output_limit: output.len(),
                ..EncodeConfig::default()
            },
        )
        .map_err(|_| ABI_ERROR)?;
        if archive.len() > output.len() {
            return Err(ABI_ERROR);
        }
        output[..archive.len()].copy_from_slice(&archive);
        i32::try_from(archive.len()).map_err(|_| ABI_ERROR)
    })
}
#[cfg_attr(target_arch = "wasm32", no_mangle)]
pub unsafe extern "C" fn cix_portable_decode(
    input_ptr: *const u8,
    input_len: u32,
    output_ptr: *mut u8,
    output_cap: u32,
) -> i32 {
    ffi(|| unsafe {
        checked_buffers(
            input_ptr,
            input_len as usize,
            output_ptr,
            output_cap as usize,
            None,
            None,
        )?;
        let input = input(input_ptr, input_len as usize);
        let output = output(output_ptr, output_cap as usize);
        let decoded = decode(
            input,
            DecodeConfig {
                output_limit: output.len(),
                archive_limit: input.len(),
                ..DecodeConfig::default()
            },
        )
        .map_err(|_| ABI_ERROR)?;
        if decoded.len() > output.len() {
            return Err(ABI_ERROR);
        }
        output[..decoded.len()].copy_from_slice(&decoded);
        i32::try_from(decoded.len()).map_err(|_| ABI_ERROR)
    })
}

#[cfg_attr(target_arch = "wasm32", no_mangle)]
pub extern "C" fn cix_portable_encoder_new(output_limit: u32) -> *mut WasmEncoder {
    match catch_unwind(AssertUnwindSafe(|| {
        StreamEncoder::new(EncodeConfig {
            output_limit: output_limit as usize,
            ..EncodeConfig::default()
        })
        .map(|value| Box::into_raw(Box::new(WasmEncoder { inner: value })))
    })) {
        Ok(Ok(ptr)) => ptr,
        _ => std::ptr::null_mut(),
    }
}
#[cfg_attr(target_arch = "wasm32", no_mangle)]
pub unsafe extern "C" fn cix_portable_encoder_free(handle: *mut WasmEncoder) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !handle.is_null() {
            unsafe { drop(Box::from_raw(handle)) }
        }
    }));
}
#[cfg_attr(target_arch = "wasm32", no_mangle)]
pub unsafe extern "C" fn cix_portable_encoder_process(
    handle_ptr: *mut WasmEncoder,
    input_ptr: *const u8,
    input_len: u32,
    output_ptr: *mut u8,
    output_cap: u32,
    finish: u32,
    consumed: *mut u32,
    produced: *mut u32,
) -> i32 {
    ffi(|| unsafe {
        let state = checked_handle(handle_ptr)?;
        checked_buffers(
            input_ptr,
            input_len as usize,
            output_ptr,
            output_cap as usize,
            Some((consumed, produced)),
            Some(state),
        )?;
        let handle = handle(handle_ptr)?;
        let input = input(input_ptr, input_len as usize);
        let output = output(output_ptr, output_cap as usize);
        let progress = if finish == 0 {
            handle.inner.process(input, output)
        } else if input.is_empty() {
            handle.inner.finish(output)
        } else {
            return Err(ABI_ARGUMENT);
        }
        .map_err(|_| ABI_ERROR)?;
        put_counts(consumed, produced, progress.consumed, progress.produced)?;
        Ok(state_code(progress.state))
    })
}

#[cfg_attr(target_arch = "wasm32", no_mangle)]
pub extern "C" fn cix_portable_decoder_new(
    output_limit: u32,
    archive_limit: u32,
) -> *mut WasmDecoder {
    match catch_unwind(AssertUnwindSafe(|| {
        StreamDecoder::new(DecodeConfig {
            output_limit: output_limit as usize,
            archive_limit: archive_limit as usize,
            ..DecodeConfig::default()
        })
        .map(|value| Box::into_raw(Box::new(WasmDecoder { inner: value })))
    })) {
        Ok(Ok(ptr)) => ptr,
        _ => std::ptr::null_mut(),
    }
}
#[cfg_attr(target_arch = "wasm32", no_mangle)]
pub unsafe extern "C" fn cix_portable_decoder_free(handle: *mut WasmDecoder) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !handle.is_null() {
            unsafe { drop(Box::from_raw(handle)) }
        }
    }));
}
#[cfg_attr(target_arch = "wasm32", no_mangle)]
pub unsafe extern "C" fn cix_portable_decoder_process(
    handle_ptr: *mut WasmDecoder,
    input_ptr: *const u8,
    input_len: u32,
    output_ptr: *mut u8,
    output_cap: u32,
    consumed: *mut u32,
    produced: *mut u32,
) -> i32 {
    ffi(|| unsafe {
        let state = checked_handle(handle_ptr)?;
        checked_buffers(
            input_ptr,
            input_len as usize,
            output_ptr,
            output_cap as usize,
            Some((consumed, produced)),
            Some(state),
        )?;
        let handle = handle(handle_ptr)?;
        let input = input(input_ptr, input_len as usize);
        let output = output(output_ptr, output_cap as usize);
        let progress = handle.inner.process(input, output).map_err(|_| ABI_ERROR)?;
        put_counts(consumed, produced, progress.consumed, progress.produced)?;
        Ok(state_code(progress.state))
    })
}
