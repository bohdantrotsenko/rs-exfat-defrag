use std::env;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rs_exfat_defrag::{DefragOptions, Volume, analyze_filtered, defrag};

fn main() -> ExitCode {
    let mut args = env::args();
    let argv0 = args.next().unwrap_or_else(|| "rs-exfat-defrag".into());
    match parse_args(&argv0, args) {
        Err(code) => code,
        Ok(cfg) => match run(cfg) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        },
    }
}

struct Config {
    path: String,
    defrag: bool,
    min_chunk: Option<u64>,
    dir_only: bool,
    dir_first: bool,
}

fn usage(argv0: &str) -> String {
    format!(
        "Usage: {argv0} [options] <filesystem>\n\
         \n\
         Options:\n\
         --defrag         rewrite allocations to reduce fragmentation\n\
         --chunk SIZE     keep extents of at least SIZE (e.g. 1M); smaller fragments are merged\n\
         --dir            only process directories\n\
         --dirfirst       pack directories at the start of the cluster heap (requires --defrag)"
    )
}

fn parse_args(argv0: &str, args: env::Args) -> Result<Config, ExitCode> {
    let mut path = None;
    let mut do_defrag = false;
    let mut min_chunk = None;
    let mut dir_only = false;
    let mut dir_first = false;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                eprintln!("{}", usage(argv0));
                return Err(ExitCode::SUCCESS);
            }
            "--defrag" => do_defrag = true,
            "--dir" => dir_only = true,
            "--dirfirst" => dir_first = true,
            "--chunk" => {
                let Some(sz) = args.next() else {
                    eprintln!("--chunk requires a size (e.g. 1M)");
                    eprintln!("{}", usage(argv0));
                    return Err(ExitCode::from(2));
                };
                match parse_size(&sz) {
                    Ok(n) => min_chunk = Some(n),
                    Err(e) => {
                        eprintln!("{e}");
                        return Err(ExitCode::from(2));
                    }
                }
            }
            other if let Some(sz) = other.strip_prefix("--chunk=") => match parse_size(sz) {
                Ok(n) => min_chunk = Some(n),
                Err(e) => {
                    eprintln!("{e}");
                    return Err(ExitCode::from(2));
                }
            },
            other if other.starts_with('-') => {
                eprintln!("unknown option: {other}");
                eprintln!("{}", usage(argv0));
                return Err(ExitCode::from(2));
            }
            other if path.is_none() => path = Some(other.to_string()),
            other => {
                eprintln!("unexpected argument: {other}");
                eprintln!("{}", usage(argv0));
                return Err(ExitCode::from(2));
            }
        }
    }
    let Some(path) = path else {
        eprintln!("{}", usage(argv0));
        return Err(ExitCode::from(2));
    };
    if min_chunk.is_some() && !do_defrag {
        eprintln!("--chunk requires --defrag");
        return Err(ExitCode::from(2));
    }
    if dir_first && !do_defrag {
        eprintln!("--dirfirst requires --defrag");
        return Err(ExitCode::from(2));
    }
    Ok(Config {
        path,
        defrag: do_defrag,
        min_chunk,
        dir_only,
        dir_first,
    })
}

fn run(cfg: Config) -> rs_exfat_defrag::Result<()> {
    if cfg.defrag {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_h = stop.clone();
        if let Err(e) = ctrlc::set_handler(move || {
            if !stop_h.swap(true, Ordering::SeqCst) {
                eprintln!("stopping work");
            }
        }) {
            eprintln!("warning: could not install Ctrl+C handler: {e}");
        }
        let mut vol = Volume::open_read_write(&cfg.path)?;
        let stats = defrag(
            &mut vol,
            &DefragOptions {
                min_chunk: cfg.min_chunk,
                dir_only: cfg.dir_only,
                dir_first: cfg.dir_first,
            },
            &stop,
        )?;
        println!(
            "defrag finished: moved {} objects, skipped {}",
            stats.moved, stats.skipped
        );
        if stats.stopped {
            println!("stopped after the current file or directory");
        }
        drop(vol);
    }

    let vol = Volume::open(&cfg.path)?;
    let entries = analyze_filtered(&vol, cfg.dir_only)?;
    println!(
        "{:4} {:>12} {:>6}  {:<40}  PATH",
        "TYPE", "SIZE", "CHUNKS", "CHUNK SIZES"
    );
    for e in &entries {
        let kind = if e.is_directory { "dir" } else { "file" };
        let sizes = e
            .chunk_sizes
            .iter()
            .map(|&s| format_size(s))
            .collect::<Vec<_>>()
            .join("+");
        println!(
            "{kind:4} {:>12} {:>6}  {sizes:<40}  {}",
            e.size,
            e.chunks(),
            e.path
        );
    }
    Ok(())
}

/// Parse sizes like `512`, `16K`, `1M`, `1GiB`.
fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty size".into());
    }
    let mut i = 0usize;
    let bytes = s.as_bytes();
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return Err(format!("invalid size: {s}"));
    }
    let n: u64 = s[..i].parse().map_err(|_| format!("invalid size: {s}"))?;
    let mut unit = s[i..].trim().to_ascii_uppercase();
    if let Some(stripped) = unit.strip_suffix('B') {
        unit = stripped.to_string();
    }
    if let Some(stripped) = unit.strip_suffix('I') {
        unit = stripped.to_string();
    }
    let mul: u64 = match unit.as_str() {
        "" => 1,
        "K" => 1024,
        "M" => 1024 * 1024,
        "G" => 1024 * 1024 * 1024,
        "T" => 1024u64 * 1024 * 1024 * 1024,
        _ => return Err(format!("invalid size unit in {s}")),
    };
    n.checked_mul(mul)
        .ok_or_else(|| format!("size {s} overflows"))
}

/// Powers of two use binary units (`16K`, `1M`); anything else stays a byte count.
fn format_size(n: u64) -> String {
    if n == 0 || !n.is_power_of_two() {
        return n.to_string();
    }
    const UNITS: [&str; 7] = ["B", "K", "M", "G", "T", "P", "E"];
    let exp = n.trailing_zeros() / 10;
    let exp = exp.min((UNITS.len() - 1) as u32);
    let value = n >> (10 * exp);
    if exp == 0 {
        format!("{n}B")
    } else {
        format!("{value}{}", UNITS[exp as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::{format_size, parse_size};

    #[test]
    fn power_of_two_sizes_use_binary_units() {
        assert_eq!(format_size(0), "0");
        assert_eq!(format_size(1), "1B");
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(1024), "1K");
        assert_eq!(format_size(2048), "2K");
        assert_eq!(format_size(16384), "16K");
        assert_eq!(format_size(1 << 20), "1M");
        assert_eq!(format_size(1 << 30), "1G");
        assert_eq!(format_size(904), "904");
        assert_eq!(format_size(49152), "49152");
        assert_eq!(format_size(98304), "98304");
    }

    #[test]
    fn parse_human_sizes() {
        assert_eq!(parse_size("512").unwrap(), 512);
        assert_eq!(parse_size("16K").unwrap(), 16 * 1024);
        assert_eq!(parse_size("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("1MiB").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("2G").unwrap(), 2 * 1024 * 1024 * 1024);
    }
}
