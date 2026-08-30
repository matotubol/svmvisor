use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::PathBuf;

use svmvisor_rompack::{RomConfig, build_readmemh, build_uefi_option_rom};

fn main() {
    if let Err(error) = run() {
        eprintln!("rompack: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let options = Options::parse(env::args().skip(1))?;
    let efi_image = fs::read(&options.input)?;
    let rom = build_uefi_option_rom(
        &efi_image,
        RomConfig {
            vendor_id: options.vendor_id,
            device_id: options.device_id,
            class_code: options.class_code,
        },
    )?;

    let memory = match (&options.memory_output, options.memory_size) {
        (Some(_), Some(memory_size)) => Some(build_readmemh(&rom, memory_size)?),
        (None, None) => None,
        _ => {
            return Err(invalid_input(
                "--memory-output and --memory-size must be provided together",
            )
            .into());
        }
    };

    fs::write(&options.output, &rom)?;
    println!("{} bytes -> {}", rom.len(), options.output.display());
    if let (Some(memory_output), Some(memory)) = (&options.memory_output, memory) {
        fs::write(memory_output, memory)?;
        println!(
            "{} bytes -> {}",
            options.memory_size.expect("validated above"),
            memory_output.display()
        );
    }
    Ok(())
}

#[derive(Debug)]
struct Options {
    input: PathBuf,
    output: PathBuf,
    memory_output: Option<PathBuf>,
    memory_size: Option<usize>,
    vendor_id: u16,
    device_id: u16,
    class_code: u32,
}

impl Options {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, Box<dyn Error>> {
        let mut input = None;
        let mut output = None;
        let mut memory_output = None;
        let mut memory_size = None;
        let mut vendor_id = None;
        let mut device_id = None;
        let mut class_code = None;
        let mut arguments = arguments;

        while let Some(argument) = arguments.next() {
            let value = match argument.as_str() {
                "--input" | "--output" | "--memory-output" | "--memory-size" | "--vendor"
                | "--device" | "--class" => arguments
                    .next()
                    .ok_or_else(|| invalid_input(format!("missing value for {argument}")))?,
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                _ => return Err(invalid_input(format!("unknown argument: {argument}")).into()),
            };

            match argument.as_str() {
                "--input" => input = Some(PathBuf::from(value)),
                "--output" => output = Some(PathBuf::from(value)),
                "--memory-output" => memory_output = Some(PathBuf::from(value)),
                "--memory-size" => memory_size = Some(parse_size("memory size", &value)?),
                "--vendor" => vendor_id = Some(parse_hex_u16("vendor ID", &value)?),
                "--device" => device_id = Some(parse_hex_u16("device ID", &value)?),
                "--class" => class_code = Some(parse_hex_u32("class code", &value)?),
                _ => unreachable!(),
            }
        }

        Ok(Self {
            input: input.ok_or_else(|| invalid_input("--input is required"))?,
            output: output.ok_or_else(|| invalid_input("--output is required"))?,
            memory_output,
            memory_size,
            vendor_id: vendor_id.ok_or_else(|| invalid_input("--vendor is required"))?,
            device_id: device_id.ok_or_else(|| invalid_input("--device is required"))?,
            class_code: class_code.ok_or_else(|| invalid_input("--class is required"))?,
        })
    }
}

fn parse_hex_u16(name: &str, value: &str) -> Result<u16, io::Error> {
    u16::from_str_radix(value.trim_start_matches("0x"), 16)
        .map_err(|_| invalid_input(format!("invalid {name}: {value}")))
}

fn parse_hex_u32(name: &str, value: &str) -> Result<u32, io::Error> {
    u32::from_str_radix(value.trim_start_matches("0x"), 16)
        .map_err(|_| invalid_input(format!("invalid {name}: {value}")))
}

fn parse_size(name: &str, value: &str) -> Result<usize, io::Error> {
    let parsed = if let Some(hexadecimal) = value.strip_prefix("0x") {
        usize::from_str_radix(hexadecimal, 16)
    } else {
        value.parse()
    };
    parsed.map_err(|_| invalid_input(format!("invalid {name}: {value}")))
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn print_usage() {
    let usage = [
        "usage: svmvisor-rompack --input <driver.efi> --output <driver.rom> \\",
        "       --vendor <hex> --device <hex> --class <hex> \\",
        "       [--memory-output <driver.mem> --memory-size <bytes>]",
    ];
    println!("{}", usage.join("\n"));
}
