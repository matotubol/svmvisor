"""Offline checks of the payload-slot OpenOCD stages with every hardware command stubbed.

Runs the checked-in card-payload-*.cfg under a plain tclsh whose `flash`,
`adapter`, `echo` and `shutdown` commands only record their arguments, and
whose transport.cfg is a stub. No OpenOCD, no adapter, no device.
"""
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

CONFIGS = Path(__file__).resolve().parents[1] / "openocd"
TCLSH = shutil.which("tclsh") or shutil.which("tclsh86") or shutil.which("jimsh")
HARNESS = """
proc flash {args} { puts "CALL flash $args" }
proc adapter {args} { puts "CALL adapter $args" }
proc echo {text} { puts "ECHO $text" }
proc shutdown {} { puts "CALL shutdown" }
"""


@unittest.skipUnless(TCLSH, "no Tcl interpreter on PATH")
class PayloadStages(unittest.TestCase):
    def run_stage(self, config, settings, files=(), sector_files=()):
        with tempfile.TemporaryDirectory() as session:
            session = Path(session)
            (session / "transport.cfg").write_text('puts "CALL transport"\n')
            for name in files:
                (session / name).write_bytes(b"\xff" * 1048576)
            for sector in sector_files:
                (session / f"sector-{sector}.bin").write_bytes(b"\0" * 65536)
            script = HARNESS + f"set SESSION {{{session.as_posix()}}}\n"
            script += "".join(f"set {name} {{{value}}}\n" for name, value in settings.items())
            script += f"source {{{(CONFIGS / config).as_posix()}}}\n"
            (session / "harness.tcl").write_text(script)
            result = subprocess.run([TCLSH, str(session / "harness.tcl")], capture_output=True, text=True, timeout=60)
            return result.returncode, result.stdout.replace(session.as_posix(), "$S"), result.stderr

    SLOT_FILES = ("slot-before-a.bin", "slot-before-b.bin", "slot-expected.bin")

    def program(self, erase, write, khz="1000", files=SLOT_FILES, sector_files=None):
        return self.run_stage("card-payload-program.cfg",
                              {"ADAPTER_KHZ": khz, "PAYLOAD_ERASE_SECTORS": erase, "PAYLOAD_WRITE_SECTORS": write},
                              files, write.split() if sector_files is None else sector_files)

    def test_shrinking_payload_erases_five_sectors_and_writes_three(self):
        code, out, err = self.program("64 65 66 67 68", "64 65 66", khz="10000")
        self.assertEqual(code, 0, err)
        calls = [line for line in out.splitlines() if line.startswith("CALL")]
        self.assertEqual(calls, [
            "CALL transport", "CALL adapter speed 10000",
            "CALL flash verify_bank xc7.spi $S/slot-before-a.bin 4194304",
            *[f"CALL flash erase_sector xc7.spi {s} {s}" for s in (64, 65, 66, 67, 68)],
            *[f"CALL flash write_bank xc7.spi $S/sector-{s}.bin {s * 65536}" for s in (64, 65, 66)],
            "CALL flash read_bank xc7.spi $S/slot-readback.bin 4194304 1048576", "CALL shutdown"])
        for marker in ("PASS card-payload-range-confined", "PASS card-payload-adapter-khz 10000",
                       "PASS card-payload-prewrite-backup-verified", "PASS card-payload-program-readback"):
            self.assertIn("ECHO " + marker, out)
        # Every addressed byte lies inside [0x400000, 0x500000).
        for line in calls:
            words = line.split()
            if "erase_sector" in line:
                self.assertTrue(64 <= int(words[4]) <= int(words[5]) <= 79)
            if "write_bank" in line:
                self.assertTrue(0x400000 <= int(words[5]) and int(words[5]) + 65536 <= 0x500000)

    def test_write_without_erase_into_erased_sectors(self):
        code, out, err = self.program("", "64 65 66")
        self.assertEqual(code, 0, err)
        self.assertNotIn("erase_sector", out)
        self.assertEqual(out.count("write_bank"), 3)

    def test_refusals_happen_before_the_transport_or_any_flash_command(self):
        cases = {
            "configuration sector": ("63", ""), "beyond the slot": ("80", ""), "sector zero": ("0", ""),
            "descending": ("65 64", ""), "duplicate": ("64 64", ""), "write outside": ("", "5"),
            "leading zero / octal": ("064", ""), "hex": ("0x40", ""), "negative": ("-64", ""),
            "command injection": ("64; flash erase_sector xc7.spi 0 79", ""),
            "bracket injection": ("[flash erase_sector xc7.spi 0 79]", ""),
            "nothing to program": ("", ""),
        }
        for name, (erase, write) in cases.items():
            with self.subTest(name):
                code, out, _ = self.program(erase, write, sector_files=())
                self.assertNotEqual(code, 0)
                self.assertNotIn("CALL", out)
        for khz in ("99", "30001", "1000; shutdown", "1e3", "", "01000"):
            with self.subTest(khz=khz):
                code, out, _ = self.program("64", "64", khz=khz)
                self.assertNotEqual(code, 0)
                self.assertNotIn("CALL", out)

    def test_missing_inputs_or_existing_readback_refuse_before_hardware(self):
        code, out, _ = self.program("64", "64", files=("slot-before-a.bin", "slot-before-b.bin"))
        self.assertNotEqual(code, 0); self.assertNotIn("CALL", out)
        code, out, _ = self.program("64", "64 65", sector_files=("64",))
        self.assertNotEqual(code, 0); self.assertNotIn("CALL", out)
        code, out, _ = self.program("64", "64", files=self.SLOT_FILES + ("slot-readback.bin",))
        self.assertNotEqual(code, 0); self.assertNotIn("CALL", out)

    def test_missing_settings_refuse(self):
        code, out, _ = self.run_stage("card-payload-program.cfg", {"ADAPTER_KHZ": "1000"}, self.SLOT_FILES)
        self.assertNotEqual(code, 0); self.assertNotIn("CALL", out)
        code, out, _ = self.run_stage("card-payload-backup.cfg", {})
        self.assertNotEqual(code, 0); self.assertNotIn("CALL", out)

    def test_backup_reads_only_the_slot_twice_and_never_writes(self):
        code, out, err = self.run_stage("card-payload-backup.cfg", {"ADAPTER_KHZ": "1000"})
        self.assertEqual(code, 0, err)
        self.assertEqual([line for line in out.splitlines() if line.startswith("CALL")], [
            "CALL transport", "CALL adapter speed 1000",
            "CALL flash read_bank xc7.spi $S/slot-before-a.bin 4194304 1048576",
            "CALL flash read_bank xc7.spi $S/slot-before-b.bin 4194304 1048576", "CALL shutdown"])
        self.assertIn("ECHO PASS card-payload-double-backup", out)
        code, out, _ = self.run_stage("card-payload-backup.cfg", {"ADAPTER_KHZ": "1000"}, files=("slot-before-a.bin",))
        self.assertNotEqual(code, 0); self.assertNotIn("CALL", out)

    def test_stage_text_contains_no_activation_or_whole_chip_command(self):
        for name in ("card-payload-backup.cfg", "card-payload-program.cfg"):
            text = "\n".join(line for line in (CONFIGS / name).read_text().splitlines() if not line.lstrip().startswith("#"))
            for forbidden in ("write_image", "erase_address", "erase_check", "pld load", "jprogram", "jstart",
                              "mass_erase", "protect", "fillb", "fillw"):
                self.assertNotIn(forbidden, text.lower(), name)
        self.assertNotIn("erase", (CONFIGS / "card-payload-backup.cfg").read_text().split("\n", 3)[3].lower())


if __name__ == "__main__":
    unittest.main()
