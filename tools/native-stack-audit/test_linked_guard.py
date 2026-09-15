"""Linked-byte rejection tests. Synthetic marker fixture; NEVER run the PE.

The ordinary unit suite needs the same clang/lld-link/llvm-objdump tools as the
guard. These tests use real boundary, canary and transition objects but a small
synthetic caller, so no result here is native-returning admission evidence.
"""
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from audit import Coff, PE, Refusal, audit_build
from run import ASSEMBLY_SOURCES


ROOT = Path(__file__).resolve().parents[2]
TEMPLATE = """
.def svmvisor_native_efi_main_inner;
.scl 2;
.type 32;
.endef
.section .text,"xr",one_only,svmvisor_native_efi_main_inner,unique,0
.globl svmvisor_native_efi_main_inner
svmvisor_native_efi_main_inner:
 sub $40, %rsp
.globl svmvisor_native_stack_sample
svmvisor_native_stack_sample:
 mov %rsp, %rax
.globl svmvisor_native_high_begin
svmvisor_native_high_begin:
 call svmvisor_native_returning_high
.globl svmvisor_native_high_end
svmvisor_native_high_end:
 add $40, %rsp
 ret
.Lfunc_end0:
.def svmvisor_native_returning_high;
.scl 2;
.type 32;
.endef
.section .text,"xr",one_only,svmvisor_native_returning_high,unique,1
.globl svmvisor_native_returning_high
svmvisor_native_returning_high:
 BODY
.Lfunc_end1:
"""


class LinkedGuardTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        for tool in ("clang", "lld-link", "llvm-objdump"):
            if not shutil.which(tool):
                raise unittest.SkipTest(f"linked checker tests require {tool}")
        (ROOT / "work").mkdir(exist_ok=True)
        cls.temporary = tempfile.TemporaryDirectory(prefix="stack-audit-tests-", dir=ROOT / "work")
        cls.directory = Path(cls.temporary.name).resolve()
        if not cls.directory.is_relative_to((ROOT / "work").resolve()):
            raise RuntimeError("temporary test path escaped work/")
        cls.native = []
        for name in ("native_boundary", "native_transition_canary", "native_transition"):
            obj = cls.directory / (name + ".obj")
            defines = ["-DSVMVISOR_NATIVE_RETURNING=1"] if name == "native_transition_canary" else []
            cls.command(["clang", *defines, "--target=x86_64-pc-windows-msvc", "-c", ROOT / ASSEMBLY_SOURCES[name], "-o", obj])
            cls.native.append(obj)

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    @staticmethod
    def command(args):
        result = subprocess.run([str(x) for x in args], cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        if result.returncode:
            raise AssertionError(result.stdout)
        return result.stdout

    def run_fixture(self, body, corrupt_pe=False, checked_helper=False):
        source = self.directory / "caller.s"
        obj = self.directory / "caller.obj"
        pe = self.directory / "caller.efi"
        link_map = self.directory / "caller.map"
        source_text = TEMPLATE.replace("BODY", body)
        if checked_helper:
            source_text = source_text.replace(" call svmvisor_native_returning_high", " call checked_high_operation", 1)
            source_text += """
.def checked_high_operation;
.scl 2;
.type 32;
.endef
.section .text,"xr",one_only,checked_high_operation,unique,2
.globl checked_high_operation
checked_high_operation:
 sub $40, %rsp
 test %edx, %edx
 jz .Lchecked_refuse
 call svmvisor_native_returning_high
.Lchecked_refuse:
 add $40, %rsp
 ret
.Lfunc_end2:
"""
        source.write_text(source_text)
        self.command(["clang", "--target=x86_64-pc-windows-msvc", "-c", source, "-o", obj])
        self.command(["lld-link", "/entry:efi_main", "/subsystem:efi_boot_service_driver", "/nodefaultlib", f"/out:{pe}", f"/map:{link_map}", obj, *self.native])
        if corrupt_pe:
            image = PE(pe)
            data = bytearray(pe.read_bytes())
            data[image.sections[0][2]] ^= 1
            pe.write_bytes(data)
        disasm = self.command(["llvm-objdump", "--disassemble", "--x86-asm-syntax=intel", pe])
        return audit_build(source.read_text(), "noredzone minsize optsize", link_map.read_text(), disasm, PE(pe), [Coff(obj), *[Coff(path) for path in self.native]])

    def test_exact_canary_transition_and_boundary_chain(self):
        report = self.run_fixture("sub $40, %rsp\n call svmvisor_native_transition_canary\n add $40, %rsp\n ret")
        self.assertGreater(report["maximum_below_sampled_rsp"], 0)
        self.assertEqual(report["maximum_below_sampled_rsp"] + report["coverage_margin_bytes"], 65536)
        self.assertEqual(len(report["outer_entry_to_inner_entry_depths_for_efi_mod64"]), 4)
        self.assertIn("svmvisor_native_transition", report["critical_functions"])

    def test_named_high_through_checked_operation_is_fully_audited(self):
        body = "sub $40, %rsp\n call svmvisor_native_transition_canary\n add $40, %rsp\n ret"
        direct = self.run_fixture(body)
        wrapped = self.run_fixture(body, checked_helper=True)
        self.assertEqual(wrapped["maximum_below_sampled_rsp"], direct["maximum_below_sampled_rsp"] + 48)
        self.assertEqual(wrapped["high_interval_edges"][0]["target"], "checked_high_operation")
        self.assertEqual(wrapped["critical_functions"]["checked_high_operation"]["edges"][0]["target"], "svmvisor_native_returning_high")

    def test_linked_stack_growth_refuses(self):
        with self.assertRaisesRegex(Refusal, "stack window exceeded"):
            self.run_fixture("sub $65576, %rsp\n call svmvisor_native_transition_canary\n add $65576, %rsp\n ret")

    def test_linked_unknown_target_refuses(self):
        with self.assertRaisesRegex(Refusal, "unknown target"):
            self.run_fixture("sub $40, %rsp\n .byte 0xe8\n .long 0x10000000\n call svmvisor_native_transition_canary\n add $40, %rsp\n ret")

    def test_linked_dynamic_frame_refuses(self):
        with self.assertRaisesRegex(Refusal, "nonconstant"):
            self.run_fixture("sub %rax, %rsp\n call svmvisor_native_transition_canary\n ret")

    def test_linked_recursion_refuses(self):
        with self.assertRaisesRegex(Refusal, "recursive"):
            self.run_fixture("sub $40, %rsp\n call svmvisor_native_returning_high\n call svmvisor_native_transition_canary\n add $40, %rsp\n ret")

    def test_changed_linked_byte_refuses(self):
        with self.assertRaisesRegex(Refusal, "machine bytes differ"):
            self.run_fixture("sub $40, %rsp\n call svmvisor_native_transition_canary\n add $40, %rsp\n ret", corrupt_pe=True)


if __name__ == "__main__":
    unittest.main()
