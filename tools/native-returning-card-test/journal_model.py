"""Externally model one BAR0 journal in pinned disposable QEMU TCG.

QEMU GDB virtual write watchpoints stop after the commit instruction. The model
copies all eight actual staged DWORDs to the committed record and acknowledges
the exact sequence. It never sets guest DRs or changes any executable byte.
"""
import argparse
import hashlib
import json
import pathlib
import re
import socket
import struct
import subprocess
import time


class Remote:
    def __init__(self, sock):
        self.sock = sock
        self.sock.settimeout(0.2)
        self.buffer = bytearray()

    def send(self, value):
        raw = value.encode("ascii")
        self.sock.sendall(b"$" + raw + b"#" + f"{sum(raw) & 255:02x}".encode())

    def receive(self, deadline):
        while time.monotonic() < deadline:
            start = self.buffer.find(b"$")
            if start >= 0:
                end = self.buffer.find(b"#", start)
                if end >= 0 and len(self.buffer) >= end + 3:
                    raw = bytes(self.buffer[start + 1:end])
                    checksum = int(self.buffer[end + 1:end + 3], 16)
                    del self.buffer[:end + 3]
                    if sum(raw) & 255 != checksum:
                        raise RuntimeError("GDB packet checksum mismatch")
                    self.sock.sendall(b"+")
                    return raw.decode("ascii")
            try:
                more = self.sock.recv(65536)
                if not more:
                    raise EOFError("QEMU GDB disconnected")
                self.buffer.extend(more)
            except socket.timeout:
                continue
        return None

    def command(self, value):
        self.send(value)
        response = self.receive(time.monotonic() + 5)
        if response is None:
            raise RuntimeError(f"GDB command timeout: {value}")
        return response

    def read(self, address, size):
        reply = self.command(f"m{address:x},{size:x}")
        data = bytes.fromhex(reply)
        if len(data) != size:
            raise RuntimeError("Short guest memory read")
        return data

    def write(self, address, data):
        if self.command(f"M{address:x},{len(data):x}:{data.hex()}") != "OK":
            raise RuntimeError("GDB guest model write failed")


