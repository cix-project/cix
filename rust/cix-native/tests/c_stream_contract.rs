use cix_native::c_api::{
    cix_options_v1, CIX_ABI_VERSION_1, CIX_PROFILE_DEFAULT, CIX_STATUS_INVALID_ARGUMENT,
    CIX_STATUS_OK,
};
use cix_native::c_stream_api::*;
use cix_native::core::{decode_buffer, NativeOptions};
use std::ptr;

fn options() -> cix_options_v1 {
    cix_options_v1 {
        abi_version: CIX_ABI_VERSION_1,
        struct_size: std::mem::size_of::<cix_options_v1>() as u32,
        profile: CIX_PROFILE_DEFAULT,
        workers: 1,
        output_limit: 8 << 20,
        memory_limit: 8 << 20,
    }
}
#[test]
fn fragmented_encoder_backpressure_empty_and_reset() {
    unsafe {
        let mut handle = ptr::null_mut();
        assert_eq!(
            cix_stream_encoder_create(&options(), &mut handle),
            CIX_STATUS_OK
        );
        let mut output = [0u8; 7];
        let mut result = cix_stream_result_v1 {
            consumed: 0,
            produced: 0,
            state: 0,
        };
        assert_eq!(
            cix_stream_encoder_process(
                handle,
                b"abc".as_ptr(),
                3,
                output.as_mut_ptr(),
                output.len(),
                &mut result
            ),
            CIX_STATUS_OK
        );
        assert!(result.produced > 0 || result.state == CIX_STREAM_NEEDS_OUTPUT);
        while result.state != CIX_STREAM_FINISHED {
            assert_eq!(
                cix_stream_encoder_finish(handle, output.as_mut_ptr(), output.len(), &mut result),
                CIX_STATUS_OK
            );
        }
        assert_eq!(cix_stream_encoder_reset(handle), CIX_STATUS_OK);
        cix_stream_encoder_destroy(handle);
    }
}
#[test]
fn rejects_invalid_ranges() {
    unsafe {
        let mut result = cix_stream_result_v1 {
            consumed: 0,
            produced: 0,
            state: 0,
        };
        assert_eq!(
            cix_stream_encoder_process(
                ptr::null_mut(),
                ptr::null(),
                1,
                ptr::null_mut(),
                0,
                &mut result
            ),
            CIX_STATUS_INVALID_ARGUMENT
        );
    }
}

#[test]
fn rejects_result_alias_before_constructing_slices() {
    unsafe {
        let mut handle = ptr::null_mut();
        assert_eq!(
            cix_stream_encoder_create(&options(), &mut handle),
            CIX_STATUS_OK
        );
        let mut result = cix_stream_result_v1 {
            consumed: 0,
            produced: 0,
            state: 0,
        };
        let status = cix_stream_encoder_process(
            handle,
            b"x".as_ptr(),
            1,
            &mut result as *mut _ as *mut u8,
            std::mem::size_of::<cix_stream_result_v1>(),
            &mut result,
        );
        assert_eq!(status, CIX_STATUS_INVALID_ARGUMENT);
        cix_stream_encoder_destroy(handle);
    }
}

#[test]
fn accepts_null_zero_length_buffers() {
    unsafe {
        let mut handle = ptr::null_mut();
        assert_eq!(
            cix_stream_encoder_create(&options(), &mut handle),
            CIX_STATUS_OK
        );
        let mut result = cix_stream_result_v1 {
            consumed: 0,
            produced: 0,
            state: 0,
        };
        assert_eq!(
            cix_stream_encoder_process(handle, ptr::null(), 0, ptr::null_mut(), 0, &mut result),
            CIX_STATUS_OK
        );
        cix_stream_encoder_destroy(handle);
    }
}

unsafe fn encode_fragmented(input: &[u8], flush: bool) -> Vec<u8> {
    let mut handle = ptr::null_mut();
    assert_eq!(
        cix_stream_encoder_create(&options(), &mut handle),
        CIX_STATUS_OK
    );
    let mut archive = Vec::new();
    let mut offset = 0;
    let mut flushed = false;
    for _ in 0..20_000 {
        let mut out = [0u8; 113];
        let mut result = cix_stream_result_v1 {
            consumed: 0,
            produced: 0,
            state: 0,
        };
        let status = if offset < input.len() {
            cix_stream_encoder_process(
                handle,
                input[offset..].as_ptr(),
                input.len() - offset,
                out.as_mut_ptr(),
                out.len(),
                &mut result,
            )
        } else if flush && !flushed {
            flushed = true;
            cix_stream_encoder_flush(handle, out.as_mut_ptr(), out.len(), &mut result)
        } else {
            cix_stream_encoder_finish(handle, out.as_mut_ptr(), out.len(), &mut result)
        };
        assert_eq!(status, CIX_STATUS_OK);
        offset += result.consumed;
        archive.extend_from_slice(&out[..result.produced]);
        if result.state == CIX_STREAM_FINISHED {
            cix_stream_encoder_destroy(handle);
            return archive;
        }
        assert!(
            result.consumed != 0 || result.produced != 0,
            "stream stalled"
        );
    }
    cix_stream_encoder_destroy(handle);
    panic!("bounded encoder loop exhausted")
}

