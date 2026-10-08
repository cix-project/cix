#[path = "../src/limits.rs"]
mod limits;
#[path = "../src/combinatorics.rs"]
mod combinatorics;
#[path = "../src/rank.rs"]
mod rank;

// Included codec tests validate their real reconstruction paths as well as
// fixture generation. Decoder stubs would make those tests fail spuriously.
#[path = "../src/adaptive.rs"]
mod adaptive;
#[path = "../src/histogram.rs"]
mod histogram;
#[path = "../src/wavelet.rs"]
mod wavelet;

fn unvar(data: &[u8], pos: &mut usize) -> Result<usize, String> {
    let mut value = 0usize;
    for shift in (0..usize::BITS as usize).step_by(7) {
        let byte = *data.get(*pos).ok_or("truncated fixture varint")?;
        *pos += 1;
        value |= ((byte & 0x7f) as usize)
            .checked_shl(shift as u32)
            .ok_or("fixture varint overflow")?;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("oversized fixture varint".into())
}

#[path = "../src/numeric.rs"]
mod numeric;

use std::io::{self, Read, Write};

fn put_u64(value: usize, out: &mut Vec<u8>) -> Result<(), String> {
    out.extend_from_slice(
        &u64::try_from(value)
            .map_err(|_| "fixture length exceeds u64")?
            .to_le_bytes(),
    );
    Ok(())
}

fn put_candidate(candidate: &numeric::EncodedCandidate, out: &mut Vec<u8>) -> Result<(), String> {
    put_u64(candidate.description.len(), out)?;
    put_u64(candidate.payload.len(), out)?;
    out.extend_from_slice(candidate.description.as_bytes());
    out.extend_from_slice(&candidate.payload);
    Ok(())
}

fn main() {
    let result = (|| -> Result<(), String> {
        let mut data = Vec::new();
        io::stdin()
            .read_to_end(&mut data)
            .map_err(|error| error.to_string())?;
        let selected = numeric::encode(&data)?;
        let candidates = numeric::encode_candidates(&data)?;
        let mut output = Vec::new();
        put_candidate(&selected, &mut output)?;
        put_u64(candidates.len(), &mut output)?;
        for candidate in &candidates {
            put_candidate(candidate, &mut output)?;
        }
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
