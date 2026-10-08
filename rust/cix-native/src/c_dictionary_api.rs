//! Versioned caller-buffer C ABI for owned Zstd dictionary handles.
//!
//! This module owns a copy of every imported or trained dictionary. A handle
//! is therefore independent of the caller's training and dictionary buffers.
//! Operations are serialized per handle, do not spawn processes, and contain
//! Rust panics at the ABI boundary. C callers remain responsible for passing
//! live, correctly sized, disjoint buffers and for not freeing a handle while
//! another thread is operating on it.

use crate::c_api::{
    CIX_STATUS_CODEC_ERROR, CIX_STATUS_INVALID_ARGUMENT, CIX_STATUS_INVALID_OPTIONS, CIX_STATUS_OK,
    CIX_STATUS_OUTPUT_TOO_SMALL, CIX_STATUS_PANIC, CIX_STATUS_RESOURCE_LIMIT,
};
use crate::dictionaries::{DictionaryIdentity, ZstdDictionary};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    ptr, slice,
    sync::Mutex,
};

const DICTIONARY_ABI_V1: u32 = 1;

#[repr(C)]
pub struct cix_zstd_dictionary {
    inner: Mutex<ZstdDictionary>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct cix_dictionary_options_v1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub level: i32,
    pub reserved: u32,
    pub output_limit: u64,
    pub memory_limit: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct cix_dictionary_identity_v1 {
    pub zstd_id: u32,
    pub sha256: [u8; 32],
}

const OPTIONS_V1_SIZE: usize = std::mem::size_of::<cix_dictionary_options_v1>();

fn range(address: usize, length: usize) -> Result<(usize, usize), i32> {
    if length > isize::MAX as usize || (length != 0 && address == 0) {
        return Err(CIX_STATUS_INVALID_ARGUMENT);
    }
    let end = address
        .checked_add(length)
        .ok_or(CIX_STATUS_INVALID_ARGUMENT)?;
    Ok((address, end))
}

fn overlaps(left: (usize, usize), right: (usize, usize)) -> bool {
    left.0 < left.1 && right.0 < right.1 && left.0 < right.1 && right.0 < left.1
}

fn disjoint(regions: &[(usize, usize)]) -> bool {
    regions.iter().enumerate().all(|(index, left)| {
        regions
            .iter()
            .skip(index + 1)
            .all(|right| !overlaps(*left, *right))
    })
}

fn dictionary_error_status(error: &str) -> i32 {
    if error.contains("resource limit")
        || error.contains("allocation")
        || error.contains("exceeded cap")
        || error.contains("memory estimate")
    {
        CIX_STATUS_RESOURCE_LIMIT
    } else {
        CIX_STATUS_CODEC_ERROR
    }
}

unsafe fn options(raw: *const cix_dictionary_options_v1) -> Result<(usize, usize, i32), i32> {
    if raw.is_null() {
        return Err(CIX_STATUS_INVALID_ARGUMENT);
    }
    // Read the stable prefix before trusting the whole structure. This keeps a
    // too-small extension struct an ABI error rather than an out-of-bounds read.
    let version = unsafe { ptr::read_unaligned(ptr::addr_of!((*raw).abi_version)) };
    let struct_size = unsafe { ptr::read_unaligned(ptr::addr_of!((*raw).struct_size)) };
    if version != DICTIONARY_ABI_V1 || (struct_size as usize) < OPTIONS_V1_SIZE {
        return Err(CIX_STATUS_INVALID_OPTIONS);
    }
    let value = unsafe { ptr::read_unaligned(raw) };
    let output_limit =
        usize::try_from(value.output_limit).map_err(|_| CIX_STATUS_RESOURCE_LIMIT)?;
    let memory_limit =
        usize::try_from(value.memory_limit).map_err(|_| CIX_STATUS_RESOURCE_LIMIT)?;
    if output_limit == 0
        || memory_limit == 0
        || value.reserved != 0
        || !(-131072..=22).contains(&value.level)
    {
        return Err(CIX_STATUS_INVALID_OPTIONS);
    }
    Ok((output_limit, memory_limit, value.level))
}

unsafe fn dictionary<'a>(
    handle: *const cix_zstd_dictionary,
) -> Result<&'a cix_zstd_dictionary, i32> {
    if handle.is_null()
        || !(handle as usize).is_multiple_of(std::mem::align_of::<cix_zstd_dictionary>())
    {
        return Err(CIX_STATUS_INVALID_ARGUMENT);
    }
    // SAFETY: the C contract permits only a live handle returned by this API.
    Ok(unsafe { &*handle })
}

