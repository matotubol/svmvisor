# Shared emulator and packaging tools

The synthetic SVM harness that used to live here (flat Multiboot guests,
emulator-only xAPIC/x2APIC/LAPIC/scheduler models and their run scripts) was
retired on 2026-09-16. The native runtime now requires x2AVIC, which QEMU TCG
does not emulate, so that harness could no longer exercise the production
interrupt path. Its dated reports under `docs/` remain as historical evidence.

What remains is shared by other tools:

- `bootstrap-tools.ps1` downloads the pinned portable QEMU 10.1 build into
  `target/synthetic-tools/`. The native preflight, transition, returning-probe,
  card and host-fault fixtures run from that directory.
- `package-relocations.py` packages a linked flat image and its relocations
  (`SVMRELO1`). `tools/native-resident/build.py` uses it for the resident
  payload; `tests/test_package_relocations.py` covers it.
- `firmware-handoff/` is the firmware-independent handoff/relocation crate used
  by the DXE `card-load-only` loader and by resident payload delivery.

```powershell
./tools/synthetic-harness/bootstrap-tools.ps1
python -m unittest discover -s tools/synthetic-harness/tests
```
