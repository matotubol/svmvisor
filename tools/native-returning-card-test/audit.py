"""Read-only audit of immutable returning-card firmware integration sessions."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify(path, expected):
    actual = sha(path)
    if actual != expected.lower():
        raise ValueError(f"Hash mismatch {path}: {actual} != {expected}")


def artifact(session, record, filename, field):
    path = session / filename
    verify(path, record[field])
    return path.read_bytes()


def audit(session):
    record = json.loads((session / "result.json").read_text(encoding="utf-8-sig"))
    detail = record.get("journalDetail", 6)
    assert detail in (6, 7)
    assert record["schema"] == (2 if detail == 7 else 1)
    terminal = record.get("terminalExitBootServices", False)
    mode = record["mode"]
    assert mode in ("Positive", "Header", "Digest", "PristineRefused", "StructuredRefused")
    assert detail == 7 or mode in ("Positive", "Header", "Digest")
    assert not terminal or (detail == 7 and mode == "StructuredRefused")
    if detail == 7:
        assert record["stopAndPostReturnBootServicesTested"] is (not terminal)
        verify(session / "launcher-build.log", record["launcherBuildLogSha256"])
    assert record["status"] == "passed"
    for item in record.get("artifactHashes", []):
        verify(session / item["path"], item["sha256"])
        assert (session / item["path"]).stat().st_size == item["bytes"]
    assert record["secureBootPolicyTested"] is False
    assert record["hardwareAccessed"] is False
    parent = artifact(session, record, "svmvisor-dxe.efi", "parentSha256")
    child = artifact(session, record, "reviewed-child.efi", "childSha256")
    slot = artifact(session, record, "payload/payload-slot.bin", "slotSha256")
    header = artifact(session, record, "payload/pe-header.bin", "pinSha256")
    launcher = artifact(session, record, "launcher.efi", "launcherSha256")
    verify(session / "esp/EFI/BOOT/BOOTX64.EFI", record["launcherSha256"])
    verify(session / "esp.img", record["espImageSha256"])
    verify(session / "delivery-manifest.json", record["deliveryManifestSha256"])
    verify(session / "child-build-evidence.bin", record["childEvidenceSha256"])
    verify(session / "journal-model.json", record["journalModelSha256"])
    verify(session / "debug.log", record["debugSha256"])
    verify(session / "serial.log", record["serialSha256"])
    assert len(parent) == record["parentBytes"] and len(child) == record["childBytes"]
    assert len(slot) == 0x100000 and len(header) == 128
    assert slot[:128] == header and slot[128:128+len(child)] == child
    assert struct.unpack_from("<Q", header, 16)[0] == len(child)
    assert header[48:80] == hashlib.sha256(child).digest()
    assert launcher.count(parent) == 1 and launcher.count(slot) == 1
    assert parent.count(header) == 1
    for pe in (parent, child):
        base = struct.unpack_from("<I", pe, 0x3c)[0]
        assert pe[base:base+4] == b"PE\0\0"
        assert struct.unpack_from("<H", pe, base+4)[0] == 0x8664
        assert struct.unpack_from("<H", pe, base+92)[0] == 11
    parent_manifest = json.loads((session / "delivery-manifest.json").read_text(encoding="utf-8-sig"))
    child_manifest = json.loads((session / "child-build-evidence.bin").read_text(encoding="utf-8-sig"))
    for folder, inputs in (("parent-source",parent_manifest["source_hashes"]),("child-source",child_manifest["source_hashes"]),("reviewed-source",record["sourceHashes"])):
        for item in inputs:
            verify(session / folder / item["path"], item["sha256"])
    model = json.loads((session / "journal-model.json").read_text())
    assert model["status"] == "passed" and model["exitCode"] == 33
    assert model["guestDebugRegistersChanged"] is False
    assert model["qemuSha256"] == "57448131c0fbaed74e059ab0f12b97d6ec278c0215330e585004102859a2be71"
    commits = model["commits"]
    positive = mode == "Positive"
    delivery_failed = mode in ("Header", "Digest")
    assert len(commits) == (6 if terminal else 3 if delivery_failed else 5)
    if detail == 7:
        assert model["schema"] == 2 and model["journalDetail"] == 7
        assert model["mode"] == mode and model["terminalExitBootServices"] is terminal
        cpu = {"PristineRefused": "max,svm=on,hypervisor=on", "StructuredRefused": "max,svm=off,hypervisor=off"}.get(mode, "max,svm=on,hypervisor=off")
        assert model["command"][model["command"].index("-cpu")+1] == cpu
        assert model["command"][model["command"].index("-machine")+1] == "q35,accel=tcg"
        result = 0x80070000 if delivery_failed else 0x20070000 if positive else 0x40070000
        phases = [0x10] if delivery_failed else [0x10, 0x28, 0x35] + ([0x40] if terminal else [])
        expected_phases = [0x20010, 0x20013] + [result | phase for phase in phases]
    else:
        expected_phases = [0x20010,0x20013,0x20060010,0x20060028,0x20060035] if positive else [0x20010,0x20013,0x80060010]
    for i, (commit, phase) in enumerate(zip(commits, expected_phases)):
        assert commit["sequence"] == i+1 and commit["acknowledged"]
        assert commit["words"][0] == i+1 and commit["words"][7] == phase
        assert struct.pack("<8I",*commit["words"]).hex() == commit["stagedHex"]
    if detail == 7:
        refusal, counts, meta = {
            "Positive": (0, 0x10001, 0x1fc2),
            "PristineRefused": (0, 0, 0x40),
            "StructuredRefused": (5, 0, 0x3c1),
            "Header": (0, 0, 0),
            "Digest": (0, 0, 0x10),
        }[mode]
        lifecycle = [0] if delivery_failed else [0, 0x4000, 0x84000] + ([0x1084000] if terminal else [])
        for commit, counter_bits in zip(commits[2:], lifecycle):
            assert commit["words"][4:7] == [refusal, counts, meta | counter_bits]
    elif positive:
        assert commits[2]["words"][4:7] == [2,65537,4]
        assert commits[3]["words"][4:6] == [1,0]
        assert commits[4]["words"][4:6] == [65537,0]
    else:
        assert commits[2]["words"][4:7] == [0,0,0 if record["mode"] == "Header" else 1]
    debug = (session / "debug.log").read_text()
    serial = (session / "serial.log").read_text()
    assert re.search(r"(?m)^FAIL ", debug) is None
    def field(name):
        values = re.findall(r"CARD "+re.escape(name)+r"=([0-9a-f]{16})",debug)
        assert len(values) == 1
        return int(values[0],16)
    assert field("outer-gpr-failures") == 0
    assert field("parent-start-status") == (0x8000000000000021 if delivery_failed else 0)
    assert field("bar1-dword-reads") == (32 if record["mode"]=="Header" else 32+(len(child)+3)//4)
    assert field("journal-pat-type")==0 or field("journal-mtrr-uc")==1
    if detail == 7:
        passes = ["firmware-journal-uc-qualified", "external-journal-model-attached", "real-parent-load-start-owner", "reentrant-supported-start-stop-refused", "parent-start-abi-canaries", "observed-host-state-unchanged", "real-loaded-image-count-restored", "exact-parent-result-journal"]
        passes += ["genuine-exit-boot-services-final-journal"] if terminal else ["permanent-one-attempt-latch-after-stop", "stop-cleanup-decode-events", "real-pci-ownership-released", "fixture-protocols-removed", "post-card-boot-services"]
        if not delivery_failed:
            passes.append("ready-after-lifecycle-journal")
        for name in passes:
            assert len(re.findall(r"(?m)^PASS "+re.escape(name)+r"$", debug)) == 1
        if terminal:
            assert field("terminal-ebs-attempts") in (1, 2)
            for name in ("stop-cleanup-decode-events", "real-pci-ownership-released", "fixture-protocols-removed", "post-card-boot-services"):
                assert "PASS " + name not in debug
    if positive:
        for name, expected in record["childObservation"].items():
            values = re.findall(r"SVMVISOR snapshot "+re.escape(name)+r"=([0-9a-f]{16})",serial)
            assert len(values)==1 and int(values[0],16)==expected
    elif mode == "StructuredRefused":
        assert record["childObservation"] == {"entry-profile": 0, "cpuid-refusal": 5}
        assert len(re.findall(r"SVMVISOR snapshot entry-profile=0000000000000000", serial)) == 1
        assert len(re.findall(r"SVMVISOR native-preflight CPUID refused code=00000005", serial)) == 1
        assert "SVMVISOR snapshot transition-" not in serial
    else:
        assert "SVMVISOR snapshot " not in serial
    return {"session":session.name,"profile":record["profile"],"mode":mode,"processors":record["processors"],"journalDetail":detail,"terminalExitBootServices":terminal,"sourceFilesVerified":len(parent_manifest["source_hashes"])+len(child_manifest["source_hashes"])+len(record["sourceHashes"]),"parentSha256":record["parentSha256"],"childSha256":record["childSha256"],"launcherSha256":record["launcherSha256"],"resultSha256":sha(session/"result.json"),"journalCommits":len(commits),"status":"passed"}


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--session",type=Path,action="append",required=True)
    parser.add_argument("--output",type=Path,required=True)
    args=parser.parse_args()
    if args.output.exists():
        parser.error("audit output must be a new path; frozen evidence is immutable")
    results=[audit(s) for s in args.session]
    args.output.write_text(json.dumps({"status":"passed","sessions":results},indent=2)+"\n",encoding="utf-8")
    print(json.dumps(results,indent=2))


if __name__=="__main__":
    main()
