//! Command parser and compatibility facade.
use crate::core::engine;
use crate::standard_formats::{self, StandardFormat, StandardOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::{atomic::Ordering, Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    process, thread,
};

enum Input {
    File(File),
    #[cfg(not(unix))]
    Stdin(io::Stdin),
}

impl Read for Input {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::File(file) => file.read(buffer),
            #[cfg(not(unix))]
            Self::Stdin(stdin) => stdin.read(buffer),
        }
    }
}

fn open_input(path: &str) -> Result<Input, String> {
    if path != "-" {
        return File::open(path).map(Input::File).map_err(engine::ioerr);
    }
    #[cfg(unix)]
    {
        use std::os::fd::FromRawFd;
        let fd = unsafe { libc::dup(libc::STDIN_FILENO) };
        if fd < 0 {
            return Err(engine::ioerr(io::Error::last_os_error()));
        }
        Ok(Input::File(unsafe { File::from_raw_fd(fd) }))
    }
    #[cfg(not(unix))]
    {
        // Stdin is consumed directly in bounded codec blocks. Windows has no
        // portable equivalent of Unix poll for an arbitrary redirected handle,
        // so this permits blocking streaming only; timed flush is rejected by
        // the engine before any blocking read.
        Ok(Input::Stdin(io::stdin()))
    }
}

#[cfg(unix)]
fn input_fd(input: &Input) -> i32 {
    use std::os::fd::AsRawFd;
    match input {
        Input::File(file) if !file.metadata().is_ok_and(|metadata| metadata.is_file()) => {
            file.as_raw_fd()
        }
        Input::File(_) => -1,
    }
}

#[cfg(not(unix))]
fn input_fd(_input: &Input) -> i32 {
    -1
}

// Whole-file BEST selection is an executable capability.  It is deliberately
// kept beside the command adapter because it reads named paths and may use the
// same-CIX full-engine worker; the reusable codec never reaches it.
#[path = "portfolio_auto.rs"]
pub(crate) mod portfolio_auto;

// BEST considers the PAQ v215-9L plan.  At the 128 MiB whole-input limit,
// its declared codec reservation plus the retained source and bounded archive
// needs just under 6 GiB.  This is an aggregate admission budget, not a
// process-RSS guarantee; an explicit --memory always replaces it.
const BEST_DEFAULT_MEMORY: usize = 6 << 30;

#[derive(Clone, Default)]
pub(crate) struct Opt {
    pub(crate) decode: bool,
    pub(crate) stdout: bool,
    pub(crate) keep: bool,
    pub(crate) force: bool,
    pub(crate) test: bool,
    pub(crate) list: bool,
    pub(crate) level: u8,
    pub(crate) threads: usize,
    pub(crate) threads_set: bool,
    pub(crate) parallelism: String,
    pub(crate) parallelism_set: bool,
    pub(crate) memory: usize,
    pub(crate) memory_set: bool,
    pub(crate) input: String,
    pub(crate) inputs: Vec<String>,
    pub(crate) output: Option<String>,
    pub(crate) route: Option<String>,
    pub(crate) backend: String,
    pub(crate) backend_set: bool,
    pub(crate) external_backend: Option<String>,
    pub(crate) profile: String,
    pub(crate) profile_set: bool,
    pub(crate) level_set: bool,
    pub(crate) portfolio: Option<String>,
    pub(crate) format: String,
    pub(crate) format_set: bool,
    pub(crate) output_format: Option<StandardFormat>,
    pub(crate) input_format: Option<StandardFormat>,
    pub(crate) block: Option<u32>,
    pub(crate) stream: bool,
    pub(crate) history: usize,
    pub(crate) independent_blocks: bool,
    pub(crate) flush_interval: Option<std::time::Duration>,
    pub(crate) explain: bool,
    pub(crate) verbose: bool,
    pub(crate) private_library_directory: Option<String>,
    pub(crate) temporary_root: Option<String>,
}
static CLI_CANCELLATION: OnceLock<Arc<std::sync::atomic::AtomicBool>> = OnceLock::new();
pub(crate) fn cancellation_token() -> Arc<std::sync::atomic::AtomicBool> {
    CLI_CANCELLATION
        .get_or_init(|| Arc::new(std::sync::atomic::AtomicBool::new(false)))
        .clone()
}
fn usage() {
    println!("CIX (pronounced six) — automatic lossless stream compression\n\nUsage: cix [--fast|--best] [-c] [-k] [-f] [-o FILE] [FILE ...]\n       uncix [-c] [-k] [-f] [-t|-l] [-o FILE] [FILE ...]\n       cixcat [FILE]\n\nFILE defaults to '-' (standard input); use '--' before a dash-prefixed filename. Input files are kept by default. Use '-' for standard input or output. Multiple named files receive their own derived output names; -o FILE and standard-output modes accept one input only. CIX archives are CIX streams, not ZIP or gzip. Tar pipelines support directories.\n\nEffort: --fast minimises compression latency; no option is balanced automatic selection; --best searches aggressively for smaller output. Numeric aliases: -1 fast, -6 default, -9 best.\n\nObservability: --explain reports per-block candidate costs and decisions; --verbose reports aggregate search statistics.\n\nAdvanced / expert controls (usually unnecessary):\n  --format auto|cixg1|cixg2|cixm6|cixf1  Select or constrain the container version\n  --route auto|ROUTE          Override route selection\n  --backend NAME              Override nested substream backend\n  --block-size SIZE           Constrain maximum uncompressed block size\n  --memory SIZE               Bound accounted working memory (FAST/default 512 MiB; BEST/decode 6 GiB)\n  --threads N                 Bound CPU workers (1..20; default up to 4)\n  --parallelism blocks|candidates  Choose ordered blocks or within-block trials\n  --history SIZE              Request retained history\n  --independent-blocks        Request model reset boundaries\n  --stream                    Require streaming operation\n  --flush-interval DURATION   Flush framed partial blocks after idle time\n  --profile fast|default|size Select external profile or native effort\n  --portfolio PROFILE         Compare CIX and direct external candidates (CIXB1)\n  --external CODEC            Explicitly wrap a direct external codec (CIXB1)");
    println!("\nStandard streams: --output-format gzip|zlib|deflate|bzip2|xz|zstd|brotli|lz4|snappy writes a genuine one-shot standard stream. Use --input-format FORMAT with -d, -t or uncix to decode/test it; raw Deflate is never guessed. Standard streams are bounded to 128 MiB input/output and do not accept CIX route, container, portfolio, stream, history or block controls.");
    println!("\nInstalled full-engine providers: --private-library-dir names an absolute private CIX provider directory; --temporary-root names an absolute private PAQ worker root. Provider files are never searched through PATH.");
}

