# Native DXE continuation preparation

This records the preparation batch of 2026-09-13, before executable activation.
The subsequent [resident activation result](native-resident-activation.md)
establishes callback return and ExitBootServices on the single-CPU validation
backend. Windows and physical resident activation remain unexecuted. The
existing physical result is still the exact 65-entry returning probe.

The user's immediate target is Windows on the existing machine under DXE.
Analysis features and general device emulation are deferred. The intended path
is a real ReadyToBoot callback that resumes its own epilogue as a guest, letting
firmware and the Windows loader continue normally.

## Implemented path

`crates/dxe/src/native/resident/mod.rs` consumes the existing 1408-byte
NativeBoundary, matching parsed GDT and VMSAVE auxiliary capture. It checks the
exact assembly stack recipe, complete return/shadow span, original flags,
selectors and linked ACK-site offsets. It derives the actual trampoline frame
and calls `guest::continuation::prepare_native`; it does not seed another loader
stack or replace native CR3. Refusal leaves VMCB and register frame unchanged.

The core serializer preserves raw CR3 PWT/PCD, GDT/IDT, segment state, TLS bases,
SYSCALL/SYSENTER state, CR2, debug registers and PAT for the admitted original
profile. It rejects unowned pending events and additional VMCB virtualization
controls. Source state remains immutable. EFER.SVME is required hardware backing;
the runtime must still implement its guest-visible MSR policy.

The callback assembly reuses `native/admission/boundary.S`. It captures flags
before CLI and restores the original callback registers/xstate/flags on refusal
or on the eventual guest epilogue. The guest first executes an exact five-byte
MOV and three-byte VMMCALL. `NativeBootstrapAck` accepts that linked site and
cookie once, checks original bootstrap RSP/RFLAGS/GPR witnesses and event state,
then advances only that instruction. An ACK proves neither the later RET nor EBS.

The separate 290-byte resident assembly switches to supplied private host
tables/stack/descriptors, owns HSAVE, and runs a Win64 VM-exit dispatcher through
the 112-byte BridgeContext. Its 95 linked instructions contain no FP/SIMD use
and no direct external symbol references. The indirect dispatcher and fault
handlers are not yet linked or audited; this is not a complete runtime proof.
The original callback object remains guest-visible only through its return.

`memory::npt::IdentityNpt` maps the admitted native physical aperture up to
1 TiB and excludes a monitor extent of at most 1 MiB in one 2 MiB window. It
uses 1 GiB leaves and splits only around the exclusion, using at most five
of eight owned table pages. All eight pages must be inside the exclusion.
The original strict 4 KiB/W^X NPT builder remains unchanged in behavior.
The native path allows ordinary guest RAM/device use with original guest CR3;
it is not a device emulator or DMA-isolation claim.

`resident/memory.rs` is the DXE caller: it checks the complete sorted supplied
map, continuous RuntimeServicesCode/Data coverage with EFI_MEMORY_RUNTIME and
WB capability, and only then builds NPT. Map/PAT evidence does not prove actual
allocation, effective MTRR caching or executable host mappings. MMIO absent
from the firmware map needs separate platform admission.

## Validation and retained evidence

Host suites cover native state serialization, transactional refusals, the
callback stack recipe and stale captures, ACK mismatches/replay, runtime map
gaps/reclaimed types, and identity-NPT boundaries. The existing core and native
returning DXE suites pass; the UEFI target checks. Exact commands, counts and
source hashes are in `work/full-continuation/summary.json`.

`tools/native-resident-audit/run.py` compiles both callback profiles and links
the resident assembly alone. It verifies the original ungated capture object's
text is byte-identical to the retained pre-batch baseline, checks exact ACK
bytes/offsets, and audits the linked assembly instruction allowlist. Evidence:
`work/full-continuation/assembly-audit-final/summary.json`. Earlier failed audit
attempts are retained; they exposed tooling/path/disassembly-format issues,
not executed firmware failures. No QEMU run or timing measurement was performed.

Fresh review found and fixed a missing VMCB 0xB8 control refusal and added exact
descriptor-byte assertions. Follow-up review found no blocking defect in the
inert preparation scope. Reports are in `work/full-continuation/research/`.

## Work before the first Windows boot attempt

1. Load a separately retained raw host payload through a legal runtime-driver
   allocation contract; bind host code/data, private mappings, descriptors,
   stacks and the actual dispatch/fault-handler closure. UEFI 2.11 7.2.1 forbids
   driver AllocatePages(EfiReservedMemoryType); Table 7.10 and 8.4.1 govern
   runtime lifetime and loaded-image virtual-address fixups.
2. Connect real ReadyToBoot capture/admission to that payload, preserve the
   actual xstate image and hidden-cache/CPU ownership contract, and execute
   ACK -> original callback RET -> benign EFI/EBS continuation. Only that
   execution can close the callback gate.
3. Integrate native per-CPU ownership/startup and the existing machine's boot
   execution policy, including EFER/MSRs, interrupts and xstate evolution.
   The earlier two-CPU emulator fixture does not establish native Windows SMP.
4. Run the chosen Windows installation with its actual protection configuration
   and record loader/EBS/kernel milestones and first unsupported exit. No
   success claim or firmware deployment follows from the current source checks.

Normative sources: local AMD APM2 revision 3.44, sections 4.5, 11.4/11.5,
15.5/15.7/15.25 and Appendix B; pinned UEFI 2.11, sections 2.3.4.2, 7.2.1,
7.4.6 and 8.4.1, Table 7.10. Native profiles are deliberately bounded; PCID,
LA57/CET and other unowned initial controls are refused rather than normalized.
