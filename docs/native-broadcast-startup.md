# Post-EBS physical broadcast startup

The physical bootstrap now sends one INIT and one SIPI with destination
shorthand **all excluding self**. Each AP discovers its assigned record before
using a stack; the BSP releases APs individually into resident capture. This
replaces the single shared current-target pointer and serial targeted hardware
IPIs. The retained independent AP paging root and copied 95-byte guest wait
continuation, including its actual-CR3 completion witness, remain unchanged.

The implementation and disposable emulator evidence are frozen separately in
`work/native-broadcast-2026-09-14/summary.json`. This is not execution evidence
on the Ryzen 9 9900X or of Windows boot. The earlier frozen
`work/native-loader-2026-09-14/summary.json` is unchanged.

## Ownership and identity

Preparation retains the existing PI inventory rule: the reported processor
total must equal the enabled total, every processor must have the expected
healthy/enabled/BSP flags, hardware IDs must be unique and equal actual CPU-local
CPUID identity, and every blocking AP observation must return successfully.
The final inventory recheck must match. Partial inventories and disabled CPUs
are refused. The broadcast assumes this trusted MP provider enumerates the
complete physical processor set; firmware-hidden processors or concurrent
external startup sources are outside the admitted platform contract.

No INIT, SIPI, APIC promotion or APIC-control write occurs during these returning
firmware callbacks. Startup runs only through the existing caller contract
after successful ExitBootServices, with the BSP's IF clear and retained current
mapping closure revalidated. No Boot Services, allocation or firmware MP call
remains on this path.

An immutable directory contains one BOOT address per assigned AP and a null BSP
slot. In long mode, before any stack access, each AP obtains its full AMD
TOPOEXT identity from CPUID.8000001E when advertised, otherwise the legacy
CPUID.1 identity, and scans at most 32 directory entries. Existing admission
limits supported IDs to the eight-bit identity used by the native capture
owner, but discovery never truncates an unexpected high TOPOEXT identity to
another CPU's stack. An unknown identity or BSP identity cannot acquire an AP
stack.
A locked claim at BOOT+104 rejects a duplicate entry. A BSP release at BOOT+108
admits only one AP at a time to the existing capture owner; later APs perform a
finite integer pause loop. An aggregate failure flag records discovery/release
failure; code 45 and the interface's all-ones failure mask identify that failure
without attributing an unknown CPU to an assigned slot. These fixed iteration
limits are not calibrated physical deadlines. Each AP release loop allows
2,147,483,647 iterations; the BSP allows 20,000,000 completion polls for each
of at most 31 APs. The numerical counts do not establish a cross-CPU elapsed-time
ordering or physical liveness guarantee. Scheduling and latency remain subjects
for the coordinating emulator and later physical measurements.

The BSP waits for each AP's completion publication from copied retained code
and verifies the actual guest CR3 before releasing the next AP. The startup
path performs no additional INIT after any AP enters SVM. Successful return
still requires all assigned APs and then the BSP to enter their resident
continuations. Failure after successful EBS retains resources and requires the
existing terminal caller behavior; it does not fabricate a retryable EBS error.

## Ryzen reset-width interval

AMD APM volume 2 revision 3.44, section 16.5, printed pages 643–644, defines
all-excluding-self shorthand independently of the destination field. Table
16-4 explicitly admits that shorthand for both edge-triggered INIT and Startup
messages. PPR 57896 revision 3.00 APIC300, printed page 61, gives the corresponding
target-specific shorthand encoding. Therefore the SIPI does not depend on an
individual destination ID remaining unique after INIT clears ExtApicIdEn.

After SIPI has arrived, an AP that remains in xAPIC mode reads the validated UC
LAPIC mapping. If extended space is advertised, assembly requires signature
00B40F40h, version 81050010h and feature 00040007h before setting APIC410 bit 2.
It preserves the other defined control bits, rejects reserved bits and verifies
readback. PPR APIC030/400/410, printed pages 56 and 64, supplies those definitions.
An unknown extended profile halts. This operation constructs the physical AP's
initial captured state; it is never invoked for a later target-owned guest INIT.
Later guest INIT must retain its separate PPR-defined reset semantics.

In the native guest-startup profile the BSP's APIC_BASE and extended control are
preserved. BSP preflight still requires its actual extended control to support
the admitted high-ID inventory. The pre-existing diagnostic profile may promote
to x2APIC after EBS. Broadcast sends preserve the ICR high half; hardware ICR low
necessarily records the bootstrap command. Before the first send, the native
guest-startup BSP saves the original canonical ICR in an aligned retained DXE
u64. Restoring the hardware low half with a write would send an IPI. Instead,
directory ABI version 6 adds a final nullable `initial_icr: *const u64` argument
to ArmRuntime, so the existing resident ICR overlay can start from that value.
APs, single-CPU activation and ordinary SMP pass null. The BSP helper refuses
an unsaved value rather than substituting zero. The caller revalidates the
field's current readable mapping; arm must copy it before guest entry and
retain no pointer across private-root or runtime virtual-map changes. This
handoff changes no guest hypercall or physical APIC register.

## Runtime destination ownership

The resident transport now records each target's actual destination mode:
standard xAPIC, extended xAPIC with four or eight matching bits, or x2APIC.
Initial admission publishes this mode before the target's guest ACK publishes
readiness. A later guest INIT really resets APIC410 to zero and publishes
four-bit matching; it does not keep a fictitious guest ExtApicIdEn value.
Guest APIC410 writes and the supported xAPIC-to-x2APIC promotion update the
same record only after their native physical write.

