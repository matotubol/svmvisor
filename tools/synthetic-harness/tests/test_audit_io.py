"""Corrupt three real I/O evidence records and require the offline audit to fail.

Usage: python test_audit_io.py POSITIVE_RESULT MISSING_RESULT BYPASS_RESULT
No guest runs, builds, evidence overwrites, or synthetic passing baselines occur.
"""
import argparse
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import sys


AUDITOR_PATH = Path(__file__).resolve().parents[1] / "audit-io.py"
spec = importlib.util.spec_from_file_location("audit_io", AUDITOR_PATH)
auditor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(auditor)


def replace_once(record, old, new):
    if old not in record["trace"]:
        raise AssertionError(f"mutation source missing: {old!r}")
    record["trace"] = record["trace"].replace(old, new, 1)


def row_metric(record, row, metric, value):
    """Replace one exact metric within a selected real row, preserving others."""
    blocks = record["trace"].split("IO id=")
    block = blocks[row + 1]
    pattern = rf"(?m)^IO {re.escape(metric)}=([0-9a-f]{{16}})$"
    match = re.search(pattern, block)
    if match is None:
        raise AssertionError(f"missing row {row} metric {metric}")
    old = int(match[1], 16)
    new = value(old) if callable(value) else value
    blocks[row + 1] = re.sub(pattern, f"IO {metric}={new:016x}", block, count=1)
    record["trace"] = "IO id=".join(blocks)


def change_cpu_argument(record):
    index = record["arguments"].index("-cpu") + 1
    record["arguments"][index] = "max,svm=off"


def joint_pending(record):
    row_metric(record, 2, "pending-before", lambda old: old ^ (1 << 32))
    row_metric(record, 2, "pending-after", lambda old: old ^ (1 << 32))


def metric_line(record, operation):
    line = re.search(r"(?m)^IO flags=[0-9a-f]{16}$", record["trace"])[0]
    replace_once(record, line + "\n", operation(line))


