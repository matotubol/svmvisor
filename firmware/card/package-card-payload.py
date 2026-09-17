"""Build checked, offline card-load-only artifacts. Never accesses hardware."""
import argparse
import hashlib
import json
import struct
from pathlib import Path

HEADER_BYTES = 128
SLOT_BYTES = 0x100000
FLASH_OFFSET = 0x400000
FLASH_BYTES = 0x1000000  # Conservative 24-bit read-address ceiling.


def digest(data):
    return hashlib.sha256(data).hexdigest()


def validate_package(package):
    if len(package) < 64:
        raise ValueError("Truncated relocation header")
    magic, base, arena, image, memory, entry, count, reserved = struct.unpack_from("<8s7Q", package)
    if magic != b"SVMRELO1" or base != 0x100000 or arena != SLOT_BYTES or reserved:
        raise ValueError("Unsupported relocation header")
    if not 0 < image <= memory <= 0xFF000 or entry >= image:
        raise ValueError("Invalid image ownership or entry")
    if len(package) != 64 + image + count * 16 or len(package) > SLOT_BYTES - HEADER_BYTES:
        raise ValueError("Truncated, oversized, or trailing relocation package")
    previous = 0
    for i in range(count):
        offset, width = struct.unpack_from("<QQ", package, 64 + image + i * 16)
        if width not in (4, 8) or offset < previous or offset + width > image:
            raise ValueError("Invalid relocation write range")
        value = int.from_bytes(package[64 + offset:64 + offset + width], "little")
        if not base <= value <= base + memory:
            raise ValueError("Relocation pointer escapes owned image")
        previous = offset + width
    return {"image_bytes": image, "memory_bytes": memory, "entry_offset": entry,
            "relocation_count": count}


def build_slot(package):
    validate_package(package)
    header = struct.pack("<8sII4Q32s48s", b"SVMCRD01", 1, HEADER_BYTES,
                         len(package), SLOT_BYTES, HEADER_BYTES, 1,
                         hashlib.sha256(package).digest(), bytes(48))
    return header + package + b"\xff" * (SLOT_BYTES - HEADER_BYTES - len(package))


def validate_slot(slot, expected_digest):
    if len(slot) != SLOT_BYTES:
        raise ValueError("Truncated or oversized card slot")
    magic, version, header_size, size, capacity, offset, flags, sha, reserved = struct.unpack_from(
        "<8sII4Q32s48s", slot)
    if (magic, version, header_size, capacity, offset, flags, reserved) != (
            b"SVMCRD01", 1, HEADER_BYTES, SLOT_BYTES, HEADER_BYTES, 1, bytes(48)):
        raise ValueError("Unsupported card envelope")
    if not 64 <= size <= SLOT_BYTES - HEADER_BYTES:
        raise ValueError("Card package exceeds slot")
    package = slot[offset:offset + size]
    if sha.hex() != expected_digest or digest(package) != expected_digest:
        raise ValueError("Card payload digest mismatch")
    if slot[offset + size:] != b"\xff" * (SLOT_BYTES - offset - size):
        raise ValueError("Unexpected data in unused card slot")
    validate_package(package)
    return package


def combine(configuration, slot, expected_digest):
    if not 0 < len(configuration) <= FLASH_OFFSET:
        raise ValueError("Configuration must fit wholly below flash offset 4 MiB")
    validate_slot(slot, expected_digest)
    combined = configuration + b"\xff" * (FLASH_OFFSET - len(configuration)) + slot
    if len(combined) != FLASH_OFFSET + SLOT_BYTES or len(combined) > FLASH_BYTES:
        raise ValueError("Combined image exceeds checked flash layout")
    # These assertions protect the exact supplied configuration bytes; the
    # absent partition tail is explicitly erased padding, never a live dump.
    assert combined[:len(configuration)] == configuration
    assert combined[FLASH_OFFSET:] == slot
    return combined


def write_artifacts(package, output, configuration=None):
    metadata = validate_package(package)
    slot = build_slot(package)
    sha = digest(package)
    validate_slot(slot, sha)
    output.mkdir(parents=True, exist_ok=True)
    (output / "payload.reloc").write_bytes(package)
    (output / "payload-slot.bin").write_bytes(slot)
    manifest = {"schema_version": 1, "image_kind": "card_load_only_review",
                "physical_run_ready": False, "execution_allowed": False,
                "flash_operation_authorized": False, "package_sha256": sha,
                "package_bytes": len(package), "slot_sha256": digest(slot),
                "slot_bytes": SLOT_BYTES, "payload_flash_offset": FLASH_OFFSET,
                "payload_bar": 1, "header_bytes": HEADER_BYTES,
                "load_policy": "validate-relocate-release-without-entry", **metadata}
    if configuration is not None:
        combined = combine(configuration, slot, sha)
        (output / "combined-review.bin").write_bytes(combined)
        manifest.update({"combined_sha256": digest(combined), "combined_bytes": len(combined),
                         "configuration_sha256": digest(configuration),
                         "configuration_bytes": len(configuration),
                         "configuration_padding_byte": 255,
                         "configuration_partition_bytes": FLASH_OFFSET})
    (output / "payload-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--payload", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--configuration", type=Path)
    args = parser.parse_args()
    result = write_artifacts(args.payload.read_bytes(), args.output,
                             args.configuration.read_bytes() if args.configuration else None)
    print(f"Card load-only review artifacts: {args.output}")
    print(f"SVMVISOR_CARD_PAYLOAD_SHA256={result['package_sha256']}")
