//! Fast card iteration for the development loader (`card-resident-dev-loader`).
//!
//! * `card-dev`: low-runtime resident build (this board; `--any-runtime`
//!   opts out) -> payload slot -> offline checks
//!   (-> `flash-card.ps1 -Action ProgramPayload` only with `--flash`).
//! * `card-snapshot`: `read_snapshot.py` into a timestamped directory.
//! * `card-loader-dev`: build and ROM-pack the development loader once.
//!
//! Nothing here touches hardware except `card-dev --flash` and
//! `card-snapshot` without `--input`, both only on the operator's command.

use std::{
    ffi::OsStr,
    fs,
    path::Path,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};
use svmvisor_card_abi::envelope::{
    DIGEST_BYTES, DIGEST_OFFSET, FLAGS_OFFSET, FLAGS_RESIDENT_BOOT, HEADER_BYTES, MIN_PE_BYTES,
    PAYLOAD_BYTES_OFFSET, PAYLOAD_OFFSET_OFFSET, RESIDENT_BOOT_MAGIC, SLOT_BYTES,
    SLOT_BYTES_OFFSET,
};

use crate::{json::Value, resident};

const SECTOR_BYTES: u64 = 0x10000;
const FIRST_SLOT_SECTOR: u64 = 64;

#[derive(Debug, PartialEq)]
struct Header {
    payload_bytes: u64,
    digest: String,
}

pub fn parse_adapter_khz(text: &str) -> Result<u32, String> {
    match text.parse::<u32>() {
        Ok(khz)
            if (100..=30000).contains(&khz) && text.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            Ok(khz)
        }
        _ => {
            Err(format!("--adapter-khz must be a decimal integer within 100..30000, not {text:?}"))
        }
    }
}