def controls():
    result = []

    def add(name, baseline, mutation):
        result.append((name, baseline, mutation))

    for baseline in ("positive", "missing", "bypass"):
        for field, value in (("status", "failed"), ("exitCode", 0),
                             ("timedOut", True), ("stderr", "unexpected emulator error"),
                             ("qemuSha256", "0" * 64), ("profile", "Unknown"),
                             ("cpu", "max,svm=off")):
            add(f"{baseline}.{field}", baseline,
                lambda record, field=field, value=value: record.__setitem__(field, value))
        add(f"{baseline}.cpu_argument", baseline, change_cpu_argument)
        add(f"{baseline}.rdtscp_profile", baseline,
            lambda record: record.__setitem__("rdtscpDisabled", not record["rdtscpDisabled"]))
        add(f"{baseline}.executed_profile_missing", baseline,
            lambda record: record.__setitem__("trace", re.sub(r"(?m)^xstate-profile=.*\n", "", record["trace"])))
        add(f"{baseline}.terminal_marker_missing", baseline,
            lambda record, baseline=baseline: replace_once(record,
                ("PASS" if baseline == "positive" else "FAIL") + " rust-dispatch\n", ""))

    for metric, value in (
        ("rip", 0x1001), ("rax", 0x123456789ABCDE00), ("flags", 2),
        ("marker", 0xFEEDFACE01234567), ("endpoint-before", 0x68),
        ("endpoint-after", 0xA5), ("pending-after", 0), ("code", 0x81),
        ("info2", 0x1002), ("entry-ticks", 0), ("service-ticks", 0),
        ("rcx", 4), ("rdx", 0x3FE), ("rsi", 0x8001), ("rdi", 0x8001),
        ("rsp", 0x9008),
    ):
        add(f"positive.stopped_{metric}", "positive",
            lambda record, metric=metric, value=value: row_metric(record, 2, metric, value))
    for name, mask in (("direction", 1), ("reserved_bit1", 2), ("string", 4),
                       ("rep", 8), ("operand_width", 0x30),
                       ("reserved_bit13", 1 << 13), ("port", 1 << 16),
                       ("backend_address_metadata", 1 << 7), ("backend_upper_metadata", 1 << 32)):
        add(f"positive.metadata_{name}", "positive",
            lambda record, mask=mask: row_metric(record, 2, "info1", lambda old: old ^ mask))
    add("positive.pending_joint_corruption", "positive", joint_pending)
    add("positive.allowed_out_no_effect", "positive",
        lambda record: row_metric(record, 0, "endpoint-after", 0x69))
    add("positive.allowed_in_no_destination_update", "positive",
        lambda record: row_metric(record, 1, "rax", 0x123456789ABCDEA5))
    add("positive.allowed_post_marker_missing", "positive",
        lambda record: row_metric(record, 0, "marker", 15))
    add("positive.case_count", "positive",
        lambda record: replace_once(record, "IO cases=0000000000000250", "IO cases=000000000000024f"))
    add("positive.duplicate_row_id", "positive",
        lambda record: replace_once(record, "IO id=0000000000000001", "IO id=0000000000000000"))
    add("positive.duplicate_metric", "positive", lambda record: metric_line(record, lambda line: line + "\n" + line + "\n"))
    add("positive.missing_metric", "positive", lambda record: metric_line(record, lambda line: ""))
    add("positive.malformed_metric", "positive", lambda record: metric_line(record, lambda line: "IO flags=garbled\n"))
    add("positive.missing_case_marker", "positive", lambda record: replace_once(record, "IO case-pass\n", ""))
    add("positive.failure_added", "positive", lambda record: record.__setitem__("trace", record["trace"] + "FAIL corrupted-control\n"))

    for baseline, probe in (("positive", "0000000000000036"), ("bypass", "0000000000000036"), ("missing", "00000000000000ff")):
        add(f"{baseline}.probe_missing", baseline,
            lambda record, probe=probe: replace_once(record, "IO-PROBE first=" + probe + "\n", ""))
        add(f"{baseline}.probe_corrupted", baseline,
            lambda record, probe=probe: replace_once(record, "IO-PROBE first=" + probe, "IO-PROBE first=0000000000000000"))

    add("missing.generic_failure_substitution", "missing",
        lambda record: replace_once(record, "REFUSE io-endpoint first-probe", "FAIL unrelated-reason"))
    add("missing.guest_entry_added", "missing", lambda record: record.__setitem__("trace", record["trace"] + "IO id=0000000000000000\n"))
    add("missing.second_probe_added", "missing", lambda record: record.__setitem__("trace", record["trace"] + "IO-PROBE second=0000000000000069\n"))
    add("missing.contradictory_bypass", "missing", lambda record: record.__setitem__("bypassBoundary", True))
    for metric, value in (("endpoint-after", 0x69), ("marker", 15), ("code", 0x7B),
                          ("rip", 0x1000), ("rax", 0), ("flags", 2),
                          ("pending-after", 0), ("entry-ticks", 0)):
        add(f"bypass.forbidden_{metric}", "bypass",
            lambda record, metric=metric, value=value: row_metric(record, 2, metric, value))
    add("bypass.duplicate_metric", "bypass", lambda record: metric_line(record, lambda line: line + "\n" + line + "\n"))
    add("bypass.missing_metric", "bypass", lambda record: metric_line(record, lambda line: ""))
    add("bypass.malformed_extra_metric", "bypass", lambda record: metric_line(record, lambda line: line + "\nIO corrupt=garbled\n"))
    add("bypass.generic_failure_only", "bypass", lambda record: record.__setitem__("trace", "FAIL rust-dispatch\n"))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("positive", type=Path)
    parser.add_argument("missing", type=Path)
    parser.add_argument("bypass", type=Path)
    args = parser.parse_args()
    baselines = {}
    baseline_report = {}
    for name in ("positive", "missing", "bypass"):
        path = getattr(args, name).resolve()
        data = path.read_bytes()
        record = json.loads(data.decode("utf-8-sig"))
        expected_flags = (name == "missing", name == "bypass")
        if (record["missingEndpoint"], record["bypassBoundary"]) != expected_flags:
            raise ValueError(f"{name}: wrong baseline scope")
        outcome = auditor.audit(record)
        baselines[name] = record
        baseline_report[name] = dict(path=str(path), sha256=hashlib.sha256(data).hexdigest(), outcome=outcome)

    results = []
    for name, baseline, mutate in controls():
        original = baselines[baseline]
        corrupted = copy.deepcopy(original)
        try:
            mutate(corrupted)
            if corrupted == original:
                raise AssertionError("mutation did not change evidence")
        except Exception as error:
            results.append(dict(control=name, rejected=False, mutationError=f"{type(error).__name__}: {error}"))
            continue
        try:
            auditor.audit(corrupted)
        except (ValueError, KeyError, IndexError, TypeError) as error:
            results.append(dict(control=name, rejected=True, reason=f"{type(error).__name__}: {error}"))
        except Exception as error:
            results.append(dict(control=name, rejected=False, unexpectedAuditError=f"{type(error).__name__}: {error}"))
        else:
            results.append(dict(control=name, rejected=False, reason="auditor accepted corrupted evidence"))

    rejected = sum(item["rejected"] for item in results)
    summary = dict(status="passed" if rejected == len(results) else "failed",
                   auditorSha256=hashlib.sha256(AUDITOR_PATH.read_bytes()).hexdigest(),
                   baselines=baseline_report, controls=len(results), rejected=rejected,
                   failures=len(results) - rejected, results=results)
    print(json.dumps(summary, indent=2))
    return 0 if summary["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
