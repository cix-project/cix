//! Versioned, process-free C buffer API for the native CIX container.
//!
//! This module deliberately exposes whole-buffer operations only.  It does not
//! present a whole-buffer implementation as an incremental API; streaming is a
//! separate ABI revision.  Contexts own their options and error text, and no
//! CIX allocation is ever returned to the caller.

use crate::core::{decode_buffer, encode_buffer, NativeError, NativeOptions, NativeProfile};
use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;
use std::sync::Mutex;

pub const CIX_ABI_VERSION_1: u32 = 1;
pub const CIX_ERROR_MESSAGE_CAPACITY: usize = 512;

pub const CIX_STATUS_OK: i32 = 0;
pub const CIX_STATUS_INVALID_ARGUMENT: i32 = 1;
pub const CIX_STATUS_INVALID_OPTIONS: i32 = 2;
pub const CIX_STATUS_OUTPUT_TOO_SMALL: i32 = 3;
pub const CIX_STATUS_CODEC_ERROR: i32 = 4;
pub const CIX_STATUS_PANIC: i32 = 5;
pub const CIX_STATUS_RESOURCE_LIMIT: i32 = 6;

pub const CIX_PROFILE_FAST: u32 = 1;
pub const CIX_PROFILE_DEFAULT: u32 = 2;
pub const CIX_PROFILE_BEST: u32 = 3;

/// C layout of the first stable options revision.  `struct_size` permits later
/// append-only revisions; v1 ignores a caller's trailing bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct cix_options_v1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub profile: u32,
    pub workers: u32,
    pub output_limit: u64,
    pub memory_limit: u64,
}

#[repr(C)]
pub struct cix_context {
    options: NativeOptions,
    operation: Mutex<()>,
    last_error: Mutex<String>,
}

const OPTIONS_V1_SIZE: usize = std::mem::size_of::<cix_options_v1>();

fn bounded_message(message: impl AsRef<str>) -> String {
    let value = message.as_ref();
    let mut end = value.len().min(CIX_ERROR_MESSAGE_CAPACITY - 1);
    while end != 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn set_error(context: &cix_context, message: impl AsRef<str>) {
    if let Ok(mut error) = context.last_error.lock() {
        *error = bounded_message(message);
    }
}

fn clear_error(context: &cix_context) {
    if let Ok(mut error) = context.last_error.lock() {
        error.clear();
    }
}

fn native_options(raw: &cix_options_v1) -> Result<NativeOptions, &'static str> {
    if raw.abi_version != CIX_ABI_VERSION_1 {
        return Err("unsupported CIX ABI version");
    }
    if (raw.struct_size as usize) < OPTIONS_V1_SIZE {
        return Err("cix_options_v1 is smaller than this ABI revision");
    }
    let profile = match raw.profile {
        CIX_PROFILE_FAST => NativeProfile::Fast,
        CIX_PROFILE_DEFAULT => NativeProfile::Default,
        CIX_PROFILE_BEST => NativeProfile::Best,
        _ => return Err("invalid CIX profile"),
    };
    if raw.workers == 0 {
        return Err("worker count must be non-zero");
    }
    let output_limit =
        usize::try_from(raw.output_limit).map_err(|_| "output limit is too large")?;
    let memory_limit =
        usize::try_from(raw.memory_limit).map_err(|_| "memory limit is too large")?;
    if memory_limit == 0 {
        return Err("memory limit must be non-zero");
    }
    Ok(NativeOptions {
        profile,
        output_limit,
        memory_limit,
        workers: raw.workers as usize,
        deadline: None,
        cancellation: None,
    })
}

pub(crate) unsafe fn native_options_from_ptr(
    raw: *const cix_options_v1,
) -> Result<NativeOptions, &'static str> {
    if raw.is_null() {
        return Err("options is null");
    }
    // Read the stable eight-byte prefix before assuming the caller allocated a
    // complete v1 object.  This makes an old/small struct an ordinary ABI error
    // rather than an out-of-bounds Rust struct read.
    let version = unsafe { ptr::read_unaligned(ptr::addr_of!((*raw).abi_version)) };
    let size = unsafe { ptr::read_unaligned(ptr::addr_of!((*raw).struct_size)) };
    if version != CIX_ABI_VERSION_1 {
        return Err("unsupported CIX ABI version");
    }
    if (size as usize) < OPTIONS_V1_SIZE {
        return Err("cix_options_v1 is smaller than this ABI revision");
    }
    // SAFETY: the advertised size now guarantees a complete v1 prefix.
    let copied = unsafe { ptr::read_unaligned(raw) };
    native_options(&copied)
}

