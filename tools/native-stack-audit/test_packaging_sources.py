"""Exercise only the card build's source enumeration, never the build script."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
ENUMERATE = r"""
$ErrorActionPreference = 'Stop'
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($env:SOURCE_SCRIPT, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count -ne 0) { throw 'Packaging script does not parse.' }
$functions = @($ast.FindAll({param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Get-ReturningSourceInputs'}, $true))
if ($functions.Count -ne 1) { throw 'Expected exactly one source enumeration function.' }
. ([scriptblock]::Create($functions[0].Extent.Text))
$paths = @(Get-ReturningSourceInputs $env:SOURCE_ROOT ($env:INCLUDE_AUDIT -eq 'yes'))
ConvertTo-Json -Compress -InputObject $paths
"""


class PackagingSourcesTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.powershell = shutil.which("pwsh") or shutil.which("powershell")
        if cls.powershell is None:
            raise unittest.SkipTest("PowerShell is required for packaging source enumeration")

    def enumerate_fixture(self, include_audit):
        # Only the parsed helper function executes. Nothing from the script's
        # top-level payload validation, build, FPGA, or tool paths can run.
        with tempfile.TemporaryDirectory(prefix="source-enumeration-") as temporary:
            fixture = Path(temporary)
            required = {
                "crates/dxe/Cargo.toml", "crates/dxe/src/main.rs",
                "crates/memory-attributes/Cargo.toml", "crates/memory-attributes/src/x86.rs",
                "crates/hypervisor/Cargo.toml", "crates/hypervisor/src/lib.rs",
                "crates/future-dependency/src/lib.rs", "crates/dxe/src/with space.S",
                "tools/rompack/src/main.rs",
                "tools/synthetic-harness/firmware-handoff/Cargo.toml",
                "tools/synthetic-harness/firmware-handoff/src/lib.rs",
                "firmware/squirrel/rtl/endpoint.sv", "firmware/squirrel/vivado/endpoint.tcl",
            }
            generated = {
                "crates/dxe/target/debug/stale.rs",
                "tools/synthetic-harness/firmware-handoff/target/debug/stale.rs",
                "firmware/squirrel/vivado/__pycache__/stale.pyc",
            }
            if include_audit:
                required |= {"tools/native-stack-audit/verify.py", "tools/native-stack-audit/run.py", "tools/native-stack-audit/audit.py", "tools/native-stack-audit/test_audit.py"}
                generated.add("tools/native-stack-audit/__pycache__/run.pyc")
            for relative in required | generated:
                path = fixture / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("fixture", encoding="utf-8")
            environment = dict(os.environ, SOURCE_SCRIPT=str(ROOT / "firmware/squirrel/build-returning-card.ps1"), SOURCE_ROOT=str(fixture), INCLUDE_AUDIT="yes" if include_audit else "no")
            result = subprocess.run([self.powershell, "-NoProfile", "-NonInteractive", "-Command", ENUMERATE], env=environment, text=True, capture_output=True, check=False)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            paths = json.loads(result.stdout.lstrip("\ufeff"))
            self.assertTrue(required <= set(paths), required - set(paths))
            self.assertTrue(generated.isdisjoint(paths))
            self.assertEqual(len(paths), len(set(paths)))
            self.assertTrue(all("\\" not in path for path in paths))
            self.assertIn("Cargo.lock", paths)
            self.assertIn("firmware/squirrel/build-returning-card.ps1", paths)
            return paths

    def test_native_candidate_captures_shared_core_and_local_dependencies(self):
        paths = self.enumerate_fixture(True)
        self.assertIn("tools/native-stack-audit/verify.py", paths)
        self.assertIn("tools/native-stack-audit/run.py", paths)
        self.assertIn("tools/native-stack-audit/audit.py", paths)
        self.assertIn("tools/native-stack-audit/test_audit.py", paths)

    def test_emulator_candidate_has_same_dependency_checkpoint_without_native_audit(self):
        paths = self.enumerate_fixture(False)
        self.assertFalse(any(path.startswith("tools/native-stack-audit/") for path in paths))


if __name__ == "__main__":
    unittest.main()
