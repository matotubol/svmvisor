# Guest APIC model and physical routing ownership

The captured INIT0->16 refusal came from our global four-bit recipient model.
This refactor defines a conventional guest APIC interface instead of trying to
infer system-fabric fanout from the physical LAPIC's local comparator.

## Guest interface

Guest xAPIC uses exact eight-bit APIC IDs. Enabled x2APIC retains its full-width
IDs. CPUID80000001.ECX.ExtApicSpace and APIC Version bit31 are hidden. Version
reads at MMIO30 and MSR803 agree. Version MSR writes and hidden AMD extended
MSRs840-842/848-853 inject #GP(0) through checked instruction/fault handling;
the guest RIP and operands remain on the faulting instruction. Valid version
reads support the existing long64 and unpaged real16/protected32 modes.

Extended MMIO registers400/410/420/480-4F0/500-530 remain explicitly unsupported
and stop before device access, consistent with the existing reserved-MMIO
policy. The interface does not claim those reserved reads return zero. DFR
readback exposes conventional reserved low28 ones, while writes pass only the
model nibble to the physical backing. MMIO accesses retain the existing DWORD
MOV instruction subset; no new arbitrary memory-operand emulation is added.

INIT/SIPI select an exact assigned guest ID and publish to the existing mailbox.
The private all-excluding-self INIT/#SX wake is a host notification, separate
from guest target selection. Readiness, ownership, queue and locking checks
remain. Narrow physical mode is now a host-normalization invariant failure,
not a guest alias. Historical snapshot predicate numbers remain decodable.

## Physical ownership

All owned extended APICs, including the BSP, normalize APIC410 bit2 before
guest readiness publication. Preparation is read-only. The final arm commit
sets R_INIT and normalizes control after the other fallible setup. It verifies
readback and attempts restoration on failure before returning. Shared mode
publication follows the admitted physical commit under the same route lock.

Hidden IER vectors must already be all enabled and extended LVTs masked/idle
at takeover. Reserved low16 bits of the first IER bank are ignored during
readback admission. The code refuses active hidden state instead of discarding
interrupts. Guest INIT resets the owned LAPIC state while retaining physical
eight-bit routing. IER/LVT reset writes remain bounded; no EOI or guest ICR
delivery is introduced. Ordinary forwarded IPIs use the same normalized IDs.

This is an explicit guest-platform policy. It does not claim that preserving
physical bit2 reproduces a native AMD APIC410 architectural INIT reset. The
guest does not expose that register. It also does not settle native fabric
distribution of a four-bit-mode explicit IPI.

## Refusal evidence

New takeover failures carry tagA1, reason, register offset and observed DWORD.
A high-operand truncation flag prevents presenting malformed64-bit MSR data
as a complete DWORD. AP failures reuse the existing full-width callback record;
BSP failures use stage84 because the callback preserves the caller's RAX and
the ordinary physical-start failure otherwise collapses to35. Only the owning
BSP writes its retained record. Guest runtime failures retain the previous
typed snapshot formats.

## Verification and limits

Independent reviews, original rendered AMD manual page maps/hashes, source
pins, tests and exact build results are retained in
`work/native-apic-model-refactor-2026-09-15`. The new guest fixture executes
CPUID/version/DFR reads, hidden/RO MSR faults with exact #GP retry sites, and
post-restart visibility. It uses explicit DWORD MOV for APIC MMIO: an initial
fixture attempt emitted a compiler-folded memory TEST outside that contract
and was corrected without expanding production instruction semantics.

The guest does not acquire a complete software LAPIC implementation. Logical
or broadcast INIT/SIPI, self-reset, APIC disable/relocation, arbitrary extended
MMIO and paged legacy32/compatibility startup access remain unsupported.
IBS capability is not broadly redesigned here; guest use requiring hidden
extended-LVT programming remains unsupported. Existing hidden-state admission
can still refuse an inherited active configuration with exact evidence.
No native exit-cost baseline, Windows boot, Hyper-V/VBS compatibility, or
malware isolation proof follows from host or emulator tests. Flash/readback
and subsequent physical boot are recorded separately for each exact image.
