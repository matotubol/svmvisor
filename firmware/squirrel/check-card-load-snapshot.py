"""Offline evaluation of a card load-only observation; never opens USB or approves flashing."""
import argparse
import importlib.util
import json
from pathlib import Path

_reader_spec = importlib.util.spec_from_file_location("card_snapshot_reader", Path(__file__).with_name("read_snapshot.py"))
_reader = importlib.util.module_from_spec(_reader_spec)
_reader_spec.loader.exec_module(_reader)


def assess(text: str, manifest: dict, native_boot: str = "not-reported", *, first_boot_after_reconfiguration: bool = False, previous_boot_id: int | None = None) -> dict:
    """Keep decoded hardware evidence separate from the operator's boot report."""
    if manifest.get("card_load_only") is not True or manifest.get("image_kind") != "card_load_only_review":
        raise ValueError("expected a card-load-only candidate manifest")
    if not all(isinstance(manifest.get(key), str) and len(manifest[key]) == 16 for key in ("fpga_build_id", "rom_build_id")):
        raise ValueError("manifest must identify both expected 64-bit build IDs")
    if native_boot not in ("normal", "failed", "not-reported"):
        raise ValueError("invalid native boot report")
    if first_boot_after_reconfiguration and previous_boot_id is not None:
        raise ValueError("choose only one freshness basis")
    if previous_boot_id is not None and (type(previous_boot_id) is not int or not 0 <= previous_boot_id <= 0xffffffff):
        raise ValueError("previous boot ID must be an unsigned DWORD")
    snapshot = _reader.consistent_snapshot(text, manifest)
    issues = []
    if not first_boot_after_reconfiguration:
        if previous_boot_id is None:
            issues.append("freshness requires a reported first boot after FPGA reconfiguration or a preboot boot-ID baseline")
        elif snapshot["boot_id"] == previous_boot_id:
            issues.append("boot ID did not change; retained success may be stale")
    payload = snapshot["card_payload_status"]
    if payload != "loaded_and_freed":
        issues.append("payload outcome is " + payload)
    if snapshot["phase"] != 0x40:
        issues.append("ExitBootServices notification is not the retained phase")
    if snapshot["detail"] != 0x2004:
        issues.append("expected clean lifecycle detail 0x2004")
    if snapshot["context"] != "0000000100010001":
        issues.append("expected one ReadyToBoot, AfterReadyToBoot and ExitBootServices notification")
    status = snapshot["hardware_status"]
    if status & 0x3800:
        issues.append("an exposed journal/TX fault bit is set")
    if status & 0x200:
        issues.append("bus mastering is reported enabled")
    if not status & 0x100:
        issues.append("memory decoding is not reported enabled")
    if not status & 1:
        issues.append("PCIe link is not reported up")
    if snapshot["rom_reads"] == 0 or snapshot["bar_writes"] == 0:
        issues.append("ROM-read or journal-write traffic is absent")
    if native_boot != "normal":
        issues.append("normal native Windows boot has not been reported" if native_boot == "not-reported" else "native Windows boot was reported unsuccessful")
    return {
        "schema_version": 1,
        "observation_result": "PASS" if not issues else "REVIEW_REQUIRED",
        "snapshot": snapshot,
        "payload_load_evidence": payload,
        "native_windows_boot_report": native_boot,
        "native_windows_boot_evidence_source": "operator report only",
        "freshness_basis": "operator reports first boot after FPGA reconfiguration" if first_boot_after_reconfiguration else "supplied preboot boot-ID baseline" if previous_boot_id is not None else "not-reported",
        "previous_boot_id": previous_boot_id,
        "review_reasons": issues,
        "scope": "load-only diagnostic observation; no payload execution or physical qualification asserted",
        "limitations": [
            "CRC and build IDs bind a coherent observation, not a trusted security attestation",
            "snapshot contains only the last committed record and may predate the attempted boot",
            "ExitBootServices notification does not establish successful return from ExitBootServices",
            "RX and sequence faults are not exported in USER2 hardware_status",
        ],
        "grants_flash_authorization": False,
        "updates_manifest_qualification": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", required=True, type=Path, help="retained raw OpenOCD log; offline only")
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--native-windows-boot", choices=("normal", "failed", "not-reported"), default="not-reported")
    freshness = parser.add_mutually_exclusive_group()
    freshness.add_argument("--first-boot-after-reconfiguration", action="store_true", help="operator confirms this was the first boot after cold power removal/reconfiguration")
    freshness.add_argument("--previous-boot-id", type=lambda value: int(value, 0), help="boot_id from a retained preboot baseline, decimal or 0x hexadecimal")
    args = parser.parse_args()
    try:
        result = assess(args.input.read_text(encoding="utf-8-sig"), json.loads(args.manifest.read_text(encoding="utf-8-sig")), args.native_windows_boot, first_boot_after_reconfiguration=args.first_boot_after_reconfiguration, previous_boot_id=args.previous_boot_id)
    except (ValueError, OSError) as exc:
        parser.exit(2, str(exc) + "\n")
    print(json.dumps(result, indent=2))
    return 0 if result["observation_result"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
