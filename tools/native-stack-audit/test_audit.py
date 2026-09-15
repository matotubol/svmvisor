"""Meaningful negative guard tests; no generated fixture is production evidence."""
import unittest

from audit import Analyzer, Instruction, Refusal, check_panic_and_probe_references, disassembly, enforce_coverage, stack_step
from run import effective_settings


def graph(*rows):
    return {address: Instruction(address, size, op, args) for address, size, op, args in rows}


def native_log(features=None, extra=""):
    if features is None:
        features = {"native-returning", "native-resource-observe", "native-preflight", "memory-attribute-f7", "memory-attribute-provider"}
    settings = " ".join(f'--cfg feature="{feature}"' for feature in sorted(features))
    return f'Running `rustc --crate-name svmvisor_dxe --crate-type bin -C opt-level=z -C panic=abort -C lto -C codegen-units=1 {settings} --target x86_64-unknown-uefi {extra}`'


class GuardTests(unittest.TestCase):
    def test_probe_configuration_is_not_an_emitted_reference(self):
        check_panic_and_probe_references('attributes #0 = { minsize noredzone "probe-stack"="_R___rust_probestack" }', {})

    def test_backend_emitted_probe_refuses_without_ir_reference(self):
        with self.assertRaisesRegex(Refusal, "actual link"):
            check_panic_and_probe_references('attributes #0 = { "probe-stack"="_R___rust_probestack" }', {"_R___rust_probestack": 0x1000})

    def test_ir_panic_reference_refuses(self):
        with self.assertRaisesRegex(Refusal, "optimized IR"):
            check_panic_and_probe_references('call void @panic_bounds_check()', {})

    def test_actual_compiler_settings_checked(self):
        log = native_log()
        self.assertEqual(effective_settings(log)["lto"], "fat")
        self.assertEqual(effective_settings(log)["features"], ["memory-attribute-f7", "memory-attribute-provider", "native-preflight", "native-resource-observe", "native-returning"])

    def test_missing_f7_reader_or_core_dependency_refuses(self):
        complete = {"native-returning", "native-resource-observe", "native-preflight", "memory-attribute-f7", "memory-attribute-provider"}
        for missing in complete:
            with self.subTest(missing=missing), self.assertRaisesRegex(Refusal, "actual rustc features"):
                effective_settings(native_log(complete - {missing}))
        legacy = {"native-returning", "native-resource-observe", "native-preflight"}
        with self.assertRaisesRegex(Refusal, "actual rustc features"):
            effective_settings(native_log(legacy))

    def test_unreviewed_mutation_fault_probe_and_emulator_features_refuse(self):
        complete = {"native-returning", "native-resource-observe", "native-preflight", "memory-attribute-f7", "memory-attribute-provider"}
        for added in ("memory-attribute-firmware", "memory-attribute-probe", "native-transition-test", "card-returning-loader", "default", "unknown"):
            with self.subTest(added=added), self.assertRaisesRegex(Refusal, "actual rustc features"):
                effective_settings(native_log(complete | {added}))

    def test_late_compiler_profile_override_refuses(self):
        log = native_log(extra="-C opt-level=0 ")
        with self.assertRaisesRegex(Refusal, "actual rustc profile"):
            effective_settings(log)

    def test_call_push_and_shadow_space_are_counted(self):
        code = graph((0x100, 4, "sub", "rsp, 0x28"), (0x104, 5, "call", "0x200"), (0x109, 4, "add", "rsp, 0x28"), (0x10d, 1, "ret", ""), (0x200, 1, "push", "rbx"), (0x201, 4, "sub", "rsp, 0x60"), (0x205, 4, "add", "rsp, 0x60"), (0x209, 1, "pop", "rbx"), (0x20a, 1, "ret", ""))
        self.assertEqual(Analyzer(code).function(0x100)["bound"], 40 + 8 + 8 + 96)

    def test_new_unknown_call_refuses(self):
        code = graph((0x100, 1, "push", "rax"), (0x101, 5, "call", "0x900"), (0x106, 1, "pop", "rax"), (0x107, 1, "ret", ""))
        with self.assertRaisesRegex(Refusal, "unknown target"):
            Analyzer(code).function(0x100)

    def test_indirect_critical_call_refuses(self):
        with self.assertRaisesRegex(Refusal, "indirect"):
            Analyzer(graph((0x100, 2, "call", "rax"))).function(0x100)

    def test_dynamic_reservation_refuses(self):
        with self.assertRaisesRegex(Refusal, "nonconstant"):
            Analyzer(graph((0x100, 3, "sub", "rsp, rax"))).function(0x100)

    def test_stack_switch_refuses(self):
        with self.assertRaisesRegex(Refusal, "RSP write"):
            Analyzer(graph((0x100, 3, "mov", "rsp, rax"))).function(0x100)

    def test_recursion_refuses(self):
        code = graph((0x100, 1, "push", "rax"), (0x101, 5, "call", "0x100"), (0x106, 1, "pop", "rax"), (0x107, 1, "ret", ""))
        with self.assertRaisesRegex(Refusal, "recursive"):
            Analyzer(code).function(0x100)

    def test_stack_growing_loop_refuses(self):
        with self.assertRaisesRegex(Refusal, "stack frame|unbounded"):
            Analyzer(graph((0x100, 1, "push", "rax"), (0x101, 2, "jmp", "0x100"))).function(0x100)

    def test_tail_recursion_refuses(self):
        with self.assertRaisesRegex(Refusal, "recursive"):
            Analyzer(graph((0x100, 2, "jmp", "0x100"))).function(0x100)

    def test_stack_growth_is_measured_and_exceeds_coverage(self):
        code = graph((0x100, 7, "sub", "rsp, 0x10008"), (0x107, 7, "add", "rsp, 0x10008"), (0x10e, 1, "ret", ""))
        with self.assertRaisesRegex(Refusal, "stack window exceeded"):
            enforce_coverage(Analyzer(code).function(0x100)["bound"])

    def test_alternate_branch_maximum_included(self):
        code = graph((0x100, 2, "je", "0x110"), (0x102, 1, "ret", ""), (0x110, 4, "sub", "rsp, 0x78"), (0x114, 4, "add", "rsp, 0x78"), (0x118, 1, "ret", ""))
        self.assertEqual(Analyzer(code).function(0x100)["bound"], 120)

    def test_private_assembly_helper_call_return_slot_included(self):
        code = graph((0x100, 1, "pushfq", ""), (0x101, 5, "call", "0x200"), (0x106, 1, "popfq", ""), (0x107, 1, "ret", ""), (0x200, 1, "pushfq", ""), (0x201, 1, "popfq", ""), (0x202, 1, "ret", ""))
        self.assertEqual(Analyzer(code).function(0x100)["bound"], 24)

    def test_constant_alignment_is_explicit(self):
        self.assertEqual(stack_step(Instruction(1, 4, "and", "rsp, -16"), 0), 8)

    def test_unaligned_call_refuses(self):
        with self.assertRaisesRegex(Refusal, "unaligned"):
            Analyzer(graph((0x100, 5, "call", "0x200"), (0x105, 1, "ret", ""), (0x200, 1, "ret", ""))).function(0x100)

    def test_byte_disassembly_parser(self):
        code = disassembly("140001000: 48 83 ec 28   sub rsp, 0x28\n140001004: c3  ret\n")
        self.assertEqual(code[0x140001000].size, 4)
        self.assertEqual(code[0x140001004].op, "ret")


if __name__ == "__main__":
    unittest.main()
