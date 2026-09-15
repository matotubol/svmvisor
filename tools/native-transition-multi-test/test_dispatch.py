"""Execute the exact integer-only production dispatcher in a host DLL.

This does not execute VMRUN/VMLOAD or prove physical restoration. The appended
test wrapper is absent from the production .text prefix, and its complete direct
control-flow graph is checked before execution. Requires Windows, clang and
lld-link. Evidence is retained in work/multi-exit-transition.
"""
from __future__ import annotations

import ctypes
import hashlib
import importlib.util
import json
import re
import struct
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK = ROOT / "work/multi-exit-transition"
SOURCE = ROOT / "crates/dxe/src/native/transition/run.S"
GUEST, JOURNAL = 704, 960
LEAVES = [0, 1, 0x40000000, 0x40000001, 0x80000000, 0x80000001,
          0xDEADBEEF, 0x40000002]
OUTPUTS = [
    [1, 0x566D7653, 0x74736554, 0x726F7369],
    [0, 0, 0x80000000, 0x60],
    [0x40000001, 0x566D7653, 0x726F7369, 0x74736554],
    [1, 0, 0, 0], [0x80000001, 0, 0, 0],
    [0, 0, 0, 0x20000000], [0, 0, 0, 0], [0, 0, 0, 0],
]
SENTINELS = [0x6D75000000000000 | x for x in
             [0x304, 0x305, 0x306, 0x308, 0x309, 0x30A, 0x30B, 0x30C, 0x30D, 0x30E]]


def qget(buffer, offset):
    return struct.unpack_from("<Q", buffer, offset)[0]


def qset(buffer, offset, value):
    struct.pack_into("<Q", buffer, offset, value & ((1 << 64) - 1))


def text_section(path):
    raw = path.read_bytes()
    machine, count = struct.unpack_from("<HH", raw)
    assert machine == 0x8664
    for index in range(count):
        offset = 20 + 40 * index
        if raw[offset:offset + 8].rstrip(b"\0") == b".text":
            size, start, _, _, relocs = struct.unpack_from("<IIIIH", raw, offset + 16)
            assert relocs == 0, "The standalone assembly must have no unresolved call/data relocation"
            return raw[start:start + size]
    raise AssertionError("no .text")


