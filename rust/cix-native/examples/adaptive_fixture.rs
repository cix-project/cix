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
    if input.len() < 3 {
        return Err("truncated fixture command".into());
    }
    let operation = input[0];
    let order = input[1] as usize;
    let bucket_bits = input[2] as usize;
    let mut pos = 3usize;
    let source_length = read_u64(&input, &mut pos)?;
    let history_length = read_u64(&input, &mut pos)?;
    let model_limit = read_u64(&input, &mut pos)?;
    let end = pos
        .checked_add(history_length)
        .ok_or("fixture history length overflow")?;
    let history = input.get(pos..end).ok_or("truncated fixture history")?;
    let payload = &input[end..];
    let output = match operation {
        0 => {
            if payload.len() != source_length {
                return Err("fixture source length mismatch".into());
            }
            cix_native::adaptive::encode_general(payload, order, bucket_bits, history, model_limit)?
        }
        1 => cix_native::adaptive::decode_general(payload, source_length, history, model_limit)?,
        _ => return Err("unknown fixture operation".into()),
    };
    io::stdout()
        .write_all(&output)
        .map_err(|error| error.to_string())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("adaptive_fixture: {error}");
        std::process::exit(1);
    }
}