fn default_options() -> Opt {
    Opt {
        level: 6,
        threads: thread::available_parallelism().map_or(1, |n| n.get().min(4)),
        parallelism: "blocks".into(),
        memory: 512 * 1024 * 1024,
        backend: "hybrid".into(),
        profile: "default".into(),
        format: "auto".into(),
        ..Opt::default()
    }
}

fn normalized_program_name(basename: &str, windows: bool) -> String {
    if windows {
        let lowercase = basename.to_ascii_lowercase();
        lowercase
            .strip_suffix(".exe")
            .unwrap_or(&lowercase)
            .to_owned()
    } else {
        basename.to_owned()
    }
}

fn decoder_program_name(args: &[String]) -> (bool, bool) {
    let program = Path::new(args.first().map(String::as_str).unwrap_or("cix"))
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let program = normalized_program_name(&program, cfg!(windows));
    let decode = program == "uncix" || program == "unsix" || program == "cixcat";
    (decode, program == "cixcat")
}

fn takes_option_value(argument: &str) -> bool {
    matches!(
        argument,
        "--threads"
            | "--parallelism"
            | "--memory"
            | "-o"
            | "--output"
            | "--route"
            | "--backend"
            | "--external"
            | "--profile"
            | "--portfolio"
            | "--format"
            | "--output-format"
            | "--input-format"
            | "--block"
            | "--block-size"
            | "--history"
            | "--flush-interval"
            | "--private-library-dir"
            | "--temporary-root"
    )
}

fn set_backend(o: &mut Opt, value: String) -> Result<(), String> {
    if !matches!(
        value.as_str(),
        "cix"
            | "raw"
            | "runs"
            | "ppm"
            | "mixture"
            | "deflate"
            | "hybrid"
            | "range"
            | "count-range"
            | "context-range-o1-b4"
            | "context-range-o1-b8"
            | "context-range-o2-b8"
            | "huffman"
    ) {
        return Err("unsupported backend".into());
    }
    o.backend = value;
    o.backend_set = true;
    Ok(())
}
fn standard_format(value: &str) -> Result<StandardFormat, String> {
    match value {
        "gzip" | "gz" => Ok(StandardFormat::Gzip),
        "zlib" => Ok(StandardFormat::Zlib),
        "deflate" | "raw-deflate" => Ok(StandardFormat::Deflate),
        "bzip2" | "bz2" => Ok(StandardFormat::Bzip2),
        "xz" => Ok(StandardFormat::Xz),
        "zstd" | "zst" => Ok(StandardFormat::Zstd),
        "brotli" | "br" => Ok(StandardFormat::Brotli),
        "lz4" | "lz4frame" | "lz4-frame" => Ok(StandardFormat::Lz4Frame),
        "snappy" | "snappyframed" | "snappy-framed" => Ok(StandardFormat::SnappyFramed),
        _ => Err(
            "standard format accepts gzip, zlib, deflate, bzip2, xz, zstd, brotli, lz4 or snappy"
                .into(),
        ),
    }
}

fn set_option_value(o: &mut Opt, argument: &str, value: String) -> Result<(), String> {
    match argument {
        "--threads" => {
            o.threads = value.parse().map_err(|_| "invalid thread count")?;
            o.threads_set = true;
        }
        "--parallelism" => {
            o.parallelism = value;
            o.parallelism_set = true;
        }
        "--memory" => {
            o.memory = parse_size(&value)?;
            o.memory_set = true;
        }
        "--route" => o.route = Some(value),
        "--backend" => set_backend(o, value)?,
        "--external" => o.external_backend = Some(value),
        "--profile" => {
            o.profile = value;
            o.profile_set = true;
        }
        "--portfolio" => o.portfolio = Some(value),
        "--format" => {
            o.format = value;
            o.format_set = true;
        }
        "--output-format" => o.output_format = Some(standard_format(&value)?),
        "--input-format" => o.input_format = Some(standard_format(&value)?),
        "--block" => o.block = Some(value.parse().map_err(|_| "invalid block size")?),
        "--block-size" => {
            o.block = Some(
                parse_size(&value)?
                    .try_into()
                    .map_err(|_| "block size overflows")?,
            )
        }
        "--history" => o.history = parse_size(&value)?,
        "--flush-interval" => o.flush_interval = Some(parse_duration(&value)?),
        "--private-library-dir" => o.private_library_directory = Some(value),
        "--temporary-root" => o.temporary_root = Some(value),
        "-o" | "--output" => o.output = Some(value),
        _ => unreachable!("value-taking options are checked before dispatch"),
    }
    Ok(())
}

