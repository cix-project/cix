//! Audit-only DHAT allocation probe for the fixed PPM eligible scan.
use cix_native::ppm;
use sha2::{Digest, Sha256};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const INPUT_BYTES: usize = 4 * 1024;

fn random_input() -> Vec<u8> {
    let mut state = 0x9e37_79b9_u32;
    (0..INPUT_BYTES)
        .map(|index| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as u8).wrapping_add(((index / 17) % 11) as u8)
        })
        .collect()
}

fn de_bruijn(t: usize, p: usize, a: &mut [u8], out: &mut Vec<u8>) {
    const K: usize = 16;
    const N: usize = 3;
    if t > N {
        if N.is_multiple_of(p) {
            out.extend_from_slice(&a[1..=p]);
        }
        return;
    }
    a[t] = a[t - p];
    de_bruijn(t + 1, p, a, out);
    for value in (a[t - p] + 1)..K as u8 {
        a[t] = value;
        de_bruijn(t + 1, t, a, out);
    }
}

fn input(case: &str) -> Vec<u8> {
    match case {
        "random" => random_input(),
        "strong" => (0..INPUT_BYTES).map(|index| b'A' + (index % 4) as u8).collect(),
        "weak" => (0..INPUT_BYTES)
            .map(|index| ((index * 17 + index / 23) & 0xff) as u8)
            .collect(),
        "absent" => {
            let mut sequence = Vec::with_capacity(INPUT_BYTES);
            de_bruijn(1, 1, &mut [0; 4], &mut sequence);
            sequence
        }
        other => panic!("unknown CIX_AUDIT_CASE={other}"),
    }
}

fn main() {
    let case = std::env::var("CIX_AUDIT_CASE").unwrap_or_else(|_| "random".into());
    let profiler = dhat::Profiler::new_heap();
    let input = input(&case);
    let payload = ppm::encode_fixed(&input, 3, &[], 0, false).expect("PPM encode");
    assert_eq!(ppm::decode(&payload, input.len()).expect("PPM decode"), input);
    println!(
        "{{\"case\":\"{case}\",\"input_sha256\":\"{:x}\",\"payload_bytes\":{},\"payload_sha256\":\"{:x}\"}}",
        Sha256::digest(&input),
        payload.len(),
        Sha256::digest(&payload),
    );
    drop(profiler);
}
