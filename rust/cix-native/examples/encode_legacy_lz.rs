use cix_native::legacy_lz::{self, ContextSpec, EncodeConfig};
use std::io::{self, Read, Write};

fn read_u64(input: &[u8], pos: &mut usize) -> Result<usize, String> {
    let end = pos.checked_add(8).ok_or("fixture header overflow")?;
    let bytes: [u8; 8] = input
        .get(*pos..end)
        .ok_or("truncated fixture header")?
        .try_into()
        .map_err(|_| "invalid fixture header")?;
    *pos = end;
    usize::try_from(u64::from_le_bytes(bytes)).map_err(|_| "fixture length overflow".into())
}

fn run() -> Result<(), String> {
    let mut input = Vec::new();
    io::stdin()
        .read_to_end(&mut input)
        .map_err(|error| error.to_string())?;
    let profile = *input.first().ok_or("missing fixture profile")?;
    let mut pos = 1usize;
    let history_length = read_u64(&input, &mut pos)?;
    let end = pos
        .checked_add(history_length)
        .ok_or("fixture history length overflow")?;
    let history = input.get(pos..end).ok_or("truncated fixture history")?;
    let source = &input[end..];
    let encoded = match profile {
        0 | 10 | 11 | 12 => {
            let config = EncodeConfig {
                version: if profile == 10 { 1 } else { 3 },
                window: 1 << 20,
                chain: 8,
                min_match: 4,
                lazy: false,
                context_models: if profile == 11 {
                    vec![
                        ContextSpec {
                            order: 1,
                            bucket_bits: 8,
                        },
                        ContextSpec {
                            order: 2,
                            bucket_bits: 10,
                        },
                    ]
                } else {
                    Vec::new()
                },
                allow_fixed_mix: profile == 12,
            };
            legacy_lz::encode_with_config(source, history, &config)?
        }
        1..=9 => legacy_lz::encode(source, history, profile)?,
        _ => return Err("invalid fixture profile".into()),
    };
    io::stdout()
        .write_all(&encoded.payload)
        .map_err(|error| error.to_string())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("encode_legacy_lz: {error}");
        std::process::exit(1);
    }
}
