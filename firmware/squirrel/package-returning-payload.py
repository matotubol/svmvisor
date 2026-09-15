"""Package a digest-bound returning PE child offline. No hardware access."""
import argparse
import hashlib
import json
import struct
from pathlib import Path

HEADER_BYTES = 128
SLOT_BYTES = 0x100000
FLASH_OFFSET = 0x400000


def digest(data):
    return hashlib.sha256(data).hexdigest()


def validate_pe(pe, resident=False):
    def word(fmt, offset):
        try:
            return struct.unpack_from(fmt, pe, offset)[0]
        except struct.error as error:
            raise ValueError("Truncated PE metadata") from error
    u16 = lambda o: word("<H", o)
    u32 = lambda o: word("<I", o)
    u64 = lambda o: word("<Q", o)
    if not 512 <= len(pe) <= SLOT_BYTES - HEADER_BYTES or pe[:2] != b"MZ":
        raise ValueError("Invalid PE size or DOS header")
    base = u32(0x3c)
    if (not 64 <= base <= len(pe) - 24 or pe[base:base+4] != b"PE\0\0"
            or u16(base+4) != 0x8664 or u16(base+20) != 240 or u16(base+22) & 3 != 2):
        raise ValueError("Expected relocatable AMD64 PE")
    opt = base + 24
    if u16(opt) != 0x20b or u16(opt+68) != (12 if resident else 11) or u32(opt+108) != 16:
        raise ValueError("Expected PE32+ EFI boot service driver")
    meta = dict(entry_rva=u32(opt+16), image_bytes=u32(opt+56), headers_bytes=u32(opt+60),
                section_alignment=u32(opt+32), file_alignment=u32(opt+36), sections=u16(base+6))
    entry, image, headers, sections = (meta[k] for k in ("entry_rva", "image_bytes", "headers_bytes", "sections"))
    if (meta["section_alignment"] != 4096 or meta["file_alignment"] != 512
            or not 0 < image <= 16*1024*1024 or image & 4095
            or not 0 < headers <= min(len(pe), image) or headers & 511
            or not headers <= entry < image or not 1 <= sections <= 16):
        raise ValueError("Unsupported PE alignment, image or entry bounds")
    table = opt + 240
    if table + sections * 40 > headers:
        raise ValueError("Section table escapes headers")
    for directory in (1, 9, 13, 14):
        if u64(opt+112+directory*8):
            raise ValueError("Imports/TLS/delay-import/CLR are unsupported")
    reloc, reloc_size = u32(opt+152), u32(opt+156)
    if bool(reloc) != bool(reloc_size) or (reloc and reloc_size < 8) or reloc + reloc_size > image:
        raise ValueError("Missing or invalid relocation directory")
    previous_virtual = previous_raw = headers
    found_entry, found_reloc = False, not reloc and not reloc_size
    for i in range(sections):
        s = table + i*40
        virtual_size, va, raw_size, raw = (u32(s+o) for o in (8, 12, 16, 20))
        flags = u32(s+36)
        extent = max(virtual_size, raw_size)
        end = va + extent
        if (not extent or va & 4095 or va < previous_virtual or end > image or raw_size & 511
                or (raw_size and (raw & 511 or raw < previous_raw or raw + raw_size > len(pe)))
                or flags & 0xa0000000 == 0xa0000000):
            raise ValueError("Overlapping, out-of-bounds, misaligned or writable-executable section")
        previous_virtual = end
        if raw_size:
            previous_raw = raw + raw_size
        if va <= entry < end:
            if flags & 0xe0000020 != 0x60000020 or entry-va >= min(raw_size, virtual_size):
                raise ValueError("Entry must be initialized executable read-only code")
            found_entry = True
        if va <= reloc and reloc + reloc_size <= va + raw_size:
            found_reloc = True
    if not found_entry or not found_reloc:
        raise ValueError("Entry or relocation directory not backed by a section")
    return meta


