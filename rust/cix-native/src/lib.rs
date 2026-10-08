pub mod c_api;
pub mod c_dictionary_api;
pub mod c_format_api;
pub mod c_stream_api;
mod cli;
mod cli_full_engine;
pub mod core;
pub mod dictionaries;
pub mod full_engine;
pub mod i18n;
mod job_cli;
pub mod jxl_bridge;
pub mod managed_sdk;
pub mod paq_bridge;
mod paq_supervisor;
mod paq_worker;
#[cfg(feature = "server")]
pub mod service;
pub mod standard_formats;

/// Retained Squash callback ABI; it is implemented by the native core.
pub use core::cix_squash_splice;
/// Compatibility CLI entry point.  Library consumers use [`core`] directly.
pub fn main_entry() {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(std::ffi::OsStr::new(paq_worker::SWITCH)) {
        std::process::exit(paq_worker::main(&args.collect::<Vec<_>>()));
    }
    if first
        .as_deref()
        .is_some_and(|value| value == "--cix-job-v1" || value == "--cix-job-capabilities-v1")
    {
        std::process::exit(job_cli::main(first.into_iter().chain(args).collect()));
    }
    cli::main_entry();
}
// Preserve the established codec primitive paths without exposing the private
// executable implementation module.
pub use core::{
    adaptive, bsc_ffi, combinatorics, external, fixedmix, huffman, legacy_lz, limits, parallel,
    ppm, rank, resources, stdout_writer, zpaq_context,
};
