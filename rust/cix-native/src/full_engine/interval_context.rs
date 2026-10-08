//! Historical `CIXZ\x01` bounded-grid compatibility.
//!
//! The `SGBI1` raw stream deliberately has no policy byte: its per-block
//! integer representation identifies fixed-bit, mixed-radix, arithmetic and
//! Rice choices directly.  `CIXZ\x01` fixes Brotli in the source runtime, but
//! accepts an injected backend here so provider selection remains outside the
//! historical wire parser.

use super::arithmetic::{vdecode, vencode};
use super::bounded_values::{inverse_interval_raw, transform_interval_raw};

pub const MAGIC: &[u8] = b"CIXZ\x01";
pub const RAW_MAGIC: &[u8] = b"SGBI1";
pub const SOURCE_BLOCK: usize = 256;

pub type EncodeBackend<'a> = dyn Fn(&[u8]) -> Result<Vec<u8>, String> + 'a;
pub type DecodeBackend<'a> = dyn Fn(&[u8], usize) -> Result<Vec<u8>, String> + 'a;

pub fn transform_raw(source: &[u8], policy: u8) -> Result<Vec<u8>, String> {
    if !matches!(policy, 1..=3) {
        return Err("invalid integer policy".into());
    }
    transform_interval_raw(source, policy, SOURCE_BLOCK)
}

pub fn inverse_raw(raw: &[u8], max_output: usize) -> Result<Vec<u8>, String> {
    if !raw.starts_with(RAW_MAGIC) {
        return Err("invalid bounded-grid transform".into());
    }
    inverse_interval_raw(raw, max_output)
}

pub fn encode_frame(
    source: &[u8],
    policy: u8,
    backend: &EncodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    let raw = transform_raw(source, policy)?;
    let payload = backend(&raw)?;
    let mut out = MAGIC.to_vec();
    out.extend(vencode(source.len() as u64));
    out.extend(crc32fast::hash(source).to_be_bytes());
    out.extend(payload);
    Ok(out)
}

pub fn decode_frame(
    frame: &[u8],
    limit: usize,
    memory: usize,
    backend: &DecodeBackend<'_>,
) -> Result<Vec<u8>, String> {
    if !frame.starts_with(MAGIC) || memory == 0 {
        return Err("invalid bounded-grid frame".into());
    }
    let (length, at) = vdecode(frame, MAGIC.len())?;
    let length = usize::try_from(length).map_err(|_| "bounded-grid output overflow")?;
    if length > limit || length > memory {
        return Err("bounded-grid output exceeds limit".into());
    }
    let checksum_at = at.checked_add(4).ok_or("bounded-grid frame overflow")?;
    let checksum: [u8; 4] = frame
        .get(at..checksum_at)
        .ok_or("truncated bounded-grid frame")?
        .try_into()
        .map_err(|_| "truncated bounded-grid checksum")?;
    let work = length
        .checked_mul(8)
        .ok_or("bounded-grid working-memory overflow")?;
    let budget = memory
        .checked_sub(work)
        .ok_or("bounded-grid combined live-buffer limit")?;
    let raw = backend(
        frame
            .get(checksum_at..)
            .ok_or("truncated bounded-grid payload")?,
        budget,
    )?;
    if raw.len() > budget {
        return Err("backend ignored bounded-grid memory limit".into());
    }
    let restored = inverse_raw(&raw, length)?;
    if restored.len() != length || crc32fast::hash(&restored) != u32::from_be_bytes(checksum) {
        return Err("bounded-grid checksum mismatch".into());
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> [(&'static [u8], u8, &'static [u8]); 6] {
        [
            (
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_interval/tiny-input.bin"
                )),
                1,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_interval/tiny-policy1.raw"
                )),
            ),
            (
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_interval/tiny-input.bin"
                )),
                2,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_interval/tiny-policy2.raw"
                )),
            ),
            (
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_interval/tiny-input.bin"
                )),
                3,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_interval/tiny-policy3.raw"
                )),
            ),
            (
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_interval/boundary256-input.bin"
                )),
                1,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_interval/boundary256-policy1.raw"
                )),
            ),
            (
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_interval/boundary256-input.bin"
                )),
                2,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_interval/boundary256-policy2.raw"
                )),
            ),
            (
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_interval/boundary256-input.bin"
                )),
                3,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/grid_interval/boundary256-policy3.raw"
                )),
            ),
        ]
    }

    #[test]
    fn interval_context_all_policy_raw_fixture_identity_and_inverse() {
        for (source, policy, raw) in fixtures() {
            assert_eq!(transform_raw(source, policy).unwrap(), raw);
            assert_eq!(inverse_raw(raw, source.len()).unwrap(), source);
        }
    }

    #[test]
    fn interval_context_frame_identity_crc_bounds_and_malformed_raw() {
        for (source, policy, raw) in fixtures() {
            let frame = encode_frame(source, policy, &|value| Ok(value.to_vec())).unwrap();
            let mut expected = MAGIC.to_vec();
            expected.extend(vencode(source.len() as u64));
            expected.extend(crc32fast::hash(source).to_be_bytes());
            expected.extend_from_slice(raw);
            assert_eq!(frame, expected);

            let memory = source.len() * 8 + raw.len();
            assert_eq!(
                decode_frame(&frame, source.len(), memory, &|value, _| Ok(value.to_vec())).unwrap(),
                source
            );
            assert!(inverse_raw(raw, source.len() - 1).is_err());
            assert!(
                decode_frame(&frame, source.len() - 1, memory, &|value, _| Ok(
                    value.to_vec()
                ))
                .is_err()
            );
            assert!(
                decode_frame(&frame, source.len(), memory - 1, &|value, _| Ok(
                    value.to_vec()
                ))
                .is_err()
            );
            assert!(
                decode_frame(&frame, source.len(), memory, &|_, budget| Ok(vec![
                    0;
                    budget
                        + 1
                ]))
                .is_err()
            );

            let (_, crc_at) = vdecode(&frame, MAGIC.len()).unwrap();
            let mut bad_crc = frame.clone();
            bad_crc[crc_at] ^= 1;
            assert!(decode_frame(&bad_crc, source.len(), memory, &|value, _| Ok(
                value.to_vec()
            ))
            .is_err());

            let mut trailing = raw.to_vec();
            trailing.push(0);
            assert!(inverse_raw(&trailing, source.len()).is_err());
            let mut wrong_parent = raw.to_vec();
            wrong_parent[5] = 0;
            assert!(inverse_raw(&wrong_parent, source.len()).is_err());
        }
        assert!(transform_raw(
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/grid_interval/tiny-input.bin"
            )),
            0
        )
        .is_err());
        assert!(decode_frame(MAGIC, 1, 1, &|value, _| Ok(value.to_vec())).is_err());
    }
}
