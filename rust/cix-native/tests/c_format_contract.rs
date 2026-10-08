use cix_native::{c_api::*, c_format_api::*};
fn options(id: u32) -> cix_format_options_v1 {
    cix_format_options_v1 {
        abi_version: 1,
        struct_size: std::mem::size_of::<cix_format_options_v1>() as u32,
        format: id,
        level: 6,
        output_limit: 4 << 20,
        memory_limit: 1 << 30,
    }
}
#[test]
fn all_standard_formats_cross_buffer_roundtrip_and_sizing() {
    let source = b"C ABI explicit standard streams\0".repeat(200);
    for id in 1..=9 {
        let opts = options(id);
        let mut bytes = vec![0; source.len() + 4096];
        let mut n = 0;
        assert_eq!(
            unsafe {
                cix_format_encode_v1(
                    &opts,
                    source.as_ptr(),
                    source.len(),
                    bytes.as_mut_ptr(),
                    bytes.len(),
                    &mut n,
                )
            },
            CIX_STATUS_OK,
            "format {id}"
        );
        bytes.truncate(n);
        assert!(!bytes.starts_with(b"CIX"));
        let mut output = vec![0; source.len()];
        assert_eq!(
            unsafe {
                cix_format_decode_v1(
                    &opts,
                    bytes.as_ptr(),
                    bytes.len(),
                    output.as_mut_ptr(),
                    output.len(),
                    &mut n,
                )
            },
            CIX_STATUS_OK,
            "format {id}"
        );
        assert_eq!(n, source.len());
        assert_eq!(output, source);
        let mut sentinel = [0x5a];
        assert_eq!(
            unsafe {
                cix_format_decode_v1(
                    &opts,
                    bytes.as_ptr(),
                    bytes.len(),
                    sentinel.as_mut_ptr(),
                    1,
                    &mut n,
                )
            },
            CIX_STATUS_OUTPUT_TOO_SMALL
        );
        assert_eq!(n, source.len());
        assert_eq!(sentinel, [0x5a]);
    }
}
#[test]
fn null_empty_and_alias_validation_do_not_create_invalid_borrows() {
    let opts = options(1);
    let mut output = [0; 64];
    let mut n = 17;
    assert_eq!(
        unsafe {
            cix_format_encode_v1(
                &opts,
                std::ptr::null(),
                0,
                output.as_mut_ptr(),
                output.len(),
                &mut n,
            )
        },
        CIX_STATUS_OK
    );
    assert!(n > 0);
    assert_eq!(
        unsafe {
            cix_format_decode_v1(
                &opts,
                output.as_ptr(),
                n,
                output.as_mut_ptr(),
                output.len(),
                &mut n,
            )
        },
        CIX_STATUS_INVALID_ARGUMENT
    );
    assert_eq!(
        unsafe {
            cix_format_encode_v1(
                &opts,
                std::ptr::null(),
                1,
                output.as_mut_ptr(),
                output.len(),
                &mut n,
            )
        },
        CIX_STATUS_INVALID_ARGUMENT
    );
    let invalid = cix_format_options_v1 {
        abi_version: 99,
        ..opts
    };
    assert_eq!(
        unsafe {
            cix_format_encode_v1(
                &invalid,
                std::ptr::null(),
                0,
                output.as_mut_ptr(),
                output.len(),
                &mut n,
            )
        },
        CIX_STATUS_INVALID_OPTIONS
    );
    let prefix = [1_u32, 8];
    assert_eq!(
        unsafe {
            cix_format_encode_v1(
                prefix.as_ptr().cast(),
                std::ptr::null(),
                0,
                output.as_mut_ptr(),
                output.len(),
                &mut n,
            )
        },
        CIX_STATUS_INVALID_OPTIONS
    );
}