fn set_flag(o: &mut Opt, argument: &str) -> Result<bool, String> {
    match argument {
        "-d" => o.decode = true,
        "-c" => o.stdout = true,
        "-k" => o.keep = true,
        "-f" => o.force = true,
        "-t" => {
            o.test = true;
            o.decode = true;
        }
        "-l" => {
            o.list = true;
            o.decode = true;
        }
        "--fast" => {
            o.level = 1;
            o.level_set = true;
        }
        "--best" => {
            o.level = 9;
            o.level_set = true;
        }
        "--explain" => o.explain = true,
        "--verbose" => o.verbose = true,
        "--stream" => o.stream = true,
        "--independent-blocks" => o.independent_blocks = true,
        _ if matches!(
            argument,
            "-1" | "-2" | "-3" | "-4" | "-5" | "-6" | "-7" | "-8" | "-9"
        ) =>
        {
            o.level = argument[1..].parse().unwrap();
            o.level_set = true;
        }
        _ if argument.starts_with('-') && argument != "-" => {
            return Err(format!("unsupported option: {argument}"));
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn handle_special_option(argument: &str) -> bool {
    match argument {
        "--help" | "-h" => {
            usage();
            println!("\nBEST compares complete compatible native containers and native-library backend archives within --memory. Ordinary stdin, non-regular and oversized inputs use bounded independent CIXW1 windows when their controls permit it; --stream, history and flush controls retain the framed encoder. Expert controls constrain their specified dimensions. Use --verbose or --explain for evaluated candidates and resource exclusions.");
            process::exit(0)
        }
        "--version" => {
            println!(
                "cix-native {} (CIXG1/CIXG2/CIXM6/CIXF1/CIXB1/CIXZ1)",
                env!("CARGO_PKG_VERSION")
            );
            process::exit(0)
        }
        _ => false,
    }
}

fn parse_command_arguments(o: &mut Opt, args: &[String]) -> Result<Vec<String>, String> {
    let mut files = vec![];
    let mut options = true;
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if options && a == "--" {
            options = false;
        } else if !options {
            files.push(a.clone());
        } else if handle_special_option(a) {
            unreachable!("special options exit before returning");
        } else if takes_option_value(a) {
            i += 1;
            if i == args.len() {
                return Err(format!("{a} requires a value"));
            }
            set_option_value(o, a, args[i].clone())?;
        } else if a.starts_with('-') && !a.starts_with("--") && a.len() > 2 {
            // Only value-free short options form a cluster. Validate the
            // complete cluster before changing any option state.
            if !a[1..]
                .bytes()
                .all(|flag| matches!(flag, b'd' | b'c' | b'k' | b'f' | b't' | b'l' | b'1'..=b'9'))
            {
                return Err(format!("unsupported option: {a}"));
            }
            for flag in a[1..].bytes() {
                set_flag(o, &format!("-{}", char::from(flag)))?;
            }
        } else if !set_flag(o, a)? {
            files.push(a.clone());
        }
        i += 1
    }
    Ok(files)
}

fn finish_parse(o: &mut Opt, files: Vec<String>) -> Result<(), String> {
    o.inputs = if files.is_empty() {
        vec!["-".into()]
    } else {
        files
    };
    o.input = o.inputs[0].clone();
    if o.stdout && o.output.as_deref().is_some_and(|output| output != "-") {
        return Err("-c conflicts with a named --output; use -o - for standard output".into());
    }
    if o.inputs.len() > 1 {
        if o.output.is_some() {
            return Err(
                "--output accepts one input; omit it to derive an output name for each file".into(),
            );
        }
        if o.stdout {
            return Err("standard-output mode accepts one input at a time".into());
        }
        if o.inputs.iter().any(|input| input == "-") {
            return Err("standard input cannot be combined with named input files".into());
        }
        let mut seen = std::collections::HashSet::new();
        if let Some(input) = o.inputs.iter().find(|input| !seen.insert(input.as_str())) {
            return Err(format!("input path appears more than once: {input}"));
        }
    }
    if o.threads == 0 {
        return Err("--threads must be positive".into());
    }
    if o.decode && !o.memory_set {
        // A default decoder must accept PAQ archives emitted by the implicit
        // BEST policy. The worker receives this as an address-space request
        // after parent buffers are reserved; it remains an admission estimate,
        // not an aggregate-process RSS guarantee.
        o.memory = BEST_DEFAULT_MEMORY;
    }
    if (o.output_format.is_some() || o.input_format.is_some()) && !o.memory_set {
        // Brotli's admitted one-shot workspace is 512 MiB in addition to
        // retained source and bounded output.
        o.memory = 1024 * 1024 * 1024;
    }
    Ok(())
}

pub(crate) fn parse() -> Result<Opt, String> {
    let mut o = default_options();
    let args: Vec<String> = env::args().collect();
    (o.decode, o.stdout) = decoder_program_name(&args);
    let files = parse_command_arguments(&mut o, &args)?;
    finish_parse(&mut o, files)?;
    Ok(o)
}
fn parse_size(s: &str) -> Result<usize, String> {
    let (n, m) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let v: usize = n.parse().map_err(|_| "invalid memory size")?;
    let mul = match m.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024 * 1024,
        "g" | "gb" | "gib" => 1024 * 1024 * 1024,
        _ => return Err("size accepts bytes, KiB, MiB or GiB".into()),
    };
    v.checked_mul(mul)
        .ok_or_else(|| "memory size overflow".into())
}
fn parse_duration(s: &str) -> Result<std::time::Duration, String> {
    let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let value: u64 = n.parse().map_err(|_| "invalid flush interval")?;
    let millis = match unit.to_ascii_lowercase().as_str() {
        "ms" => value,
        "" | "s" | "sec" | "secs" => value.checked_mul(1000).ok_or("flush interval overflow")?,
        "m" | "min" | "mins" => value.checked_mul(60_000).ok_or("flush interval overflow")?,
        _ => return Err("flush interval accepts ms, seconds or minutes".into()),
    };
    if millis == 0 {
        return Err("flush interval must be positive".into());
    }
    Ok(std::time::Duration::from_millis(millis))
}
pub use crate::core::engine::*;

fn selected_output_path(o: &Opt) -> String {
    o.output.clone().unwrap_or_else(|| {
        if let Some(format) = o.output_format {
            if o.input == "-" {
                return "-".into();
            }
            let suffix = match format {
                StandardFormat::Gzip => "gz",
                StandardFormat::Zlib => "zlib",
                StandardFormat::Deflate => "deflate",
                StandardFormat::Bzip2 => "bz2",
                StandardFormat::Xz => "xz",
                StandardFormat::Zstd => "zst",
                StandardFormat::Brotli => "br",
                StandardFormat::Lz4Frame => "lz4",
                StandardFormat::SnappyFramed => "sz",
            };
            return format!("{}.{}", o.input, suffix);
        }
        if o.decode {
            cixf1_original_name(&o.input).unwrap_or_else(|| default_output(&o.input, true))
        } else {
            default_output(&o.input, false)
        }
    })
}

fn path_identity(path: &str) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        let path = Path::new(path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}

/// Reject output plans which could overwrite another requested input before
/// processing the first file.  Individual writes retain `atomic_file`'s
/// no-overwrite check for files created concurrently after this validation.
fn validate_multi_file_plan(o: &Opt) -> Result<(), String> {
    if o.inputs.len() < 2 || o.test || o.list {
        return Ok(());
    }
    let inputs: std::collections::HashSet<_> =
        o.inputs.iter().map(|input| path_identity(input)).collect();
    let mut outputs = std::collections::HashSet::new();
    for input in &o.inputs {
        let mut file_options = o.clone();
        file_options.input.clone_from(input);
        let output = selected_output_path(&file_options);
        if output == "-" {
            return Err("standard-output mode accepts one input at a time".into());
        }
        let output_identity = path_identity(&output);
        if inputs.contains(&output_identity) {
            return Err(format!(
                "derived output path conflicts with an input path: {output}"
            ));
        }
        if !outputs.insert(output_identity) {
            return Err(format!(
                "multiple inputs derive the same output path: {output}"
            ));
        }
    }
    Ok(())
}

fn standard_mode(o: &Opt) -> Option<StandardFormat> {
    if o.decode {
        o.input_format
    } else {
        o.output_format
    }
}

fn validate_standard_options(o: &Opt) -> Result<(), String> {
    if o.output_format.is_some() && o.input_format.is_some() {
        return Err("--output-format and --input-format cannot be combined".into());
    }
    if o.output_format.is_some() && o.decode {
        return Err("--output-format is an encoding option; use --input-format to decode".into());
    }
    if o.input_format.is_some() && !o.decode {
        return Err("--input-format requires decode mode (-d, -t, or uncix)".into());
    }
    if standard_mode(o).is_none() {
        return Ok(());
    }
    if o.list
        || o.format_set
        || o.route.is_some()
        || o.backend_set
        || o.external_backend.is_some()
        || o.portfolio.is_some()
        || o.stream
        || o.flush_interval.is_some()
        || o.history != 0
        || o.independent_blocks
        || o.block.is_some()
        || o.profile_set
        || o.parallelism_set
        || o.threads_set
    {
        return Err("standard formats conflict with CIX container, route, backend, portfolio, stream, history, block, profile, parallelism, or thread overrides".into());
    }
    if o.decode && o.level_set {
        return Err(
            "compression effort options cannot be used while decoding a standard stream".into(),
        );
    }
    Ok(())
}

fn read_standard_input(mut source: Input, memory: usize) -> Result<Vec<u8>, String> {
    let cap = standard_formats::MAX_STANDARD_INPUT;
    let oversized_file = matches!(&source, Input::File(file) if file.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > cap as u64));
    if oversized_file {
        return Err("standard format input exceeds 128 MiB".into());
    }
    if memory == 0 {
        return Err("standard format memory limit must be non-zero".into());
    }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 64 << 10];
    loop {
        let read = source.read(&mut chunk).map_err(engine::ioerr)?;
        if read == 0 {
            break;
        }
        let next = bytes
            .len()
            .checked_add(read)
            .ok_or("standard format input size overflow")?;
        if next > cap {
            return Err("standard format input exceeds 128 MiB".into());
        }
        if next
            .checked_add(bytes.capacity())
            .and_then(|n| n.checked_add(chunk.len()))
            .is_none_or(|n| n > memory)
        {
            return Err("standard format input exceeds memory limit".into());
        }
        bytes
            .try_reserve_exact(read)
            .map_err(|_| "standard input allocation failed")?;
        if bytes.capacity() > memory {
            return Err("standard format input buffer exceeds memory limit".into());
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    Ok(bytes)
}

fn execute_standard<R: Write>(o: &Opt, destination: &mut R) -> Result<(), String> {
    let source = read_standard_input(open_input(&o.input)?, o.memory)?;
    let memory_limit = o
        .memory
        .checked_sub(source.capacity().saturating_sub(source.len()))
        .ok_or("standard input buffer exceeds memory limit")?;
    let output_limit = if o.decode {
        standard_formats::MAX_STANDARD_INPUT
    } else {
        source
            .len()
            .saturating_add(2 << 20)
            .min(standard_formats::MAX_STANDARD_INPUT)
    };
    let options = StandardOptions {
        level: o.level,
        output_limit,
        memory_limit,
    };
    let output = if o.decode {
        standard_formats::decode(
            o.input_format.expect("validated input format"),
            &source,
            &options,
        )
    } else {
        standard_formats::encode(
            o.output_format.expect("validated output format"),
            &source,
            &options,
        )
    }
    .map_err(|error| error.to_string())?;
    destination.write_all(&output).map_err(engine::ioerr)
}

fn execute_standard_to_stdout(o: &Opt) -> Result<(), String> {
    let mut lock = engine::stdout_writer::StdoutWriter::new().map_err(engine::ioerr)?;
    execute_standard(o, &mut lock)
}

fn execute_standard_to_file(o: &Opt, out: &str) -> Result<(), String> {
    if o.input == out {
        return Err("input and output paths must differ".into());
    }
    atomic_file(out, o.force, |destination| execute_standard(o, destination))?;
    eprintln!("{out}");
    Ok(())
}

fn verify_or_list(o: &Opt) -> Result<bool, String> {
    if !(o.test || o.list) {
        return Ok(false);
    }
    let f = open_input(&o.input)?;
    let report = crate::cli_full_engine::decode(o, f, io::sink(), true)?;
    if o.list {
        eprintln!("{}", crate::cli_full_engine::list_line(&report));
    }
    eprintln!("archive OK");
    Ok(true)
}

fn report_automatic_exclusion(o: &Opt, automatic_best: bool) {
    if (o.explain || o.verbose) && !automatic_best {
        if let Some(reason) = portfolio_auto::exclusion_reason(o) {
            eprintln!("cix: effort=best whole_archive=no reason={reason}");
        }
    }
}

fn execute_to_stdout(
    o: &Opt,
    block: u32,
    forced: Option<&str>,
    automatic_best: bool,
    automatic_persistent_best: bool,
) -> Result<(), String> {
    if !o.decode {
        if let Some(report) = crate::cli_full_engine::encode_whole(
            o,
            &mut engine::stdout_writer::StdoutWriter::new().map_err(ioerr)?,
        )? {
            if o.explain || o.verbose {
                crate::cli_full_engine::report_selection(&report);
            }
            return Ok(());
        }
        if (o.explain || o.verbose) && crate::cli_full_engine::encode_ineligibility(o).is_some() {
            eprintln!(
                "cix: full_engine=not_admitted reason={}",
                crate::cli_full_engine::encode_ineligibility(o).unwrap()
            );
        }
        if crate::cli_full_engine::window_encode_ineligibility(o).is_none() {
            let src = open_input(&o.input)?;
            let mut lock = engine::stdout_writer::StdoutWriter::new().map_err(ioerr)?;
            if let Some(summary) = crate::cli_full_engine::encode_windows(o, src, &mut lock)? {
                if o.explain || o.verbose {
                    crate::cli_full_engine::report_window_summary(&summary);
                }
                return Ok(());
            }
        }
    }
    let src = open_input(&o.input)?;
    let fd = input_fd(&src);
    let mut lock = engine::stdout_writer::StdoutWriter::new().map_err(ioerr)?;
    if o.decode {
        crate::cli_full_engine::decode(o, src, &mut lock, false).map(|_| ())
    } else {
        encode_selected(
            src,
            &mut lock,
            EncodeSelection {
                o,
                block,
                forced,
                input_fd: fd,
                automatic_best,
                automatic_persistent_best,
            },
        )
    }
}

fn execute_to_file(
    o: &Opt,
    out: &str,
    block: u32,
    forced: Option<&str>,
    automatic_best: bool,
    automatic_persistent_best: bool,
) -> Result<(), String> {
    if o.input == out {
        return Err("input and output paths must differ".into());
    }
    atomic_file(out, o.force, |dst| {
        if !o.decode {
            if let Some(report) = crate::cli_full_engine::encode_whole(o, dst)? {
                if o.explain || o.verbose {
                    crate::cli_full_engine::report_selection(&report);
                }
                return Ok(());
            }
            if (o.explain || o.verbose) && crate::cli_full_engine::encode_ineligibility(o).is_some()
            {
                eprintln!(
                    "cix: full_engine=not_admitted reason={}",
                    crate::cli_full_engine::encode_ineligibility(o).unwrap()
                );
            }
            if crate::cli_full_engine::window_encode_ineligibility(o).is_none() {
                let src = open_input(&o.input)?;
                if let Some(summary) = crate::cli_full_engine::encode_windows(o, src, dst)? {
                    if o.explain || o.verbose {
                        crate::cli_full_engine::report_window_summary(&summary);
                    }
                    return Ok(());
                }
            }
        }
        let src = open_input(&o.input)?;
        let fd = input_fd(&src);
        if o.decode {
            crate::cli_full_engine::decode(o, src, dst, false).map(|_| ())
        } else {
            encode_selected(
                src,
                dst,
                EncodeSelection {
                    o,
                    block,
                    forced,
                    input_fd: fd,
                    automatic_best,
                    automatic_persistent_best,
                },
            )
        }
    })?;
    eprintln!("{out}");
    Ok(())
}

fn run_one(o: &mut Opt) -> Result<(), String> {
    if standard_mode(o).is_some() {
        if o.test {
            execute_standard(o, &mut io::sink())?;
            eprintln!("archive OK");
            return Ok(());
        }
        let out = selected_output_path(o);
        if o.stdout || out == "-" {
            execute_standard_to_stdout(o)?;
        } else {
            execute_standard_to_file(o, &out)?;
        }
        return Ok(());
    }
    let block = prepare_block(o)?;
    let forced = o.route.as_deref().filter(|r| *r != "auto");
    validate_candidate_selection(o, forced)?;
    let out = selected_output_path(o);
    if verify_or_list(o)? {
        return Ok(());
    }
    let automatic_best = portfolio_auto::eligible(o);
    let automatic_persistent_best = automatic_persistent_stream_eligible(o, automatic_best, block);
    report_automatic_exclusion(o, automatic_best);
    if o.stdout || out == "-" {
        execute_to_stdout(o, block, forced, automatic_best, automatic_persistent_best)?;
    } else {
        execute_to_file(
            o,
            &out,
            block,
            forced,
            automatic_best,
            automatic_persistent_best,
        )?;
    }
    let _ = (o.keep, o.stream);
    Ok(())
}

fn run() -> Result<(), String> {
    let mut o = parse()?;
    validate_options(&mut o)?;
    validate_multi_file_plan(&o)?;
    let mut failures = 0usize;
    for input in o.inputs.clone() {
        let mut file_options = o.clone();
        file_options.input = input.clone();
        if let Err(error) = run_one(&mut file_options) {
            failures += 1;
            eprintln!("cix: {input}: {error}");
        }
    }
    if failures == 0 {
        Ok(())
    } else {
        Err(format!(
            "{failures} of {} input files failed",
            o.inputs.len()
        ))
    }
}

pub fn main_entry() {
    let cancellation = cancellation_token();
    if let Err(e) = ctrlc::set_handler(move || {
        engine::limits::INTERRUPTED.store(true, Ordering::Relaxed);
        cancellation.store(true, Ordering::Relaxed);
    }) {
        eprintln!("cix: could not install interrupt handler: {e}");
        process::exit(2);
    }
    if let Err(e) = run() {
        eprintln!("cix: {e}");
        process::exit(2)
    }
}

/// Full-engine-only route trial.  The SDK never installs this callback, so a
/// buffer/stream encode cannot discover the executable or create a child.
pub(crate) fn portfolio_cix_candidate(
    data: &[u8],
    level: u8,
    memory: usize,
    retained_bytes: usize,
    route: Option<&str>,
    timeout_seconds: u64,
) -> Result<Option<Vec<u8>>, String> {
    use std::process::{Command, Stdio};
    let input_len = data.len();
    let output_cap = input_len.saturating_mul(2).saturating_add(2 * 1024 * 1024);
    let child_memory = memory
        .checked_sub(retained_bytes)
        .and_then(|value| value.checked_sub(input_len))
        .and_then(|value| value.checked_sub(output_cap));
    let Some(child_memory) = child_memory else {
        return Ok(None);
    };
    if child_memory < 128 * 1024 {
        return Ok(None);
    }
    let executable = env::current_exe().map_err(engine::ioerr)?;
    let mut command = Command::new(executable);
    command
        .arg(format!("-{level}"))
        .args(["--format", "cixg1", "--stream", "--block-size", "65536B"])
        .arg("--memory")
        .arg(format!("{child_memory}B"))
        .args(["--threads", "1", "-c"]);
    if let Some(route) = route {
        command.args(["--route", route]);
    }
    command
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(engine::ioerr)?;
    let mut child_stdin = child
        .stdin
        .take()
        .ok_or("portfolio child stdin unavailable")?;
    let input = data.to_vec();
    let writer = thread::spawn(move || child_stdin.write_all(&input));
    let child_stdout = child
        .stdout
        .take()
        .ok_or("portfolio child stdout unavailable")?;
    let reader = thread::spawn(move || {
        let mut out = Vec::with_capacity(input_len.saturating_add(1024));
        child_stdout
            .take(output_cap as u64)
            .read_to_end(&mut out)
            .map(|_| out)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_seconds);
    let status = loop {
        if let Err(error) = engine::check_interrupted() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = writer.join();
            let _ = reader.join();
            return Err(error);
        }
        if let Some(status) = child.try_wait().map_err(engine::ioerr)? {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(std::time::Duration::from_millis(10));
    };
    let _ = writer.join();
    let output = reader
        .join()
        .map_err(|_| "portfolio child output thread panicked")?
        .map_err(engine::ioerr)?;
    Ok(match status {
        Some(status) if status.success() && !output.is_empty() && output.len() < output_cap => {
            Some(output)
        }
        _ => None,
    })
}
pub(crate) fn cixf1_original_name(input: &str) -> Option<String> {
    if input == "-" {
        return None;
    }
    let mut file = File::open(input).ok()?;
    let end = file.seek(SeekFrom::End(0)).ok()?;
    if end < 9 || file.seek(SeekFrom::End(-9)).ok().is_none() {
        return None;
    }
    let mut footer = [0u8; 9];
    file.read_exact(&mut footer).ok()?;
    if &footer[4..] != b"CIXF1" {
        return None;
    }
    let len = u32::from_le_bytes(footer[..4].try_into().ok()?) as u64;
    if len > 1024 * 1024 || len + 9 > end {
        return None;
    }
    file.seek(SeekFrom::Start(end - len - 9)).ok()?;
    let mut name = vec![0u8; len as usize];
    file.read_exact(&mut name).ok()?;
    let name = std::str::from_utf8(&name).ok()?;
    let base = Path::new(name).file_name()?.to_str()?;
    if base.is_empty() || base == "." || base == ".." {
        return None;
    }
    let parent = Path::new(input).parent().unwrap_or_else(|| Path::new("."));
    Some(parent.join(base).to_string_lossy().into_owned())
}

pub(crate) fn default_output(input: &str, dec: bool) -> String {
    if input == "-" {
        return "-".into();
    }
    let p = Path::new(input);
    if dec {
        if p.extension().map(|x| x == "cix").unwrap_or(false) {
            let mut q = p.to_path_buf();
            q.set_extension("");
            q.to_string_lossy().into_owned()
        } else {
            format!("{input}.out")
        }
    } else {
        format!("{input}.cix")
    }
}
pub(crate) fn atomic_file(
    path: &str,
    force: bool,
    run: impl FnOnce(&mut File) -> Result<(), String>,
) -> Result<(), String> {
    let p = Path::new(path);
    if fs::symlink_metadata(p).is_ok() && !force {
        return Err(format!("refusing to overwrite {path} (use -f)"));
    }
    let parent = p.parent().unwrap_or(Path::new("."));
    let name = p.file_name().unwrap_or_default().to_string_lossy();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = parent.join(format!(".{name}.cix-tmp-{}-{stamp}", process::id()));
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(ioerr)?;
    let result = run(&mut f).and_then(|()| check_interrupted());
    if result.is_ok() {
        if let Err(e) = f.sync_all() {
            drop(f);
            let _ = fs::remove_file(&tmp);
            return Err(ioerr(e));
        }
    }
    drop(f);
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if force {
        fs::rename(&tmp, p).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            ioerr(e)
        })?;
    } else {
        fs::hard_link(&tmp, p).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            if e.kind() == io::ErrorKind::AlreadyExists {
                format!("refusing to overwrite {path} (use -f)")
            } else {
                ioerr(e)
            }
        })?;
        fs::remove_file(&tmp).map_err(ioerr)?;
    }
    if let Some(dir) = p.parent() {
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}
fn block_min(o: &Opt) -> usize {
    o.block.unwrap_or(default_block(o.level)).min(MAX_BLOCK) as usize
}

