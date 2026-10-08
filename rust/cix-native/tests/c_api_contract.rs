use cix_native::c_api::{
    cix_context_create, cix_context_destroy, cix_decode_buffer, cix_encode_buffer, cix_options_v1,
    cix_options_v1_default, CIX_STATUS_OK, CIX_STATUS_OUTPUT_TOO_SMALL,
};
use std::ptr;

#[test]
fn c_buffer_api_reports_needed_then_round_trips() {
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

    let source = b"C ABI exact buffer round trip";
    let mut needed = 0;
    assert_eq!(
        unsafe {
            cix_encode_buffer(
                context,
                source.as_ptr(),
                source.len(),
                ptr::null_mut(),
                0,
                &mut needed,
            )
        },
        CIX_STATUS_OUTPUT_TOO_SMALL
    );
    let mut archive = vec![0; needed];
    assert_eq!(
        unsafe {
            cix_encode_buffer(
                context,
                source.as_ptr(),
                source.len(),
                archive.as_mut_ptr(),
                archive.len(),
                &mut needed,
            )
        },
        CIX_STATUS_OK
    );
    archive.truncate(needed);

    let mut decoded_needed = 0;
    assert_eq!(
        unsafe {
            cix_decode_buffer(
                context,
                archive.as_ptr(),
                archive.len(),
                ptr::null_mut(),
                0,
                &mut decoded_needed,
            )
        },
        CIX_STATUS_OUTPUT_TOO_SMALL
    );
    let mut decoded = vec![0; decoded_needed];
    assert_eq!(
        unsafe {
            cix_decode_buffer(
                context,
                archive.as_ptr(),
                archive.len(),
                decoded.as_mut_ptr(),
                decoded.len(),
                &mut decoded_needed,
            )
        },
        CIX_STATUS_OK
    );
    assert_eq!(&decoded[..decoded_needed], source);
    unsafe { cix_context_destroy(context) };
}
