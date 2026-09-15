# Native loader and retained-memory review — 2026-09-14

The existing private resident runtime does not need permanent ownership of the
firmware's original page tables. The remaining dependency is the diagnostic AP
bootstrap and its captured guest waiting continuation. The smallest useful
lifetime change is an owned bootstrap root plus a successful-EBS-return handoff
that consumes the low trampoline before returning to the loader. A generic
emulated platform, another host paging owner, and indefinite pinning of firmware
memory are unnecessary for this change.

This began as an independent source review of the intentionally dirty authoritative
tree at `C:/Users/mato/.codex/worktrees/7a58/svmvisor`. Root subsequently assigned
this reviewer the bounded physical-xAPIC admission fixes described below. Root
owns the simultaneous runtime xAPIC integration and all builds/tests;
the observations below identify the reviewed symbols because their line numbers
can change during that integration. No hardware, Windows, ESP, firmware or
frozen evidence was modified. No executable or timing measurement was run by
this reviewer.

The loader root/lifetime recommendations remain future work in this batch.
The implemented changes are restricted to `physical_boot.rs`, `physical.S`, the
existing core `memory/mtrrs.rs` owner and `tests/native_mtrrs.rs`.

## Current ownership, with actual callers

| Resource | Actual owner and use | Lifetime consequence |
|---|---|---|
| Raw per-CPU resident image, host tables, stacks, VMCBs, HSAVE and mailbox | `native/resident/allocation.rs` allocates RuntimeServicesCode through AllocateAnyPages; `host/resident/runtime.rs::prepare` builds a private four-table root inside each runtime slot. `runtime.S::svmvisor_resident_enter` switches to it before the VM loop. | Already independent of firmware page tables and original DXE code after commit. Keep this owner. |
| Original BSP CR3 and caller stack | `activation.rs::callback` captures the current native state and `prepare_callback` retains it in the guest VMCB/frame. | These remain ordinary loader-owned guest resources. After the guest switches CR3/stack, the host instruction reader uses that new stopped state. The host does not require the obsolete BSP root forever. |
| Low SIPI page | `physical_boot.rs::prepare` allocates one low LoaderCode page and patches it with the firmware CR3 and long-mode entry. | LoaderCode is reclaimable after EBS. Retention in the fixture is an explicit consumer promise, not an OS reservation. It is needed only until all physical APs have left the trampoline; later guest SIPIs use the guest's vector. |
| AP initial root | `physical_boot.rs::prepare` patches `svmvisor_ap_root` to `cfg.cr3`; `physical.S` loads it before calling the capture callback. | The AP guest VMCB still owns that borrowed firmware root after host entry. `svmvisor_ap_main` spins as a guest on that root until OS guest INIT/SIPI. Freeing it merely because the AP entered a private host is incorrect. |
| AP bootstrap stacks/GDT/IDT and guest waiting code | `physical_boot.rs::BOOT` and `physical.S` are in the loaded runtime DXE image, outside the excluded monitor pool. | They are retained by the runtime image and remain accessible through guest NPT. Their initial guest linear mappings must survive until the OS replaces that guest state. |
| Firmware memory-map snapshot | `activation.rs::MAP`, copied again into each runtime's `RAM`. | This is explicitly a RAM/aperture observation. It is not a lease on whichever firmware allocation later occupies a conventional-memory descriptor. Do not turn its old memory types into claims of current ownership. |

The current native instruction reader already walks the stopped guest CR3 via
`host::paging::translate`, creates a bounded private read-only/NX aperture,
checks WB RAM outside the excluded pool, and removes the alias before returning.
It therefore has a useful caller for nonidentity guest mappings already. No new
general guest address-space layer is needed.

## Smallest complete loader/lifetime batch

1. Build a bounded bootstrap identity root in retained runtime DXE storage before
   publishing activation. Cover the exact bootstrap code/data/table/stack closure,
   callback admission reads and raw runtime entries. Retain the existing private
   host roots. A narrowly admitted low-address bootstrap root is sufficient;
   copying arbitrary firmware paging trees creates unnecessary ownership work.
2. Patch the low trampoline to the owned root and capture AP guests using it.
   Separate the original firmware-root observation from validation of this owned
   root: `admission_observer` currently compares `current.cr3` with `BOOT_CFG`.
   Validate current firmware mappings for pre-entry accesses and owned bootstrap
   translations for the AP path, without pretending they are the same CR3.
3. Integrate activation at a seam which runs after the original EBS service has
   returned success and before normal loader execution resumes. Ordinary
   ReadyToBoot currently refuses multiple CPUs; the SMP profile only publishes
   `ActivationInterface`, which the disposable fixture explicitly calls.
   An EBS event does not establish that all firmware callbacks have completed.
   Preserve original EBS arguments, exact failure/retry results and calling CPU
   state. Perform all allocation and MP observation before the first EBS attempt.
