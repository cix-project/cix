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
    let mut pos = 0usize;
    let source_length = read_u64(&input, &mut pos)?;
    let history_length = read_u64(&input, &mut pos)?;
    let end = pos
        .checked_add(history_length)
        .ok_or("fixture history length overflow")?;
    let history = input.get(pos..end).ok_or("truncated fixture history")?;
    pos = end;
    let decoded = cix_native::legacy_lz::decode(&input[pos..], source_length, history, 1 << 16)?;
    io::stdout()
        .write_all(&decoded)
        .map_err(|error| error.to_string())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("decode_legacy_lz: {error}");
        std::process::exit(1);
    }
}