pub(crate) fn automatic_persistent_stream_eligible(
    o: &Opt,
    whole_archive_best: bool,
    block: u32,
) -> bool {
    o.level >= 9
        && !whole_archive_best
        && !o.decode
        && !o.test
        && !o.list
        && o.external_backend.is_none()
        && o.portfolio.is_none()
        && o.route.as_deref().is_none_or(|route| route == "auto")
        && !o.backend_set
        && o.format == "auto"
        && o.history == 0
        && !o.independent_blocks
        && (!o.parallelism_set || o.parallelism == "candidates")
        && engine::persistent_stream_required_memory(block) <= o.memory
}

struct PersistentStreamOptions {
    block: u32,
    input_fd: i32,
    flush_interval: Option<Duration>,
    memory: usize,
    threads: usize,
    explain: bool,
    verbose: bool,
}

/// Commit an admitted non-seekable BEST source through the codec primitive.
fn encode_automatic_persistent_stream<R: Read, W: Write>(
    src: R,
    dst: W,
    options: PersistentStreamOptions,
) -> Result<(), String> {
    engine::encode_persistent_stream(
        src,
        dst,
        engine::PersistentStreamRequest {
            block: options.block,
            input_fd: options.input_fd,
            flush_interval: options.flush_interval,
            memory: options.memory,
            workers: options.threads,
            explain: options.explain,
            verbose: options.verbose,
        },
    )
}

