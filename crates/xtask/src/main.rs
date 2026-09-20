//! `cargo xtask`: the one host-side entry point of the build pipeline.
//!
//! * `cargo xtask resident --output <fresh-dir> [--low-runtime]`
//!   builds, links, packages and audits the resident payload and its DXE shim.
//! * `cargo xtask sources` prints the current source manifest (the same JSON a
//!   build records as `source-manifest.json`).
//! * `cargo xtask card-dev [--any-runtime] [--flash] [--adapter-khz N]`,
//!   `cargo xtask card-snapshot [read_snapshot.py options]` and
//!   `cargo xtask card-loader-dev`: the development-loader iteration loop
//!   (see `card.rs` and firmware/card/README.md).
//! * `cargo xtask rompack --input <driver.efi> --output <driver.rom> ...` wraps
//!   a loader image in its PCI option ROM (see `rompack.rs`).

use std::{path::PathBuf, process::ExitCode};

mod audit;
mod card;
mod json;
mod relocations;
mod resident;
mod rompack;

const USAGE: &str = "usage: cargo xtask resident --output <fresh-dir> [--low-runtime]\n       cargo xtask sources\n       \
cargo xtask card-dev [--any-runtime] [--flash] [--adapter-khz N]\n       \
cargo xtask card-snapshot [--input <log>] [--manifest <manifest.json>]\n       \
cargo xtask card-loader-dev\n       \
cargo xtask rompack --input <driver.efi> --output <driver.rom> --vendor <hex> --device <hex> \
--class <hex> [--memory-output <driver.mem> --memory-size <bytes>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("resident") => {
            let (mut output, mut low_runtime) = (None, false);
            let mut rest = args[1..].iter();
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--output" => {
                        output =
                            Some(PathBuf::from(rest.next().ok_or("--output needs a directory")?))
                    }
                    "--low-runtime" => low_runtime = true,
                    // The only profile; accepted for existing command lines.
                    "--boot" => {}
                    other => match other.strip_prefix("--output=") {
                        Some(path) => output = Some(PathBuf::from(path)),
                        None => return Err(format!("unknown argument {other}\n{USAGE}")),
                    },
                }
            }
            resident::build(&output.ok_or(format!("--output is required\n{USAGE}"))?, low_runtime)
        }
        Some("sources") if args.len() == 1 => {
            println!("{}", resident::manifest_json(&resident::sources(&resident::root())?));
            Ok(())
        }
        Some("card-dev") => {
            // This repository targets one board, whose firmware only retains a
            // runtime allocation below 1 GiB: low-runtime is the default here.
            let (mut low_runtime, mut flash, mut adapter_khz) = (true, false, 1000);
            let mut rest = args[1..].iter();
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--low-runtime" => low_runtime = true,
                    "--any-runtime" => low_runtime = false,
                    // The only switch that reaches hardware; never implied.
                    "--flash" => flash = true,
                    "--adapter-khz" => {
                        adapter_khz = card::parse_adapter_khz(
                            rest.next().ok_or("--adapter-khz needs a number")?,
                        )?
                    }
                    other => match other.strip_prefix("--adapter-khz=") {
                        Some(khz) => adapter_khz = card::parse_adapter_khz(khz)?,
                        None => return Err(format!("unknown argument {other}\n{USAGE}")),
                    },
                }
            }
            card::dev(low_runtime, flash, adapter_khz)
        }
        Some("card-snapshot") => card::snapshot(&args[1..]),
        Some("card-loader-dev") if args.len() == 1 => card::loader_dev(),
        Some("rompack") => rompack::command(&args[1..]),
        _ => Err(USAGE.into()),
    }
}
