//! Per-call borrowed presentation for one semantically retained C buffer error.
//! The caller resolves locale; no catalog, pointer, locale or callback is kept.

use crate::c_api::{
    cix_context, cix_context_last_error, Diagnostic, CIX_ERROR_MESSAGE_CAPACITY,
    CIX_STATUS_INVALID_ARGUMENT, CIX_STATUS_OK, CIX_STATUS_OUTPUT_TOO_SMALL, CIX_STATUS_PANIC,
};
use crate::i18n::{Catalog, Direction, Localizer, Number, PluralCategory};
use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::{mem, ptr, slice};

pub const CIX_PRESENTATION_ABI_VERSION_1: u32 = 1;
const MESSAGE_ID: &str = "sdk.buffer.output_too_small";
const MAX_LEAF_BYTES: usize = CIX_ERROR_MESSAGE_CAPACITY - 1;

/// Append-only C record. Only this v1 prefix is read; trailing bytes are ignored.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct cix_buffer_presentation_v1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub output_too_small_pattern: *const c_char,
    pub output_too_small_pattern_len: usize,
}

type Region = (usize, usize);

fn region(pointer: *const u8, length: usize) -> Option<Region> {
    let start = pointer as usize;
    if length == 0 {
        return Some((start, start));
    }
    if pointer.is_null() || length > isize::MAX as usize {
        return None;
    }
    Some((start, start.checked_add(length)?))
}

fn overlaps(left: Region, right: Region) -> bool {
    left.0 != left.1 && right.0 != right.1 && left.0 < right.1 && right.0 < left.1
}

struct BorrowedLeaf<'a>(&'a str);

impl Catalog for BorrowedLeaf<'_> {
    fn locale(&self) -> &str { "und" }
    fn direction(&self) -> Direction { Direction::Ltr }
    fn pattern(&self, id: &str, _: PluralCategory) -> Option<&str> {
        (id == MESSAGE_ID).then_some(self.0)
    }
    fn plural_category(&self, _: &Number) -> PluralCategory { PluralCategory::Other }
    fn format_number(&self, number: &Number) -> String { number.to_string() }
}