/// Execute the selected encoder after option validation. Keeping this dispatch
/// separate from filesystem handling means stdout and atomic-file output use
/// precisely the same encoder selection rules.
pub(crate) struct EncodeSelection<'a> {
    pub(crate) o: &'a Opt,
    pub(crate) block: u32,
    pub(crate) forced: Option<&'a str>,
    pub(crate) input_fd: i32,
    pub(crate) automatic_best: bool,
    pub(crate) automatic_persistent_best: bool,
}

pub(crate) fn encode_selected<R: Read, W: Write>(
    src: R,
    dst: W,
    selection: EncodeSelection<'_>,
) -> Result<(), String> {
    let EncodeSelection {
        o,
        block,
        forced,
        input_fd,
        automatic_best,
        automatic_persistent_best,
    } = selection;
    if automatic_best {
        return portfolio_auto::encode(o, dst);
    }
    if o.format == "cixm6" || o.format == "cixf1" {
        let name = cixm6_file_name(o)?;
        return encode_m6_timed(
            src,
            dst,
            block as usize,
            name,
            o.level,
            o.memory,
            M6ReadOptions {
                input_fd,
                flush_interval: o.flush_interval,
            },
        );
    }
    if let Some(profile) = o.portfolio.as_deref() {
        return encode_portfolio(src, dst, profile, o.memory, &portfolio_cix_candidate);
    }
    if let Some(backend) = o.external_backend.as_deref() {
        return encode_external(src, dst, backend, &o.profile, o.memory);
    }
    if automatic_persistent_best {
        return encode_automatic_persistent_stream(
            src,
            dst,
            PersistentStreamOptions {
                block,
                input_fd,
                flush_interval: o.flush_interval,
                memory: o.memory,
                threads: o.threads,
                explain: o.explain,
                verbose: o.verbose,
            },
        );
    }
    encode_with_strategy(
        src,
        dst,
        EncodeOptions {
            block,
            level: o.level,
            forced,
            backend: &o.backend,
            backend_set: o.backend_set,
            format: &o.format,
            input_fd,
            flush_interval: o.flush_interval,
            memory: o.memory,
            workers: o.threads,
            explain: o.explain,
            verbose: o.verbose,
            strategy: &o.parallelism,
        },
    )
}

