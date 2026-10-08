# Native differential harness contract

The qualification job must compile `profile_contract.c` and the supplied
`native_differential.rs` with `cix-portable` plus the C profile library.  For
every block, the Rust side constructs the profile envelope around the exact
native CIXG1 choices restricted to raw, RLE, and one `substream(4, input,
adaptive::encode(input))`; it compares encoded bytes when both representations
are beneficial.  The decoder half of the job compares decoded bytes. Include all lengths 0..512,
the 8 KiB metadata boundary, the 128 KiB data boundary, long runs, random
bytes, and corruptions of each length, route, coder, adaptive bit length, and
trailing byte.

`profile_contract.c` contains frozen raw, RLE, and coder-4 vectors to make the
wire checks runnable without an external generator.  The Rust harness is kept
outside the production crate until the later SquashFS tools pin supplies its
qualification build target; it must never select composition/rank or coder 5.
