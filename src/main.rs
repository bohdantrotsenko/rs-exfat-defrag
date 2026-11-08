use std::env;
use std::process::ExitCode;

use rs_exfat_defrag::{Volume, analyze};

fn main() -> ExitCode {
    let mut args = env::args();
    let argv0 = args.next().unwrap_or_else(|| "rs-exfat-defrag".into());
    let Some(path) = args.next() else {
        eprintln!("Usage: {argv0} <filesystem>");
        return ExitCode::from(2);
    };
    if let Some(extra) = args.next() {
        eprintln!("unexpected argument: {extra}");
        eprintln!("Usage: {argv0} <filesystem>");
        return ExitCode::from(2);
    }

    match run(&path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(path: &str) -> rs_exfat_defrag::Result<()> {
    let vol = Volume::open(path)?;
    let entries = analyze(&vol)?;

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
    use super::format_size;

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
}