/// Build, package and check one development payload; flash only on request.
pub fn dev(low_runtime: bool, flash: bool, adapter_khz: u32) -> Result<(), String> {
    let root = resident::root();
    let base = root.join("target").join("card-dev");
    let out = base.join(fresh_name());
    io(fs::create_dir_all(&out), "create", &out)?;
    let (resident_dir, payload_dir) = (out.join("resident"), out.join("payload"));
    println!("card-dev: {}", out.display());

    println!(
        "[1/4] cargo xtask resident{} -> resident/",
        if low_runtime { " --low-runtime" } else { "" }
    );
    resident::build(&resident_dir, low_runtime)?;

    println!("[2/4] package-payload.py --resident -> payload/");
    let card = root.join("firmware/card");
    let driver = resident_dir.join("driver.efi");
    run_logged(
        &root,
        "python",
        &[
            "-B".as_ref(),
            card.join("package-payload.py").as_os_str(),
            "--resident".as_ref(),
            "--payload".as_ref(),
            driver.as_os_str(),
            "--output".as_ref(),
            payload_dir.as_os_str(),
        ],
        "payload packaging",
        &out.join("package-payload.log"),
    )?;

    println!("[3/4] verify-resident-build.py");
    // resident::build has just proven the working tree equals its source
    // snapshot, so the verifier's `cargo xtask sources` round trip is skipped.
    let child = payload_dir.join("native-child.efi");
    run_logged(
        &root,
        "python",
        &[
            "-B".as_ref(),
            card.join("verify-resident-build.py").as_os_str(),
            "--evidence".as_ref(),
            resident_dir.as_os_str(),
            "--image".as_ref(),
            child.as_os_str(),
            "--no-current-source-check".as_ref(),
        ],
        "resident build verification",
        &out.join("verify-resident-build.log"),
    )?;

    println!("[4/4] flash-card.ps1 -Action CheckPayload (offline)");
    flash_card(
        &root,
        &["-Action".as_ref(), "CheckPayload".as_ref(), "-BuildPath".as_ref(), out.as_os_str()],
        "offline payload check",
    )?;

    let header_path = payload_dir.join("pe-header.bin");
    let header = parse_header(&io(fs::read(&header_path), "read", &header_path)?)?;
    let child_sha = resident::sha(&child)?;
    if child_sha != header.digest
        || io(fs::metadata(&child), "inspect", &child)?.len() != header.payload_bytes
    {
        return Err("packaged header does not describe the packaged child".into());
    }
    let slot_sha = resident::sha(&payload_dir.join("payload-slot.bin"))?;
    let (first, last) = covering_sectors(header.payload_bytes);
    let record = Value::Map(vec![
        ("schema_version".into(), Value::Int(1)),
        ("loader_mode".into(), Value::str("dev")),
        ("low_runtime".into(), Value::Bool(low_runtime)),
        ("child_bytes".into(), Value::Int(header.payload_bytes as i64)),
        ("child_sha256".into(), Value::str(&child_sha)),
        ("header_sha256".into(), Value::Str(resident::sha(&header_path)?)),
        ("slot_sha256".into(), Value::str(&slot_sha)),
        ("covering_sectors".into(), Value::ints((first..=last).map(|sector| sector as i64))),
        ("offline_check_passed".into(), Value::Bool(true)),
        ("hardware_accessed".into(), Value::Bool(false)),
    ]);
    let record_path = out.join("card-dev.json");
    io(fs::write(&record_path, record.dump()), "write", &record_path)?;
    write_pointer(&base, "latest", &out)?;

    println!();
    println!("header digest (child sha256): {}", header.digest);
    println!(
        "child: {} bytes; header+child: {} bytes; slot sectors {first}..{last} of 64..79",
        header.payload_bytes,
        header.payload_bytes + HEADER_BYTES as u64
    );
    println!("slot sha256: {slot_sha}");
    println!("latest -> {}", out.display());
    let khz = adapter_khz.to_string();
    println!(
        "flash (touches hardware):\n  powershell -NoProfile -ExecutionPolicy Bypass -File firmware\\card\\flash-card.ps1 \
        -Action ProgramPayload -BuildPath \"{}\" -AdapterKhz {khz} -ConfirmFlash",
        out.display()
    );
    if !flash {
        println!("not flashed: pass --flash to program the payload slot.");
        return Ok(());
    }
    println!("--flash: programming the payload slot at {khz} kHz");
    flash_card(
        &root,
        &[
            "-Action".as_ref(),
            "ProgramPayload".as_ref(),
            "-BuildPath".as_ref(),
            out.as_os_str(),
            "-AdapterKhz".as_ref(),
            khz.as_ref(),
            "-ConfirmFlash".as_ref(),
        ],
        "payload programming",
    )?;
    println!("power-cycle the target, then: cargo xtask card-snapshot");
    Ok(())
}

/// `read_snapshot.py` with its own options passed through; `--live` unless an
/// `--input` log is named.
pub fn snapshot(passthrough: &[String]) -> Result<(), String> {
    let root = resident::root();
    if passthrough
        .iter()
        .any(|arg| arg == "--output-dir" || arg.starts_with("--output-dir=") || arg == "--summary")
    {
        return Err("card-snapshot chooses --output-dir and --summary itself".into());
    }
    let out = root.join("target").join("card-snapshots").join(fresh_name());
    let parent = out.parent().unwrap();
    io(fs::create_dir_all(parent), "create", parent)?;
    let reader = root.join("firmware/card/read_snapshot.py");
    let mut args: Vec<&OsStr> = vec!["-B".as_ref(), reader.as_os_str()];
    if !passthrough
        .iter()
        .any(|arg| arg == "--input" || arg.starts_with("--input=") || arg == "--live")
    {
        args.push("--live".as_ref());
    }
    args.extend(passthrough.iter().map(|arg| OsStr::new(arg.as_str())));
    args.extend(["--output-dir".as_ref(), out.as_os_str(), "--summary".as_ref()]);
    let result = run(&root, "python".as_ref(), &args, "snapshot read");
    if out.is_dir() {
        write_pointer(parent, "latest", &out)?;
        println!("snapshot directory: {}", out.display());
    }
    result
}