fn error_status(error: NativeError) -> (i32, String) {
    match error {
        NativeError::InvalidOptions(message) => (CIX_STATUS_INVALID_OPTIONS, message.into()),
        NativeError::OutputLimit => (
            CIX_STATUS_RESOURCE_LIMIT,
            "CIX output exceeds context output limit".into(),
        ),
        NativeError::Codec(message) => (CIX_STATUS_CODEC_ERROR, message),
    }
}

unsafe fn input<'a>(input: *const u8, input_len: usize) -> Result<&'a [u8], &'static str> {
    if input_len == 0 {
        return Ok(&[]);
    }
    if input.is_null() {
        return Err("input is null with non-zero length");
    }
    if input_len > isize::MAX as usize || (input as usize).checked_add(input_len).is_none() {
        return Err("input range is invalid");
    }
    // SAFETY: the C caller contract requires `input_len` readable bytes.
    Ok(unsafe { slice::from_raw_parts(input, input_len) })
}

unsafe fn output<'a>(
    output: *mut u8,
    output_capacity: usize,
) -> Result<&'a mut [u8], &'static str> {
    if output_capacity == 0 {
        return Ok(&mut []);
    }
    if output.is_null() {
        return Err("output is null with non-zero capacity");
    }
    if output_capacity > isize::MAX as usize
        || (output as usize).checked_add(output_capacity).is_none()
    {
        return Err("output range is invalid");
    }
    // SAFETY: the C caller contract requires `output_capacity` writable bytes.
    Ok(unsafe { slice::from_raw_parts_mut(output, output_capacity) })
}

fn overlaps(left: *const u8, left_len: usize, right: *const u8, right_len: usize) -> bool {
    if left_len == 0 || right_len == 0 {
        return false;
    }
    let left_start = left as usize;
    let right_start = right as usize;
    match (
        left_start.checked_add(left_len),
        right_start.checked_add(right_len),
    ) {
        (Some(left_end), Some(right_end)) => left_start < right_end && right_start < left_end,
        // A wrapping caller range is invalid, so conservatively reject it.
        _ => true,
    }
}

fn run_buffer(
    context: &cix_context,
    input_bytes: &[u8],
    output_bytes: &mut [u8],
    needed: &mut usize,
    encode: bool,
) -> i32 {
    let _serialized = match context.operation.lock() {
        Ok(lock) => lock,
        Err(_) => {
            set_error(context, "CIX context operation lock is poisoned");
            return CIX_STATUS_PANIC;
        }
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        if encode {
            encode_buffer(input_bytes, &context.options)
        } else {
            decode_buffer(input_bytes, &context.options)
        }
    }));
    match result {
        Ok(Ok(result)) => {
            *needed = result.len();
            if result.len() > output_bytes.len() {
                set_error(context, "caller output buffer is too small");
                return CIX_STATUS_OUTPUT_TOO_SMALL;
            }
            output_bytes[..result.len()].copy_from_slice(&result);
            clear_error(context);
            CIX_STATUS_OK
        }
        Ok(Err(error)) => {
            let (status, message) = error_status(error);
            set_error(context, message);
            status
        }
        Err(_) => {
            set_error(context, "panic contained at CIX C ABI boundary");
            CIX_STATUS_PANIC
        }
    }
}