fn identity_from_raw(value: cix_dictionary_identity_v1) -> DictionaryIdentity {
    DictionaryIdentity {
        dict_id: value.zstd_id,
        sha256: value.sha256,
    }
}

struct OperationBuffers {
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_capacity: usize,
    written: *mut usize,
}

fn operation_regions(
    handle: *const cix_zstd_dictionary,
    raw_options: *const cix_dictionary_options_v1,
    identity: Option<*const cix_dictionary_identity_v1>,
    buffers: &OperationBuffers,
) -> Result<(), i32> {
    if buffers.written.is_null()
        || !(buffers.written as usize).is_multiple_of(std::mem::align_of::<usize>())
    {
        return Err(CIX_STATUS_INVALID_ARGUMENT);
    }
    let base = [
        range(handle as usize, std::mem::size_of::<cix_zstd_dictionary>())?,
        range(raw_options as usize, OPTIONS_V1_SIZE)?,
        range(buffers.input as usize, buffers.input_len)?,
        range(buffers.output as usize, buffers.output_capacity)?,
        range(buffers.written as usize, std::mem::size_of::<usize>())?,
    ];
    if !disjoint(&base) {
        return Err(CIX_STATUS_INVALID_ARGUMENT);
    }
    if let Some(identity) = identity {
        let identity = range(
            identity as usize,
            std::mem::size_of::<cix_dictionary_identity_v1>(),
        )?;
        if base.iter().any(|region| overlaps(*region, identity)) {
            return Err(CIX_STATUS_INVALID_ARGUMENT);
        }
    }
    Ok(())
}

unsafe fn copy_input<'a>(input: *const u8, input_len: usize) -> &'a [u8] {
    if input_len == 0 {
        &[]
    } else {
        // SAFETY: `operation_regions` checked the numeric range and the C
        // caller contract supplies readable bytes for that range.
        unsafe { slice::from_raw_parts(input, input_len) }
    }
}

fn write_result(result: Vec<u8>, output: *mut u8, capacity: usize, written: *mut usize) -> i32 {
    // SAFETY: operation_regions checked writable `written`; the C contract
    // keeps it live for this call.
    unsafe { ptr::write(written, result.len()) };
    if result.len() > capacity {
        return CIX_STATUS_OUTPUT_TOO_SMALL;
    }
    if !result.is_empty() {
        // SAFETY: operation_regions proved disjointness; capacity is enough.
        unsafe { ptr::copy_nonoverlapping(result.as_ptr(), output, result.len()) };
    }
    CIX_STATUS_OK
}

