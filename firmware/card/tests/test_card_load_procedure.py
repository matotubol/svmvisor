"""Offline procedure tests. OpenOCD commands are replaced before config loading."""
from pathlib import Path
import hashlib
import os
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
CFG = ROOT / "firmware/card/openocd"
EXE = ROOT / "target/firmware/tools/openocd/bin/openocd.exe"
SCRATCH = ROOT / "target/firmware/card/card-load-offline-tests"


class ProcedureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        SCRATCH.mkdir(parents=True, exist_ok=True)
        if hashlib.sha256(EXE.read_bytes()).hexdigest() != "9732b05af7e0f6a05a0051371e49af42515662ad309ddcc87f86f9b434ce96d8":
            raise RuntimeError("Only the reviewed local executable may run mock tests")

    def run_config(self, name="program", fault=""):
        with tempfile.TemporaryDirectory(dir=SCRATCH) as folder:
            directory = Path(folder)
            for source in CFG.glob("card-load-*.cfg"):
                shutil.copyfile(source, directory / source.name.replace("card-load-", ""))
            if name == "program":
                for item in ("combined.bin", "before-a.bin", "before-b.bin"):
                    with (directory / item).open("wb") as f:
                        f.truncate(5242880 if fault != "size" else 8)
            # Every command capable of hardware access is replaced with a mock.
            # No original command is called, and native shutdown stops auto-init.
            mock = r'''
foreach name {interface ftdi_vid_pid ftdi_channel ftdi_layout_init reset_config adapter_khz jtag pld target flash init reset} {
    if {[llength [info commands $name]]} { rename $name original_$name }
    proc $name {args} { return "" }
}
proc jtag {args} {
    if {[lindex $args 0] == "cget"} {
        if {$::fault == "fpga"} { return 0x03622093 }
        return 0x0362d093
    }
}
proc capture {script} { error "Sector output must use command return, not log capture" }
proc sector_fixture {} {
    set identity "SPIFI flash information:\n  Device 'issi is25lp256d' (ID 0x0019609d)\n"
    if {$::fault == "jedec"} { set identity "Device 'other' (ID 0x00123456)\n" }
    set count 512
    if {$::fault == "sectors"} { set count 511 }
    for {set i 0} {$i < $count} {incr i} {
        set size 65536
        if {$::fault == "sector_size" && $i == 79} { set size 131072 }
        set index $i
        if {$::fault == "duplicate" && $i == 79} { set index 78 }
        if {$::fault == "out_of_order" && $i == 79} { set index 80 }
        append identity [format "\t#%3i: 0x%8.8x (0x%x 64kB) not protected\n" $index [expr {$i * 65536}] $size]
    }
    return $identity
}
proc flash {args} {
    echo "MOCK_FLASH $args"
    set verb [lindex $args 0]
    if {$verb == "info"} { return [sector_fixture] }
    if {$verb == "list"} {
        set size 33554432
        if {$::fault == "geometry"} { set size 16777216 }
        return [list [list name xc7.spi driver jtagspi base 0 size $size bus_width 0 chip_width 0]]
    }
    if {$verb == "verify_bank" && $::fault == "backup_mismatch"} { error "mock backup mismatch" }
    if {$verb == "read_bank"} {
        set f [open [lindex $args 2] wb]
        seek $f 5242879
        puts -nonewline $f "\x00"
        close $f
    }
}
'''
            mock = mock.replace("\\n", "\n").replace("\\x00", "\x00")
            config = directory / "mock.cfg"
            config.write_text(f"set SESSION {{{directory.as_posix()}}}\nset fault {{{fault}}}\n" + mock + f"\nsource [file join $SESSION {name}.cfg]\n")
            result = subprocess.run([str(EXE), "-f", str(config)], capture_output=True, text=True, timeout=20)
            return result.returncode, result.stdout + result.stderr

    def test_program_requires_verified_backup_before_erase(self):
        status, trace = self.run_config()
        self.assertEqual(status, 0, trace)
        self.assertIn("PASS card-program-readback", trace)
        self.assertLess(trace.index("verify_bank xc7.spi"), trace.index("write_image erase"))
        self.assertIn(" 0 bin", trace)
        self.assertIn("readback.bin 0 5242880", trace)

    def test_backup_uses_two_reads_and_independent_verify(self):
        status, trace = self.run_config("backup")
        self.assertEqual(status, 0, trace)
        self.assertEqual(trace.count("MOCK_FLASH read_bank"), 2)
        self.assertIn("PASS card-double-backup", trace)
        self.assertNotIn("write_image", trace)

    def test_failures_prevent_erase(self):
        for fault in ("fpga", "jedec", "geometry", "sectors", "sector_size", "duplicate", "out_of_order", "size", "backup_mismatch"):
            with self.subTest(fault=fault):
                status, trace = self.run_config(fault=fault)
                self.assertNotEqual(status, 0, trace)
                self.assertNotIn("MOCK_FLASH write_image", trace)

    def test_checkonly_missing_confirmation_and_whatif_never_launch(self):
        command = r'''
$ErrorActionPreference='Stop'
function global:Start-Process { throw 'HARDWARE_LAUNCH_FORBIDDEN' }
& ./firmware/card/card-load-test.ps1
$denied=$false
try { & ./firmware/card/card-load-test.ps1 -Action Program } catch { if ($_.Exception.Message -notmatch 'ConfirmFlash') { throw }; $denied=$true }
if (-not $denied) { throw 'Unconfirmed Program accepted' }
& ./firmware/card/card-load-test.ps1 -Action Program -ConfirmFlash -WhatIf
'''
        result = subprocess.run(["pwsh.exe", "-NoProfile", "-Command", command], cwd=ROOT, capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_real_process_helper_quotes_paths_and_captures_exit(self):
        with tempfile.TemporaryDirectory(prefix="offline space;", dir=SCRATCH) as folder:
            directory = Path(folder)
            shutil.copyfile(EXE, directory / "openocd.exe")
            (directory / "program.cfg").write_text('echo "PASS card-target-id-and-geometry"\necho "PASS card-program-readback"\nshutdown\n')
            env = dict(os.environ, SVMVISOR_OFFLINE_SESSION=str(directory))
            command = r'''
$ErrorActionPreference='Stop'
. ./firmware/card/card-load-validation.ps1
$tokens=$null; $errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PWD 'firmware/card/card-load-test.ps1'),[ref]$tokens,[ref]$errors)
if ($errors) { throw 'Parse errors' }
$function=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Invoke-CardSession'},$true)
Invoke-Expression $function.Extent.Text
$session=$env:SVMVISOR_OFFLINE_SESSION
$sessionTcl=ConvertTo-CardLoadTclPath $session
$record=@{hardware_accessed=$false}
function Save-Record {}
Invoke-CardSession 'program.cfg' 'PASS card-program-readback'
'''
            result = subprocess.run(["pwsh.exe", "-NoProfile", "-Command", command], cwd=ROOT, env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_pure_backup_hash_bounds_and_tcl_path_validation(self):
        with tempfile.TemporaryDirectory(dir=SCRATCH) as folder:
            directory = Path(folder)
            for name in ("a.bin", "b.bin"):
                with (directory / name).open("wb") as f:
                    f.truncate(5242880)
            env = dict(os.environ, SVMVISOR_OFFLINE_SESSION=str(directory))
            command = r'''
$ErrorActionPreference='Stop'
. ./firmware/card/card-load-validation.ps1
$a=Join-Path $env:SVMVISOR_OFFLINE_SESSION 'a.bin'; $b=Join-Path $env:SVMVISOR_OFFLINE_SESSION 'b.bin'
$null=Assert-CardLoadBackups $a $b
$f=[IO.File]::OpenWrite($b); $f.WriteByte(1); $f.Dispose()
$rejected=$false; try { Assert-CardLoadBackups $a $b } catch { $rejected=$true }; if (-not $rejected) { throw 'Mismatching backup accepted' }
[IO.File]::WriteAllBytes($b,[byte[]]@(0))
$rejected=$false; try { Assert-CardLoadBackups $a $b } catch { $rejected=$true }; if (-not $rejected) { throw 'Short backup accepted' }
foreach($bad in @('x{y','x}y','x"y',"x`ny","x`ry")) {
    $rejected=$false; try { ConvertTo-CardLoadTclPath $bad } catch { $rejected=$true }; if (-not $rejected) { throw 'Unsafe path accepted' }
}
if ((ConvertTo-CardLoadTclPath 'C:\safe path;ok') -ne 'C:/safe path;ok') { throw 'Safe path altered' }
'''
            result = subprocess.run(["pwsh.exe", "-NoProfile", "-Command", command], cwd=ROOT, env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
