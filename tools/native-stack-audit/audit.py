"""Fail-closed stack/CFG audit of an exact linked native-returning PE.

Only run.py creates production evidence. This module also supports tiny synthetic
instruction graphs for rejection tests; those cannot create a passing build record.
"""
from __future__ import annotations

import hashlib
import json
import re
import struct
from collections import deque
from dataclasses import dataclass
from pathlib import Path


class Refusal(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise Refusal(message)


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


@dataclass(frozen=True)
class Instruction:
    address: int
    size: int
    op: str
    args: str = ""

    @property
    def next(self):
        return self.address + self.size


def disassembly(text):
    result = {}
    for line in text.splitlines():
        match = re.match(r"^\s*([0-9a-fA-F]+):\s+((?:[0-9a-fA-F]{2}\s+)+)\s*(\S+)(?:\s+(.*))?$", line)
        if not match:
            continue
        address, raw, op, args = match.groups()
        result[int(address, 16)] = Instruction(int(address, 16), len(raw.split()), op, (args or "").split(" #")[0])
    require(result, "empty linked disassembly")
    return result


def symbols_from_map(text):
    result = {}
    for line in text.splitlines():
        match = re.match(r"\s*[0-9a-fA-F]{4}:[0-9a-fA-F]+\s+(\S+)\s+([0-9a-fA-F]{16})\s", line)
        if match:
            name, address = match.groups()
            value = int(address, 16)
            require(name not in result or result[name] == value, f"ambiguous map symbol {name}")
            result[name] = value
    require(result, "empty linker symbol map")
    return result


def direct_target(inst):
    match = re.fullmatch(r"(0x[0-9a-fA-F]+)(?:\s+<.*>)?", inst.args)
    require(match is not None, f"unknown/indirect transfer at {inst.address:x}: {inst.op} {inst.args}")
    return int(match[1], 16)


def immediate(value):
    require(re.fullmatch(r"-?(?:0x[0-9a-fA-F]+|[0-9]+)", value) is not None, f"nonconstant stack operand {value}")
    return int(value, 0)


def stack_step(inst, depth, entry_mod=8, known_modulus=16):
    """Exact downward RSP delta. EFI function entry is 8 mod 16.

    Entry modulo 64 is supplied separately for the outer boundary alignment.
    A constant AND can be calculated only when its alignment <= known modulus.
    """
    op, args = inst.op, inst.args
    operands = [item.strip() for item in args.split(",")]
    first = operands[0] if operands else ""
    if op.startswith(("push", "pop")):
        require(op in {"push", "pushfq", "pop", "popfq"}, f"unsupported stack instruction {op}")
    if op in {"push", "pushfq"}:
        require(not args.startswith(("word ptr", "cs", "ss", "ds", "es", "fs", "gs")), "unsupported narrow push")
        require(not re.fullmatch(r"(?:[abcd]x|[sb]p|[sd]i|r(?:[89]|1[0-5])w)", args), "unsupported 16-bit push")
        return depth + 8
    if op in {"pop", "popfq"}:
        require(first not in {"rsp", "esp", "sp"} and not args.startswith("word ptr"), "unsupported pop/stack switch")
        require(not re.fullmatch(r"(?:[abcd]x|[sb]p|[sd]i|r(?:[89]|1[0-5])w)", args), "unsupported 16-bit pop")
        require("[" not in args, "memory-destination POP needs an explicit stack convention")
        return depth - 8
    require(op not in {"enter", "leave", "iret", "iretd", "iretq", "retf", "lret", "lretq", "lcall", "ljmp", "syscall", "sysret", "sysretq", "sysenter", "sysexit", "int", "into"}, f"unreviewed implicit stack/control operation {op}")
    if first in {"rsp", "esp", "sp", "spl"}:
        require(first == "rsp", f"partial stack-pointer write {op} {args}")
        if op in {"sub", "add"} and len(operands) == 2:
            delta = immediate(operands[1])
            return depth + (delta if op == "sub" else -delta)
        if op == "lea" and len(operands) == 2:
            match = re.fullmatch(r"\[rsp(?: ([+-]) (0x[0-9a-fA-F]+|[0-9]+))?\]", operands[1])
            require(match is not None, f"dynamic stack address {args}")
            sign, amount = match.groups()
            delta = int(amount, 0) if amount else 0
            return depth - (delta if sign != "-" else -delta)
        if op == "and" and len(operands) == 2:
            mask = immediate(operands[1]) & ((1 << 64) - 1)
            alignment = ((~mask) & ((1 << 64) - 1)) + 1
            require(alignment <= known_modulus and alignment > 0 and alignment & (alignment - 1) == 0, f"unsupported stack alignment {alignment}")
            return depth + ((entry_mod - depth) % alignment)
        if op in {"cmp", "test"}:
            return depth
        raise Refusal(f"dynamic/unreviewed RSP write at {inst.address:x}: {op} {args}")
    require(not (op in {"xchg", "xadd"} and any(x in {"rsp", "esp", "sp", "spl"} for x in operands)), "stack exchange")
    return depth


def stack_memory(inst, depth, compiler_frame=False):
    """Track direct stack addresses; pointers to borrowed objects are separate operands.

    Indexed RSP addresses are refused. LEA is not a memory access. All standard
    x86 widths are handled, with 512 bytes for FXSAVE and refusal for dynamic XSAVE
    on RSP (the real transition uses separately admitted arena pointers).
    """
    low, high = depth, -depth
    if inst.op == "lea":
        return low, high
    for match in re.finditer(r"(?:(byte|word|dword|qword|xmmword|ymmword|zmmword) ptr )?\[([^]]*\brsp\b[^]]*)\]", inst.args):
        width, address = match.groups()
        offset = re.fullmatch(r"rsp(?: ([+-]) (0x[0-9a-fA-F]+|[0-9]+))?", address)
        if offset is None and compiler_frame:
            # A compiler-generated indexed local is a memory-safety/allocated-
            # object obligation, not a dynamic RSP allocation. The emitted
            # noredzone contract places valid locals in the measured frame.
            continue
        require(offset is not None, f"indexed assembly stack operand at {inst.address:x}: {address}")
        sign, number = offset.groups()
        delta = int(number, 0) * (-1 if sign == "-" else 1) if number else 0
        size = {"byte": 1, "word": 2, "dword": 4, "qword": 8, "xmmword": 16, "ymmword": 32, "zmmword": 64}.get(width)
        if size is None:
            require(inst.op in {"fxsave64", "fxrstor64", "sgdt", "sidt", "lgdt", "lidt"}, f"unknown stack access width at {inst.address:x}")
            size = 512 if inst.op.startswith("fx") else 10
        low = max(low, depth - delta)
        high = max(high, delta + size - depth)
    return low, high


class Analyzer:
    def __init__(self, instructions, symbols=None, executable_ranges=None, compiler_ranges=()):
        self.code = instructions
        self.symbols = symbols or {}
        self.names = {address: name for name, address in self.symbols.items()}
        self.ranges = executable_ranges
        self.compiler_ranges = compiler_ranges
        self.cache = {}
        self.calls = {}
        self.entries = {direct_target(inst) for inst in instructions.values() if inst.op == "call" and re.fullmatch(r"0x[0-9a-fA-F]+(?:\s+<.*>)?", inst.args)}

    def label(self, address):
        return self.names.get(address, f"0x{address:x}")

    def instruction(self, address):
        require(address in self.code, f"unknown target/noninstruction at {address:x}")
        inst = self.code[address]
        require(inst.op not in {"<unknown>", ".byte"}, f"undecodable reachable instruction at {address:x}")
        if self.ranges is not None:
            require(any(start <= address and inst.next <= end for start, end in self.ranges), f"transfer outside verified executable object bytes at {address:x}")
        return inst

    def successors(self, inst):
        if inst.op == "ret":
            require(not inst.args, "callee-cleanup RET is outside EFI x64 ABI")
            return []
        if inst.op in {"ud2", "int3"}:
            raise Refusal(f"reachable trap at {inst.address:x}")
        if inst.op == "jmp":
            return [direct_target(inst)]
        if inst.op.startswith("j") or inst.op.startswith("loop"):
            return [direct_target(inst), inst.next]
        return [inst.next]

    def walk(self, start, stops=(), ancestry=(), critical=True, allowed=None, initial_depth=0, entry_mod=8, known_modulus=16):
        """All branch alternatives are included, even mutually exclusive conditions.

        A join must have identical stack depth. This rejects dynamic loops and
        stack-growing cycles without an arbitrary iteration bound or path pruning.
        Calls are summarized recursively; firmware calls may be opaque ONLY in
        the outer compiler-frame pass before/after the marked HIGH interval.
        """
        require(start not in ancestry, f"recursive critical call graph: {[self.label(x) for x in (*ancestry, start)]}")
        seen, queue = {}, deque([(start, initial_depth)])
        bound, upper, path = max(0, initial_depth), -initial_depth, [self.label(start)]
        edges, reached, returns = [], {}, 0
        while queue:
            address, depth = queue.popleft()
            if allowed is not None and address not in allowed:
                continue
            if address in seen:
                require(seen[address] == depth, f"inconsistent/unbounded stack at {address:x}: {seen[address]} versus {depth}")
                continue
            seen[address] = depth
            if address in stops:
                reached[address] = depth
                continue
            inst = self.instruction(address)
            after = stack_step(inst, depth, entry_mod, known_modulus)
            require(after >= 0 or not critical, f"stack unwound above entry at {address:x}")
            compiler_frame = any(low <= address < high for low, high in self.compiler_ranges)
            local, local_upper = stack_memory(inst, depth, compiler_frame) if critical else (depth, -depth)
            if max(local, after) > bound:
                bound, path = max(local, after), [self.label(start), f"0x{address:x}"]
            upper = max(upper, local_upper)
            if inst.op == "call":
                if critical:
                    target = direct_target(inst)
                    self.instruction(target)
                    require((8 - depth) % 16 == 0, f"unaligned critical CALL at {address:x}")
                    child = self.function(target, (*ancestry, start))
                    candidate = depth + 8 + child["bound"]
                    if candidate > bound:
                        bound, path = candidate, [self.label(start), *child["path"]]
                    upper = max(upper, child["upper"] - depth - 8)
                    edges.append({"site": hex(address), "target": self.label(target), "rsp_depth": depth, "return_slot": 8, "callee_bound": child["bound"]})
                queue.append((inst.next, after))
                continue
            if inst.op == "ret":
                require(not critical or after == 0, f"unbalanced RET at {address:x}: depth {after}")
                returns += 1
            for target in self.successors(inst):
                if critical and target != inst.next and (target in self.entries or target == start):
                    require(after == 0, f"tail transfer with outstanding stack frame at {address:x}")
                    child = self.function(target, (*ancestry, start))
                    if child["bound"] > bound:
                        bound, path = child["bound"], [self.label(start), *child["path"]]
                    upper = max(upper, child["upper"])
                    edges.append({"site": hex(address), "target": self.label(target), "rsp_depth": 0, "return_slot": 0, "callee_bound": child["bound"], "tail": True})
                    continue
                queue.append((target, after))
        return {"bound": bound, "upper": upper, "path": path, "edges": edges, "depths": seen, "stops": reached, "returns": returns}

    def control_graph(self, start, stops=()):
        graph, queue = {}, deque([start])
        while queue:
            address = queue.popleft()
            if address in graph:
                continue
            if address in stops:
                graph[address] = []
                continue
            inst = self.instruction(address)
            graph[address] = self.successors(inst)
            queue.extend(graph[address])
        return graph

    def paths_to(self, start, stop):
        graph = self.control_graph(start, {stop})
        reverse = {}
        for source, targets in graph.items():
            for target in targets:
                reverse.setdefault(target, []).append(source)
        keep, queue = set(), deque([stop])
        while queue:
            address = queue.popleft()
            if address not in keep:
                keep.add(address)
                queue.extend(reverse.get(address, []))
        require(start in keep, f"target {stop:x} unreachable from {start:x}")
        return keep

    def function(self, start, ancestry=()):
        require(start not in ancestry, f"recursive critical call graph at {self.label(start)}")
        if start not in self.cache:
            report = self.walk(start, ancestry=ancestry)
            self.cache[start] = report
            self.calls[self.label(start)] = {key: report[key] for key in ("bound", "upper", "path", "edges", "returns")}
        return self.cache[start]


class Coff:
    def __init__(self, path):
        self.path = Path(path)
        self.data = self.path.read_bytes()
        machine, count, _, symbol_offset, symbol_count, optional, _ = struct.unpack_from("<HHIIIHH", self.data)
        require(machine == 0x8664 and optional == 0, f"unsupported COFF format: {path}")
        strings = symbol_offset + symbol_count * 18

        def name(raw):
            if raw[:4] == b"\0" * 4:
                offset = struct.unpack_from("<I", raw, 4)[0] + strings
                return self.data[offset:self.data.index(0, offset)].decode()
            if raw.startswith(b"/"):
                offset = strings + int(raw[1:].rstrip(b"\0"))
                return self.data[offset:self.data.index(0, offset)].decode()
            return raw.rstrip(b"\0").decode()

        self.sections = []
        for index in range(count):
            raw, _, _, size, ptr, rel, _, nrel, _, flags = struct.unpack_from("<8sIIIIIIHHI", self.data, 20 + 40 * index)
            require(nrel != 65535, "COFF relocation overflow unsupported")
            relocations = [struct.unpack_from("<IIH", self.data, rel + i * 10) for i in range(nrel)]
            self.sections.append({"name": name(raw), "raw": self.data[ptr:ptr + size], "relocations": relocations, "flags": flags, "symbols": []})
        self.symbols = {}
        index = 0
        while index < symbol_count:
            raw, value, section, typ, storage, aux = struct.unpack_from("<8sIhHBB", self.data, symbol_offset + index * 18)
            symbol = {"name": name(raw), "value": value, "section": section, "type": typ, "storage": storage}
            self.symbols[index] = symbol
            if section > 0:
                self.sections[section - 1]["symbols"].append(symbol)
            index += 1 + aux

    def executable_signature(self):
        return [(s["name"], s["raw"], [(off, typ, self.symbols[target]["name"]) for off, target, typ in s["relocations"]]) for s in self.sections if s["flags"] & 0x20000000 and s["raw"]]


class PE:
    def __init__(self, path):
        self.data = Path(path).read_bytes()
        require(self.data[:2] == b"MZ", "missing DOS signature")
        pe = struct.unpack_from("<I", self.data, 0x3c)[0]
        require(self.data[pe:pe + 4] == b"PE\0\0", "missing PE signature")
        machine, count, _, _, _, optional, _ = struct.unpack_from("<HHIIIHH", self.data, pe + 4)
        require(machine == 0x8664 and struct.unpack_from("<H", self.data, pe + 24)[0] == 0x20b, "not AMD64 PE32+")
        require(struct.unpack_from("<H", self.data, pe + 24 + 68)[0] == 11, "not EFI boot service driver")
        self.base = struct.unpack_from("<Q", self.data, pe + 24 + 24)[0]
        self.sections = []
        for index in range(count):
            raw, virtual_size, rva, size, ptr, _, _, _, _, flags = struct.unpack_from("<8sIIIIIIHHI", self.data, pe + 24 + optional + index * 40)
            self.sections.append((self.base + rva, size, ptr, flags))

    def bytes(self, address, count):
        for start, size, ptr, _ in self.sections:
            if start <= address and address + count <= start + size:
                return self.data[ptr + address - start:ptr + address - start + count]
        raise Refusal(f"PE extent not backed by bytes {address:x}+{count}")


def contributions_from_lld_map(text, image_base):
    """Retain exact object/section extents; symbols and output rows are excluded."""
    rows = []
    for line in text.splitlines():
        match = re.fullmatch(r"\s*([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+([0-9]+)\s+(.+):\(([^()]*)\)\s*", line)
        if match:
            address, size, alignment, owner, name = match.groups()
            rows.append((image_base + int(address, 16), int(size, 16), int(alignment), owner, name))
    return rows


def relocated_section(pe, obj, section, base, bases, symbols):
    patched = bytearray(section["raw"])
    for offset, target_index, typ in section["relocations"]:
        target = obj.symbols[target_index]
        if target["name"] in symbols:
            destination = symbols[target["name"]]
        else:
            require(target["section"] in bases, f"unresolved COFF relocation {target['name']}")
            destination = bases[target["section"]] + target["value"]
        width = 8 if typ == 1 else 4
        require(0 <= offset <= len(patched) - width, "COFF relocation outside section")
        if 4 <= typ <= 9:
            value = destination + struct.unpack_from("<i", patched, offset)[0] - (base + offset + typ)
            struct.pack_into("<i", patched, offset, value)
        elif typ == 1:
            value = destination + struct.unpack_from("<Q", patched, offset)[0]
            struct.pack_into("<Q", patched, offset, value)
        elif typ == 3:
            value = destination - pe.base + struct.unpack_from("<I", patched, offset)[0]
            struct.pack_into("<I", patched, offset, value)
        else:
            raise Refusal(f"unknown executable COFF relocation {typ}")
    return bytes(patched)


def bind_anonymous_readonly(pe, obj, bases, symbols, contributions):
    """Bind anonymous constant tables through exact LLD ownership and bytes.

    Only referenced read-only initialized data is eligible. Its full contents,
    including every relocation, must match a unique retained contribution from
    the same object, whose identity is established by named executable extents.
    Data never enters the executable ranges used by the control-flow analyzer.
    """
    needed = set()
    for index, section in enumerate(obj.sections, 1):
        if index in bases and section["flags"] & 0x20000000:
            for _, target_index, _ in section["relocations"]:
                target = obj.symbols[target_index]
                if target["name"] not in symbols and target["section"] not in bases:
                    needed.add(target["section"])
    if not needed:
        return []
    require(contributions, "unresolved COFF relocation requires LLD contribution map")
    owners = set()
    for index, section in enumerate(obj.sections, 1):
        if index in bases and section["flags"] & 0x20000000 and section["raw"]:
            matches = {owner for address, size, _, owner, name in contributions if address == bases[index] and size == len(section["raw"]) and name == section["name"]}
            require(len(matches) == 1, "ambiguous/missing executable LLD contribution")
            owners.update(matches)
    require(len(owners) == 1, "inconsistent executable LLD object ownership")
    owner = owners.pop()
    reports = []
    for index in sorted(needed):
        require(1 <= index <= len(obj.sections), "unknown external COFF relocation")
        section = obj.sections[index - 1]
        require(section["flags"] & 0xe0000040 == 0x40000040 and section["raw"], "anonymous relocation target is not read-only initialized data")
        candidates = {(address, alignment) for address, size, alignment, source, name in contributions if source == owner and name == section["name"] and size == len(section["raw"])}
        matched = []
        for address, alignment in candidates:
            require(alignment > 0 and alignment & (alignment - 1) == 0, "invalid LLD data alignment")
            require(address % alignment == 0, "misaligned LLD data contribution")
            backing = [extent for extent in pe.sections if extent[0] <= address and address + len(section["raw"]) <= extent[0] + extent[1]]
            require(len(backing) == 1, "ambiguous/missing PE backing for anonymous data")
            require(backing[0][3] & 0xc0000000 == 0x40000000, "anonymous data PE backing is not read-only")
            # UEFI lld merges .rdata into executable .text. The COFF data
            # restriction and exclusion from verified CFG ranges remain strict;
            # an executable output-section flag does not turn data into code.
            candidate_bases = dict(bases)
            candidate_bases[index] = address
            patched = relocated_section(pe, obj, section, address, candidate_bases, symbols)
            if patched == pe.bytes(address, len(patched)):
                matched.append((address, patched))
        require(len(matched) == 1, "anonymous read-only contribution missing, changed or ambiguous")
        address, patched = matched[0]
        bases[index] = address
        reports.append({"section": index, "name": section["name"], "address": hex(address), "bytes": len(patched), "sha256": hashlib.sha256(patched).hexdigest(), "lld_object": owner})
    return reports


def verify_objects(pe, objects, symbols, lld_map_text=""):
    """Reapply every supported executable COFF relocation and compare actual PE.

    No relocation bytes are masked. REL32 targets, including canary->transition,
    are resolved against exact linker symbols or byte-verified anonymous
    read-only LLD contributions. All machine bytes must match.
    """
    verified, reports = [], []
    contributions = contributions_from_lld_map(lld_map_text, pe.base)
    for obj in objects:
        bases = {}
        for index, section in enumerate(obj.sections, 1):
            # Static Rust function symbols are deliberately included; only section
            # definition names (.text/.rdata) are excluded because they repeat.
            candidates = {symbols[s["name"]] - s["value"] for s in section["symbols"] if s["name"] in symbols and (not s["name"].startswith(".") or s["type"] & 0x20)}
            require(len(candidates) <= 1, f"ambiguous linked contribution {obj.path.name}:{index}")
            if candidates:
                bases[index] = candidates.pop()
        anonymous = bind_anonymous_readonly(pe, obj, bases, symbols, contributions)
        linked = 0
        for index, section in enumerate(obj.sections, 1):
            if not section["flags"] & 0x20000000 or not section["raw"] or index not in bases:
                continue
            base = bases[index]
            patched = relocated_section(pe, obj, section, base, bases, symbols)
            require(patched == pe.bytes(base, len(patched)), f"object/PE machine bytes differ: {obj.path.name}, section {index} at {base:x}")
            verified.append((base, base + len(patched)))
            linked += len(patched)
        require(linked > 0, f"no linked executable bytes from {obj.path}")
        reports.append({"object": str(obj.path), "sha256": sha256(obj.path), "verified_linked_code_bytes": linked, "anonymous_readonly_bindings": anonymous})
    return verified, reports


def compiler_function(asm, marker):
    """Compiler function boundaries survive internal global audit labels."""
    current, found = None, []
    for line in asm.splitlines():
        match = re.match(r"^\s*\.def\s+(\S+);", line)
        if match:
            current = match[1]
        if line.strip() == marker + ":":
            found.append(current)
        if line.startswith(".Lfunc_end"):
            current = None
    require(len(found) == 1 and found[0], f"missing/duplicate compiler marker {marker}")
    return found[0]


def check_panic_and_probe_references(llvm_text, symbols):
    # LLVM records the selected probe implementation as an attribute on even
    # tiny functions. That configuration string is not an emitted reference.
    # Backend-generated calls may appear only in the actual linked object/map,
    # so inspect that symbol set as well as optimized IR references.
    references = re.sub(r'"probe-stack"="[^"\n]*"', "", llvm_text)
    forbidden = r"svmvisor_dxe_must_not_panic|panic_bounds_check|panic_fmt|__rust_probestack|__chkstk"
    require(re.search(forbidden, references) is None, "panic/stack-probe reference survived optimized IR")
    require(not any(re.search(forbidden, name) for name in symbols), "panic/stack-probe symbol survived actual link")


def audit_build(asm_text, llvm_text, map_text, linked_disassembly, pe, objects, lld_map_text=""):
    symbols = symbols_from_map(map_text)
    for marker in ("svmvisor_native_stack_sample", "svmvisor_native_high_begin", "svmvisor_native_high_end", "svmvisor_native_returning_high", "svmvisor_native_transition_canary", "svmvisor_native_transition"):
        require(marker in symbols, f"missing linked marker {marker}")
    require("noredzone" in llvm_text and "minsize" in llvm_text and "optsize" in llvm_text, "missing optimized/no-red-zone IR attributes")
    check_panic_and_probe_references(llvm_text, symbols)
    sample, begin, end = (symbols[x] for x in ("svmvisor_native_stack_sample", "svmvisor_native_high_begin", "svmvisor_native_high_end"))
    owner = compiler_function(asm_text, "svmvisor_native_stack_sample")
    require(owner == compiler_function(asm_text, "svmvisor_native_high_begin") == compiler_function(asm_text, "svmvisor_native_high_end"), "sample and full HIGH interval must share one final compiler frame")
    require(owner in symbols, "compiler owner missing from linker map")
    actual_owners = []
    for marker in ("svmvisor_native_stack_sample", "svmvisor_native_high_begin", "svmvisor_native_high_end"):
        sections = [section for section in objects[0].sections if any(s["name"] == marker for s in section["symbols"])]
        require(len(sections) == 1, f"marker {marker} not unique in actual linked Rust object")
        names = [s["name"] for s in sections[0]["symbols"] if s["type"] & 0x20 and s["value"] == 0]
        require(names == [owner], f"sibling .s marker owner differs from actual linked Rust object: {marker}")
        actual_owners.append(names[0])
    ranges, object_reports = verify_objects(pe, objects, symbols, lld_map_text)
    compiler_ranges, _ = verify_objects(pe, objects[:1], symbols, lld_map_text)
    analysis = Analyzer(disassembly(linked_disassembly), symbols, ranges, compiler_ranges)
    sample_inst = analysis.instruction(sample)
    require(sample_inst.op == "mov" and re.fullmatch(r"r(?:[abcd]x|bp|[sd]i|[89]|1[0-5]), rsp", sample_inst.args), "sample marker is not the exact MOV reg,RSP")
    caller = analysis.walk(symbols[owner], critical=False)
    require(all(x in caller["depths"] for x in (sample, begin, end)), "unreachable caller marker")
    sample_depth, begin_depth = caller["depths"][sample], caller["depths"][begin]
    require(sample_depth >= 0 and begin_depth >= 0, "invalid caller-frame depths")
    without_sample = analysis.control_graph(symbols[owner], {sample})
    require(begin not in without_sample, "stack sample does not dominate HIGH entry")
    # Establish the retained upper-frame relationship from this same linked
    # entry wrapper. Examine only paths that actually call the Rust owner, so
    # early refusal/restoration does not become a fictional admission path.
    require(owner == "svmvisor_native_efi_main_inner", "sample owner must be the real assembly-entry Rust callee")
    boundary_code = analysis.control_graph(symbols["efi_main"])
    inner_sites = [address for address in boundary_code if analysis.instruction(address).op == "call" and direct_target(analysis.instruction(address)) == symbols[owner]]
    require(len(inner_sites) == 1, "boundary must have one exact Rust-inner call")
    inner_site = inner_sites[0]
    boundary_paths = analysis.paths_to(symbols["efi_main"], inner_site)
    inner_depths = []
    for residue in (8, 24, 40, 56):
        frame = analysis.walk(symbols["efi_main"], stops={inner_site}, critical=False, allowed=boundary_paths, entry_mod=residue, known_modulus=64)
        inner_depths.append(frame["stops"][inner_site] + 8)
    require(min(inner_depths) >= 40, "inner caller home area exceeds original EFI entry stack top")
    # The marked interval starts inside the compiler frame, whose actual RSP
    # modulo is carried by initial_depth; this preserves CALL alignment checks.
    high = analysis.walk(begin, stops={end}, initial_depth=begin_depth)
    require(set(high["stops"]) == {end} and high["returns"] == 0, "HIGH escapes without the common end marker")
    require(high["stops"][end] == caller["depths"][end], "HIGH exit changes caller stack")
    require("svmvisor_native_returning_high" in analysis.calls, "named returning HIGH function is not reachable from interval")
    require("svmvisor_native_transition_canary" in analysis.calls and "svmvisor_native_transition" in analysis.calls, "actual canary and transition not reachable from HIGH")
    maximum = high["bound"] - sample_depth
    enforce_coverage(maximum)
    # An EFI x64 function may access its return slot and caller's 32-byte home
    # area. The assembly entry puts this whole inner-call area below its retained
    # boundary, whose complete containment is checked at resource preparation.
    require(high["upper"] <= 40 and caller["upper"] <= 40, "compiler stack operands exceed inner EFI return/home area")
    return {
        "status": "pass", "schema": 1, "scope": "final native-returning synchronous HIGH interval",
        "compiler_frame": owner, "sample_rva": hex(sample - pe.base), "high_begin_rva": hex(begin - pe.base), "high_end_rva": hex(end - pe.base),
        "sample_depth_below_compiler_entry": sample_depth, "high_begin_depth_below_compiler_entry": begin_depth,
        "sample_to_high_downward_delta": begin_depth - sample_depth,
        "outer_entry_to_inner_entry_depths_for_efi_mod64": inner_depths,
        "sample_dominates_high_entry": True,
        "maximum_below_sampled_rsp": maximum, "lower_window_bytes": 65536, "coverage_margin_bytes": 65536 - maximum,
        "maximum_above_compiler_entry": high["upper"], "critical_path": high["path"],
        "high_interval_edges": high["edges"], "critical_functions": analysis.calls, "object_binding": object_reports,
        "premises": ["Conforming EFI x64 entry and ordinary pre/post-HIGH firmware calls preserve their caller stack and return normally.", "Original unchanged asynchronous firmware handlers, MP callback stacks, firmware continuations, SMM and DMA retain the documented cooperating-firmware validity/lifetime contracts.", "This proves synchronous host-stack reservation/use, not arbitrary pointer memory safety. Valid compiler-generated local accesses stay in their allocated frame/home area under emitted noredzone semantics; source-level local-array/pointer bounds remain caller-review obligations.", "The independent arena layout covers the fixed guest stack. No new handler, private host stack switch, or dynamic call target is admitted.", "Runtime resource preparation retains the entire boundary and checked upper stack extent through return; current permissions/WB/lifetimes remain separate admission checks."],
    }


def enforce_coverage(maximum):
    require(maximum >= 0, "negative stack bound")
    require(maximum <= 65536, f"stack window exceeded: {maximum} > 65536")
