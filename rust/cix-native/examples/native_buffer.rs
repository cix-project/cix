//! Minimal process-free Rust consumer of the native CIX buffer API.
use cix_native::core::{decode_buffer, encode_buffer, NativeOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = b"A small application-owned byte buffer.";
    let options = NativeOptions {
        output_limit: 1 << 20,
        ..NativeOptions::default()
    };
    let archive = encode_buffer(source, &options)?;
    let restored = decode_buffer(&archive, &options)?;
    assert_eq!(restored, source);
    println!(
        "Restored {} bytes from a {}-byte CIX archive.",
        restored.len(),
        archive.len()
    );
    Ok(())
}
