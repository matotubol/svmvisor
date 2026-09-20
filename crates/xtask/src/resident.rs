//! Build a raw resident payload, audit its complete linked code, then its DXE
//! shim. No firmware execution. Output directories are fresh and retain
//! failure logs.

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use sha2::{Digest, Sha256};

use crate::{audit, json::Value, relocations};

/// Files and directories whose exact bytes define a resident build.
const SOURCE_FILES: [&str; 4] =
    ["Cargo.toml", "Cargo.lock", ".cargo/config.toml", "rust-toolchain.toml"];
const SOURCE_DIRECTORIES: [&str; 5] = [
    "crates/card-abi",
    "crates/hypervisor",
    "crates/launcher",
    "crates/resident-payload",
    "crates/xtask",
];
const SKIPPED_DIRECTORIES: [&str; 2] = ["target", "__pycache__"];

struct Build {
    root: PathBuf,
    out: PathBuf,
}

impl Build {
    /// Run one tool from the repository root; `<name>.log` keeps its merged
    /// stdout and stderr whether or not it succeeds.
    fn command(
        &self,
        args: &[&OsStr],
        name: &str,
        env: &[(&str, &OsStr)],
    ) -> Result<String, String> {
        let tool = args[0].to_string_lossy();
        let executable = which(&tool).ok_or(format!("missing tool {tool}"))?;
        let log = self.out.join(format!("{name}.log"));
        let sink = io(fs::File::create(&log), "create", &log)?;
        let status = Command::new(&executable)
            .args(&args[1..])
            .current_dir(&self.root)
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .stdout(io(sink.try_clone(), "create", &log)?)
            .stderr(sink)
            .status()
            .map_err(|error| format!("{name}: cannot run {}: {error}", executable.display()))?;
        let output =
            String::from_utf8_lossy(&io(fs::read(&log), "read", &log)?).replace("\r\n", "\n");
        if !status.success() {
            let tail: String = {
                let characters: Vec<char> = output.chars().collect();
                characters[characters.len().saturating_sub(6000)..].iter().collect()
            };
            let code = status.code().map_or("unknown".to_string(), |code| code.to_string());
            return Err(format!("{name} exit {code}\n{tail}"));
        }
        Ok(output)
    }
}

