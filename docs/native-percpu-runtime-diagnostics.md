# Native per-CPU diagnostic runtime

This batch makes the next physical Windows stall observable; it does not establish that Windows boots. Native AMD Family 1Ah Model 44h B0 remains the admitted target.

## Observation

Each resident image records sampled VM exits and resumptions, initial guest acknowledgements, SYSCFG transactions, filtered PAUSE loops, stopped refusals, host faults and terminal rendezvous masks. Records carry the boot/image identities, CPU, sequence, TSC, six full-width values and event-specific auxiliary data. The card preserves latest progress and first fault separately for each of 32 CPUs. USER3 exposes these through the bounded read-only JTAG protocol described in native-percpu-diagnostic-transport.md.

A PAUSE VM exit reloads the nonzero 4096 instruction filter at VMRUN and resumes at the unchanged RIP. Hardware executes the instruction and its debug/event semantics. Only CPUs advertising PauseFilter enable this intercept. PAUSE publication is bounded to one attempt per 50 million TSC ticks per CPU; this is not a watchdog for loops without PAUSE and is not a measured wall-clock period.

Host exception stubs use the private IST, copy vector/error/RIP/CS/RFLAGS/RSP/SS and CR2/CR3 before entering Rust, and latch the first frame. The Rust callback reads no mutable dispatcher state. Deliberate UD, GP, PF and broken-original-RSP cases exercised the actual assembly in disposable OVMF. This is not physical proof of the card export.

## PCI lifetime

The runtime reserves distinct private UC aliases at image offsets FB000 (endpoint configuration) and FC000 (BAR0), independent of the scratch reader and LAPIC aliases. Complete initial CPU acknowledgement is required before live publication; all CPUs then own their config intercepts. No later all-CPU terminal rendezvous is needed for a per-CPU record.

All rebuilt NPTs protect writes to the entire validated ECAM aperture at 2 MiB boundaries, including upstream bridges. The first protected write publishes TRANSPORT_REVOKING under the shared lifetime guard, permanently revokes publication, restores only the owning NPT PDE write bits, requests a full TLB flush and retries unchanged guest RIP. Hardware performs the guest instruction. This deliberately ends observation at PCI reconfiguration instead of inventing a device configuration model.

Scalar CF8/CFC I/O uses the architectural IOIO operand/next-RIP evidence and a prepared continuation. Every CFC data OUT revokes logging before the native port operation, including accesses to bridges. Unsupported modes/widths/string/REP operations remain explicit stopped refusals. MMIO_CONFIG writes are explicitly refused before hardware mutation; dynamic ECAM relocation is unsupported. The legacy terminal exporter also honors permanent revocation.

## Limits and evidence

Before every admitted publication the runtime rechecks physical MMIO_CONFIG, effective UC eligibility, endpoint identity/decode/BAR and image IDs. A same-device read drains posted writes after the commit. Progress tries the shared guard once; first-fault export makes at most256 attempts. A fault while holding that guard cannot unwind it, and can leave only the private retained first frame. Logging before all initial CPU acknowledgements, faults in auxiliary-state switch windows, damaged private descriptors/mappings, firmware/SMM routing changes, reset and clock loss before CDC acknowledgement are not fully covered. A missing record never establishes that execution stopped at the last recorded instruction.

Retained batch evidence: work/native-diagnostics-2026-09-15. Static analysis, disposable execution, routed FPGA checks, programming/readback and subsequent physical boot are separate evidence classes. No Windows security settings or motherboard BIOS policy are changed.