fn cixm6_file_name(o: &Opt) -> Result<Option<&str>, String> {
    if o.format != "cixf1" {
        return Ok(None);
    }
    Path::new(&o.input)
        .file_name()
        .and_then(|name| name.to_str())
        .map(Some)
        .ok_or_else(|| "input basename is not valid UTF-8".into())
}

fn validate_execution_mode(o: &mut Opt) -> Result<(), String> {
    if !matches!(o.parallelism.as_str(), "blocks" | "candidates") {
        return Err("--parallelism accepts blocks or candidates".into());
    }
    if !o.threads_set
        && (o.decode
            || o.external_backend.is_some()
            || o.portfolio.is_some()
            || !matches!(o.format.as_str(), "auto" | "cixg1" | "cixg2"))
    {
        o.threads = 1;
    }
    if o.decode
        && (o.route.is_some()
            || o.backend_set
            || o.block.is_some()
            || o.independent_blocks
            || o.level_set
            || o.profile_set
            || o.parallelism_set)
    {
        return Err("compression options cannot be used while decoding".into());
    }
    Ok(())
}

fn apply_native_profile(o: &mut Opt) -> Result<(), String> {
    if matches!(o.format.as_str(), "cixm6" | "cixf1") && (o.independent_blocks || o.backend_set) {
        return Err("CIXM6 does not support --independent-blocks or --backend overrides".into());
    }
    if (o.external_backend.is_some() || o.portfolio.is_some())
        && (o.format != "auto" || o.backend_set || o.block.is_some() || o.independent_blocks)
    {
        return Err("external/portfolio encoding does not support --format, --backend, --block-size or --independent-blocks overrides".into());
    }
    if o.profile_set && o.level_set {
        return Err("--profile and -1..-9/--fast/--best specify the same effort; use one".into());
    }
    if o.profile_set && o.external_backend.is_none() && o.portfolio.is_none() {
        if !matches!(o.format.as_str(), "auto" | "cixg1" | "cixg2") {
            return Err("native --profile currently applies to CIXG1 auto selection".into());
        }
        o.level = match o.profile.as_str() {
            "fast" => 1,
            "default" | "current" => 6,
            "size" => 9,
            _ => return Err("native --profile accepts fast, default or size".into()),
        };
    }
    Ok(())
}

