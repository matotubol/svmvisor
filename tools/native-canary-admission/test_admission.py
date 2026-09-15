"""Assemble and inspect the real canary; simulate only its CPUID admission.

No assembled instructions are executed. Requires clang and llvm-objdump on PATH.
Run from any directory: python tools/native-canary-admission/test_admission.py
Evidence is retained under work/native-canary-admission.
"""
import hashlib
import re
import struct
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "crates/dxe/src/native/transition/canary.S"
WORK = ROOT / "work/native-canary-admission"
DEFAULT_TEXT_SHA256 = "c991ecc5c5e5925a3c4bcf1f933d87d276cc505c7242edd0288be97a775ad914"
# Reviewed bounded multi-exit transition; the original canary executable,
# relocation and admission-policy pins above/below remain unchanged.
TRANSITION_SHA256 = "f9e8e7193916b1034cc165b32e4075f8fe96ceeca46d52222760e94691b0ac06"
AMD = (0x68747541, 0x69746e65, 0x444D4163)  # EBX, EDX, ECX
TCG = (0x54474354, 0x47435447, 0x43544743)
SIGNATURE = 0x00B40F40
FEATURES = 0x07000020


def coff_text_and_relocations(path):
    raw = path.read_bytes()
    machine, count, _, symbols, symbol_count, optional, _ = struct.unpack_from("<HHIIIHH", raw)
    assert machine == 0x8664 and optional == 0
    string_table = symbols + 18 * symbol_count

    def symbol_name(index):
        name = raw[symbols + 18 * index: symbols + 18 * index + 8]
        if name[:4] == b"\0" * 4:
            start = string_table + struct.unpack_from("<I", name, 4)[0]
            return raw[start:raw.index(b"\0", start)].decode()
        return name.rstrip(b"\0").decode()

    sections, relocations = {}, []
    for index in range(count):
        offset = 20 + index * 40
        name = raw[offset:offset + 8].rstrip(b"\0").decode()
        size, start, relocs = struct.unpack_from("<III", raw, offset + 16)
        reloc_count = struct.unpack_from("<H", raw, offset + 32)[0]
        sections[name] = raw[start:start + size]
        for i in range(reloc_count):
            address, symbol, kind = struct.unpack_from("<IIH", raw, relocs + 10 * i)
            relocations.append((name, address, symbol_name(symbol), kind))
    assert all(not data for name, data in sections.items() if name != ".text")
    return sections[".text"], relocations


def instructions(path):
    output = subprocess.check_output(
        ["llvm-objdump", "-d", "--no-show-raw-insn", str(path)], text=True)
    path.with_suffix(".disasm.txt").write_text(output)
    rows = []
    for line in output.splitlines():
        match = re.match(r"\s*([0-9a-f]+):\s+(.+)", line)
        if match:
            code = match[2].split("#")[0].split("<")[0].strip().split(None, 1)
            rows.append((int(match[1], 16), code[0], code[1].strip() if len(code) == 2 else ""))
    return rows


def gate_bounds(rows):
    # The existing CPL guard ends immediately before the two build-specific gates.
    cpl = next(i for i, (_, op, args) in enumerate(rows) if (op, args) == ("movw", "%cs, %ax"))
    start = cpl + 3
    body = next(i for i, (_, op, args) in enumerate(rows) if (op, args) == ("movl", "%ecx, %r8d"))
    controls = next(i for i, (_, op, args) in enumerate(rows) if (op, args) == ("movq", "%cr0, %rax"))
    refusal = next(address for address, op, args in reversed(rows)
                   if (op, args) == ("movq", "0x10(%r10), %rax"))
    return start, body, controls, refusal


def simulate_gate(rows, leaves):
    """Interpret actual decoded integer/branch instructions until first CR read.

    This tests supplied CPUID observations, not real CPU/MSR/xstate behavior.
    Unknown instructions or branch destinations fail instead of being skipped.
    """
    start, _, end, refusal = gate_bounds(rows)
    registers = {name: 0 for name in ("%eax", "%ebx", "%ecx", "%edx", "%r8d")}
    zero = carry = False
    trace = []

    def value(operand):
        return int(operand[1:], 0) & 0xFFFFFFFF if operand.startswith("$") else registers[operand]

    for _, op, args in rows[start:end]:
        if op == "cpuid":
            leaf = registers["%eax"]
            trace.append((leaf, registers["%ecx"]))
            assert leaf in leaves, f"unexpected CPUID leaf {leaf:#x}"
            registers.update(zip(("%eax", "%ebx", "%ecx", "%edx"), leaves[leaf]))
        elif op in ("jb", "jne", "je"):
            assert int(args, 0) == refusal, "gate has another outgoing branch"
            if {"jb": carry, "jne": not zero, "je": zero}[op]:
                return False, trace
        else:
            source, destination = args.split(", ")
            left, right = value(source), value(destination)
            if op == "movl":
                registers[destination] = left
            elif op in ("andl", "xorl", "testl", "cmpl"):
                result = {"andl": right & left, "xorl": right ^ left,
                          "testl": right & left, "cmpl": (right - left) & 0xFFFFFFFF}[op]
                zero, carry = result == 0, op == "cmpl" and right < left
                if op in ("andl", "xorl"):
                    registers[destination] = result
            else:
                raise AssertionError(f"unmodeled gate instruction {op} {args}")
    return True, trace


def observations(vendor=AMD, signature=SIGNATURE, ecx=0, edx=FEATURES, maximum=1, tcg=TCG):
    return {0: (maximum, vendor[0], vendor[2], vendor[1]),
            1: (signature, 0, ecx, edx),
            0x40000000: (0x40000001, tcg[0], tcg[2], tcg[1])}


class CanaryAdmissionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        WORK.mkdir(parents=True, exist_ok=True)
        cls.objects = {}
        for mode in ("default", "native"):
            path = WORK / f"{mode}.obj"
            command = ["clang", "--target=x86_64-pc-windows-msvc", "-c", str(SOURCE), "-o", str(path)]
            if mode == "native":
                command.append("-DSVMVISOR_NATIVE_RETURNING=1")
            subprocess.run(command, check=True)
            cls.objects[mode] = (*coff_text_and_relocations(path), instructions(path))

    def test_default_executable_bytes_and_relocations_match_reviewed_original(self):
        text, relocs, _ = self.objects["default"]
        self.assertEqual(hashlib.sha256(text).hexdigest(), DEFAULT_TEXT_SHA256)
        self.assertEqual(relocs, [(".text", 0x449, "svmvisor_native_transition", 4)])
        self.assertEqual(hashlib.sha256((ROOT / "crates/dxe/src/native/transition/run.S").read_bytes()).hexdigest(),
                         TRANSITION_SHA256)

    def test_native_diff_is_only_gate_and_shifted_branch_displacements(self):
        old, old_relocs, before = self.objects["default"]
        new, new_relocs, after = self.objects["native"]
        start, body, _, _ = gate_bounds(before)
        native_start, native_body, _, _ = gate_bounds(after)
        self.assertEqual(before[start][0], after[native_start][0])
        shift = after[native_body][0] - before[body][0]
        self.assertEqual(shift, 29)
        self.assertEqual(old[before[body][0]:], new[after[native_body][0]:])
        self.assertEqual(new_relocs, [(name, address + shift, symbol, kind)
                                     for name, address, symbol, kind in old_relocs])
        self.assertEqual(start, native_start)
        for i, ((address, op, args), (new_address, new_op, new_args)) in enumerate(zip(before[:start], after[:native_start])):
            self.assertEqual((address, op), (new_address, new_op))
            end = before[i + 1][0]
            if op in ("je", "jne"):
                self.assertEqual(int(new_args, 0), int(args, 0) + shift)
                self.assertEqual(old[address:address + 2], new[address:address + 2])
                self.assertEqual(end - address, 6)
            else:
                self.assertEqual(args, new_args)
                self.assertEqual(old[address:end], new[address:end])

    def test_both_objects_keep_one_literal_efer_read_and_exact_transition_call(self):
        for _, relocs, rows in self.objects.values():
            self.assertEqual(len(relocs), 1)
            self.assertEqual(sum(op == "callq" for _, op, _ in rows), 1)
            self.assertEqual(sum(op == "rdmsr" for _, op, _ in rows), 1)
            site = next(i for i, (_, op, _) in enumerate(rows) if op == "rdmsr")
            self.assertEqual(rows[site - 1][1:], ("movl", "$0xc0000080, %ecx"))
            self.assertFalse(any(op in ("wrmsr", "xsetbv", "vmrun", "vmload", "vmsave", "clgi", "stgi")
                                 or (op.startswith("mov") and re.search(r", %cr\d+$", args))
                                 for _, op, args in rows))

    def test_native_accepts_only_positive_target_observations_at_this_gate(self):
        rows = self.objects["native"][2]
        self.assertEqual(simulate_gate(rows, observations()), (True, [(0, 0), (1, 0)]))
        self.assertTrue(simulate_gate(rows, observations(maximum=0xFFFFFFFF, ecx=0x7FFFFFFF, edx=0xFFFFFFFF))[0])
        self.assertFalse(simulate_gate(rows, observations(maximum=0))[0])
        self.assertFalse(simulate_gate(rows, observations(ecx=0x80000000))[0])
        for register in range(3):
            for bit in range(32):
                with self.subTest(vendor_register=register, bit=bit):
                    vendor = list(AMD)
                    vendor[register] ^= 1 << bit
                    self.assertFalse(simulate_gate(rows, observations(vendor=vendor))[0])
        for bit in range(32):
            with self.subTest(signature_bit=bit):
                self.assertFalse(simulate_gate(rows, observations(signature=SIGNATURE ^ (1 << bit)))[0])

    def test_required_shared_features_refuse_individually_in_both_modes(self):
        for mode, (_, _, rows) in self.objects.items():
            for bit in (5, 24, 25, 26):
                with self.subTest(mode=mode, absent_bit=bit):
                    self.assertFalse(simulate_gate(rows, observations(edx=FEATURES & ~(1 << bit)))[0])

    def test_tcg_has_no_native_fallback_and_native_has_no_tcg_fallback(self):
        default, native = self.objects["default"][2], self.objects["native"][2]
        self.assertEqual(simulate_gate(default, observations(ecx=0x80000000)),
                         (True, [(0x40000000, 0), (1, 0)]))
        for register in range(3):
            for bit in range(32):
                vendor = list(TCG)
                vendor[register] ^= 1 << bit
                self.assertFalse(simulate_gate(default, observations(tcg=vendor))[0])
        # A correct TCG leaf cannot override a native identity/hypervisor refusal.
        for observed in (observations(vendor=(0, 0, 0)), observations(signature=0),
                         observations(ecx=0x80000000)):
            admitted, trace = simulate_gate(native, observed)
            self.assertFalse(admitted)
            self.assertNotIn(0x40000000, [leaf for leaf, _ in trace])
        self.assertFalse(simulate_gate(default, observations(tcg=(0, 0, 0)))[0])


if __name__ == "__main__":
    unittest.main(verbosity=2)