fn run_operation(
    handle: *const cix_zstd_dictionary,
    raw_options: *const cix_dictionary_options_v1,
    identity: Option<*const cix_dictionary_identity_v1>,
    buffers: OperationBuffers,
    decode: bool,
) -> i32 {
    if let Err(status) = operation_regions(handle, raw_options, identity, &buffers) {
        return status;
    }
    // SAFETY: operation_regions validated the aligned, writable destination.
    unsafe { ptr::write(buffers.written, 0) };
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<Vec<u8>, i32> {
        let (output_limit, memory_limit, level) = unsafe { options(raw_options) }?;
        // The dictionary implementation owns a temporary output vector while
        // the caller's destination remains live. Reserve both in the C ABI
        // admission budget, rather than treating caller storage as free.
        let memory_limit = memory_limit
            .checked_sub(buffers.output_capacity)
            .and_then(|remaining| remaining.checked_sub(std::mem::size_of::<cix_zstd_dictionary>()))
            .ok_or(CIX_STATUS_RESOURCE_LIMIT)?;
        let input = unsafe { copy_input(buffers.input, buffers.input_len) };
        let dictionary = unsafe { dictionary(handle) }?;
        let locked = dictionary.inner.lock().map_err(|_| CIX_STATUS_PANIC)?;
        if decode {
            let identity = identity.ok_or(CIX_STATUS_INVALID_ARGUMENT)?;
            let declared = unsafe { ptr::read_unaligned(identity) };
            locked
                .decode(
                    input,
                    output_limit,
                    memory_limit,
                    &identity_from_raw(declared),
                )
                .map_err(|error| dictionary_error_status(&error))
        } else {
            locked
                .encode(input, level, output_limit, memory_limit)
                .map_err(|error| dictionary_error_status(&error))
        }
    }));
    match result {
        Ok(Ok(bytes)) => write_result(
            bytes,
            buffers.output,
            buffers.output_capacity,
            buffers.written,
        ),
        Ok(Err(status)) => status,
        Err(_) => CIX_STATUS_PANIC,
    }
}