fn validate_threads_and_format(o: &mut Opt) -> Result<(), String> {
    // BEST searches memory-intensive native backends as well as block routes.
    // An explicit resource constraint is authoritative; raising search effort
    // alone must not silently exceed a caller's declared memory budget.
    if o.level >= 9 && !o.memory_set && !o.decode && !o.test && !o.list {
        o.memory = BEST_DEFAULT_MEMORY;
    }
    if o.threads > 20 {
        return Err("--threads is limited to 20 workers".into());
    }
    if o.threads > 1
        && (o.decode
            || !matches!(o.format.as_str(), "auto" | "cixg1" | "cixg2")
            || o.portfolio.is_some()
            || o.external_backend.is_some())
    {
        return Err(
            "--threads >1 applies to native CIXG1/CIXG2 block or candidate selection".into(),
        );
    }
    if o.memory < 128 * 1024 {
        return Err("--memory must be at least 128 KiB for a CIXG1 frame".into());
    }
    if !matches!(
        o.format.as_str(),
        "auto" | "cixg1" | "cixg2" | "cixm6" | "cixf1"
    ) {
        return Err("--format accepts auto, cixg1, cixg2, cixm6 or cixf1".into());
    }
    if matches!(o.format.as_str(), "cixm6" | "cixf1")
        && (o.decode
            || o.test
            || o.list
            || o.external_backend.is_some()
            || o.portfolio.is_some()
            || o.route.is_some())
    {
        return Err(
            "--format cixm6 conflicts with decode, portfolio, external backend and route selection"
                .into(),
        );
    }
    Ok(())
}

fn validate_m6_options(o: &Opt) -> Result<(), String> {
    if o.format == "cixf1" && o.input == "-" {
        return Err("--format cixf1 requires a named input to store its basename".into());
    }
    if matches!(o.format.as_str(), "cixm6" | "cixf1") && o.parallelism_set {
        return Err("--parallelism is not supported by CIXM6/CIXF1 encoding".into());
    }
    if matches!(o.format.as_str(), "cixm6" | "cixf1")
        && o.memory < (1 << 20) + block_min(o).saturating_mul(3) + (256 << 10)
    {
        return Err(
            "CIXM6 encoder needs --memory for history, source and candidate buffers".into(),
        );
    }
    Ok(())
}

fn validate_portfolio_options(o: &Opt) -> Result<(), String> {
    if let Some(p) = o.portfolio.as_deref() {
        if o.decode
            || o.test
            || o.list
            || o.external_backend.is_some()
            || o.route.is_some()
            || o.profile_set
        {
            return Err(
                "--portfolio conflicts with decode, --external, --route and --profile".into(),
            );
        }
        if o.stream || o.flush_interval.is_some() {
            return Err(
                "bounded portfolio selection conflicts with --stream/--flush-interval".into(),
            );
        }
        if !matches!(p, "fast" | "default" | "current" | "size") {
            return Err("--portfolio accepts fast, default or size".into());
        }
    }
    Ok(())
}