/// The selected leaf declares zero arguments. Reject single braces before
/// rendering so malformed/undeclared syntax preserves the retained English.
/// Localizer still performs normal definition validation and brace processing.
fn only_escaped_braces(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if matches!(bytes[index], b'{' | b'}') {
            if bytes.get(index + 1) != Some(&bytes[index]) {
                return false;
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    true
}

fn render_diagnostic(snapshot: Diagnostic, pattern: Option<&str>) -> String {
    let Diagnostic { english, message } = snapshot;
    let Some(message) = message.filter(|value| value.id == MESSAGE_ID && value.args.is_empty()) else {
        return english;
    };
    let Some(pattern) = pattern.filter(|value|
        !value.is_empty() && value.len() <= MAX_LEAF_BYTES
            && !value.as_bytes().contains(&0) && only_escaped_braces(value)
    ) else {
        return english;
    };
    Localizer::new(&BorrowedLeaf(pattern))
        .render(&message)
        .ok()
        .filter(|value| !value.is_empty() && value.len() <= MAX_LEAF_BYTES
            && !value.as_bytes().contains(&0))
        .unwrap_or(english)
}

unsafe fn presented_call(
    context: *const cix_context,
    presentation: *const cix_buffer_presentation_v1,
    output: *mut c_char,
    output_capacity: usize,
    needed: *mut usize,
) -> i32 {
    // NULL is the exact existing English entry point, including its validation.
    if presentation.is_null() {
        return unsafe { cix_context_last_error(context, output, output_capacity, needed) };
    }
    if context.is_null() || needed.is_null()
        || (context as usize) % mem::align_of::<cix_context>() != 0
        || (needed as usize) % mem::align_of::<usize>() != 0
    {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    let (Some(context_region), Some(output_region), Some(count_region), Some(prefix_region)) = (
        region(context.cast(), mem::size_of::<cix_context>()),
        region(output.cast(), output_capacity),
        region(needed.cast(), mem::size_of::<usize>()),
        region(presentation.cast(), 2 * mem::size_of::<u32>()),
    ) else {
        return CIX_STATUS_INVALID_ARGUMENT;
    };
    if overlaps(output_region, count_region)
        || overlaps(context_region, output_region)
        || overlaps(context_region, count_region)
        || overlaps(prefix_region, output_region)
        || overlaps(prefix_region, count_region)
        || overlaps(prefix_region, context_region)
    {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: the caller supplies an immutable readable eight-byte prefix.
    // Range checks precede reads; a small record is never read as a full v1.
    let version = unsafe { ptr::read_unaligned(ptr::addr_of!((*presentation).abi_version)) };
    let size = unsafe { ptr::read_unaligned(ptr::addr_of!((*presentation).struct_size)) };
    if version != CIX_PRESENTATION_ABI_VERSION_1
        || (size as usize) < mem::size_of::<cix_buffer_presentation_v1>()
    {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    let Some(record_region) = region(presentation.cast(), mem::size_of::<cix_buffer_presentation_v1>()) else {
        return CIX_STATUS_INVALID_ARGUMENT;
    };
    if overlaps(record_region, output_region) || overlaps(record_region, count_region)
        || overlaps(record_region, context_region)
    {
        return CIX_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: the caller advertises a complete immutable v1 prefix.
    let record = unsafe { ptr::read_unaligned(presentation) };
    let Some(pattern_region) = region(record.output_too_small_pattern.cast(), record.output_too_small_pattern_len) else {
        return CIX_STATUS_INVALID_ARGUMENT;
    };
    if overlaps(pattern_region, output_region) || overlaps(pattern_region, count_region)
        || overlaps(pattern_region, context_region)
    {
        return CIX_STATUS_INVALID_ARGUMENT;
    }

    // SAFETY: the caller keeps the live context alive through this call.
    // Clone both owned fields under one mutex, then unlock before presentation.
    let snapshot = match unsafe { &*context }.diagnostic_snapshot() {
        Ok(value) => value,
        Err(status) => return status,
    };
    // Oversized entries fall back without reading their bytes. Ineligible
    // diagnostics likewise need no candidate read or locale work.
    let eligible = snapshot.message.as_ref()
        .is_some_and(|value| value.id == MESSAGE_ID && value.args.is_empty());
    let pattern = if eligible && (1..=MAX_LEAF_BYTES).contains(&record.output_too_small_pattern_len) {
        // SAFETY: the caller supplies immutable readable candidate bytes;
        // invalid UTF-8 is an ordinary whole-English fallback.
        let bytes = unsafe { slice::from_raw_parts(
            record.output_too_small_pattern.cast::<u8>(), record.output_too_small_pattern_len,
        ) };
        std::str::from_utf8(bytes).ok()
    } else {
        None
    };
    let text = render_diagnostic(snapshot, pattern);
    let bytes = text.as_bytes();
    let Some(required) = bytes.len().checked_add(1) else {
        return CIX_STATUS_PANIC;
    };
    // All argument checks and the owned snapshot completed before either write.
    unsafe { ptr::write(needed, required) };
    if output_capacity < required {
        return CIX_STATUS_OUTPUT_TOO_SMALL;
    }
    // SAFETY: validated writable output has room for bytes plus one NUL.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast::<u8>(), bytes.len());
        *output.cast::<u8>().add(bytes.len()) = 0;
    }
    CIX_STATUS_OK
}

/// Copies a per-call presentation of the then-current diagnostic snapshot.
/// The retained English and semantic identity are never changed by this getter.
///
/// # Safety
///
/// The context is a live CIX handle, not being destroyed. The count pointer is
/// a properly aligned writable usize; output is writable for its capacity
/// (NULL only at zero capacity). A non-NULL presentation has a readable
/// eight-byte prefix and its advertised complete v1 prefix exists; pattern
/// bytes are readable and immutable for the entire call. Output/count do not
/// alias one another, the input record, candidate bytes or context; input
/// storage does not alias the context. No pointer or allocation is retained.
/// Separate calls snapshot independently; query/copy need not observe the
/// same diagnostic.
#[no_mangle]
pub unsafe extern "C" fn cix_context_last_error_presented_v1(
    context: *const cix_context,
    presentation: *const cix_buffer_presentation_v1,
    output: *mut c_char,
    output_capacity: usize,
    needed: *mut usize,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| unsafe {
        presented_call(context, presentation, output, output_capacity, needed)
    }))
    .unwrap_or(CIX_STATUS_PANIC)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::c_api::{
        cix_context_create, cix_context_destroy, cix_encode_buffer,
        cix_options_v1, cix_options_v1_default, CIX_PROFILE_FAST,
    };
    use crate::i18n::{Message, Value};

    struct Context(*mut cix_context);
    impl Context {
        fn new() -> Self {
            let mut options = mem::MaybeUninit::<cix_options_v1>::uninit();
            assert_eq!(unsafe { cix_options_v1_default(options.as_mut_ptr()) }, CIX_STATUS_OK);
            let mut options = unsafe { options.assume_init() };
            options.profile = CIX_PROFILE_FAST;
            options.workers = 1;
            let mut context = ptr::null_mut();
            assert_eq!(unsafe { cix_context_create(&options, &mut context) }, CIX_STATUS_OK);
            assert!(!context.is_null());
            Self(context)
        }
        fn select_short_buffer(&self) -> usize {
            let input = b"borrowed presentation";
            let mut needed = usize::MAX;
            assert_eq!(unsafe {
                cix_encode_buffer(self.0, input.as_ptr(), input.len(), ptr::null_mut(), 0, &mut needed)
            }, CIX_STATUS_OUTPUT_TOO_SMALL);
            assert!(needed > 0);
            needed
        }
        fn clear_with_success(&self, archive_len: usize) {
            let input = b"borrowed presentation";
            let mut output = vec![0xa5; archive_len];
            let mut needed = usize::MAX;
            assert_eq!(unsafe {
                cix_encode_buffer(self.0, input.as_ptr(), input.len(),
                    output.as_mut_ptr(), output.len(), &mut needed)
            }, CIX_STATUS_OK);
            assert_eq!(needed, archive_len);
        }
    }
    impl Drop for Context {
        fn drop(&mut self) { unsafe { cix_context_destroy(self.0) }; }
    }
    fn record(pattern: &[u8]) -> cix_buffer_presentation_v1 {
        cix_buffer_presentation_v1 {
            abi_version: CIX_PRESENTATION_ABI_VERSION_1,
            struct_size: mem::size_of::<cix_buffer_presentation_v1>() as u32,
            output_too_small_pattern: pattern.as_ptr().cast(),
            output_too_small_pattern_len: pattern.len(),
        }
    }
    fn copy(context: &Context, presentation: *const cix_buffer_presentation_v1) -> Vec<u8> {
        let mut needed = usize::MAX;
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, presentation, ptr::null_mut(), 0, &mut needed)
        }, CIX_STATUS_OUTPUT_TOO_SMALL);
        let required = needed;
        let mut short = vec![0xa5_u8; required - 1];
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, presentation,
                short.as_mut_ptr().cast(), short.len(), &mut needed)
        }, CIX_STATUS_OUTPUT_TOO_SMALL);
        assert_eq!(needed, required);
        assert!(short.iter().all(|&value| value == 0xa5));
        let mut bytes = vec![0xa5_u8; required + 1];
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, presentation,
                bytes.as_mut_ptr().cast(), required, &mut needed)
        }, CIX_STATUS_OK);
        assert_eq!(needed, required);
        assert_eq!(bytes[required - 1], 0);
        assert_eq!(bytes[required], 0xa5);
        bytes.truncate(required - 1);
        bytes
    }
    fn invalid_untouched(
        context: &Context, presentation: *const cix_buffer_presentation_v1,
    ) {
        let mut bytes = [0xa5_u8; 64];
        let mut needed = 99;
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, presentation,
                bytes.as_mut_ptr().cast(), bytes.len(), &mut needed)
        }, CIX_STATUS_INVALID_ARGUMENT);
        assert_eq!(needed, 99);
        assert_eq!(bytes, [0xa5; 64]);
    }

    #[test]
    fn selected_utf8_leaf_uses_brace_processing_and_keeps_english_getter_unchanged() {
        let context = Context::new();
        context.select_short_buffer();
        let presentation = record("mémoire {{courte}}".as_bytes());
        let translated = copy(&context, &presentation);
        assert_eq!(translated, "mémoire {courte}".as_bytes());
        assert_eq!(translated.len() + 1, 18);
        assert_eq!(copy(&context, ptr::null()), b"caller output buffer is too small");
        assert_eq!(copy(&context, &presentation), translated);
    }

    #[test]
    fn missing_invalid_empty_nul_or_oversized_candidates_fall_back_whole() {
        let context = Context::new();
        context.select_short_buffer();
        let long = vec![b'x'; 512];
        let patterns: [&[u8]; 8] = [
            b"", b"{", b"}", b"{needed}", b"bad\0pattern", &[0xff],
            &long, b"{{{}}",
        ];
        for pattern in patterns {
            assert_eq!(copy(&context, &record(pattern)), b"caller output buffer is too small");
        }
        let mut missing = record(b"");
        missing.output_too_small_pattern = ptr::null();
        assert_eq!(copy(&context, &missing), b"caller output buffer is too small");
        assert_eq!(copy(&context, &record(&vec![b'x'; 511])), vec![b'x'; 511]);
        assert_eq!(copy(&context, &record("🧭 {{名}}".as_bytes())), "🧭 {名}".as_bytes());
    }

    #[test]
    fn success_clears_origin_and_each_call_snapshots_the_current_diagnostic() {
        let context = Context::new();
        let archive_len = context.select_short_buffer();
        let presentation = record(b"translated");
        assert_eq!(copy(&context, &presentation), b"translated");
        context.clear_with_success(archive_len);
        assert_eq!(copy(&context, &presentation), b"");
        assert_eq!(copy(&context, ptr::null()), b"");
    }

    #[test]
    fn invalid_version_small_prefix_and_null_nonempty_pattern_leave_writes_untouched() {
        let context = Context::new();
        let mut presentation = record(b"translated");
        presentation.abi_version = 2;
        invalid_untouched(&context, &presentation);
        #[repr(C)]
        struct Prefix { abi_version: u32, struct_size: u32 }
        let prefix = Prefix { abi_version: 1, struct_size: 8 };
        invalid_untouched(&context, (&prefix as *const Prefix).cast());
        presentation.abi_version = 1;
        presentation.output_too_small_pattern = ptr::null();
        presentation.output_too_small_pattern_len = 1;
        invalid_untouched(&context, &presentation);
    }

    #[test]
    fn wrapping_ranges_null_handles_and_misaligned_counts_are_rejected_before_writes() {
        let context = Context::new();
        let mut presentation = record(b"translated");
        presentation.output_too_small_pattern_len = usize::MAX;
        invalid_untouched(&context, &presentation);
        invalid_untouched(&context, (usize::MAX - 3) as *const cix_buffer_presentation_v1);
        presentation = record(b"translated");
        let mut needed = 99;
        let mut output = [0xa5_u8; 64];
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(ptr::null(), &presentation,
                output.as_mut_ptr().cast(), output.len(), &mut needed)
        }, CIX_STATUS_INVALID_ARGUMENT);
        assert_eq!(needed, 99);
        assert_eq!(output, [0xa5; 64]);
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, &presentation,
                output.as_mut_ptr().cast(), usize::MAX, &mut needed)
        }, CIX_STATUS_INVALID_ARGUMENT);
        assert_eq!(needed, 99);
        assert_eq!(output, [0xa5; 64]);
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, &presentation,
                ptr::null_mut(), 1, &mut needed)
        }, CIX_STATUS_INVALID_ARGUMENT);
        assert_eq!(needed, 99);
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, &presentation,
                output.as_mut_ptr().cast(), output.len(), ptr::null_mut())
        }, CIX_STATUS_INVALID_ARGUMENT);
        // Aligned backing makes the +1 count address definitely unaligned.
        let mut backing = [usize::MAX; 2];
        let unaligned = unsafe { backing.as_mut_ptr().cast::<u8>().add(1).cast::<usize>() };
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, &presentation,
                output.as_mut_ptr().cast(), output.len(), unaligned)
        }, CIX_STATUS_INVALID_ARGUMENT);
        assert_eq!(backing, [usize::MAX; 2]);
        assert_eq!(output, [0xa5; 64]);
    }

    #[test]
    fn input_record_pattern_count_context_and_output_overlap_never_write() {
        let context = Context::new();
        let mut presentation = record(b"translated");
        let raw_record = &mut presentation as *mut cix_buffer_presentation_v1;
        let before_version = presentation.abi_version;
        let mut needed = 99;
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, raw_record,
                raw_record.cast(), 1, &mut needed)
        }, CIX_STATUS_INVALID_ARGUMENT);
        assert_eq!(needed, 99);
        assert_eq!(presentation.abi_version, before_version);
        let mut backing = [usize::MAX; 8];
        let bytes = backing.as_mut_ptr().cast::<u8>();
        let output_len = mem::size_of_val(&backing);
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, &presentation,
                bytes.cast(), output_len, backing.as_mut_ptr())
        }, CIX_STATUS_INVALID_ARGUMENT);
        assert_eq!(backing, [usize::MAX; 8]);
        presentation.output_too_small_pattern = bytes.cast();
        presentation.output_too_small_pattern_len = output_len;
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, &presentation,
                bytes.cast(), output_len, &mut needed)
        }, CIX_STATUS_INVALID_ARGUMENT);
        assert_eq!(needed, 99);
        assert_eq!(backing, [usize::MAX; 8]);
        let mut output = [0xa5_u8; 64];
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, &presentation,
                output.as_mut_ptr().cast(), output.len(), backing.as_mut_ptr())
        }, CIX_STATUS_INVALID_ARGUMENT);
        assert_eq!(backing, [usize::MAX; 8]);
        assert_eq!(output, [0xa5; 64]);
        presentation = record(b"translated");
        let raw_record = &mut presentation as *mut cix_buffer_presentation_v1;
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, raw_record,
                output.as_mut_ptr().cast(), output.len(), raw_record.cast::<usize>())
        }, CIX_STATUS_INVALID_ARGUMENT);
        assert_eq!(presentation.abi_version, 1);
        assert_eq!(presentation.struct_size, mem::size_of::<cix_buffer_presentation_v1>() as u32);
        assert_eq!(output, [0xa5; 64]);
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, &presentation,
                context.0.cast(), 1, &mut needed)
        }, CIX_STATUS_INVALID_ARGUMENT);
        assert_eq!(needed, 99);
        assert_eq!(unsafe {
            cix_context_last_error_presented_v1(context.0, &presentation,
                output.as_mut_ptr().cast(), output.len(), context.0.cast::<usize>())
        }, CIX_STATUS_INVALID_ARGUMENT);
        assert_eq!(copy(&context, ptr::null()), b"");
    }

    #[test]
    fn append_only_record_ignores_trailing_bytes() {
        let context = Context::new();
        context.select_short_buffer();
        #[repr(C)]
        struct Extended { prefix: cix_buffer_presentation_v1, tail: [u8; 8] }
        let extended = Extended {
            prefix: cix_buffer_presentation_v1 {
                struct_size: mem::size_of::<Extended>() as u32,
                ..record(b"translated")
            },
            tail: [0xa5; 8],
        };
        assert_eq!(copy(&context, &extended.prefix), b"translated");
        assert_eq!(extended.tail, [0xa5; 8]);
    }

    #[test]
    fn independent_contexts_can_share_the_same_immutable_candidate() {
        let pattern = "mémoire {{courte}}".as_bytes();
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2).map(|_| scope.spawn(|| {
                let context = Context::new();
                let archive_len = context.select_short_buffer();
                let presentation = record(pattern);
                assert_eq!(copy(&context, &presentation), "mémoire {courte}".as_bytes());
                assert_eq!(copy(&context, ptr::null()), b"caller output buffer is too small");
                context.clear_with_success(archive_len);
                assert_eq!(copy(&context, &presentation), b"");
            })).collect();
            for handle in handles { handle.join().unwrap(); }
        });
    }

    #[test]
    fn unselected_identity_and_extra_internal_number_preserve_retained_english() {
        for message in [
            None,
            Some(Message::new("another.id")),
            Some(Message::new(MESSAGE_ID).arg("needed", Value::Number(Number::Unsigned(18)))),
        ] {
            assert_eq!(render_diagnostic(Diagnostic {
                english: "retained\0English".into(), message,
            }, Some("translated")), "retained\0English");
        }
        for pattern in [Some("{needed}"), Some("}"), None] {
            assert_eq!(render_diagnostic(Diagnostic {
                english: "retained English".into(), message: Some(Message::new(MESSAGE_ID)),
            }, pattern), "retained English");
        }
    }
}
