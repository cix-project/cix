#[allow(dead_code)]
#[path = "../src/expertmix.rs"]
mod expertmix;
#[path = "../src/limits.rs"]
mod limits;
#[path = "../src/combinatorics.rs"]
mod combinatorics;
#[allow(dead_code)]
#[path = "../src/rank.rs"]
mod rank;

use std::io::{self, Read, Write};

fn read_u64(data: &[u8], pos: &mut usize) -> Result<usize, String> {
    let end = pos.checked_add(8).ok_or("fixture offset overflow")?;
    let bytes: [u8; 8] = data
        .get(*pos..end)
        .ok_or("truncated fixture header")?
        .try_into()
        .unwrap();
    *pos = end;
    usize::try_from(u64::from_le_bytes(bytes)).map_err(|_| "fixture length overflow".into())
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
        let context_limit = read_u64(&input, &mut pos)?;
        let prefix_end = pos
            .checked_add(prefix_length)
            .ok_or("fixture prefix overflow")?;
        let prefix = input
            .get(pos..prefix_end)
            .ok_or("truncated fixture prefix")?;
        let payload = &input[prefix_end..];
        let decoded = expertmix::decode_bounded(payload, source_length, prefix, context_limit)?;
        io::stdout()
            .write_all(&decoded)
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
