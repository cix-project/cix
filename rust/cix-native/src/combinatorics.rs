//! Exact, interruptible combinatorial primitives for encoder-only rank paths.
//!
//! These keep the canonical `BigUint` results used by CIX.  The colex helper
//! advances adjacent binomial coefficients rather than rebuilding every term
//! from its factorial product.  Inputs from the encoders are increasing
//! positions; unordered input deliberately falls back to the definition so
//! callers retain its historical result.

use num_bigint::BigUint;
use num_traits::{One, Zero};

const POLL_MASK: usize = 255;

pub(crate) fn choose_checked(n: usize, k: usize) -> Result<BigUint, String> {
    crate::limits::check()?;
    if k > n {
        return Ok(BigUint::zero());
    }
    let k = k.min(n - k);
    let mut value = BigUint::one();
    for i in 1..=k {
        if i & POLL_MASK == 0 {
            crate::limits::check()?;
        }
        value *= n - k + i;
        value /= i;
    }
    Ok(value)
}

pub(crate) fn colex_rank_checked(positions: &[usize]) -> Result<BigUint, String> {
    crate::limits::check()?;
    if positions.len() < 2 || !increasing_positions(positions)? {
        return definition_rank(positions);
    }
    increasing_rank(positions)
}

fn increasing_positions(positions: &[usize]) -> Result<bool, String> {
    for (index, pair) in positions.windows(2).enumerate() {
        if index & POLL_MASK == 0 {
            crate::limits::check()?;
        }
        if pair[0] >= pair[1] {
            return Ok(false);
        }
    }
    Ok(true)
}

fn definition_rank(positions: &[usize]) -> Result<BigUint, String> {
    let mut rank = BigUint::zero();
    for (index, &position) in positions.iter().enumerate() {
        if index & POLL_MASK == 0 {
            crate::limits::check()?;
        }
        rank += choose_checked(position, index + 1)?;
    }
    Ok(rank)
}

fn increasing_rank(positions: &[usize]) -> Result<BigUint, String> {
    // term is C(position, index + 1).  For an increasing position sequence,
    // incrementing n and then k gives the next term exactly:
    // C(n + 1, k) = C(n, k) * (n + 1) / (n + 1 - k),
    // C(n, k + 1) = C(n, k) * (n - k) / (k + 1).
    let mut position = positions[0];
    let mut width = 1usize;
    let mut term = BigUint::from(position);
    let mut rank = term.clone();
    for (index, &next_position) in positions.iter().enumerate().skip(1) {
        if index & POLL_MASK == 0 {
            crate::limits::check()?;
        }
        let next_width = width.checked_add(1).ok_or("colex width overflow")?;
        let advance_cost = next_position.saturating_sub(position).saturating_add(1);
        let direct_cost = next_width.min(next_position.saturating_sub(next_width));
        if direct_cost <= advance_cost {
            term = choose_checked(next_position, next_width)?;
            position = next_position;
            width = next_width;
            rank += &term;
            continue;
        }
        advance_term(&mut term, &mut position, width, next_position)?;
        // Strictly increasing positions guarantee position >= width here.
        term *= position - width;
        term /= next_width;
        width = next_width;
        rank += &term;
    }
    Ok(rank)
}

fn advance_term(
    term: &mut BigUint,
    position: &mut usize,
    width: usize,
    next_position: usize,
) -> Result<(), String> {
    let mut advanced = 0usize;
    while *position < next_position {
        if advanced & POLL_MASK == 0 {
            crate::limits::check()?;
        }
        *position += 1;
        advanced += 1;
        if *position < width {
            *term = BigUint::zero();
        } else if *position == width {
            *term = BigUint::one();
        } else {
            *term *= *position;
            *term /= *position - width;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{choose_checked, colex_rank_checked};
    use num_bigint::BigUint;
    use num_traits::Zero;

    fn slow(positions: &[usize]) -> BigUint {
        positions
            .iter()
            .enumerate()
            .fold(BigUint::zero(), |rank, (index, &position)| {
                rank + choose_checked(position, index + 1).unwrap()
            })
    }

    fn visit(n: usize, k: usize, next: usize, values: &mut Vec<usize>) {
        if values.len() == k {
            assert_eq!(colex_rank_checked(values).unwrap(), slow(values));
            return;
        }
        for value in next..n {
            values.push(value);
            visit(n, k, value + 1, values);
            values.pop();
        }
    }

    #[test]
    fn exact_for_every_small_combination() {
        for n in 0..=12 {
            for k in 0..=n {
                visit(n, k, 0, &mut Vec::new());
            }
        }
    }

    #[test]
    fn wide_and_noncanonical_inputs_retain_definition() {
        let wide: Vec<usize> = (0..256).map(|index| index * 2 + 1).collect();
        assert!(colex_rank_checked(&wide).unwrap().bits() > 128);
        assert_eq!(colex_rank_checked(&wide).unwrap(), slow(&wide));
        for values in [vec![0, 1, 2, 3], vec![1, 1, 4], vec![9, 2, 17], vec![0]] {
            assert_eq!(colex_rank_checked(&values).unwrap(), slow(&values));
        }
    }

    #[test]
    fn sparse_large_gaps_use_the_exact_short_binomial_path() {
        for values in [
            vec![1, 1_000_000],
            vec![0, usize::MAX],
            vec![1, 1_000_000, 2_000_000],
        ] {
            assert_eq!(colex_rank_checked(&values).unwrap(), slow(&values));
        }
    }

    #[test]
    fn observes_candidate_deadline_in_long_rank_work() {
        let _deadline = crate::limits::DeadlineGuard::new(std::time::Duration::ZERO);
        assert_eq!(
            choose_checked(usize::MAX, 1).unwrap_err(),
            crate::limits::DEADLINE_ERROR
        );
        assert_eq!(
            colex_rank_checked(&[0, usize::MAX]).unwrap_err(),
            crate::limits::DEADLINE_ERROR
        );
    }

    #[test]
    fn observes_deadline_while_validating_position_order() {
        let positions: Vec<usize> = (0..1024).collect();
        let _deadline = crate::limits::DeadlineGuard::new(std::time::Duration::ZERO);
        assert_eq!(
            colex_rank_checked(&positions).unwrap_err(),
            crate::limits::DEADLINE_ERROR
        );
    }
}