/// The development loader and its 32 KiB option ROM, packed exactly like
/// `firmware/card/build-card.ps1` packs the pinned loader.
pub fn loader_dev() -> Result<(), String> {
    let root = resident::root();
    let base = root.join("target").join("card-dev").join("loader");
    let out = base.join(fresh_name());
    io(fs::create_dir_all(&out), "create", &out)?;
    let manifest = root.join("Cargo.toml");
    let (loader_target, rompack_target) = (out.join("loader-cargo"), out.join("rompack-cargo"));
    println!("card-loader-dev: {}", out.display());
    run(
        &root,
        "cargo".as_ref(),
        &[
            "build".as_ref(),
            "--locked".as_ref(),
            "--manifest-path".as_ref(),
            manifest.as_os_str(),
            "--package".as_ref(),
            "svmvisor-card-loader".as_ref(),
            "--profile".as_ref(),
            "dxe".as_ref(),
            "--features".as_ref(),
            "card-resident-dev-loader".as_ref(),
            "--target".as_ref(),
            "x86_64-unknown-uefi".as_ref(),
            "--target-dir".as_ref(),
            loader_target.as_os_str(),
        ],
        "development loader build",
    )?;
    let (efi, rom, memory) =
        (out.join("svmvisor-dxe.efi"), out.join("svmvisor-dxe.rom"), out.join("svmvisor-dxe.mem"));
    let built = loader_target.join("x86_64-unknown-uefi/dxe/svmvisor-card-loader.efi");
    io(fs::copy(&built, &efi).map(drop), "copy", &built)?;
    run(
        &root,
        "cargo".as_ref(),
        &[
            "run".as_ref(),
            "--locked".as_ref(),
            "--quiet".as_ref(),
            "--release".as_ref(),
            "--manifest-path".as_ref(),
            manifest.as_os_str(),
            "--package".as_ref(),
            "svmvisor-rompack".as_ref(),
            "--target-dir".as_ref(),
            rompack_target.as_os_str(),
            "--".as_ref(),
            "--input".as_ref(),
            efi.as_os_str(),
            "--output".as_ref(),
            rom.as_os_str(),
            "--memory-output".as_ref(),
            memory.as_os_str(),
            "--memory-size".as_ref(),
            "32768".as_ref(),
            "--vendor".as_ref(),
            "0x10ee".as_ref(),
            "--device".as_ref(),
            "0x0666".as_ref(),
            "--class".as_ref(),
            "0xff0000".as_ref(),
        ],
        "32 KiB ROM packaging",
    )?;
    let (loader_sha, rom_sha) = (resident::sha(&efi)?, resident::sha(&rom)?);
    let record = Value::Map(vec![
        ("schema_version".into(), Value::Int(1)),
        ("loader_mode".into(), Value::str("dev")),
        ("loader_feature".into(), Value::str("card-resident-dev-loader")),
        ("loader_bytes".into(), Value::Int(io(fs::metadata(&efi), "inspect", &efi)?.len() as i64)),
        ("loader_sha256".into(), Value::str(&loader_sha)),
        ("rom_bytes".into(), Value::Int(32768)),
        (
            "rom_bytes_used".into(),
            Value::Int(io(fs::metadata(&rom), "inspect", &rom)?.len() as i64),
        ),
        ("rom_sha256".into(), Value::str(&rom_sha)),
        ("hardware_accessed".into(), Value::Bool(false)),
    ]);
    let record_path = out.join("loader-dev.json");
    io(fs::write(&record_path, record.dump()), "write", &record_path)?;
    write_pointer(&base, "latest", &out)?;
    println!("development loader: {loader_sha}  {}", efi.display());
    println!("32 KiB option ROM : {rom_sha}  {}", rom.display());
    println!(
        "one-time slow step (Vivado, ~1 h), which rebuilds this same loader and must report the same loader_sha256:"
    );
    println!(
        "  firmware\\card\\build-card.ps1 -PayloadPath <driver.efi> -PayloadSha256 <sha256> -PayloadEvidencePath <summary.json> \
        -ResidentBuildPath <resident dir> -DevLoader -BuildFpga"
    );
    Ok(())
}