unsafe fn decode_fragmented(archive: &[u8]) -> Result<Vec<u8>, i32> {
    let mut handle = ptr::null_mut();
    if cix_stream_decoder_create(&options(), &mut handle) != CIX_STATUS_OK {
        return Err(-1);
    }
    let mut output = Vec::new();
    let mut offset = 0;
    for _ in 0..20_000 {
        let mut out = [0u8; 97];
        let mut result = cix_stream_result_v1 {
            consumed: 0,
            produced: 0,
            state: 0,
        };
        let status = if offset < archive.len() {
            cix_stream_decoder_process(
                handle,
                archive[offset..].as_ptr(),
                archive.len() - offset,
                out.as_mut_ptr(),
                out.len(),
                &mut result,
            )
        } else {
            cix_stream_decoder_finish(handle, out.as_mut_ptr(), out.len(), &mut result)
        };
        if status != CIX_STATUS_OK {
            cix_stream_decoder_destroy(handle);
            return Err(status);
        }
        offset += result.consumed;
        output.extend_from_slice(&out[..result.produced]);
        if result.state == CIX_STREAM_FINISHED {
            cix_stream_decoder_destroy(handle);
            return Ok(output);
        }
        assert!(
            result.consumed != 0 || result.produced != 0,
            "stream stalled"
        );
    }
    cix_stream_decoder_destroy(handle);
    panic!("bounded decoder loop exhausted")
}

#[test]
fn fragmented_cross_block_roundtrip_and_core_compatibility() {
    unsafe {
        let input: Vec<u8> = (0..65_537).map(|n| (n as u8).wrapping_mul(31)).collect();
        let archive = encode_fragmented(&input, false);
        assert_eq!(decode_fragmented(&archive).unwrap(), input);
        assert_eq!(
            decode_buffer(&archive, &NativeOptions::default()).unwrap(),
            input
        );
    }
}

#[test]
fn flush_closes_partial_archive_compatible_with_core() {
    unsafe {
        let input = b"partial C stream flush";
        let archive = encode_fragmented(input, true);
        assert_eq!(
            decode_buffer(&archive, &NativeOptions::default()).unwrap(),
            input
        );
    }
}

#[test]
fn corrupted_decoder_poison_requires_reset() {
    unsafe {
        let input = b"corruption contract";
        let mut archive = encode_fragmented(input, false);
        *archive.last_mut().unwrap() ^= 1;
        assert!(decode_fragmented(&archive).is_err());
        let mut handle = ptr::null_mut();
        assert_eq!(
            cix_stream_decoder_create(&options(), &mut handle),
            CIX_STATUS_OK
        );
        let mut out = [0u8; 512];
        let mut result = cix_stream_result_v1 {
            consumed: 0,
            produced: 0,
            state: 0,
        };
        let _ = cix_stream_decoder_process(
            handle,
            archive.as_ptr(),
            archive.len(),
            out.as_mut_ptr(),
            out.len(),
            &mut result,
        );
        assert_eq!(
            cix_stream_decoder_finish(handle, out.as_mut_ptr(), out.len(), &mut result),
            cix_native::c_api::CIX_STATUS_CODEC_ERROR
        );
        assert_eq!(cix_stream_decoder_reset(handle), CIX_STATUS_OK);
        cix_stream_decoder_destroy(handle);
    }
}

#[test]
fn create_rejects_misaligned_overflow_and_options_overlap_before_write() {
    unsafe {
        let valid_options = options();
        #[repr(align(16))]
        struct Aligned([u8; std::mem::size_of::<*mut cix_stream_encoder>() + 8]);
        let mut bytes = Aligned([0; std::mem::size_of::<*mut cix_stream_encoder>() + 8]);
        let misaligned = bytes
            .0
            .as_mut_ptr()
            .add(1)
            .cast::<*mut cix_stream_encoder>();
        assert_eq!(
            cix_stream_encoder_create(&valid_options, misaligned),
            CIX_STATUS_INVALID_ARGUMENT
        );
        let overflow = (usize::MAX - std::mem::size_of::<*mut cix_stream_encoder>() + 1)
            as *mut *mut cix_stream_encoder;
        assert_eq!(
            cix_stream_encoder_create(&valid_options, overflow),
            CIX_STATUS_INVALID_ARGUMENT
        );

        let mut overlapping = options();
        let before = overlapping.abi_version;
        let out = (&mut overlapping as *mut cix_options_v1).cast::<*mut cix_stream_encoder>();
        assert_eq!(
            cix_stream_encoder_create(&overlapping, out),
            CIX_STATUS_INVALID_ARGUMENT
        );
        assert_eq!(overlapping.abi_version, before);
    }
}
