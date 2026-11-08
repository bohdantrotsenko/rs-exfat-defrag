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

    println!("{:4} {:>12} {:>6}  PATH", "TYPE", "SIZE", "CHUNKS");
    for e in &entries {
        let kind = if e.is_directory { "dir" } else { "file" };
        println!("{kind:4} {:>12} {:>6}  {}", e.size, e.chunks, e.path);
    }
    Ok(())
}