fn validate_external_options(o: &Opt) -> Result<(), String> {
    if o.external_backend.is_some() {
        if o.decode || o.test || o.list {
            return Err("--external is an encoding option".into());
        }
        if o.stream || o.flush_interval.is_some() {
            return Err(
                "bounded CIXB1 external encoding conflicts with --stream/--flush-interval".into(),
            );
        }
        if o.input == "-" {
            return Err("CIXB1 external profiles require a named, bounded input file".into());
        }
        if o.route.is_some() {
            return Err("--external conflicts with --route".into());
        }
        if !matches!(o.profile.as_str(), "fast" | "default" | "current" | "size") {
            return Err("--profile accepts fast, default or size".into());
        }
    }
    Ok(())
}

fn validate_route_memory(o: &Opt) -> Result<(), String> {
    if o.route.as_deref() == Some("lz")
        && !(o.level <= 2 && (!o.backend_set || o.backend == "deflate"))
        && o.memory < 16 * 1024 * 1024
    {
        return Err(
            "--route lz requires --memory of at least 16 MiB for its bounded match index".into(),
        );
    }
    if o.route.as_deref() == Some("ppm") && o.memory < 64 * 1024 * 1024 {
        return Err(
            "--route ppm requires --memory of at least 64 MiB for bounded context tables".into(),
        );
    }
    if o.route.as_deref() == Some("mixture") && o.memory < 16 * 1024 * 1024 {
        return Err("--route mixture requires at least 16 MiB of --memory".into());
    }
    Ok(())
}

fn validate_route_state(o: &Opt) -> Result<(), String> {
    if o.history != 0 {
        if o.independent_blocks {
            return Err("--history conflicts with --independent-blocks".into());
        }
        return Err("persistent history is not supported by the current native routes".into());
    }
    if o.decode && o.flush_interval.is_some() {
        return Err("--flush-interval applies to encoding only".into());
    }
    if o.flush_interval.is_some() && !cfg!(unix) {
        return Err("--flush-interval requires Unix poll support on this platform".into());
    }
    Ok(())
}

fn validate_route_name(o: &Opt) -> Result<(), String> {
    if let Some(r) = &o.route {
        if ![
            "auto",
            "raw",
            "runs",
            "composition",
            "lz",
            "predictor",
            "bwt",
            "stride",
            "phrases",
            "ppm",
            "mixture",
            "deflate",
        ]
        .contains(&r.as_str())
        {
            return Err(format!("unsupported native route: {r}"));
        }
    }
    Ok(())
}

pub(crate) fn validate_options(o: &mut Opt) -> Result<(), String> {
    validate_standard_options(o)?;
    if standard_mode(o).is_some() {
        return Ok(());
    }
    validate_execution_mode(o)?;
    apply_native_profile(o)?;
    validate_threads_and_format(o)?;
    validate_m6_options(o)?;
    validate_portfolio_options(o)?;
    validate_external_options(o)?;
    validate_route_memory(o)?;
    validate_route_state(o)?;
    validate_route_name(o)
}

pub(crate) fn prepare_block(o: &mut Opt) -> Result<u32, String> {
    let requested_block = o.block.unwrap_or(default_block(o.level));
    if requested_block == 0 {
        return Err("--block-size must be positive".into());
    }
    let block = requested_block.min(MAX_BLOCK);
    if o.route.as_deref() == Some("mixture") {
        let need = (block as usize)
            .saturating_mul(4096)
            .saturating_add(8 * 1024 * 1024);
        if need > o.memory {
            return Err(format!(
                "mixture route needs about {need} bytes; reduce --block-size or raise --memory"
            ));
        }
    }
    if requested_block > MAX_BLOCK {
        eprintln!("cix: CIXG1 limits frames to 65536 bytes; effective block size is 65536 bytes");
    }
    o.threads = automatic_workers(o.threads, o.threads_set, block, o.memory);
    let estimated_working = (block as usize)
        .saturating_mul(8usize.saturating_mul(o.threads.max(1)))
        .saturating_add(65536usize.saturating_mul(o.threads.max(1)));
    if !o.decode && estimated_working > o.memory {
        return Err(format!("block needs about {estimated_working} bytes working memory; reduce --block-size or raise --memory"));
    }
    Ok(block)
}

pub(crate) fn validate_candidate_selection(o: &Opt, forced: Option<&str>) -> Result<(), String> {
    if !o.decode
        && o.external_backend.is_none()
        && o.portfolio.is_none()
        && matches!(o.format.as_str(), "auto" | "cixg1" | "cixg2")
    {
        let profile = selection::profile(&[], 1);
        let route =
            forced.map(|name| selection::ROUTES.iter().position(|r| *r == name).unwrap() as u8);
        selection::generate_candidates(
            &profile,
            selector_effort_name(o.level),
            route,
            o.backend_set.then_some(o.backend.as_str()),
        )?;
        if o.format == "cixg1"
            && o.backend_set
            && (o.backend.starts_with("context-range-") || o.backend == "huffman")
        {
            return Err(
                "versioned substream coder requires --format cixg2 or --format auto".into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    fn options(args: &[&str], decode: bool) -> Opt {
        let mut options = default_options();
        options.decode = decode;
        let args = args
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        let files = parse_command_arguments(&mut options, &args).expect("parse options");
        finish_parse(&mut options, files).expect("finish parse");
        validate_options(&mut options).expect("validate options");
        options
    }

    #[test]
    fn implicit_best_and_decode_memory_admit_paq_while_explicit_caps_win() {
        let best = options(&["cix", "--best", "input"], false);
        assert_eq!(best.memory, BEST_DEFAULT_MEMORY);
        assert!(!best.memory_set);

        let decode = options(&["uncix", "archive"], true);
        assert_eq!(decode.memory, BEST_DEFAULT_MEMORY);
        assert!(!decode.memory_set);

        let explicit = options(&["cix", "--best", "--memory", "768MiB", "input"], false);
        assert_eq!(explicit.memory, 768 << 20);
        assert!(explicit.memory_set);

        let explicit_decode = options(&["uncix", "--memory", "768MiB", "archive"], true);
        assert_eq!(explicit_decode.memory, 768 << 20);
        assert!(explicit_decode.memory_set);
    }

    #[test]
    fn program_name_normalization_is_platform_specific() {
        assert_eq!(normalized_program_name("uncix.ExE", true), "uncix");
        assert_eq!(normalized_program_name("CIXCAT.eXe", true), "cixcat");
        assert_eq!(normalized_program_name("UNCIX", true), "uncix");
        assert_eq!(normalized_program_name("uncix.exe", false), "uncix.exe");
        assert_eq!(normalized_program_name("Uncix", false), "Uncix");
    }
}