4. The low LoaderCode lease ends after every AP has published its guest
   continuation and before handing control back to the loader. Do not free it
   through Boot Services after successful EBS. On partial activation failure,
   retain all active resources and terminate with bounded evidence; do not return
   an ordinary retryable EBS error after firmware has already shut down.
5. Exercise guest CR3 replacement and deliberate reclamation of fixture-owned
   obsolete paging pages after all users have switched away. Then exercise a
   nonidentity runtime map and a runtime-service call through the new virtual
   pointer. These witnesses test the actual lifetime claim; another identity-map
   run cannot close it.

This review does not choose a specific EBS hook mechanism without reviewing its
actual implementation and ownership. The normal loader seam is presently absent,
not a hidden feature of the configuration-table ABI.

Changing the low page to RuntimeServicesCode with AllocateMaxAddress is not a
portable one-line fix. UEFI 2.11 §7.2.1 requires AllocateAnyPages for those runtime
types in drivers/applications not targeted at a specific implementation. A
firmware-specific exception would require explicit target admission and actual
map proof. Making the existing LoaderCode page temporary avoids that exception.

## Virtual-address continuation

`tools/native-resident/fixture/src/main.rs::install_identity_map` sets every
runtime descriptor's `virt_start` equal to `phys_start`, calls SetVirtualAddressMap,
then calls GetTime. This invokes callbacks and image fixups but never changes an
address; its comments accurately acknowledge that it retains the existing root.

UEFI SetVirtualAddressMap updates assigned runtime pointers and reapplies loaded
runtime-image fixups. The separately copied raw monitor image is not a loaded
PE runtime image, and its private host linear/physical addresses are not guest
firmware virtual pointers. Keep those values stable; do not apply ConvertPointer
blindly to VMCB physical addresses, host CR3, NPT roots or private host pointers.

Conversely, any DXE interface or pointer used through firmware's new virtual
mapping needs the appropriate lifetime/conversion contract. No virtual-address
change registration exists in the native activation owner today. Its diagnostic
AP code and bootstrap tables also need an explicit decision about continued use
under their retained physical guest root while firmware applies PE fixups. A
nonidentity fixture should check this concurrently with AP wake/startup and
should obtain/use the converted RuntimeServices pointer, then remove obsolete
identity aliases where the fixture owns them. Raw monitor closure must remain
unchanged and resident throughout.

## Review of the concurrent physical xAPIC integration

The reviewed root edits add `send_startup` and preserve the BSP's native mode in
the explicit guest-startup profile. The following were sent to root during the
review; they are actionable checks, not claims that the final integrated source
still contains each issue.

* Before the first direct `FEE00310/FEE00300` MMIO access, prove the current
  firmware mapping is identity, present, writable and supervisor, with an
  admitted uncacheable effective type. APIC_BASE equality alone is insufficient.
  Reuse `activation.rs`'s existing paging walker and safe WB table-backing reader;
  do not call `mapped`, which intentionally requires the target itself to be WB
  RAM. MMIO target admission is separate from page-table backing admission.
* Reuse `memory::mtrrs::Mtrrs` (reexported by `resident::launch`) for any added
  bounded MTRR query. PAT UC alone is not proof of effective UC on AMD:
  APM2 Table 7-11 distinguishes UC+UC from UC combined with WB/WT/WP/WC (CD).
  Preserve a documented admitted cache combination; do not add another MTRR
  parser in the bootstrap module. Recheck live mappings before startup because
  a prepublication walk does not freeze firmware page-table contents.
* The APIC base field extends through bit 51. A check using only
  `base & 0xffff_f000` can admit a relocated high physical page whose low bits
  match FEE00000. Compare the complete architectural/address-policy field.
* INIT preserves AE/EXTD. `physical.S`'s OR of the desired mode cannot make an
  already-x2APIC AP become xAPIC. Require compatible base/mode on every observed
  CPU and verify the actual AP mode before capture. Refuse mismatches; do not
  attempt the architecturally invalid direct x2APIC-to-xAPIC transition.
* Bound ICR delivery-status polling before as well as after changing the xAPIC
  ICR. High-before-low ordering and the existing publication MFENCE are useful;
  they do not prove that a preceding firmware IPI was already idle. Timeout
  should preserve failed-state evidence before the terminal stop.

### Implemented admission fixes

`physical_boot.rs::validate_lapic` now checks the full architectural APIC base,
walks the actual firmware mapping before MMIO, admits only an identity writable
supervisor mapping, and applies the shared MTRR/PAT UC predicate. Page-table
backing is still checked as WB RAM; leaf PCD/PWT bits select PAT rather than
being confused with the next table's cache type. The check runs during bootstrap
preparation, per-CPU MP admission and again on the BSP before first startup.
The x2APIC path has no physical MMIO read and therefore does not require a
firmware LAPIC MMIO mapping merely to issue MSR writes.

