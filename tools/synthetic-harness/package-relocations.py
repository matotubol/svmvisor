"""Package an already-linked ELF64 image for the emulator's checked loader.

No relinking happens at the requested load address. Retained ELF RELA records
identify every absolute image pointer; PC-relative references stay invariant.
Only the deliberately small static x86-64 relocation vocabulary is accepted.
"""
import argparse
import struct
from pathlib import Path

BASE = 0x100000
ARENA = 0x100000
LIMIT = 0xFF000


def checked_slice(data, offset, size):
    if offset < 0 or size < 0 or offset + size > len(data):
        raise ValueError("ELF range lies outside file")
    return data[offset:offset + size]


def package(elf, image):
    if elf[:7] != b"\x7fELF\x02\x01\x01":
        raise ValueError("Expected little-endian ELF64")
    header = struct.unpack("<16sHHIQQQIHHHHHH", checked_slice(elf, 0, 64))
    if header[1:3] != (2, 62) or header[11] != 64:
        raise ValueError("Expected linked x86-64 ELF with ordinary section headers")
    sections = [struct.unpack("<IIQQQQIIQQ", checked_slice(elf, header[6] + i * 64, 64))
                for i in range(header[12])]
    symbols = {}
    symbol_tables = {}
    for index, section in enumerate(sections):
        if section[1] != 2:
            continue
        if section[9] != 24 or section[5] % 24:
            raise ValueError("Invalid symbol table")
        strings = sections[section[6]]
        names = checked_slice(elf, strings[4], strings[5])
        table = []
        for offset in range(section[4], section[4] + section[5], 24):
            symbol = struct.unpack("<IBBHQQ", checked_slice(elf, offset, 24))
            name = names[symbol[0]:].split(b"\0", 1)[0].decode("ascii")
            symbols[name] = symbol[4]
            table.append(symbol)
        symbol_tables[index] = table
    if symbols.get("image_start") != BASE:
        raise ValueError("Unexpected linked image base")
    memory_bytes = symbols["image_bss_end"] - BASE
    if not 0 < len(image) <= memory_bytes <= LIMIT:
        raise ValueError("Image/BSS overlaps handoff page")
    if symbols["image_load_end"] - BASE != len(image):
        raise ValueError("Flat image length differs from ELF load extent")
    # Verify the supplied flat image is exactly the ELF's allocated file data.
    for section in sections:
        if section[2] & 2 and section[1] != 8 and section[5]:
            offset = section[3] - BASE
            if checked_slice(image, offset, section[5]) != checked_slice(elf, section[4], section[5]):
                raise ValueError("Flat image differs from ELF section")
    entry = symbols["entry_uefi"] - BASE
    if not 0 <= entry < len(image):
        raise ValueError("Entry outside initialized image")
    relocations = []
    retained = 0
    for section in sections:
        if section[1] not in (4, 9):
            continue
        target = sections[section[7]]
        if not target[2] & 2:
            continue
        if section[1] != 4 or section[9] != 24 or section[5] % 24:
            raise ValueError("Only ELF64 RELA relocation tables are supported")
        table = symbol_tables[section[6]]
        for offset in range(section[4], section[4] + section[5], 24):
            address, info, addend = struct.unpack("<QQq", checked_slice(elf, offset, 24))
            kind, symbol_index = info & 0xFFFFFFFF, info >> 32
            if kind == 0:
                continue
            retained += 1
            symbol = table[symbol_index]
            value = symbol[4]
            if symbol[3] == 0 or not BASE <= value <= BASE + memory_bytes:
                raise ValueError(f"Relocation targets unowned/undefined symbol at {address:#x}")
            if kind not in (1, 2, 4, 9, 10, 11):
                raise ValueError(f"Unsupported absolute/relative relocation type {kind}")
            width = 8 if kind == 1 else 4
            location = address - BASE
            raw = checked_slice(image, location, width)
            if address < target[3] or address + width > target[3] + target[5]:
                raise ValueError("Relocation lies outside its target section")
            if kind == 9:
                # lld may relax a GOT load to an invariant direct LEA. Otherwise
                # the linker-created GOT slot has no retained RELA of its own:
                # synthesize its pointer relocation from the resolved operand.
                resolved = address + int.from_bytes(raw, "little", signed=True) - addend
                if resolved == value:
                    continue
                slot = resolved - BASE
                owned_data = any(s[2] & 3 == 3 and s[3] <= resolved and
                                 resolved + 8 <= s[3] + s[5] and s[1] != 8
                                 for s in sections)
                if resolved % 8 or not owned_data or int.from_bytes(
                        checked_slice(image, slot, 8), "little") != value:
                    raise ValueError("GOTPCREL does not resolve to an owned matching GOT slot")
                relocations.append((slot, 8))
                continue
            if kind in (2, 4):
                # PC32/PLT32 addends contain -4 for the displacement field.
                if not BASE <= value + addend + 4 <= BASE + memory_bytes:
                    raise ValueError("PC-relative reference escapes image ownership")
                expected = value + addend - address
                if int.from_bytes(raw, "little", signed=True) != expected:
                    raise ValueError("Linked relative value differs from relocation")
                continue
            expected = value + addend
            if not BASE <= expected <= BASE + memory_bytes:
                raise ValueError("Absolute reference escapes image ownership")
            if int.from_bytes(raw, "little", signed=kind == 11) != expected:
                raise ValueError("Linked absolute value differs from relocation")
            relocations.append((location, width))
    if not retained or not relocations:
        raise ValueError("No retained absolute relocations; link with --emit-relocs")
    relocations = sorted(set(relocations))
    end = 0
    for offset, width in relocations:
        if offset < end:
            raise ValueError("Overlapping or duplicate relocation writes")
        end = offset + width
    metadata = struct.pack("<8s7Q", b"SVMRELO1", BASE, ARENA, len(image),
                           memory_bytes, entry, len(relocations), 0)
    return metadata + image + b"".join(struct.pack("<QQ", *r) for r in relocations)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--elf", type=Path, required=True)
    parser.add_argument("--image", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = package(args.elf.read_bytes(), args.image.read_bytes())
    args.output.write_bytes(result)
    print(f"Packaged {struct.unpack_from('<Q', result, 48)[0]} runtime relocations: {args.output}")
