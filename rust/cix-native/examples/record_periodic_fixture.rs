#[allow(dead_code)]
#[path = "../src/bitplane.rs"]
mod bitplane;
#[path = "../src/limits.rs"]
mod limits;
#[allow(dead_code)]
#[path = "../src/rank.rs"]
mod rank;
#[allow(dead_code)]
#[path = "../src/combinatorics.rs"]
mod combinatorics;
#[allow(dead_code)]
#[path = "../src/record_periodic.rs"]
mod record_periodic;
#[allow(dead_code)]
#[path = "../src/sparse_runs.rs"]
mod sparse_runs;

use std::io::{self, Read, Write};

fn main() {
    let result = (|| -> Result<(), String> {
        let mut args = std::env::args().skip(1);
        let kind = args.next().ok_or("expected record or periodic")?;
        let beam: usize = args
            .next()
            .ok_or("expected beam")?
            .parse()
            .map_err(|_| "invalid beam")?;
        if args.next().is_some() {
            return Err("unexpected fixture arguments".into());
        }
        let mut source = Vec::new();
        io::stdin()
            .read_to_end(&mut source)
            .map_err(|error| error.to_string())?;
        let (payload, description) = match kind.as_str() {
            "record" => record_periodic::encode_record(&source, beam)?,
            "periodic" => record_periodic::encode_periodic(&source, beam)?,
            _ => return Err("expected record or periodic".into()),
        };
        eprintln!("{description}");
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
