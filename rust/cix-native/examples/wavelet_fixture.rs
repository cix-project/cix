#[path = "../src/limits.rs"]
mod limits;
#[path = "../src/combinatorics.rs"]
mod combinatorics;
#[allow(dead_code)]
#[path = "../src/rank.rs"]
mod rank;
#[allow(dead_code)]
#[path = "../src/wavelet.rs"]
mod wavelet;

use std::io::{self, Read, Write};

fn main() {
    let result = (|| -> Result<(), String> {
        let mut input = Vec::new();
        io::stdin()
            .read_to_end(&mut input)
            .map_err(|error| error.to_string())?;
        let args: Vec<String> = std::env::args().collect();
        let output = if args.len() == 1 {
            wavelet::encode(&input)
        } else if args.len() == 3 && args[1] == "--decode" {
            let source_length = args[2]
                .parse::<usize>()
                .map_err(|_| "invalid source length".to_string())?;
            wavelet::decode(&input, source_length)?
        } else {
            return Err("usage: wavelet_fixture [--decode SOURCE_LENGTH]".into());
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
