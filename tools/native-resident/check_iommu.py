"""Read-only AMD IVRS decoder and x2APIC/direct-AVIC capability check.

Normative formats: AMD 48882 rev 3.11, sections 3.4.3 and 5.2,
Tables 85-113 (visually reviewed in docs/x2avic-iommu-owner-2026-09-16.md).
This consumes an ACPI file; it never maps or writes hardware. Passing the
capability check does not establish live IOMMU configuration or ownership.
"""

import argparse
import hashlib
import json
from pathlib import Path


class IvrsError(ValueError):
    """Malformed or unsupported IVRS representation."""


def _number(data, offset, width):
    if offset < 0 or offset + width > len(data):
        raise IvrsError("truncated IVRS field")
    return int.from_bytes(data[offset:offset + width], "little")


def _devices(data, mixed):
    result = []
    offset = 0
    pending_range = None
    variable_seen = False
    while offset < len(data):
        kind = data[offset]
        if kind == 0xF0:
            if not mixed or len(data) - offset < 22:
                raise IvrsError("invalid ACPI HID entry")
            variable_seen = True
            uid_format = data[offset + 20]
            uid_length = data[offset + 21]
            if uid_format not in (0, 1, 2) or (uid_format == 0 and uid_length):
                raise IvrsError("invalid HID UID format")
            length = 22 + uid_length
        elif kind < 0x80:
            if variable_seen:
                raise IvrsError("fixed entry follows variable entry")
            length = 4 if kind < 0x40 else 8
        else:
            raise IvrsError(f"unsupported IVHD device entry {kind:#x}")
        if offset + length > len(data):
            raise IvrsError("truncated IVHD device entry")
        raw = data[offset:offset + length]
        entry = {"type": kind, "device_id": _number(raw, 1, 2),
                 "dte_settings": raw[3], "raw": raw.hex()}
        if pending_range is not None and kind != 4:
            raise IvrsError("range start is not immediately followed by range end")
        if kind in (3, 0x43, 0x47):
            pending_range = entry["device_id"]
        elif kind == 4:
            if pending_range is None or entry["device_id"] < pending_range or raw[3]:
                raise IvrsError("invalid IVHD range end")
            entry["range_start"] = pending_range
            pending_range = None
        elif kind == 0:
            if any(raw):
                raise IvrsError("nonzero IVHD padding")
        elif kind not in (1, 2, 0x42, 0x46, 0x48, 0xF0):
            raise IvrsError(f"reserved IVHD device entry {kind:#x}")
        if kind in (0x42, 0x43):
            if raw[4] or raw[7]:
                raise IvrsError("nonzero alias reserved field")
            entry["source_device_id"] = _number(raw, 5, 2)
        if kind in (0x46, 0x47):
            entry["extended_dte_settings"] = _number(raw, 4, 4)
        if kind == 0x48:
            if entry["device_id"] or raw[7] not in (1, 2):
                raise IvrsError("invalid special device entry")
            entry.update(handle=raw[4], source_device_id=_number(raw, 5, 2),
                         variety="ioapic" if raw[7] == 1 else "hpet")
        if kind == 0xF0:
            entry.update(hid=raw[4:12].hex(), cid=raw[12:20].hex(),
                         uid_format=uid_format, uid=raw[22:].hex())
        result.append(entry)
        offset += length
    if pending_range is not None:
        raise IvrsError("unterminated IVHD range")
    return result