/// Writes the v1 defaults.  The caller owns this plain value.
///
/// # Safety
///
/// `out` must be a non-null, properly aligned, writable `cix_options_v1` for
/// the duration of this call. It must not alias memory concurrently accessed
/// by another thread.
#[no_mangle]
pub unsafe extern "C" fn cix_options_v1_default(out: *mut cix_options_v1) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if out.is_null() {
            return CIX_STATUS_INVALID_ARGUMENT;
        }
        let defaults = NativeOptions::default();
        let options = cix_options_v1 {
            abi_version: CIX_ABI_VERSION_1,
            struct_size: OPTIONS_V1_SIZE as u32,
            profile: CIX_PROFILE_DEFAULT,
            workers: u32::try_from(defaults.workers).unwrap_or(1),
            output_limit: defaults.output_limit as u64,
            memory_limit: defaults.memory_limit as u64,
        };
        // SAFETY: null was rejected and the caller provides writable storage.
        unsafe { ptr::write(out, options) };
        CIX_STATUS_OK
    }))
    .unwrap_or(CIX_STATUS_PANIC)
}

/// Creates an independently owned, reusable native buffer context.
///
/// # Safety
///
/// `options` must point to a readable, initialized v1 options structure and
/// `out_context` to a writable, properly aligned pointer slot. The two regions
/// must not overlap and must remain valid for the call. A successful call
/// transfers ownership of the returned handle to the caller, which must later
/// destroy it exactly once.
#[no_mangle]
pub unsafe extern "C" fn cix_context_create(
    options: *const cix_options_v1,
    out_context: *mut *mut cix_context,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if out_context.is_null() {
            return CIX_STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: null was rejected and the caller provides writable storage.
        unsafe { ptr::write(out_context, ptr::null_mut()) };
        if options.is_null() {
            return CIX_STATUS_INVALID_ARGUMENT;
        }
        let native = match unsafe { native_options_from_ptr(options) } {
            Ok(value) => value,
            Err(_) => return CIX_STATUS_INVALID_OPTIONS,
        };
        let context = Box::new(cix_context {
            options: native,
            operation: Mutex::new(()),
            last_error: Mutex::new(String::new()),
        });
        // SAFETY: ownership transfers to the caller.
        unsafe { ptr::write(out_context, Box::into_raw(context)) };
        CIX_STATUS_OK
    }))
    .unwrap_or(CIX_STATUS_PANIC)
}

/// Releases a context created by [`cix_context_create`].  Passing null is a no-op.
///
/// # Safety
///
/// A non-null `context` must be the live handle returned by
/// [`cix_context_create`], and this must be its only destruction. No other
/// thread may use or destroy the handle while this call runs.
#[no_mangle]
pub unsafe extern "C" fn cix_context_destroy(context: *mut cix_context) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !context.is_null() {
            // SAFETY: ownership is returned exactly once by the caller contract.
            unsafe { drop(Box::from_raw(context)) };
        }
    }));
}

unsafe fn buffer_call(
    context: *mut cix_context,
    input_ptr: *const u8,
    input_len: usize,
    output_ptr: *mut u8,
    output_capacity: usize,
    needed: *mut usize,
    encode: bool,
) -> i32 {
    if context.is_null() || needed.is_null() {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    if overlaps(
        input_ptr,
        input_len,
        output_ptr.cast_const(),
        output_capacity,
    ) || overlaps(
        input_ptr,
        input_len,
        needed.cast(),
        std::mem::size_of::<usize>(),
    ) || overlaps(
        output_ptr.cast_const(),
        output_capacity,
        needed.cast(),
        std::mem::size_of::<usize>(),
    ) {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: null was rejected and the caller provides writable storage.
    unsafe { ptr::write(needed, 0) };
    // SAFETY: null was rejected; lifetime remains caller-owned for this call.
    let context = unsafe { &*context };
    let input_bytes = match unsafe { input(input_ptr, input_len) } {
        Ok(value) => value,
        Err(message) => {
            set_error(context, message);
            return CIX_STATUS_INVALID_ARGUMENT;
        }
    };
    let output_bytes = match unsafe { output(output_ptr, output_capacity) } {
        Ok(value) => value,
        Err(message) => {
            set_error(context, message);
            return CIX_STATUS_INVALID_ARGUMENT;
        }
    };
    let mut required = 0;
    let status = run_buffer(context, input_bytes, output_bytes, &mut required, encode);
    // SAFETY: null was rejected and the caller provides writable storage.
    unsafe { ptr::write(needed, required) };
    status
}

/// Compresses one complete buffer.  On `CIX_STATUS_OUTPUT_TOO_SMALL`, `needed`
/// receives the complete required byte count and output is not modified.
///
/// # Safety
///
/// `context` must be a live context not concurrently being destroyed.
/// `input_ptr` must designate `input_len` readable bytes and `output_ptr`
/// `output_capacity` writable bytes (null is permitted only for an empty
/// region). `needed` must be a writable `usize`. These regions must be
/// non-overlapping and valid for the complete call.
#[no_mangle]
pub unsafe extern "C" fn cix_encode_buffer(
    context: *mut cix_context,
    input_ptr: *const u8,
    input_len: usize,
    output_ptr: *mut u8,
    output_capacity: usize,
    needed: *mut usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| unsafe {
        buffer_call(
            context,
            input_ptr,
            input_len,
            output_ptr,
            output_capacity,
            needed,
            true,
        )
    }))
    .unwrap_or_else(|_| {
        if !context.is_null() {
            // SAFETY: caller ownership keeps the context alive for this call.
            unsafe { set_error(&*context, "panic contained at CIX C ABI boundary") };
        }
        CIX_STATUS_PANIC
    })
}

