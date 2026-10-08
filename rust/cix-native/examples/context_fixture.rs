#[path = "../src/limits.rs"]
mod limits;
#[path = "../src/combinatorics.rs"]
mod combinatorics;
#[allow(dead_code)]
#[path = "../src/rank.rs"]
mod rank;

use std::io::{self, Read, Write};

fn read_u64(input: &[u8], pos: &mut usize) -> Result<usize, String> {
    let end = pos.checked_add(8).ok_or("fixture offset overflow")?;
    let bytes: [u8; 8] = input
        .get(*pos..end)
        .ok_or("truncated fixture header")?
        .try_into()
        .unwrap();
    *pos = end;
    usize::try_from(u64::from_le_bytes(bytes)).map_err(|_| "fixture value overflow".into())
}

fn main() {
    let result = (|| -> Result<(), String> {
        let mut input = Vec::new();
        io::stdin()
            .read_to_end(&mut input)
            .map_err(|error| error.to_string())?;
        let mut pos = 0;
        let source_length = read_u64(&input, &mut pos)?;
        let prefix_length = read_u64(&input, &mut pos)?;
        let order = read_u64(&input, &mut pos)?;
        let bucket_bits = read_u64(&input, &mut pos)?;
        let prefix_end = pos
            .checked_add(prefix_length)
            .ok_or("fixture prefix offset overflow")?;
        let prefix = input
            .get(pos..prefix_end)
            .ok_or("truncated fixture prefix")?;
        let payload = input
            .get(prefix_end..)
            .ok_or("invalid fixture payload offset")?;

        let args: Vec<String> = std::env::args().collect();
        let output = if args.len() == 1 {
            if payload.len() != source_length {
                return Err("fixture source length mismatch".into());
            }
            rank::encode_context_type_classes(payload, order, bucket_bits, prefix)?
        } else if args.len() == 2 && args[1] == "--decode" {
            rank::decode_context_type_classes(payload, source_length, prefix)?
        } else {
            return Err("usage: context_fixture [--decode]".into());
        };
        io::stdout()
            .write_all(&output)
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
