//! Qualification-only source; compiled by the pinned SquashFS integration job.
//! It intentionally compares only the frozen coder-4 profile, never CIXG1's
//! composition/rank or count-range candidates.

extern "C" {
    fn cix_squashfs_profile_encode(kind: std::os::raw::c_int, input: *const u8, input_len: usize,
        output: *mut u8, output_cap: usize, written: *mut usize) -> std::os::raw::c_int;
    fn cix_squashfs_profile_decode(kind: std::os::raw::c_int, input: *const u8, input_len: usize,
        output: *mut u8, output_cap: usize, written: *mut usize) -> std::os::raw::c_int;
    fn cix_squashfs_profile_decode_workspace_size() -> usize;
    fn cix_squashfs_profile_decode_with_workspace(kind: std::os::raw::c_int,
        input: *const u8, input_len: usize, output: *mut u8, output_cap: usize,
        workspace: *mut std::ffi::c_void, workspace_len: usize,
        written: *mut usize) -> std::os::raw::c_int;
}

fn leb(mut value: usize, out: &mut Vec<u8>) {
    while value >= 128 { out.push((value as u8 & 127) | 128); value >>= 7; }
    out.push(value as u8);
}
fn rle(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new(); let mut at = 0;
    while at < input.len() { let mut end = at + 1; while end < input.len() && input[end] == input[at] { end += 1; }
        out.push(input[at]); leb(end - at, &mut out); at = end; }
    out
}
fn envelope(method: u8, input: &[u8], payload: Vec<u8>) -> Vec<u8> {
    let mut out = vec![1, method]; out.extend_from_slice(&(input.len() as u32).to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes()); out.extend_from_slice(&payload); out
}
fn native_choice(input: &[u8]) -> Option<Vec<u8>> {
    let mut best = envelope(0, input, input.to_vec());
    let runs = envelope(1, input, rle(input)); if runs.len() < best.len() { best = runs; }
    let adaptive = cix_portable::adaptive::encode(input);
    let mut route = vec![1, 4]; route.extend_from_slice(&(input.len() as u32).to_le_bytes());
    route.extend_from_slice(&(adaptive.len() as u32).to_le_bytes()); route.extend_from_slice(&adaptive);
    let coded = envelope(2, input, route); if coded.len() < best.len() { best = coded; }
    (best.len() < input.len()).then_some(best)
}
pub fn compare_one(input: &[u8], metadata: bool) {
    let expected = native_choice(input); let mut actual = vec![0u8; input.len().max(1)]; let mut n = 0usize;
    let kind = if metadata { 1 } else { 0 };
    let status = unsafe { cix_squashfs_profile_encode(kind, input.as_ptr(), input.len(), actual.as_mut_ptr(), actual.len(), &mut n) };
    match expected {
        Some(bytes) => {
            assert_eq!(status, 0);
            assert_eq!(&actual[..n], bytes.as_slice());

            let mut stack_output = vec![0u8; input.len().max(1)];
            let mut stack_written = 0usize;
            assert_eq!(unsafe { cix_squashfs_profile_decode(kind, actual.as_ptr(), n,
                stack_output.as_mut_ptr(), stack_output.len(), &mut stack_written) }, 0);
            assert_eq!(&stack_output[..stack_written], input);

            let workspace_size = unsafe { cix_squashfs_profile_decode_workspace_size() };
            let mut workspace = vec![0u32; (workspace_size + 3) / 4];
            let mut workspace_output = vec![0u8; input.len().max(1)];
            let mut workspace_written = 0usize;
            assert_eq!(unsafe { cix_squashfs_profile_decode_with_workspace(kind,
                actual.as_ptr(), n, workspace_output.as_mut_ptr(), workspace_output.len(),
                workspace.as_mut_ptr().cast(), workspace.len() * std::mem::size_of::<u32>(),
                &mut workspace_written) }, 0);
            assert_eq!(&workspace_output[..workspace_written], input);
        }
        None => assert_eq!(status, 1),
    }
}
