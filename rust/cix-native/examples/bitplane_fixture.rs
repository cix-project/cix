#[allow(dead_code)]
#[path = "../src/bitplane.rs"]
mod bitplane;
#[path = "../src/limits.rs"]
mod limits;
#[path = "../src/combinatorics.rs"]
mod combinatorics;
#[allow(dead_code)]
#[path = "../src/rank.rs"]
mod rank;

use std::io::{self, Read, Write};

fn main() {
    let result = (|| -> Result<(), String> {
        let mut input = Vec::new();
        io::stdin()
            .read_to_end(&mut input)
            .map_err(|error| error.to_string())?;
        let args: Vec<String> = std::env::args().collect();
        let payload = if args.len() == 1 {
            bitplane::encode(&input)
        } else if args.len() == 3 && args[1] == "--decode" {
            let source_length = args[2]
                .parse::<usize>()
                .map_err(|_| "invalid source length".to_string())?;
            bitplane::decode(&input, source_length)?
        } else {
            return Err("usage: bitplane_fixture [--decode SOURCE_LENGTH]".into());
        };
        io::stdout()
            .write_all(&payload)
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