/// Build the one supported native profile: SMP guest startup behind the
/// successful-EBS interposer. `low_runtime` adds the platform-specific
/// below-1GiB runtime allocation.
pub fn build(out: &Path, low_runtime: bool) -> Result<(), String> {
    let root = root();
    let out = io(std::path::absolute(out), "resolve", out)?;
    if let Some(parent) = out.parent() {
        io(fs::create_dir_all(parent), "create", parent)?;
    }
    // Fresh output only: an existing directory is an error, never reused.
    io(fs::create_dir(&out), "create fresh output directory", &out)?;
    let manifest = sources(&root)?;
    let manifest_path = out.join("source-manifest.json");
    io(fs::write(&manifest_path, manifest_json(&manifest)), "write", &manifest_path)?;
    for (name, hash) in &manifest {
        let destination = out.join("source").join(name);
        let parent = destination.parent().unwrap();
        io(fs::create_dir_all(parent), "create", parent)?;
        copy(&root.join(name), &destination)?;
        if &sha(&destination)? != hash {
            return Err(format!("source changed during snapshot: {name}"));
        }
    }
    let build = Build { root: root.clone(), out: out.clone() };
    let path = |relative: &str| root.join(relative).into_os_string();
    let artifact = |name: &str| out.join(name).into_os_string();
    let target = root.join("target/native-resident-cargo");
    let resident = "crates/launcher/src/native/resident";

    let payload_manifest = path("crates/resident-payload/Cargo.toml");
    build.command(
        &[
            "cargo".as_ref(),
            "rustc".as_ref(),
            "--manifest-path".as_ref(),
            &payload_manifest,
            "--target".as_ref(),
            "x86_64-unknown-none".as_ref(),
            "--target-dir".as_ref(),
            target.as_os_str(),
            "--release".as_ref(),
            "--".as_ref(),
            "-C".as_ref(),
            "relocation-model=static".as_ref(),
            "-C".as_ref(),
            "code-model=small".as_ref(),
        ],
        "payload-cargo",
        &[],
    )?;
    copy(
        &target.join("x86_64-unknown-none/release/libsvmvisor_resident_payload.a"),
        &out.join("payload.a"),
    )?;
    for name in ["runtime", "irq", "fault"] {
        let source = path(&format!("{resident}/{name}.S"));
        let object = artifact(&format!("{name}.o"));
        build.command(
            &[
                "clang".as_ref(),
                "--target=x86_64-unknown-none".as_ref(),
                "-c".as_ref(),
                &source,
                "-o".as_ref(),
                &object,
            ],
            &format!("{name}-compile"),
            &[],
        )?;
    }
    let (script, elf, image, package) = (
        path("crates/resident-payload/payload.ld"),
        artifact("payload.elf"),
        artifact("payload.bin"),
        out.join("payload.reloc"),
    );
    build.command(
        &[
            "ld.lld".as_ref(),
            "-m".as_ref(),
            "elf_x86_64".as_ref(),
            "--gc-sections".as_ref(),
            "--emit-relocs".as_ref(),
            "-T".as_ref(),
            &script,
            &artifact("runtime.o"),
            &artifact("irq.o"),
            &artifact("fault.o"),
            &artifact("payload.a"),
            "-o".as_ref(),
            &elf,
        ],
        "payload-link",
        &[],
    )?;
    build.command(
        &["llvm-objcopy".as_ref(), "-O".as_ref(), "binary".as_ref(), &elf, &image],
        "payload-flat",
        &[],
    )?;

    let relocation_log = out.join("payload-relocations.log");
    let packaged = relocations::package(
        &io(fs::read(&elf), "read", Path::new(&elf))?,
        &io(fs::read(&image), "read", Path::new(&image))?,
    );
    let message = match &packaged {
        Ok(bytes) => format!(
            "Packaged {} runtime relocations: {}\n",
            relocations::relocation_count(bytes),
            package.display()
        ),
        Err(error) => format!("{error}\n"),
    };
    io(fs::write(&relocation_log, &message), "write", &relocation_log)?;
    let packaged = packaged.map_err(|error| format!("payload-relocations: {error}"))?;
    io(fs::write(&package, packaged), "write", &package)?;

    let undefined = build.command(
        &["llvm-nm".as_ref(), "--undefined-only".as_ref(), &elf],
        "undefined",
        &[],
    )?;
    if !undefined.trim().is_empty() {
        return Err("resident payload has external symbol dependencies".into());
    }
    let text = build.command(
        &["llvm-objdump".as_ref(), "-d".as_ref(), "--no-show-raw-insn".as_ref(), &elf],
        "disassembly",
        &[],
    )?;
    let debug_reset_audit = audit::audit_debug_reset(&text)?;
    let host_fault_audit = audit::audit_host_fault(&text)?;
    let count = audit::audit_extended_state(&text)?;

    let feature = if low_runtime {
        "native-resident-boot,native-resident-low-runtime"
    } else {
        "native-resident-boot"
    };
    build.command(
        &[
            "cargo".as_ref(),
            "build".as_ref(),
            "--locked".as_ref(),
            "-p".as_ref(),
            "svmvisor-launcher".as_ref(),
            "--target".as_ref(),
            "x86_64-unknown-uefi".as_ref(),
            "--target-dir".as_ref(),
            target.as_os_str(),
            "--release".as_ref(),
            "--features".as_ref(),
            feature.as_ref(),
        ],
        "dxe-cargo",
        &[("SVMVISOR_RESIDENT_PAYLOAD", package.as_os_str())],
    )?;
    let driver = out.join("driver.efi");
    copy(&target.join("x86_64-unknown-uefi/release/svmvisor-launcher.efi"), &driver)?;
    audit::audit_runtime_driver(&io(fs::read(&driver), "read", &driver)?)?;

    let object = out.join("physical-audit.obj");
    build.command(
        &[
            "clang".as_ref(),
            "--target=x86_64-pc-windows-msvc".as_ref(),
            "-c".as_ref(),
            &path(&format!("{resident}/physical.S")),
            "-o".as_ref(),
            object.as_os_str(),
        ],
        "physical-audit-compile",
        &[],
    )?;
    let listing = build.command(
        &["llvm-objdump".as_ref(), "-t".as_ref(), "-r".as_ref(), object.as_os_str()],
        "physical-audit-relocations",
        &[],
    )?;
    let bootstrap_audit =
        audit::audit_copied_ap_wait(&io(fs::read(&object), "read", &object)?, &listing)?;
    let boot_object = artifact("boot-audit.obj");
    build.command(
        &[
            "clang".as_ref(),
            "--target=x86_64-pc-windows-msvc".as_ref(),
            "-c".as_ref(),
            &path(&format!("{resident}/boot.S")),
            "-o".as_ref(),
            &boot_object,
        ],
        "boot-audit-compile",
        &[],
    )?;
    build.command(
        &["llvm-objdump".as_ref(), "-d".as_ref(), "-r".as_ref(), &boot_object],
        "boot-audit-disassembly",
        &[],
    )?;

    if sources(&root)? != manifest {
        return Err("source changed during build; retry from a stable snapshot".into());
    }
    let mut artifacts = BTreeMap::new();
    for entry in io(fs::read_dir(&out), "list", &out)? {
        let entry = io(entry, "list", &out)?;
        if io(fs::metadata(entry.path()), "inspect", &entry.path())?.is_file() {
            artifacts.insert(
                entry.file_name().to_string_lossy().into_owned(),
                Value::Str(sha(&entry.path())?),
            );
        }
    }
    // verify-resident-build.py requires these fixed profile fields.
    let mut result = vec![
        ("test_output".to_string(), Value::Bool(false)),
        ("payload_only".into(), Value::Bool(false)),
        ("boot".into(), Value::Bool(true)),
        ("low_runtime".into(), Value::Bool(low_runtime)),
        ("linked_instruction_count".into(), Value::Int(count as i64)),
        ("bootstrap_audit".into(), bootstrap_audit),
        ("debug_reset_audit".into(), debug_reset_audit),
        ("host_fault_audit".into(), host_fault_audit),
        ("no_fp_simd_xstate_instructions".into(), Value::Bool(true)),
        ("undefined_symbols".into(), Value::Int(0)),
    ];
    let report = Value::Map(result.clone()).dump();
    result.push(("artifacts".into(), Value::Map(artifacts.into_iter().collect())));
    let summary = out.join("summary.json");
    io(fs::write(&summary, Value::Map(result).dump()), "write", &summary)?;
    println!("{report}");
    Ok(())
}