/// Imports an owned Zstd dictionary. `memory_limit` must account for both the
/// caller's input and the copied bytes retained in the resulting handle.
///
/// # Safety
///
/// `bytes` must designate `bytes_len` readable bytes and `out_handle` a
/// writable, aligned pointer slot. They must not overlap and remain valid for
/// the call. On success the caller owns the returned handle and must free it
/// exactly once.
#[no_mangle]
pub unsafe extern "C" fn cix_zstd_dictionary_create_v1(
    bytes: *const u8,
    bytes_len: usize,
    memory_limit: u64,
    out_handle: *mut *mut cix_zstd_dictionary,
) -> i32 {
    if out_handle.is_null()
        || !(out_handle as usize).is_multiple_of(std::mem::align_of::<*mut cix_zstd_dictionary>())
    {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    let regions = [
        range(bytes as usize, bytes_len),
        range(
            out_handle as usize,
            std::mem::size_of::<*mut cix_zstd_dictionary>(),
        ),
    ];
    if regions.iter().any(Result::is_err) || !disjoint(&regions.map(Result::unwrap)) {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: caller provided a writable handle result pointer.
    unsafe { ptr::write(out_handle, ptr::null_mut()) };
    let memory_limit = match usize::try_from(memory_limit) {
        Ok(value) if value != 0 => value,
        _ => return CIX_STATUS_INVALID_OPTIONS,
    };
    let create_need = bytes_len
        .checked_mul(2)
        .and_then(|need| need.checked_add(std::mem::size_of::<cix_zstd_dictionary>()));
    if bytes_len == 0 || create_need.is_none_or(|need| need > memory_limit) {
        return CIX_STATUS_RESOURCE_LIMIT;
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<ZstdDictionary, i32> {
        let source = unsafe { copy_input(bytes, bytes_len) };
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(bytes_len)
            .map_err(|_| CIX_STATUS_RESOURCE_LIMIT)?;
        owned.extend_from_slice(source);
        ZstdDictionary::from_bytes(owned).map_err(|error| dictionary_error_status(&error))
    }));
    match result {
        Ok(Ok(dictionary)) => {
            // SAFETY: `out_handle` was validated and initialized above.
            unsafe {
                ptr::write(
                    out_handle,
                    Box::into_raw(Box::new(cix_zstd_dictionary {
                        inner: Mutex::new(dictionary),
                    })),
                );
            }
            CIX_STATUS_OK
        }
        Ok(Err(status)) => status,
        Err(_) => CIX_STATUS_PANIC,
    }
}

/// Trains an owned dictionary from contiguous samples and their exact sizes.
///
/// # Safety
///
/// `samples` and `sample_sizes` must designate readable regions of their
/// stated sizes, and `out_handle` a writable, aligned pointer slot. The three
/// regions must be disjoint and live for the call. A successful result handle
/// is caller-owned and must be freed exactly once.
#[no_mangle]
pub unsafe extern "C" fn cix_zstd_dictionary_train_v1(
    samples: *const u8,
    samples_len: usize,
    sample_sizes: *const usize,
    sample_count: usize,
    dictionary_capacity: usize,
    memory_limit: u64,
    out_handle: *mut *mut cix_zstd_dictionary,
) -> i32 {
    let sizes_bytes = match sample_count.checked_mul(std::mem::size_of::<usize>()) {
        Some(value) => value,
        None => return CIX_STATUS_INVALID_ARGUMENT,
    };
    if out_handle.is_null()
        || !(out_handle as usize).is_multiple_of(std::mem::align_of::<*mut cix_zstd_dictionary>())
        || (sample_count != 0
            && (!(sample_sizes as usize).is_multiple_of(std::mem::align_of::<usize>())
                || sample_sizes.is_null()))
    {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    let regions = [
        range(samples as usize, samples_len),
        range(sample_sizes as usize, sizes_bytes),
        range(
            out_handle as usize,
            std::mem::size_of::<*mut cix_zstd_dictionary>(),
        ),
    ];
    if regions.iter().any(Result::is_err) {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    let regions = regions.map(Result::unwrap);
    if !disjoint(&regions) {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: validated as a writable, disjoint caller location.
    unsafe { ptr::write(out_handle, ptr::null_mut()) };
    let memory_limit = match usize::try_from(memory_limit) {
        Ok(value) if value != 0 => value,
        _ => return CIX_STATUS_INVALID_OPTIONS,
    };
    // Account for the caller-owned training blob and sizes while the native
    // trainer's independently admitted workspace is live.
    let views_bytes = match sample_count.checked_mul(std::mem::size_of::<&[u8]>()) {
        Some(value) => value,
        None => return CIX_STATUS_RESOURCE_LIMIT,
    };
    let inner_memory = samples_len
        .checked_add(sizes_bytes)
        .and_then(|external| external.checked_add(views_bytes))
        .and_then(|external| external.checked_add(std::mem::size_of::<cix_zstd_dictionary>()))
        .and_then(|external| memory_limit.checked_sub(external));
    let Some(inner_memory) = inner_memory else {
        return CIX_STATUS_RESOURCE_LIMIT;
    };
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<ZstdDictionary, i32> {
        let bytes = unsafe { copy_input(samples, samples_len) };
        let sizes = if sample_count == 0 {
            &[]
        } else {
            // SAFETY: `regions` established the numeric range and alignment;
            // the C caller provides a readable array.
            unsafe { slice::from_raw_parts(sample_sizes, sample_count) }
        };
        let mut offset = 0usize;
        let mut views = Vec::new();
        views
            .try_reserve_exact(sample_count)
            .map_err(|_| CIX_STATUS_RESOURCE_LIMIT)?;
        for &size in sizes {
            let end = offset
                .checked_add(size)
                .ok_or(CIX_STATUS_INVALID_ARGUMENT)?;
            let sample = bytes.get(offset..end).ok_or(CIX_STATUS_INVALID_ARGUMENT)?;
            views.push(sample);
            offset = end;
        }
        if offset != bytes.len() {
            return Err(CIX_STATUS_INVALID_ARGUMENT);
        }
        ZstdDictionary::train(&views, dictionary_capacity, inner_memory)
            .map_err(|error| dictionary_error_status(&error))
    }));
    match result {
        Ok(Ok(dictionary)) => {
            // SAFETY: `out_handle` remains owned by the caller for this call.
            unsafe {
                ptr::write(
                    out_handle,
                    Box::into_raw(Box::new(cix_zstd_dictionary {
                        inner: Mutex::new(dictionary),
                    })),
                );
            }
            CIX_STATUS_OK
        }
        Ok(Err(status)) => status,
        Err(_) => CIX_STATUS_PANIC,
    }
}

/// Frees a handle returned by `create` or `train`. A null handle is allowed.
/// The caller must not call this concurrently with any operation on the handle.
///
/// # Safety
///
/// A non-null `handle` must be a live handle returned by this API and may be
/// passed here only once. No other operation may access it while it is freed.
#[no_mangle]
pub unsafe extern "C" fn cix_zstd_dictionary_free(handle: *mut cix_zstd_dictionary) {
    if !handle.is_null()
        && (handle as usize).is_multiple_of(std::mem::align_of::<cix_zstd_dictionary>())
    {
        // SAFETY: the C contract permits only one free of a live API handle.
        unsafe { drop(Box::from_raw(handle)) };
    }
}

/// Exports the exact dictionary bytes into caller-owned storage. On
/// `CIX_STATUS_OUTPUT_TOO_SMALL`, `written` receives the required byte count
/// and `output` remains untouched.
///
/// # Safety
///
/// `handle` must be live, `output` must designate `output_capacity` writable
/// bytes, and `written` an aligned writable `usize`. The regions must not
/// overlap and must remain valid for the call; the handle must not be freed
/// concurrently.
#[no_mangle]
pub unsafe extern "C" fn cix_zstd_dictionary_bytes_v1(
    handle: *const cix_zstd_dictionary,
    output: *mut u8,
    output_capacity: usize,
    written: *mut usize,
) -> i32 {
    if written.is_null() || !(written as usize).is_multiple_of(std::mem::align_of::<usize>()) {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    let regions = [
        range(handle as usize, std::mem::size_of::<cix_zstd_dictionary>()),
        range(output as usize, output_capacity),
        range(written as usize, std::mem::size_of::<usize>()),
    ];
    if regions.iter().any(Result::is_err) || !disjoint(&regions.map(Result::unwrap)) {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: `written` is a validated aligned, writable caller location.
    unsafe { ptr::write(written, 0) };
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), i32> {
        let dictionary = unsafe { dictionary(handle) }?;
        let locked = dictionary.inner.lock().map_err(|_| CIX_STATUS_PANIC)?;
        let bytes = locked.as_bytes();
        // SAFETY: `written` remains valid for the call under the C contract.
        unsafe { ptr::write(written, bytes.len()) };
        if bytes.len() > output_capacity {
            return Err(CIX_STATUS_OUTPUT_TOO_SMALL);
        }
        if !bytes.is_empty() {
            // SAFETY: the range checks above establish output capacity and
            // disjointness from the handle.
            unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len()) };
        }
        Ok(())
    }));
    match result {
        Ok(Ok(())) => CIX_STATUS_OK,
        Ok(Err(status)) => status,
        Err(_) => CIX_STATUS_PANIC,
    }
}

