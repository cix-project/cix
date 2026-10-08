#[allow(dead_code)]
#[path = "../src/histogram.rs"]
mod histogram;
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

fn counts(data: &[u8]) -> [usize; 256] {
    let mut counts = [0usize; 256];
    for &value in data {
        counts[value as usize] += 1;
    }
    counts
}

fn main() {
    let result = (|| -> Result<(), String> {
        let mut input = Vec::new();
        io::stdin()
            .read_to_end(&mut input)
            .map_err(|error| error.to_string())?;
        let args: Vec<String> = std::env::args().collect();
        let output = match args.as_slice() {
            [_, option] if option == "--best" => histogram::encode_best(&counts(&input))?.0,
            [_, option, mode] if option == "--candidate-mode" => {
                let mode = mode.parse::<u8>().map_err(|_| "invalid mode".to_string())?;
                histogram::encode_candidates(&counts(&input))?
                    .into_iter()
                    .find_map(|(_, payload)| (payload.get(1) == Some(&mode)).then_some(payload))
                    .ok_or("candidate mode not found")?
            }
            [_, option] if option == "--type-class-candidate" => {
                let incumbent = rank::encode_type_class(&input)?;
                histogram::type_class_candidate(&input, &incumbent)?
                    .ok_or("no eligible histogram candidate")?
            }
            [_, option, total] if option == "--decode-counts" => {
                let total = total
                    .parse::<usize>()
                    .map_err(|_| "invalid total".to_string())?;
                let counts = histogram::decode_counts(&input, total)?;
                let mut output = Vec::with_capacity(256 * 8);
                for count in counts {
                    output.extend_from_slice(
                        &u64::try_from(count)
                            .map_err(|_| "count does not fit fixture envelope")?
                            .to_le_bytes(),
                    );
                }
                output
            }
            [_, option, total] if option == "--decode-type-class" => {
                let total = total
                    .parse::<usize>()
                    .map_err(|_| "invalid total".to_string())?;
                histogram::decode_type_class(&input, total)?
            }
            _ => return Err("invalid histogram fixture arguments".into()),
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