def decode_ivrs(data):
    """Validate and decode the complete bounded table, preserving source data."""
    if len(data) < 48 or data[:4] != b"IVRS":
        raise IvrsError("missing IVRS header")
    if _number(data, 4, 4) != len(data):
        raise IvrsError("IVRS length differs from file length")
    if sum(data) & 0xFF:
        raise IvrsError("invalid IVRS checksum")
    if data[8] not in (1, 2) or any(data[40:48]):
        raise IvrsError("unsupported IVRS revision or reserved header")
    ivinfo = _number(data, 36, 4)
    if ivinfo & 0xFF80001C:
        raise IvrsError("nonzero IVinfo reserved fields")
    blocks = []
    offset = 48
    while offset < len(data):
        if offset + 4 > len(data):
            raise IvrsError("truncated IVDB header")
        kind, flags = data[offset:offset + 2]
        length = _number(data, offset + 2, 2)
        if length < 4 or offset + length > len(data):
            raise IvrsError("invalid IVDB length")
        raw = data[offset:offset + length]
        block = {"type": kind, "offset": offset, "length": length, "flags": flags}
        if kind in (0x10, 0x11, 0x40):
            header_length = 24 if kind == 0x10 else 40
            if length < header_length or (kind == 0x40 and data[8] != 2):
                raise IvrsError("invalid IVHD header")
            if kind != 0x10 and not ivinfo & 1:
                raise IvrsError("extended IVHD without IVinfo.EFRSup")
            block.update(device_id=_number(raw, 4, 2), capability_offset=_number(raw, 6, 2),
                         mmio_base=_number(raw, 8, 8), segment=_number(raw, 16, 2),
                         iommu_info=_number(raw, 18, 2), attributes=_number(raw, 20, 4),
                         devices=_devices(raw[header_length:], kind == 0x40))
            if kind != 0x10:
                efr = _number(raw, 24, 8)
                block.update(efr=f"0x{efr:016x}", efr2=f"0x{_number(raw, 32, 8):016x}",
                             gasup=bool(efr & (1 << 7)), gamsup=(efr >> 21) & 7,
                             xtsup=bool(efr & (1 << 2)))
        elif kind in (0x20, 0x21, 0x22):
            if length != 32 or flags & 0xF0 or any(raw[10:16]):
                raise IvrsError("invalid IVMD")
            start, size = _number(raw, 16, 8), _number(raw, 24, 8)
            if not size or start + size > (1 << 64):
                raise IvrsError("invalid IVMD memory range")
            block.update(device_id=_number(raw, 4, 2), auxiliary=_number(raw, 6, 2),
                         segment=_number(raw, 8, 2), start=start, size=size,
                         exclusion=bool(flags & 8), read=bool(flags & 2),
                         write=bool(flags & 4), unity=bool(flags & 1))
        else:
            raise IvrsError(f"unsupported IVDB type {kind:#x}")
        blocks.append(block)
        offset += length
    # Multiple format descriptions of one unit are not multiple physical units.
    units = {}
    for block in blocks:
        if block["type"] not in (0x10, 0x11, 0x40):
            continue
        key = (block["segment"], block["device_id"], block["capability_offset"])
        units.setdefault(key, []).append(block)
    if not units:
        raise IvrsError("IVRS has no IOMMU")
    checks = []
    for key, descriptions in units.items():
        extended = [b for b in descriptions if b["type"] != 0x10]
        if len({(b["mmio_base"], b.get("efr"), b.get("efr2")) for b in extended}) > 1:
            raise IvrsError("conflicting extended descriptions of one IOMMU")
        selected = next((b for b in extended if b["type"] == 0x11), None)
        selected = selected or (extended[0] if extended else descriptions[0])
        failures = []
        if not extended:
            failures.append("no extended IVHD EFR image; GAMSup unknown")
        else:
            if not selected["gasup"]:
                failures.append("GASup is clear")
            if selected["gamsup"] != 1:
                failures.append("GAMSup is not the supported guest virtual APIC mode 001b")
            if not selected["xtsup"]:
                failures.append("XTSup is clear; system x2APIC profile requires XTEn (section 2.2.5.3)")
        checks.append({"segment": key[0], "device_id": key[1], "capability_offset": key[2],
                       "selected_ivhd_offset": selected["offset"], "failures": failures})
    return {"source_sha256": hashlib.sha256(data).hexdigest(), "length": len(data),
            "revision": data[8], "ivinfo": f"0x{ivinfo:08x}",
            "preboot_dma_remap_support": bool(ivinfo & 2), "blocks": blocks,
            "iommu_checks": checks,
            "x2apic_direct_avic_capabilities_advertised": all(not c["failures"] for c in checks),
            "live_configuration_verified": False,
            "note": "Firmware capability evidence only; no live register reads or hardware writes."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ivrs", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        report = decode_ivrs(args.ivrs.read_bytes())
    except (OSError, IvrsError) as error:
        parser.exit(2, f"IVRS refused: {error}\n")
    rendered = json.dumps(report, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    return 0 if report["x2apic_direct_avic_capabilities_advertised"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