def build_header(pe, resident=False):
    meta = validate_pe(pe, resident)
    header = struct.pack("<8sII4Q32s4H6I16s", b"SVMBPE01" if resident else b"SVMPE001", 1, HEADER_BYTES,
                         len(pe), SLOT_BYTES, HEADER_BYTES, 4 if resident else 2, hashlib.sha256(pe).digest(),
                         0x8664, 12 if resident else 11, 0x20b, 0, *meta.values(), bytes(16))
    assert len(header) == HEADER_BYTES
    return header


def build_slot(pe, resident=False):
    return build_header(pe, resident) + pe + b"\xff" * (SLOT_BYTES - HEADER_BYTES - len(pe))


def validate_slot(slot, pinned_header, resident=False):
    if len(slot) != SLOT_BYTES or len(pinned_header) != HEADER_BYTES or slot[:128] != pinned_header:
        raise ValueError("Slot extent or immutable header mismatch")
    size = struct.unpack_from("<Q", pinned_header, 16)[0]
    if not 512 <= size <= SLOT_BYTES - HEADER_BYTES:
        raise ValueError("PE exceeds slot bounds")
    pe = slot[128:128+size]
    if build_header(pe, resident) != pinned_header:
        raise ValueError("PE digest or metadata mismatch")
    if slot[128+size:] != b"\xff" * (SLOT_BYTES-128-size):
        raise ValueError("Unused slot tail is not erased padding")
    return pe


def combine(configuration, slot, pinned_header, resident=False):
    if not 0 < len(configuration) <= FLASH_OFFSET:
        raise ValueError("Configuration must fit below 4 MiB")
    validate_slot(slot, pinned_header, resident)
    return configuration + b"\xff" * (FLASH_OFFSET-len(configuration)) + slot


def write_artifacts(pe, output, configuration=None, resident=False):
    meta = validate_pe(pe, resident)
    header, slot = build_header(pe, resident), build_slot(pe, resident)
    validate_slot(slot, header, resident)
    # An invocation creates its own immutable result. Never overwrite an older
    # candidate, a golden artifact, or an output from a failed previous attempt.
    output.mkdir(parents=True, exist_ok=False)
    artifacts = {"native-child.efi": pe, "pe-header.bin": header, "payload-slot.bin": slot}
    result = dict(schema_version=1, image_kind="native_resident_pe_review" if resident else "native_returning_pe_review", physical_run_ready=False,
                  hardware_accessed=False, payload_format="SVMBPE01" if resident else "SVMPE001", payload_bytes=len(pe), payload_sha256=digest(pe),
                  header_bytes=HEADER_BYTES, header_sha256=digest(header), slot_bytes=SLOT_BYTES, slot_sha256=digest(slot),
                  payload_flash_offset=FLASH_OFFSET, payload_bar=1, required_parent_feature="card-resident-loader" if resident else "card-returning-loader",
                  child_return_contract="EFI_SUCCESS plus explicit armed acknowledgement; retain all non-error child returns until reset" if resident else "EFI_UNSUPPORTED; no persistent interfaces; mailbox is Rust-inner evidence only",
                  load_policy="pinned-memory-LoadImage-StartImage-runtime-retention" if resident else "pinned-memory-LoadImage-StartImage-return-cleanup", **meta)
    if configuration is not None:
        combined = combine(configuration, slot, header, resident)
        artifacts["combined-review.bin"] = combined
        result.update(combined_bytes=len(combined), combined_sha256=digest(combined),
                      configuration_bytes=len(configuration), configuration_sha256=digest(configuration),
                      configuration_partition_bytes=FLASH_OFFSET, configuration_padding_byte=255)
    for name, data in artifacts.items():
        (output / name).write_bytes(data)
    (output / "payload-manifest.json").write_text(json.dumps(result, indent=2)+"\n", encoding="utf-8")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--payload", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--configuration", type=Path)
    parser.add_argument("--resident", action="store_true", help="Runtime subsystem12 SVMBPE01 child; separate card-resident-loader")
    args = parser.parse_args()
    result = write_artifacts(args.payload.read_bytes(), args.output,
                             args.configuration.read_bytes() if args.configuration else None, args.resident)
    print(json.dumps(result, indent=2))