One bounded routing guard in the first shared mailbox serializes destination
selection and queue publication against physical APIC_BASE, APIC410 and reset
commits. The commit token borrows this guard. Source matching includes every
CPU, including the source, and requires one match whose full admitted identity
equals the explicit destination. Source aliases, other ambiguous matches,
physical broadcast encodings, logical destinations and guest shorthand/self
startup remain refused before queue publication. Every assigned CPU must be
ready because the subsequent private hardware INIT uses all-excluding-self.

The guard is released before that notification, any target poll or guest
re-entry. Private INIT is consumed through the existing R_INIT/private #SX
path. An unrelated CPU with an empty mailbox resumes without guest INIT or a
physical interrupt EOI. Only queued commands can reset/start guest state.
Ordinary native IPIs retain their existing physical forwarding path.

Guard acquisition permits 64 CAS/pause attempts. Source contention re-enters
the unchanged guest instruction, bounded to 1,024 retries, with no guest
register, RIP, queue or hardware commit. Target contention stays within the
existing 20,000,000-iteration stopped-host service budget. These bounds are
not elapsed-time guarantees; contention and broadcast fanout add unmeasured
physical overhead. Terminal errors preserve the existing stopped-state policy.

For example, after guest INIT an extended target with ID 16 may have four-bit
matching while source ID 0 remains in eight-bit mode. Explicit destination 16
can still select that target uniquely. If another four-bit target or the source
also matches, the command is refused. This removes the unconditional high-ID
reset rejection while preserving the actual guest routing constraints.

## Measured checkpoint

All 700 host tests passed (413 core and 287 DXE). New routing tests cover
contention before mutation, publication before unlocked notification, high-ID
reset matching, source/target aliases, broadcast encodings, readiness, native
ExtControl changes and full-width x2APIC IDs. PI admission cases retain refusal
for disabled subsets, CPU-local ID mismatch and final inventory/status changes.

Five builds share the same 252-file source manifest: diagnostic boot,
production boot, the earlier explicit startup profile, single CPU and ordinary
SMP. The complete linked raw payload audits found no forbidden FP/SIMD/xstate
instructions or undefined symbols: 13,008 diagnostic and 10,765 production
instructions. The copied wait remains 95 bytes with zero section relocations.
The production image was built and audited, not executed.

Ten final disposable emulator scenarios passed, with 116 AP restarts. They
include xAPIC boot at 2/24/32 CPUs, replacement loader page tables at 24 CPUs,
nonidentity runtime mapping, x2APIC level-interrupt behavior, xAPIC promotion,
the earlier explicit startup and ordinary profiles, and admission refusals.
The frozen QEMU #SX backend and OVMF images were reused without modification.
QEMU's standard APIC does not exercise the exact Ryzen extended-register
hardware path; that path has manual review and pure routing/reset tests only.

The BSP ICR witness compares guest readback with the snapshot after firmware
returns and before our physical sends. A first fixture compared EBS entry
instead and failed: OVMF itself changed 0x4687 to 0xC4687 during its EBS AP-loop
relocation. The pinned local EDK2 stable202408 source, commit
b158dad150bf02879668f72ce306445250838201, confirms that callback and broadcast
in DxeMpLib.c and MpLib.c. A separate 24-CPU attempt completed the guest workload
but failed parsing because empty wake traces interleaved. Final diagnostics
retain every CPU's wake count and print command evidence on queued targets;
the failed attempts remain recorded and are excluded from passing totals.

## Fidelity limits and sources

All pre-capture bootstrap instructions are integer assembly; no compiler FP code
runs before the native boundary saves admitted xstate. Unsupported runtime exits
still require authoritative stopped VMCB/register state and no arbitrary RIP
advance. Initial loader admission still requires XSS=0, XCR0=3/7 and a save
image no larger than 1,024 bytes. Initial CR4 still excludes PCID, FSGSBASE,
SMEP/SMAP, PKE and CET. The exact target PPR supports a larger XCR0 mask 0x2E7,
uncompacted image up to 0x988 and CET XSS mask 0x1800; no observation establishes
that Windows enables those features before EBS. Later native guest XSETBV and
AVX-512 execution are separate from that initial loader capture gate. PPR
CPUID D.1:EAX=0xF provides no basis for adding an XFD blocker.

Intercepted CPUID/MSR execution remains limited to long64 and the bounded
unpaged startup path; legacy paging/compatibility, unsupported prefixes and
debug single-step remain stopped. Intercepted TSC_RATIO, C0010100–C00101FF and
SYS_CFG writes are not all implemented by the dispatcher. Their occurrence
during an ordinary Windows boot is unobserved. Nested SVM/Hyper-V/VBS remains
unsupported. High-ID initial BSP admission still requires its actual extended
routing enabled, and native inventory/capture still bounds IDs below 255.

No physical timing, Windows image execution, protection configuration change,
Hyper-V/VBS/HVCI/PatchGuard/Secure Boot compatibility test, device/DMA isolation
or malware execution was performed. Physical first boot and protected Windows
compatibility remain unvalidated; this is not containment evidence.

Sources were read from the supplied local primary-source library and matching
extracts: AMD APM2 24593 revision 3.44 SHA256
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`,
and AMD PPR 57896 revision 3.00, Family 1Ah Model 44h B0, SHA256
`643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5`.
The existing inventory owner cites PI 1.10 II-13.4.1/.5/.8 and the EBS owner cites
UEFI 2.11 section 7.4.6. No processor applicability beyond the admitted profiles
is inferred from emulator behavior.
