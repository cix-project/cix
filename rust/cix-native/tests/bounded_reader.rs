#[path = "../src/bounded_reader.rs"]
mod bounded_reader;

use std::io::{self, Read};

struct Partial {
    data: Vec<u8>,
    at: usize,
    step: usize,
}
impl Read for Partial {
    fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        if self.at == self.data.len() {
            return Ok(0);
        }
        let n = (self.data.len() - self.at).min(self.step).min(dst.len());
        dst[..n].copy_from_slice(&self.data[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

struct InterruptThenPartial {
    inner: Partial,
    interrupted: bool,
    fail: bool,
}
impl Read for InterruptThenPartial {
    fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        if !self.interrupted {
            self.interrupted = true;
            return Err(io::Error::new(io::ErrorKind::Interrupted, "retry"));
        }
        if self.fail {
            return Err(io::Error::other("injected"));
        }
        self.inner.read(dst)
    }
}

#[test]
fn preserves_irregular_partial_reads() {
    let data: Vec<u8> = (0..251).cycle().take(999).collect();
    let got = bounded_reader::read_bounded(
        Partial {
            data: data.clone(),
            at: 0,
            step: 7,
        },
        1_024,
        1_024,
    )
    .unwrap();
    assert_eq!(got, data);
    assert!(got.capacity() <= 1_024);
}

#[test]
fn grows_across_chunks_without_exceeding_small_cap() {
    let data = vec![4; 65_537];
    let got = bounded_reader::read_bounded(
        Partial {
            data: data.clone(),
            at: 0,
            step: 3_001,
        },
        65_537,
        65_537,
    )
    .unwrap();
    assert_eq!(got, data);
    assert!(got.capacity() <= 65_537);
}

#[test]
fn retries_interruptions_and_propagates_other_errors() {
    let data = vec![8; 17];
    let got = bounded_reader::read_bounded(
        InterruptThenPartial {
            inner: Partial {
                data: data.clone(),
                at: 0,
                step: 2,
            },
            interrupted: false,
            fail: false,
        },
        32,
        32,
    )
    .unwrap();
    assert_eq!(got, data);
    assert_eq!(
        bounded_reader::read_bounded(
            InterruptThenPartial {
                inner: Partial {
                    data,
                    at: 0,
                    step: 2
                },
                interrupted: true,
                fail: true
            },
            32,
            32
        )
        .unwrap_err()
        .kind(),
        io::ErrorKind::Other
    );
}

#[test]
fn limit_boundary_and_stack_overflow_probe() {
    let data = vec![9; 32];
    assert_eq!(
        bounded_reader::read_bounded(&data[..], 32, 32).unwrap(),
        data
    );
    assert!(bounded_reader::read_bounded(&[9; 33][..], 32, 32).is_err());
}

#[test]
fn separate_admission_limit_is_enforced() {
    assert!(bounded_reader::read_bounded(&[1; 17][..], 32, 16).is_err());
    assert!(bounded_reader::read_bounded(&[][..], 16, 17).is_err());
}

#[test]
fn zero_limits_and_conservative_budget_are_explicit() {
    assert!(bounded_reader::read_bounded(&[][..], 0, 0)
        .unwrap()
        .is_empty());
    assert!(bounded_reader::read_bounded(&[1][..], 0, 0).is_err());
    assert_eq!(bounded_reader::conservative_allowance(64 * 1024), 0);
    assert_eq!(
        bounded_reader::conservative_allowance(3 * 64 * 1024),
        64 * 1024
    );
}