def main():
    ap = argparse.ArgumentParser()
    for option in ("qemu", "firmware", "vars", "disk", "session"):
        ap.add_argument("--" + option, required=True)
    ap.add_argument("--processors", type=int, choices=(1, 2, 4), required=True)
    ap.add_argument("--journal-detail", type=int, choices=(6, 7, 8), default=6)
    ap.add_argument("--mode", choices=("Positive", "Header", "Digest", "Admission", "PristineRefused", "StructuredRefused"), default="Positive")
    ap.add_argument("--terminal-ebs", action="store_true")
    ap.add_argument("--resident", action="store_true")
    ap.add_argument("--x2apic-off", action="store_true")
    args = ap.parse_args()
    if args.x2apic_off and not args.resident:
        ap.error("x2apic-off requires the explicit resident fixture")
    if args.resident:
        if args.processors != 2 or args.journal_detail != 8 or args.mode not in ("Positive", "Header", "Digest", "Admission") or args.terminal_ebs:
            ap.error("resident mode requires two CPUs, detail 8, Positive/Header/Digest and no terminal-ebs")
    elif args.processors == 2 or args.journal_detail == 8:
        ap.error("two CPUs/detail 8 require explicit resident mode")
    if args.mode in ("PristineRefused", "StructuredRefused") and args.journal_detail != 7:
        ap.error("refusal scenarios require explicit detail 7")
    if args.terminal_ebs and (args.mode != "StructuredRefused" or args.journal_detail != 7):
        ap.error("terminal EBS requires StructuredRefused detail 7")
    session = pathlib.Path(args.session)
    qemu_hash = hashlib.sha256(pathlib.Path(args.qemu).read_bytes()).hexdigest()
    expected_qemu = "677158d2f10933bfc8770e3741a3c6ebf33466d1f7f71fee87e6aec3e009b240" if args.resident else "57448131c0fbaed74e059ab0f12b97d6ec278c0215330e585004102859a2be71"
    if qemu_hash != expected_qemu:
        raise RuntimeError("QEMU executable pin mismatch")
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    cpu = {"PristineRefused": "max,svm=on,hypervisor=on", "StructuredRefused": "max,svm=off,hypervisor=off"}.get(args.mode, "max,svm=on,hypervisor=off")
    if args.x2apic_off:
        cpu += ",x2apic=off"
    command = [args.qemu, "-machine", "q35,accel=tcg", "-cpu", cpu, "-m", "256M", "-smp", str(args.processors),
               "-drive", f"if=pflash,format=raw,readonly=on,file={args.firmware}",
               "-drive", f"if=pflash,format=raw,file={args.vars}",
               "-drive", f"format=raw,snapshot=on,file={args.disk}",
               "-display", "none", "-serial", f"file:{session / 'serial.log'}", "-monitor", "none", "-nic", "none", "-no-reboot",
               "-debugcon", f"file:{session / 'debug.log'}", "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
               "-S", "-gdb", f"tcp:127.0.0.1:{port}"]
    record = {"schema": 2 if args.journal_detail == 7 else 1, "journalDetail": args.journal_detail,
              "mode": args.mode, "resident": args.resident, "terminalExitBootServices": args.terminal_ebs,
              "model": "external-ram-backed-bar0-journal", "guestDebugRegistersChanged": False,
              "qemuSha256": qemu_hash, "command": command, "commits": [], "status": "running"}
    process = None
    try:
        with open(session / "stdout.log", "wb") as stdout, open(session / "stderr.log", "wb") as stderr:
            process = subprocess.Popen(command, stdout=stdout, stderr=stderr, creationflags=subprocess.CREATE_NO_WINDOW)
            deadline = time.monotonic() + 100
            sock = socket.socket()
            while True:
                try:
                    sock.connect(("127.0.0.1", port))
                    break
                except ConnectionRefusedError:
                    if process.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError("QEMU GDB not ready")
                    time.sleep(0.05)
            with sock:
                remote = Remote(sock)
                record["initialStop"] = remote.command("?")
                remote.send("c")
                base = None
                while time.monotonic() < deadline:
                    debug = session / "debug.log"
                    trace = debug.read_text(errors="replace") if debug.exists() else ""
                    found = re.search(r"CARD journal-base=([0-9a-f]{16})", trace)
                    if found:
                        base = int(found.group(1), 16)
                        break
                    if process.poll() is not None:
                        raise RuntimeError("QEMU exited before journal gate")
                    time.sleep(0.02)
                if base is None or not 0x100000 <= base <= 0xffff000 or base & 4095:
                    raise RuntimeError("Invalid or missing exact journal page")
                sock.sendall(b"\x03")
                record["gateStop"] = remote.receive(time.monotonic() + 5)
                if remote.command(f"Z2,{base+0x60:x},4") != "OK":
                    raise RuntimeError("QEMU write watchpoint unavailable")
                record["journalBase"] = base
                remote.write(base + 0x100, struct.pack("<I", 1))
                remote.send("c")
                previous = 0
                while time.monotonic() < deadline:
                    try:
                        stop = remote.receive(time.monotonic() + 0.3)
                    except (EOFError, ConnectionResetError, ConnectionAbortedError):
                        break
                    if stop is None:
                        if process.poll() is not None:
                            break
                        continue
                    if stop.startswith(("W", "X")):
                        break
                    if not re.search(r"watch:" + f"{base+0x60:x}" + r";", stop):
                        raise RuntimeError(f"Unexpected debugger stop: {stop}")
                    staged = remote.read(base + 0x40, 32)
                    sequence = struct.unpack("<I", remote.read(base + 0x60, 4))[0]
                    words = list(struct.unpack("<8I", staged))
                    ack = struct.unpack("<I", remote.read(base + 0x2c, 4))[0]
                    if sequence != words[0] or sequence != (previous + 1) & 0xffffffff or ack != previous:
                        raise RuntimeError(f"Invalid commit stage/sequence/ack: {words}, {sequence}, {ack}")
                    remote.write(base + 0x80, staged)
                    remote.write(base + 0x2c, struct.pack("<I", sequence))
                    if remote.read(base + 0x80, 32) != staged or struct.unpack("<I", remote.read(base + 0x2c, 4))[0] != sequence:
                        raise RuntimeError("Model acknowledgment readback failed")
                    record["commits"].append({"stop": stop, "sequence": sequence, "words": words,
                                               "stagedHex": staged.hex(), "acknowledged": True})
                    previous = sequence
                    remote.send("c")
                else:
                    raise RuntimeError("QEMU integration timeout")
            process.wait(timeout=5)
            record["exitCode"] = process.returncode
            if process.returncode != 33:
                raise RuntimeError(f"Guest integration failed: QEMU exit {process.returncode}")
            record["status"] = "passed"
    except Exception as error:
        record["status"] = "failed"
        record["error"] = str(error)
        raise
    finally:
        if process is not None and process.poll() is None:
            process.kill()
            process.wait()
        (session / "journal-model.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