/// `<utc>-<8 hex>`: sortable, and unique across quick successive runs.
fn fresh_name() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let mut hash = Sha256::new();
    hash.update(now.as_nanos().to_le_bytes());
    hash.update(std::process::id().to_le_bytes());
    let id: String = hash.finalize().iter().take(4).map(|byte| format!("{byte:02x}")).collect();
    format!("{}-{id}", utc_stamp(now.as_secs()))
}

/// Proleptic Gregorian date from seconds since the Unix epoch, as
/// `YYYYMMDDThhmmssZ` (Howard Hinnant's `civil_from_days`).
fn utc_stamp(seconds: u64) -> String {
    let (days, rest) = ((seconds / 86400) as i64, seconds % 86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The fields of the 128-byte `SVMBPE01` envelope this tool reports. The
/// loader (`crates/card-loader/src/delivery/child_image.rs`) and `flash-card.ps1` own
/// the full policy; this only refuses to describe something else.
fn parse_header(header: &[u8]) -> Result<Header, String> {
    let word = |offset: usize| u64::from_le_bytes(header[offset..offset + 8].try_into().unwrap());
    if header.len() != HEADER_BYTES || header[..8] != RESIDENT_BOOT_MAGIC {
        return Err("pe-header.bin is not a 128-byte SVMBPE01 envelope".into());
    }
    let payload_bytes = word(PAYLOAD_BYTES_OFFSET);
    if word(SLOT_BYTES_OFFSET) != SLOT_BYTES as u64
        || word(PAYLOAD_OFFSET_OFFSET) != HEADER_BYTES as u64
        || word(FLAGS_OFFSET) != FLAGS_RESIDENT_BOOT
        || !(MIN_PE_BYTES as u64..=(SLOT_BYTES - HEADER_BYTES) as u64).contains(&payload_bytes)
    {
        return Err("pe-header.bin does not describe a resident payload slot".into());
    }
    Ok(Header {
        payload_bytes,
        digest: header[DIGEST_OFFSET..DIGEST_OFFSET + DIGEST_BYTES]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}

/// First and last 64 KiB flash sector holding header + child.
fn covering_sectors(payload_bytes: u64) -> (u64, u64) {
    let extent = HEADER_BYTES as u64 + payload_bytes;
    (FIRST_SLOT_SECTOR, FIRST_SLOT_SECTOR + extent.div_ceil(SECTOR_BYTES) - 1)
}

/// Like `run`, but the tool's merged output goes to `log` (shown on failure).
fn run_logged(
    root: &Path,
    program: &str,
    args: &[&OsStr],
    what: &str,
    log: &Path,
) -> Result<(), String> {
    let executable = resident::which(program).ok_or(format!("missing tool {program}"))?;
    let sink = io(fs::File::create(log), "create", log)?;
    let status = Command::new(&executable)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(io(sink.try_clone(), "create", log)?)
        .stderr(sink)
        .status()
        .map_err(|error| format!("{what}: cannot run {}: {error}", executable.display()))?;
    if status.success() {
        return Ok(());
    }
    let output = String::from_utf8_lossy(&io(fs::read(log), "read", log)?).into_owned();
    Err(format!("{what} failed; {}\n{}", log.display(), output.trim_end()))
}

/// `flash-card.ps1` needs a PowerShell host; prefer PowerShell 7 when installed.
fn flash_card(root: &Path, arguments: &[&OsStr], what: &str) -> Result<(), String> {
    let host = if resident::which("pwsh").is_some() { "pwsh" } else { "powershell" };
    let script = root.join("firmware/card/flash-card.ps1");
    let mut args: Vec<&OsStr> = vec![
        "-NoProfile".as_ref(),
        "-ExecutionPolicy".as_ref(),
        "Bypass".as_ref(),
        "-File".as_ref(),
        script.as_os_str(),
    ];
    args.extend_from_slice(arguments);
    run(root, host.as_ref(), &args, what)
}

/// Run a tool from the repository root with inherited output.
fn run(root: &Path, program: &OsStr, args: &[&OsStr], what: &str) -> Result<(), String> {
    let name = program.to_string_lossy();
    let executable = resident::which(&name).ok_or(format!("missing tool {name}"))?;
    let status = Command::new(&executable)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| format!("{what}: cannot run {}: {error}", executable.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{what} failed ({})",
            status.code().map_or("signal".into(), |code| code.to_string())
        ))
    }
}

fn write_pointer(directory: &Path, name: &str, target: &Path) -> Result<(), String> {
    let pointer = directory.join(name);
    io(fs::write(&pointer, format!("{}\n", target.display())), "write", &pointer)
}

fn io<T>(result: std::io::Result<T>, what: &str, path: &Path) -> Result<T, String> {
    result.map_err(|error| format!("{what} {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_stamp_handles_epoch_leap_days_and_century_rules() {
        assert_eq!(utc_stamp(0), "19700101T000000Z");
        assert_eq!(utc_stamp(951_782_400), "20000229T000000Z");
        assert_eq!(utc_stamp(1_709_251_199), "20240229T235959Z");
        assert_eq!(utc_stamp(1_789_726_530), "20260918T101530Z");
        assert_eq!(utc_stamp(4_107_542_400), "21000301T000000Z");
    }

    #[test]
    fn fresh_names_sort_by_time_and_carry_an_id() {
        let name = fresh_name();
        assert_eq!(name.len(), 16 + 1 + 8);
        assert!(name[17..].bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn adapter_speed_is_numbers_only_and_bounded() {
        assert_eq!(parse_adapter_khz("1000"), Ok(1000));
        assert_eq!(parse_adapter_khz("30000"), Ok(30000));
        for bad in ["", "99", "30001", "+1000", "1000 ", "1e3", "0x3e8", "1000;shutdown", "-1"] {
            assert!(parse_adapter_khz(bad).is_err(), "{bad:?}");
        }
    }

    fn header(bytes: u64) -> Vec<u8> {
        let mut header = vec![0u8; 128];
        header[..8].copy_from_slice(b"SVMBPE01");
        for (offset, value) in [(16, bytes), (24, 0x100000), (32, 128), (40, 4)] {
            header[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        header[48..80].fill(0xab);
        header
    }

    #[test]
    fn header_report_refuses_other_envelopes() {
        assert_eq!(
            parse_header(&header(153088)),
            Ok(Header { payload_bytes: 153088, digest: "ab".repeat(32) })
        );
        let mut returning = header(153088);
        returning[..8].copy_from_slice(b"SVMPE001");
        assert!(parse_header(&returning).is_err());
        assert!(parse_header(&header(0x100000 - 127)).is_err());
        assert!(parse_header(&header(511)).is_err());
        assert!(parse_header(&header(153088)[..127]).is_err());
    }

    #[test]
    fn covering_sectors_stay_inside_the_slot() {
        assert_eq!(covering_sectors(153088), (64, 66));
        assert_eq!(covering_sectors(300 * 1024), (64, 68));
        assert_eq!(covering_sectors(65536 - 128), (64, 64));
        assert_eq!(covering_sectors(65536 - 127), (64, 65));
        assert_eq!(covering_sectors(0x100000 - 128), (64, 79));
    }
}