/// Decompresses one complete buffer.  On `CIX_STATUS_OUTPUT_TOO_SMALL`, `needed`
/// receives the complete required byte count and output is not modified.
///
/// # Safety
///
/// `context` must be a live context not concurrently being destroyed.
/// `input_ptr` must designate `input_len` readable bytes and `output_ptr`
/// `output_capacity` writable bytes (null is permitted only for an empty
/// region). `needed` must be a writable `usize`. These regions must be
/// non-overlapping and valid for the complete call.
#[no_mangle]
pub unsafe extern "C" fn cix_decode_buffer(
    context: *mut cix_context,
    input_ptr: *const u8,
    input_len: usize,
    output_ptr: *mut u8,
    output_capacity: usize,
    needed: *mut usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| unsafe {
        buffer_call(
            context,
            input_ptr,
            input_len,
            output_ptr,
            output_capacity,
            needed,
            false,
        )
    }))
    .unwrap_or_else(|_| {
        if !context.is_null() {
            // SAFETY: caller ownership keeps the context alive for this call.
            unsafe { set_error(&*context, "panic contained at CIX C ABI boundary") };
        }
        CIX_STATUS_PANIC
    })
}

/// Copies the context-owned, bounded diagnostic text.  `needed` includes the
/// terminating NUL.  This function does not replace the stored diagnostic.
///
/// # Safety
///
/// `context` must be a live context not concurrently being destroyed.
/// `output` must designate `output_capacity` writable bytes when non-empty,
/// and `needed` must be a writable `usize`. The output and count regions must
/// not overlap and all supplied storage must remain valid for the call.
#[no_mangle]
pub unsafe extern "C" fn cix_context_last_error(
    context: *const cix_context,
    output: *mut c_char,
    output_capacity: usize,
    needed: *mut usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| unsafe {
        if context.is_null()
            || needed.is_null()
            || (output.is_null() && output_capacity != 0)
            || output_capacity > isize::MAX as usize
            || (output_capacity != 0 && (output as usize).checked_add(output_capacity).is_none())
            || overlaps(
                output.cast(),
                output_capacity,
                needed.cast(),
                std::mem::size_of::<usize>(),
            )
        {
            return CIX_STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: null was rejected and the caller keeps the context alive.
        let context = &*context;
        let error = match context.last_error.lock() {
            Ok(value) => value,
            Err(_) => return CIX_STATUS_PANIC,
        };
        let bytes = error.as_bytes();
        let required = bytes.len() + 1;
        // SAFETY: null was rejected and the caller provides writable storage.
        ptr::write(needed, required);
        if output_capacity < required {
            return CIX_STATUS_OUTPUT_TOO_SMALL;
        }
        // SAFETY: capacity was checked for `required` bytes.
        ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast::<u8>(), bytes.len());
        *output.cast::<u8>().add(bytes.len()) = 0;
        CIX_STATUS_OK
    }))
    .unwrap_or(CIX_STATUS_PANIC)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> *mut cix_context {
        let mut options = std::mem::MaybeUninit::<cix_options_v1>::uninit();
        assert_eq!(
            unsafe { cix_options_v1_default(options.as_mut_ptr()) },
            CIX_STATUS_OK
        );
        let options = unsafe { options.assume_init() };
        let mut context = ptr::null_mut();
        assert_eq!(
            unsafe { cix_context_create(&options, &mut context) },
            CIX_STATUS_OK
        );
        context
    }

    #[test]
    fn truncating_utf8_diagnostic_stays_valid() {
        let message = bounded_message("é".repeat(300));
        assert!(message.len() < CIX_ERROR_MESSAGE_CAPACITY);
        assert!(std::str::from_utf8(message.as_bytes()).is_ok());
    }

    #[test]
    fn bounded_diagnostic_allocation_and_context_retention() {
        let context = OwnedContext(context());
        for (input, expected) in [
            ("a".repeat(65_536), "a".repeat(511)),
            ("é".repeat(32_768), "é".repeat(255)),
            (String::new(), String::new()),
            ("short é\0text".to_owned(), "short é\0text".to_owned()),
        ] {
            let message = bounded_message(&input);
            assert_eq!(message.as_bytes(), expected.as_bytes());
            assert!(message.capacity() < CIX_ERROR_MESSAGE_CAPACITY);

            let native_context = unsafe { &*context.0 };
            set_error(native_context, &input);
            {
                let retained = native_context.last_error.lock().unwrap();
                assert_eq!(retained.as_bytes(), expected.as_bytes());
                assert!(retained.capacity() < CIX_ERROR_MESSAGE_CAPACITY);
            }
            assert_eq!(diagnostic(&context), expected.as_bytes());
        }
    }

    #[test]
    fn short_options_prefix_is_an_error_without_a_full_struct_read() {
        #[repr(C)]
        struct Prefix {
            abi_version: u32,
            struct_size: u32,
        }
        let prefix = Prefix {
            abi_version: CIX_ABI_VERSION_1,
            struct_size: 8,
        };
        let mut context = ptr::null_mut();
        assert_eq!(
            unsafe { cix_context_create((&prefix as *const Prefix).cast(), &mut context) },
            CIX_STATUS_INVALID_OPTIONS
        );
        assert!(context.is_null());
    }

    #[test]
    fn impossible_length_and_overlapping_buffers_are_rejected() {
        let context = context();
        let mut needed = 99;
        assert_eq!(
            unsafe {
                cix_encode_buffer(
                    context,
                    ptr::null(),
                    usize::MAX,
                    ptr::null_mut(),
                    0,
                    &mut needed,
                )
            },
            CIX_STATUS_INVALID_ARGUMENT
        );
        let mut bytes = [0_u8; 32];
        assert_eq!(
            unsafe {
                cix_encode_buffer(
                    context,
                    bytes.as_ptr(),
                    bytes.len(),
                    bytes.as_mut_ptr(),
                    bytes.len(),
                    &mut needed,
                )
            },
            CIX_STATUS_INVALID_ARGUMENT
        );
        unsafe { cix_context_destroy(context) };
    }


    // Test consumers own handles through the public API, including cleanup on
    // assertion unwinding. No private context fields or codec entry points are used.
    struct OwnedContext(*mut cix_context);

    impl OwnedContext {
        fn new() -> Self {
            let mut options = std::mem::MaybeUninit::<cix_options_v1>::uninit();
            assert_eq!(
                unsafe { cix_options_v1_default(options.as_mut_ptr()) },
                CIX_STATUS_OK
            );
            let mut options = unsafe { options.assume_init() };
            options.profile = CIX_PROFILE_FAST;
            options.workers = 1;
            let mut handle = ptr::null_mut();
            assert_eq!(
                unsafe { cix_context_create(&options, &mut handle) },
                CIX_STATUS_OK
            );
            assert!(!handle.is_null());
            Self(handle)
        }
    }

    impl Drop for OwnedContext {
        fn drop(&mut self) {
            // Each handle stays on its creating thread and is destroyed once.
            unsafe { cix_context_destroy(self.0) };
        }
    }

    fn diagnostic(context: &OwnedContext) -> Vec<u8> {
        let mut needed = usize::MAX;
        assert_eq!(
            unsafe { cix_context_last_error(context.0, ptr::null_mut(), 0, &mut needed) },
            CIX_STATUS_OUTPUT_TOO_SMALL
        );
        assert!((1..=CIX_ERROR_MESSAGE_CAPACITY).contains(&needed));
        let required = needed;
        let mut short = vec![0x5a_u8; required - 1];
        assert_eq!(
            unsafe {
                cix_context_last_error(
                    context.0, short.as_mut_ptr().cast(), short.len(), &mut needed,
                )
            },
            CIX_STATUS_OUTPUT_TOO_SMALL
        );
        assert_eq!(needed, required);
        assert!(short.iter().all(|&byte| byte == 0x5a));
        let mut bytes = vec![0x5a_u8; required + 1];
        assert_eq!(
            unsafe {
                cix_context_last_error(
                    context.0, bytes.as_mut_ptr().cast(), required, &mut needed,
                )
            },
            CIX_STATUS_OK
        );
        assert_eq!(needed, required);
        assert_eq!(bytes[required - 1], 0);
        assert_eq!(bytes[required], 0x5a);
        bytes.truncate(required - 1);
        assert!(std::str::from_utf8(&bytes).is_ok());
        bytes
    }

    fn public_roundtrip(context: &OwnedContext, input: &[u8]) -> Vec<u8> {
        let mut needed = usize::MAX;
        assert_eq!(
            unsafe {
                cix_encode_buffer(
                    context.0, input.as_ptr(), input.len(), ptr::null_mut(), 0, &mut needed,
                )
            },
            CIX_STATUS_OUTPUT_TOO_SMALL
        );
        let archive_len = needed;
        assert!(archive_len > 0);
        let mut short = vec![0xa5; archive_len - 1];
        assert_eq!(
            unsafe {
                cix_encode_buffer(
                    context.0, input.as_ptr(), input.len(), short.as_mut_ptr(),
                    short.len(), &mut needed,
                )
            },
            CIX_STATUS_OUTPUT_TOO_SMALL
        );
        assert_eq!(needed, archive_len);
        assert!(short.iter().all(|&byte| byte == 0xa5));
        let mut archive = vec![0xa5; archive_len + 1];
        assert_eq!(
            unsafe {
                cix_encode_buffer(
                    context.0, input.as_ptr(), input.len(), archive.as_mut_ptr(),
                    archive_len, &mut needed,
                )
            },
            CIX_STATUS_OK
        );
        assert_eq!(needed, archive_len);
        assert_eq!(archive[archive_len], 0xa5);
        archive.truncate(archive_len);
        assert!(diagnostic(context).is_empty());

        let query_status = unsafe {
            cix_decode_buffer(
                context.0, archive.as_ptr(), archive.len(), ptr::null_mut(), 0, &mut needed,
            )
        };
        assert_eq!(needed, input.len());
        assert_eq!(query_status, if input.is_empty() {
            CIX_STATUS_OK
        } else {
            CIX_STATUS_OUTPUT_TOO_SMALL
        });
        if !input.is_empty() {
            let mut short = vec![0x5a; input.len() - 1];
            assert_eq!(
                unsafe {
                    cix_decode_buffer(
                        context.0, archive.as_ptr(), archive.len(), short.as_mut_ptr(),
                        short.len(), &mut needed,
                    )
                },
                CIX_STATUS_OUTPUT_TOO_SMALL
            );
            assert_eq!(needed, input.len());
            assert!(short.iter().all(|&byte| byte == 0x5a));
        }
        let mut restored = vec![0x5a; input.len() + 1];
        assert_eq!(
            unsafe {
                cix_decode_buffer(
                    context.0, archive.as_ptr(), archive.len(), restored.as_mut_ptr(),
                    input.len(), &mut needed,
                )
            },
            CIX_STATUS_OK
        );
        assert_eq!(needed, input.len());
        assert_eq!(&restored[..needed], input);
        assert_eq!(restored[needed], 0x5a);
        assert!(diagnostic(context).is_empty());
        archive
    }

    #[test]
    fn public_c_abi_roundtrip_reports_needed_and_preserves_short_buffers() {
        let context = OwnedContext::new();
        public_roundtrip(&context, &[]);
        public_roundtrip(&context, b"CIX public ABI\0roundtrip\xff\n");
        let input: Vec<u8> = (0..1024).map(|index| (index % 256) as u8).collect();
        public_roundtrip(&context, &input);
    }

    #[test]
    fn public_c_abi_invalid_input_and_codec_errors_reset_needed() {
        let context = OwnedContext::new();
        let mut output = [0xa5; 64];
        let mut needed = usize::MAX;
        assert_eq!(
            unsafe {
                cix_encode_buffer(
                    context.0, ptr::null(), 1, output.as_mut_ptr(), output.len(), &mut needed,
                )
            },
            CIX_STATUS_INVALID_ARGUMENT
        );
        assert_eq!(needed, 0);
        assert_eq!(diagnostic(&context), b"input is null with non-zero length");
        needed = usize::MAX;
        let invalid_archive = b"not a CIX archive";
        assert_eq!(
            unsafe {
                cix_decode_buffer(
                    context.0, invalid_archive.as_ptr(), invalid_archive.len(),
                    output.as_mut_ptr(), output.len(), &mut needed,
                )
            },
            CIX_STATUS_CODEC_ERROR
        );
        assert_eq!(needed, 0);
        assert_eq!(output, [0xa5; 64]);
        assert!(!diagnostic(&context).is_empty());
        public_roundtrip(&context, b"reusable after errors");
    }

    #[test]
    fn public_c_abi_concurrent_owned_contexts_isolate_buffers_and_diagnostics() {
        let (left_tx, left_rx) = std::sync::mpsc::channel();
        let (right_tx, right_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let handles: Vec<_> = [(left_tx, right_rx), (right_tx, left_rx)]
                .into_iter()
                .enumerate()
                .map(|(worker, (tx, rx))| {
                scope.spawn(move || {
                    // Channel rendezvous coordinates real calls, with bounded
                    // failure rather than a barrier hanging if a peer panics.
                    let rendezvous = || {
                        tx.send(()).expect("public ABI peer disconnected");
                        rx.recv_timeout(std::time::Duration::from_secs(60))
                            .expect("public ABI peer failed to reach rendezvous");
                    };
                    let context = OwnedContext::new();
                    let input = vec![if worker == 0 { 0x17 } else { 0xe9 }; 1024];
                    // Both contexts exist before real codec operations begin.
                    rendezvous();
                    let archive = public_roundtrip(&context, &input);
                    let mut needed = usize::MAX;
                    let status = if worker == 0 {
                        unsafe {
                            cix_encode_buffer(
                                context.0, ptr::null(), 1, ptr::null_mut(), 0, &mut needed,
                            )
                        }
                    } else {
                        unsafe {
                            cix_decode_buffer(
                                context.0, archive.as_ptr(), archive.len(),
                                ptr::null_mut(), 0, &mut needed,
                            )
                        }
                    };
                    assert_eq!(status, if worker == 0 {
                        CIX_STATUS_INVALID_ARGUMENT
                    } else {
                        CIX_STATUS_OUTPUT_TOO_SMALL
                    });
                    assert_eq!(needed, if worker == 0 { 0 } else { input.len() });
                    let expected = if worker == 0 {
                        b"input is null with non-zero length".as_slice()
                    } else {
                        b"caller output buffer is too small".as_slice()
                    };
                    rendezvous();
                    assert_eq!(diagnostic(&context), expected);
                    rendezvous();
                    if worker == 0 {
                        public_roundtrip(&context, &input);
                    }
                    // A success clearing one diagnostic must leave the other intact.
                    rendezvous();
                    if worker == 1 {
                        assert_eq!(diagnostic(&context), expected);
                        public_roundtrip(&context, &input);
                    }
                    (input, archive)
                })
            }).collect();
            let results: Vec<_> = handles.into_iter()
                .map(|handle| handle.join().expect("public ABI consumer thread panicked"))
                .collect();
            assert_ne!(results[0].0, results[1].0);
            assert_ne!(results[0].1, results[1].1);
        });
    }
}