def build_and_check():
    WORK.mkdir(parents=True, exist_ok=True)
    production, testing = WORK / "production.obj", WORK / "host-test.obj"
    for out, defines in [(production, []), (testing, ["-DSVMVISOR_NATIVE_MULTI_HOST_TEST=1"])]:
        subprocess.run(["clang", "--target=x86_64-pc-windows-msvc", *defines,
                        "-c", str(SOURCE), "-o", str(out)], check=True)
    prefix, appended = text_section(production), text_section(testing)
    assert len(appended) > len(prefix) and appended[:len(prefix)] == prefix
    subprocess.run(["lld-link", "/dll", "/noentry", "/nodefaultlib", "/timestamp:0",
                    "/export:svmvisor_native_multi_dispatch_test", str(testing),
                    f"/out:{WORK / 'dispatch-test.dll'}"], check=True)
    disasm = subprocess.check_output(["llvm-objdump", "-d", "--x86-asm-syntax=intel",
                                     str(testing)], text=True)
    (WORK / "host-test.disasm.txt").write_text(disasm)
    spec = importlib.util.spec_from_file_location("audit", ROOT / "tools/native-stack-audit/audit.py")
    import sys
    audit = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = audit
    spec.loader.exec_module(audit)
    rows = audit.disassembly(disasm)
    entry = int(re.search(r"([0-9a-f]+) <svmvisor_native_multi_dispatch_test>:", disasm)[1], 16)
    # Traverse the wrapper and the exact production helper's entire direct CFG,
    # following both conditional branches and private calls, not a lucky trace.
    allowed = {"push", "pop", "sub", "add", "mov", "movabs", "and", "or", "xor",
               "cmp", "test", "lea", "inc", "dec", "bt", "call", "ret", "jmp"}
    pending, states, seen, calls = [(entry, 0, ())], set(), set(), set()
    while pending:
        pc, depth, returns = pending.pop()
        key = (pc, depth, returns)
        if key in states:
            continue
        states.add(key)
        seen.add(pc)
        inst = rows[pc]
        assert inst.op in allowed or inst.op.startswith("j"), inst
        if inst.op == "ret":
            if returns:
                destination, caller_depth = returns[-1]
                assert depth == caller_depth + 8, "private helper stack imbalance"
                pending.append((destination, caller_depth, returns[:-1]))
            else:
                assert depth == 0, "test wrapper stack imbalance"
            continue
        depth = audit.stack_step(inst, depth)
        if inst.op == "call":
            assert (8 - depth) % 16 == 0, f"unaligned private call at {pc:x}"
            calls.add(pc)
            pending.append((audit.direct_target(inst), depth + 8, returns + ((inst.next, depth),)))
            continue
        if inst.op.startswith("j"):
            pending.append((audit.direct_target(inst), depth, returns))
            if inst.op == "jmp":
                continue
        pending.append((inst.next, depth, returns))
    evidence = {
        "source_sha256": hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
        "production_text_bytes": len(prefix),
        "production_text_sha256": hashlib.sha256(prefix).hexdigest(),
        "host_test_production_prefix_exact": True,
        "test_wrapper_only_appended_bytes": len(appended) - len(prefix),
        "wrapper_reachable_integer_instructions": len(seen),
        "reachable_privileged_or_external_instructions": 0,
        "aligned_private_call_sites": len(calls),
        "all_private_return_stack_deltas_balanced": True,
    }
    (WORK / "host-execution-binding.json").write_text(json.dumps(evidence, indent=2) + "\n")
    dll = ctypes.WinDLL(str(WORK / "dispatch-test.dll"))
    function = dll.svmvisor_native_multi_dispatch_test
    function.argtypes, function.restype = [ctypes.c_void_p, ctypes.c_uint64], ctypes.c_uint64
    return dll, function


class State:
    def __init__(self):
        self.raw_context = ctypes.create_string_buffer(1088 + 63)
        self.raw_vmcb = ctypes.create_string_buffer(4096 + 4095)
        self.address = (ctypes.addressof(self.raw_context) + 63) & ~63
        self.vmcb_address = (ctypes.addressof(self.raw_vmcb) + 4095) & ~4095
        self.c = (ctypes.c_ubyte * 1088).from_address(self.address)
        self.v = (ctypes.c_ubyte * 4096).from_address(self.vmcb_address)
        for offset, value in [(0, 1), (8, 1088), (32, self.vmcb_address),
                              (136, 0x10FF), (144, 1), (184, 2)]:
            qset(self.c, offset, value)

    def reserve(self, index):
        return qget(self.c, GUEST + 184 + index * 8)

    def capture(self, step):
        # Supply hardware-equivalent fields for one exit. No physical instruction
        # is executed by this fixture. The actual helper owns all state changes.
        round_, query = divmod(step, 2)
        if step == 64:
            code, rip, rax, gprs = 0x81, 0x10FF, 1, [32, 32, 0, *SENTINELS, 32]
        elif not query:
            code, rip = 0x72, 0x1086
            rax = 0xAABBCCDD00000000 | LEAVES[round_ % 8]
            gprs = [0x1122334400000000, round_, 0x778899AA00000303, *SENTINELS, round_]
        else:
            code, rip, rax = 0x81, 0x10BF, 0
            gprs = [0, round_, OUTPUTS[round_ % 8][1], *SENTINELS, round_]
        fields = [code, 0x123456789ABCDEF0, 0xFFEEBBAA99887766, 7, rip, 0x9000, 2, rax,
                  *gprs, 15]
        for i, value in enumerate(fields):
            qset(self.c, GUEST + 8 * i, value)
        qset(self.c, JOURNAL + 64, step + 1)
        qset(self.c, JOURNAL + 72, step + 1)
        qset(self.v, 0x578, rip)
        qset(self.v, 0x5F8, rax)
        qset(self.v, 0xC8, rip + (2 if code == 0x72 else 3))
        # Make the resumption dirty/flush writes observable.
        qset(self.v, 0xC0, 0xFFFFFFFFFFFFFFFF)
        struct.pack_into("<I", self.v, 0x5C, 0xA5A5A5A5)

    def before(self, step, dispatch, nrips=8):
        for previous in range(step):
            self.capture(previous)
            assert dispatch(self.address, nrips) == 1
        self.capture(step)
        return self


class DispatchTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dll, cls.dispatch = build_and_check()

    def reject(self, state, expected, nrips=8, outcome=7):
        raw_guest, raw_vmcb = bytes(state.c)[GUEST:GUEST + 184], bytes(state.v)
        counts = [state.reserve(i) for i in range(3)]
        actual = [qget(state.c, JOURNAL + i) for i in [64, 72]]
        self.assertEqual(self.dispatch(state.address, nrips), 0)
        self.assertEqual(qget(state.c, JOURNAL + 8), outcome)
        self.assertEqual(state.reserve(3), expected)
        self.assertEqual(bytes(state.c)[GUEST:GUEST + 184], raw_guest)
        self.assertEqual(bytes(state.v), raw_vmcb)
        self.assertEqual([state.reserve(i) for i in range(3)], counts)
        self.assertEqual([qget(state.c, JOURNAL + i) for i in [64, 72]], actual)

    def test_complete_65_exit_traces_with_and_without_nrips(self):
        for nrips in [0, 8]:
            state = State()
            for step in range(65):
                state.capture(step)
                before_guest = bytes(state.c)[GUEST:GUEST + 184]
                if nrips == 0:
                    qset(state.v, 0xC8, 0xFFFF000012340000)
                before_vmcb = bytes(state.v)
                result = self.dispatch(state.address, nrips)
                self.assertEqual(result, int(step < 64))
                self.assertEqual(state.reserve(3), 0)
                self.assertEqual(state.reserve(6), int(nrips != 0))
                self.assertEqual(state.reserve(7), 0)
                self.assertEqual(state.reserve(8), 0)
                if step < 64:
                    self.assertEqual(state.reserve(2), step + 1)
                    self.assertEqual(qget(state.v, 0xC0), 0)
                    self.assertEqual(struct.unpack_from("<I", state.v, 0x5C)[0], 1)
                    self.assertEqual(qget(state.v, 0x578), 0x1088 if step % 2 == 0 else 0x10C2)
                    if step % 2 == 0:
                        expected = OUTPUTS[(step // 2) % 8]
                        self.assertEqual([qget(state.v, 0x5F8), qget(state.c, GUEST + 80),
                                          qget(state.c, GUEST + 64), qget(state.c, GUEST + 72)], expected)
                        # Only RCX/RDX/RBX software return values may change.
                        self.assertEqual(bytes(state.c)[GUEST + 88:GUEST + 184], before_guest[88:184])
                        self.assertEqual(bytes(state.c)[GUEST:GUEST + 64], before_guest[:64])
                    else:
                        self.assertEqual(qget(state.v, 0x5F8), 1)
                        self.assertEqual(bytes(state.c)[GUEST:GUEST + 184], before_guest)
                else:
                    self.assertEqual(qget(state.c, JOURNAL + 8), 12)
                    self.assertEqual([state.reserve(i) for i in range(3)], [32, 32, 64])
                    self.assertEqual(bytes(state.v), before_vmcb)
                    self.assertEqual(bytes(state.c)[GUEST:GUEST + 184], before_guest)

    def test_every_rejected_continuation_preserves_raw_and_partial_counts(self):
        for step in range(65):
            with self.subTest(step=step):
                state = State().before(step, self.dispatch)
                qset(state.v, 0xC8, qget(state.v, 0xC8) + 1)
                self.reject(state, 7)
                state = State().before(step, self.dispatch)
                qset(state.c, GUEST + 32, qget(state.c, GUEST + 32) + 1)
                self.reject(state, 2)
                state = State().before(step, self.dispatch)
                qset(state.c, GUEST + 24, (1 << 31) | 7)
                self.reject(state, 6)

    def test_all_64_bits_of_all_persistent_gprs_at_all_phase_shapes(self):
        for step in [0, 1, 16, 17, 62, 63, 64]:
            for index in range(3, 14):
                for bit in range(64):
                    state = State().before(step, self.dispatch)
                    offset = GUEST + 64 + index * 8
                    qset(state.c, offset, qget(state.c, offset) ^ (1 << bit))
                    self.reject(state, 4)

    def test_all_64_bits_of_protocol_operands_rejected(self):
        for step in [0, 1, 2, 3, 4, 5, 62, 63, 64]:
            for field in [56, 64, 72, 80]:
                for bit in range(64):
                    state = State().before(step, self.dispatch)
                    offset = GUEST + field
                    qset(state.c, offset, qget(state.c, offset) ^ (1 << bit))
                    self.reject(state, 3)

    def test_wrong_exit_classes_terminate_without_fake_resume(self):
        for step in [0, 1, 20, 21, 62, 63, 64]:
            for exit_code, outcome, failure in [
                (0x60, 10, 0), (0x61, 3, 0), (0x63, 4, 0),
                (0xFFFFFFFFFFFFFFFF, 6, 0), (0x40, 5, 0), (0x46, 5, 0),
                (0x5F, 5, 0), (0x78, 7, 8), (0x400, 7, 8),
                (0x81 if step % 2 == 0 and step < 64 else 0x72, 7, 8),
            ]:
                state = State().before(step, self.dispatch)
                qset(state.c, GUEST, exit_code)
                self.reject(state, failure, outcome=outcome)
                if exit_code not in [0x72, 0x81]:
                    self.assertEqual(state.reserve(6), 0)

    def test_corrupt_count_and_order_boundaries(self):
        for step in [0, 1, 2, 63, 64]:
            for offset in [JOURNAL + 64, JOURNAL + 72, GUEST + 184,
                           GUEST + 192, GUEST + 200]:
                for value in [0, 1, 31, 32, 33, 64, 65, 66, 0xFFFFFFFFFFFFFFFF]:
                    state = State().before(step, self.dispatch)
                    if qget(state.c, offset) == value:
                        continue
                    qset(state.c, offset, value)
                    self.reject(state, 1)
        # Equal totals with an impossible pair ordering must still reject.
        state = State().before(3, self.dispatch)
        qset(state.c, GUEST + 184, 3)
        qset(state.c, GUEST + 192, 0)
        self.reject(state, 1)

    def test_stack_and_all_disallowed_flag_bits(self):
        for step in [0, 1, 64]:
            for bit in range(64):
                state = State().before(step, self.dispatch)
                qset(state.c, GUEST + 40, 0x9000 ^ (1 << bit))
                self.reject(state, 5)
                if (1 << bit) & 0x8D7:
                    continue
                state = State().before(step, self.dispatch)
                qset(state.c, GUEST + 48, 2 | (1 << bit))
                self.reject(state, 5)
            state = State().before(step, self.dispatch)
            qset(state.c, GUEST + 48, 0)
            self.reject(state, 5)
            for arithmetic in range(64):
                state = State().before(step, self.dispatch)
                flags = 2
                for i, bit in enumerate([0, 2, 4, 6, 7, 11]):
                    if arithmetic & (1 << i):
                        flags |= 1 << bit
                qset(state.c, GUEST + 48, flags)
                self.assertEqual(self.dispatch(state.address, 8), int(step < 64))


if __name__ == "__main__":
    unittest.main(verbosity=2)