/// Returns the immutable identity of a live dictionary handle.
///
/// # Safety
///
/// `handle` must be live and `out_identity` must be writable storage for one
/// identity value. They must not overlap, remain valid for the call, and the
/// handle must not be freed concurrently.
#[no_mangle]
pub unsafe extern "C" fn cix_zstd_dictionary_identity_v1(
    handle: *const cix_zstd_dictionary,
    out_identity: *mut cix_dictionary_identity_v1,
) -> i32 {
    let regions = [
        range(handle as usize, std::mem::size_of::<cix_zstd_dictionary>()),
        range(
            out_identity as usize,
            std::mem::size_of::<cix_dictionary_identity_v1>(),
        ),
    ];
    if out_identity.is_null()
        || regions.iter().any(Result::is_err)
        || !disjoint(&regions.map(Result::unwrap))
    {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    let result = catch_unwind(AssertUnwindSafe(
        || -> Result<cix_dictionary_identity_v1, i32> {
            let dictionary = unsafe { dictionary(handle) }?;
            let locked = dictionary.inner.lock().map_err(|_| CIX_STATUS_PANIC)?;
            let identity = locked.identity();
            Ok(cix_dictionary_identity_v1 {
                zstd_id: identity.dict_id,
                sha256: identity.sha256,
            })
        },
    ));
    match result {
        Ok(Ok(identity)) => {
            // SAFETY: `out_identity` was range-validated above.
            unsafe { ptr::write_unaligned(out_identity, identity) };
            CIX_STATUS_OK
        }
        Ok(Err(status)) => status,
        Err(_) => CIX_STATUS_PANIC,
    }
}

/// Encodes one ordinary Zstd frame using this dictionary and the requested level.
///
/// # Safety
///
/// `handle` and `raw_options` must reference live, readable API objects.
/// `input` must provide `input_len` readable bytes, `output` must provide
/// `output_capacity` writable bytes, and `written` a writable aligned `usize`.
/// All supplied regions must be disjoint and live for the call; the handle may
/// not be freed concurrently.
#[no_mangle]
pub unsafe extern "C" fn cix_zstd_dictionary_encode_v1(
    handle: *const cix_zstd_dictionary,
    raw_options: *const cix_dictionary_options_v1,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_capacity: usize,
    written: *mut usize,
) -> i32 {
    run_operation(
        handle,
        raw_options,
        None,
        OperationBuffers {
            input,
            input_len,
            output,
            output_capacity,
            written,
        },
        false,
    )
}

/// Decodes exactly one ordinary Zstd frame with its declared dictionary
/// identity. This is mandatory for ID-0 frames and verified for all frames.
///
/// # Safety
///
/// `handle`, `raw_options`, and `declared_identity` must reference live,
/// readable API objects. `input` must provide `input_len` readable bytes,
/// `output` `output_capacity` writable bytes, and `written` an aligned
/// writable `usize`. These regions must be disjoint and live for the call;
/// the handle may not be freed concurrently.
#[no_mangle]
pub unsafe extern "C" fn cix_zstd_dictionary_decode_v1(
    handle: *const cix_zstd_dictionary,
    raw_options: *const cix_dictionary_options_v1,
    declared_identity: *const cix_dictionary_identity_v1,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_capacity: usize,
    written: *mut usize,
) -> i32 {
    run_operation(
        handle,
        raw_options,
        Some(declared_identity),
        OperationBuffers {
            input,
            input_len,
            output,
            output_capacity,
            written,
        },
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> cix_dictionary_options_v1 {
        cix_dictionary_options_v1 {
            abi_version: DICTIONARY_ABI_V1,
            struct_size: OPTIONS_V1_SIZE as u32,
            level: 3,
            reserved: 0,
            output_limit: 4096,
            memory_limit: 128 << 20,
        }
    }

    fn training_blob() -> (Vec<u8>, Vec<usize>) {
        let samples = (0..64u8)
            .map(|seed| {
                (0..96u8)
                    .map(|offset| seed.wrapping_mul(17).wrapping_add(offset % 13))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let sizes = samples.iter().map(Vec::len).collect::<Vec<_>>();
        (samples.concat(), sizes)
    }

    #[test]
    fn lifecycle_identity_and_caller_buffer_contract() {
        let (blob, sizes) = training_blob();
        let mut handle = ptr::null_mut();
        assert_eq!(
            unsafe {
                cix_zstd_dictionary_train_v1(
                    blob.as_ptr(),
                    blob.len(),
                    sizes.as_ptr(),
                    sizes.len(),
                    1024,
                    128 << 20,
                    &mut handle,
                )
            },
            CIX_STATUS_OK
        );
        let mut identity = cix_dictionary_identity_v1 {
            zstd_id: 0,
            sha256: [0; 32],
        };
        assert_eq!(
            unsafe { cix_zstd_dictionary_identity_v1(handle, &mut identity) },
            CIX_STATUS_OK
        );
        let mut exported = [0u8; 4096];
        let mut exported_len = 0usize;
        assert_eq!(
            unsafe { cix_zstd_dictionary_bytes_v1(handle, ptr::null_mut(), 0, &mut exported_len) },
            CIX_STATUS_OUTPUT_TOO_SMALL
        );
        assert!(exported_len > 0);
        assert_eq!(
            unsafe {
                cix_zstd_dictionary_bytes_v1(
                    handle,
                    exported.as_mut_ptr(),
                    exported.len(),
                    &mut exported_len,
                )
            },
            CIX_STATUS_OK
        );
        let mut imported = ptr::null_mut();
        assert_eq!(
            unsafe {
                cix_zstd_dictionary_create_v1(
                    exported.as_ptr(),
                    exported_len,
                    128 << 20,
                    &mut imported,
                )
            },
            CIX_STATUS_OK
        );
        let mut imported_identity = cix_dictionary_identity_v1 {
            zstd_id: 0,
            sha256: [0; 32],
        };
        assert_eq!(
            unsafe { cix_zstd_dictionary_identity_v1(imported, &mut imported_identity) },
            CIX_STATUS_OK
        );
        assert_eq!(imported_identity.sha256, identity.sha256);
        assert_eq!(imported_identity.zstd_id, identity.zstd_id);
        let input = b"dictionary C ABI lifecycle dictionary C ABI lifecycle";
        let mut encoded = [0u8; 4096];
        let mut encoded_len = 0usize;
        assert_eq!(
            unsafe {
                cix_zstd_dictionary_encode_v1(
                    handle,
                    &options(),
                    input.as_ptr(),
                    input.len(),
                    encoded.as_mut_ptr(),
                    encoded.len(),
                    &mut encoded_len,
                )
            },
            CIX_STATUS_OK
        );
        let mut decoded = [0u8; 4096];
        let mut decoded_len = 0usize;
        assert_eq!(
            unsafe {
                cix_zstd_dictionary_decode_v1(
                    imported,
                    &options(),
                    &identity,
                    encoded.as_ptr(),
                    encoded_len,
                    decoded.as_mut_ptr(),
                    decoded.len(),
                    &mut decoded_len,
                )
            },
            CIX_STATUS_OK
        );
        assert_eq!(&decoded[..decoded_len], input);
        unsafe {
            cix_zstd_dictionary_free(imported);
            cix_zstd_dictionary_free(handle);
        };
    }

    #[test]
    fn rejects_wrong_identity_small_output_and_malformed_training() {
        let (blob, sizes) = training_blob();
        let mut handle = ptr::null_mut();
        assert_eq!(
            unsafe {
                cix_zstd_dictionary_train_v1(
                    blob.as_ptr(),
                    blob.len(),
                    sizes.as_ptr(),
                    sizes.len(),
                    1024,
                    128 << 20,
                    &mut handle,
                )
            },
            CIX_STATUS_OK
        );
        let input = b"caller output contract";
        let mut short = [0u8; 1];
        let mut needed = 0usize;
        assert_eq!(
            unsafe {
                cix_zstd_dictionary_encode_v1(
                    handle,
                    &options(),
                    input.as_ptr(),
                    input.len(),
                    short.as_mut_ptr(),
                    short.len(),
                    &mut needed,
                )
            },
            CIX_STATUS_OUTPUT_TOO_SMALL
        );
        assert!(needed > short.len());
        let mut encoded = [0u8; 4096];
        let mut encoded_len = 0usize;
        assert_eq!(
            unsafe {
                cix_zstd_dictionary_encode_v1(
                    handle,
                    &options(),
                    input.as_ptr(),
                    input.len(),
                    encoded.as_mut_ptr(),
                    encoded.len(),
                    &mut encoded_len,
                )
            },
            CIX_STATUS_OK
        );
        let wrong = cix_dictionary_identity_v1 {
            zstd_id: 0,
            sha256: [1; 32],
        };
        let mut decoded = [0u8; 4096];
        let mut decoded_len = 0usize;
        assert_eq!(
            unsafe {
                cix_zstd_dictionary_decode_v1(
                    handle,
                    &options(),
                    &wrong,
                    encoded.as_ptr(),
                    encoded_len,
                    decoded.as_mut_ptr(),
                    decoded.len(),
                    &mut decoded_len,
                )
            },
            CIX_STATUS_CODEC_ERROR
        );
        let malformed_sizes = [blob.len() - 1];
        let mut ignored = ptr::null_mut();
        assert_eq!(
            unsafe {
                cix_zstd_dictionary_train_v1(
                    blob.as_ptr(),
                    blob.len(),
                    malformed_sizes.as_ptr(),
                    malformed_sizes.len(),
                    1024,
                    128 << 20,
                    &mut ignored,
                )
            },
            CIX_STATUS_INVALID_ARGUMENT
        );
        assert!(ignored.is_null());
        unsafe { cix_zstd_dictionary_free(handle) };
    }
}