/// The repository root: this crate lives at `crates/xtask`.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/xtask lives two levels below the root")
        .to_path_buf()
}

/// Root-relative '/' paths to SHA-256, ordered like sorted `pathlib` paths:
/// component-wise, case-insensitively on Windows.
pub fn sources(root: &Path) -> Result<Vec<(String, String)>, String> {
    let mut names: Vec<String> = SOURCE_FILES.iter().map(|name| name.to_string()).collect();
    for directory in SOURCE_DIRECTORIES {
        walk(&root.join(directory), directory, &mut names)?;
    }
    let key = |name: &String| -> Vec<String> {
        name.split('/')
            .map(|part| if cfg!(windows) { part.to_lowercase() } else { part.to_string() })
            .collect()
    };
    names.sort_by_key(key);
    names.dedup();
    names.into_iter().map(|name| Ok((name.clone(), sha(&root.join(&name))?))).collect()
}

pub fn sha(path: &Path) -> Result<String, String> {
    let digest = Sha256::digest(io(fs::read(path), "read", path)?);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn manifest_json(manifest: &[(String, String)]) -> String {
    Value::Map(manifest.iter().map(|(name, hash)| (name.clone(), Value::str(hash))).collect())
        .dump()
}

/// PATH lookup with the platform's executable extensions.
pub(crate) fn which(tool: &str) -> Option<PathBuf> {
    let extensions: Vec<OsString> = if cfg!(windows) {
        let listed = std::env::var_os("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
        std::iter::once(OsString::new())
            .chain(std::env::split_paths(&listed).map(PathBuf::into_os_string))
            .collect()
    } else {
        vec![OsString::new()]
    };
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|directory| {
        extensions.iter().find_map(|extension| {
            let mut name = OsString::from(tool);
            name.push(extension);
            let candidate = directory.join(name);
            candidate.is_file().then_some(candidate)
        })
    })
}

fn walk(directory: &Path, relative: &str, found: &mut Vec<String>) -> Result<(), String> {
    for entry in io(fs::read_dir(directory), "list", directory)? {
        let entry = io(entry, "list", directory)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| format!("non-Unicode source name {name:?}"))?;
        let path = entry.path();
        let child = format!("{relative}/{name}");
        // Follows links like the original's Path.is_file()/rglob.
        let kind = io(fs::metadata(&path), "inspect", &path)?;
        if kind.is_dir() {
            if !SKIPPED_DIRECTORIES.contains(&name.as_str()) {
                walk(&path, &child, found)?;
            }
        } else if kind.is_file() && !SKIPPED_DIRECTORIES.contains(&name.as_str()) {
            found.push(child);
        }
    }
    Ok(())
}

fn io<T>(result: std::io::Result<T>, what: &str, path: &Path) -> Result<T, String> {
    result.map_err(|error| format!("{what} {}: {error}", path.display()))
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    fs::copy(from, to)
        .map(drop)
        .map_err(|error| format!("copy {} to {}: {error}", from.display(), to.display()))
}