`memory::mtrrs::Mtrrs` now has one shared checked range/type parser for its WB
predicate and new `page_is_uc(page, pat_type)` caller. Effective UC accepts an
MTRR UC page combined with PAT UC, UC-minus, WT, WP or WB. It rejects PAT WC,
invalid encodings, malformed/undefined ranges and out-of-width addresses. The
initial profile still requires enabled MTRRs and addresses at or above1MiB.
Four new tests cover all256 PAT bytes against the five legal MTRR types, exact
range edges, overlap precedence and malformed observations. They were added but
not run by this reviewer; root is responsible for execution evidence.

AP assembly now verifies the fixed base, keeps a matching mode, or performs and
checks the legal xAPIC-to-x2APIC promotion. It refuses demotion. Preadmission
conservatively refuses an x2APIC AP when the initially observed BSP is xAPIC;
APs initially in xAPIC may follow a BSP that promotes to x2APIC before calling
activation. The chosen desired mode is published with the existing bootstrap
record and MFENCE.

`send_startup` now bounds delivery-status polling before and after its xAPIC
high/low write pair. Reasons40/41 distinguish a previous pending command from a
submitted command that did not become idle. The caller publishes the target's
failed bit before returning the diagnostic activation error. This preserves the
existing explicit-consumer failure contract; it is not permission for a future
normal-loader EBS wrapper to return an ordinary retryable error after EBS success.

A subsequent physical-bootstrap review found that the PPR four-bit destination
rule must be checked before any physical INIT, not only at runtime arm. The MP
admission observer now reads the actual LAPIC version only after its mapping/cache
check. Native xAPIC with an extended version is admitted only for the exact
Family1Ah Model44h B0 signature `00b40f40` and version `81050010`, and only when
the existing `NativeIcr` topology owner admits reset extended control0. That
requires unique IDs below15 across the complete immutable assignment list;
Fh is broadcast and larger IDs can alias after INIT clears ExtApicIdEn. Unknown
extended profiles and the target's24-thread extended-xAPIC inventory are refused
before activation publication. Initial x2APIC CPUs do not take this xAPIC gate.
This is a deliberate physical-startup compatibility limit, not a successful
24-thread physical bootstrap claim. The LAPIC mapping validator is also exposed
to its existing activation parent for an immediate pre-arm recheck.

## Exit correctness and compatibility limits

The loader handoff is a native call/return boundary, not a newly supported guest
VM exit. Keep original EBS failure/retry semantics and do not fabricate success
after incomplete activation. Once VMRUN has committed, unsupported exits remain
terminal with the original stopped VMCB/register frame authoritative. A paging
walk or NPT refusal is not automatically a guest #PF and must not advance RIP.

For future reclamation/nonidentity tests, use intercepted CPUID or supported
MSR instructions at the new guest addresses to prove the private reader follows
the current guest tables. A successful direct instruction that never exits does
not prove this read path. Ensure negative unmapped/protected cases retain RIP,
registers and pending-event state. Observe both guest execution and host closure
so a retained identity alias cannot silently rescue the test.

All persistent host paths must retain their no-firmware/no-allocation/no-FP
contract and linked audit. Bootstrap work also cannot introduce compiler FP
before the native assembly xstate capture. Diagnostic counted poll loops are
bounded work, not calibrated physical delays; no latency distribution or native
clock baseline was measured here.

There is no new Windows boot evidence. Hyper-V/nested SVM, VBS, HVCI, PatchGuard,
Secure Boot and protected-memory configurations remain untested or unsupported
according to their existing owners; no protection should be disabled to produce
a passing result. Current admission also refuses nonzero encryption capability
leaves. Closing page lifetime or xAPIC startup alone does not close actual Ryzen
admission, normal Windows boot or malware containment.

## Primary references checked

* Local UEFI 2.11, SHA256
  `a64b8e442004b91becc3de9afaf8ca61b259a9a3b436accb6b3711ab5400cee9`:
  §7.2/Table 7.10 printed153 (PDF237), §7.2.1 printed154–155
  (PDF238–239), §7.4.6 printed203–204 (PDF287–288), §8.4.1–8.4.2
  printed236–238 (PDF320–322). The local PDF was read directly. The official
  [UEFI boot-services page](https://uefi.org/specs/UEFI/2.11/07_Services_Boot_Services.html)
  and [runtime-services page](https://uefi.org/specs/UEFI/2.11/08_Services_Runtime_Services.html)
  returned HTTP403 during this review; no independent successful download is
  claimed.
* AMD APM2 rev3.44, March2026, supplied extract
  `work/qemu-corrections/amd-apm-vol2.txt`, document SHA256
  `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`:
  §7.4 memory types, §7.8.5/Table7-11 PAT/MTRR combination,
  §16.3.2 APIC mapping, §16.9.1 mode transitions, §16.10 INIT mode retention.
* PI1.10 II-13.4.1 ownership/event-order contract is retained in the supplied
  handoff and `docs/native-startup-activation.md`; the already pinned PI review
  is `work/native-percpu/pi-1.10-review.md`. This review did not replace that
  earlier MP-services analysis.
