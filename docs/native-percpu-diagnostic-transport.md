# Per-CPU retained diagnostic transport (2026-09-15)

The new USER3 path reads fixed FPGA-owned CPU records without stopping CPUs,
waiting for the terminal barrier, accessing host memory, or issuing PCIe requests.
USER2 retains the legacy lifecycle/last-event snapshot. This adds no DMA or
arbitrary-memory-read capability and does not change the PCIe TX guard.

## BAR0 producer protocol

BAR0offset0x004 advertises schema1, USER2capabilitybit16 and USER3 perCPUcapabilitybit17 (0x00030001).

Legacy ring capacity is32 records, aperture0x100..0x4ff. Existing staging0x40..0x5f,
commit0x60, most-recent record0x80..0x9f and USER2 wire format remain unchanged.

CPU slot s (0..31) owns the80-byte window at0x600+s*0x50. Write19 aligned full-BE
DWORDs in any order, then write DW0 sequence again to DWORD19 (offset0x4c).
Each slot has independent staging; no shared producer lock is required. A partial
record, badBE/alignment, zero sequence, schema mismatch, or reserved metadata
bits causes refusal and a sticky transport error. Transaction reset cancels all
incomplete staging but never clears committed evidence.

|Payload DWORD|Meaning|
|---|---|
|0|Nonzero sequence|
|1|Event bits0..7; schema=1 bits8..15; sticky-fault request bit16; bits17..31zero|
|2|Boot ID|
|3|Physical APIC ID|
|4..5|TSC, low DWORD first|
|6..17|Six event-specific64-bit context values, low DWORD first|
|18|Event-specific32-bit auxiliary value|

Every valid commit replaces that CPU's progress record. The first fault-marked
commit also fills its first-fault bank, which remains unchanged until FPGA
reconfiguration/power loss. Thus fault identity must be interpreted with bootID;
a warm reset does not erase historical first-fault evidence. No BAR0 write can
clear it. Diagnostic-window reads currently return0; use the existing identity
register for any required posted-write drain, not a spin on a diagnostic window.

## Retention and observation limits

A background user-clock round robin mirrors all64 banks (32 progress plus32
first-fault) through an acknowledged XPM mailbox into board-clock distributed
RAM. Each transferred record is coherent. No JTAG clock or all-CPU stop is
required for mirroring. Board copies continue to be readable after PCIe/user
clock loss and transaction reset.

This is bounded last-known state, not an instruction trace or simultaneous CPU
register capture. Intermediate progress may be coalesced. A posted MMIO write
lost before the endpoint accepts it, a CPU fault before publication, or a
user-clock stop before that bank's CDC transfer can leave older evidence.
There is no claim of observing an uninstrumented guest loop. Runtime publication
frequency and event selection belong to the CPU runtime; transport does not
introduce a polling loop or synchronous JTAG dependency in VM-exit handling.
Timing overhead on the physical Windows platform has not yet been measured.

## USER3 reader protocol

UG470v1.17 Table10-2 specifies USER3 IR=0x22 for the monolithic7-series device.
The new1024-bit scan uses low6 TDI bits as a bounded bank selector. A selector is
accepted only after a full1024-bit scan. Request and reply each use acknowledged
CDC; software repeats scans while checking returned bank, IDs and CRC. Incoming
bits have no write/clear/program/PCIe effect. Capture freezes one reply until the
scan ends, even if publication proceeds concurrently. Scan speed remains1MHz.

|Frame DWORD|Meaning|
|---|---|
|0|0x55504353|
|1|0x04000001:128-byte frame/schema1|
|2..3|FPGA build ID|
|4..5|ROM build ID|
|6|Bank bits0..5; published-valid bit6; remainingbitszero|
|7|Sticky transporterrors:bit0invalidwrite/alignment,bit1invalidcommit|
|8..26|Opaque19-DWORD CPU payload|
|27..30|Reservedzero|
|31|IEEE CRC32 of first124bytes|

Banks0..31 are lastprogress;32..63 are firstfault. The decoder reports missing
banks, rejected frames, empty records and non-simultaneous capture explicitly.
It can report valid USER3 evidence when USER2 lacks a stable pair. Mixed-build
frames cannot be merged into one bank collection. Per-event names/fields match
the runtime event table; unsupported event numbers remain visible as raw data.

## Source review and validation evidence

Primary source: AMD UG470v1.17 (December5,2023), applicable to Artix7 XC7A35T,
SHA25652aa1f1943cdf631b5ff3d859a8d5905dc9fd198522e0e9d63cf3f87271d615c.
Visually reviewed PDFpages161,162,163,164 (printed same), completeTable10-2including
continuation and SSI footnote, Table10-3, and referenced USER/BSCANE2 section.
SSI-only larger-IR caveat does not apply to this monolithic target. Pages/rendered
images are retained underwork/native-diagnostics-2026-09-15/transport.

Fault-injection RTL simulation covers interleaved CPU0/31 publication, initial
fault persistence through later faults/progress, interrupted staging at reset,
publication during active scan, and JTAG readout with the PCIe clock stopped.
The Python decoder accepts the exact RTL-produced frame and rejects corruption,
wrong build IDs, and false sticky-fault bank metadata. Existing completer and
payload-completer simulations still exercise completion-only request behavior.
These are mechanism checks; they do not prove Windows boots.

Baseline latest prior package resource usage:8231 LUTs,8359 registers,4 BRAMtiles.
Provisional/final routed-resource and timing outcomes are retained separately;
this document does not substitute for their checks or physical boot evidence.

The UG470p164 timing cross-reference was followed to DS181v1.27.1(July3,2024),
SHA2563f0cb482d645e217ae834e9370c1320300abb85a8046287250f9a48856aed070,
completeTable66PDF/printedpages58..60 includingnotes. The -2 boundary-scan
column specifies3nsTDIsetup/2nshold and maximum66MHzTCK. Our reader uses1MHz;
new TDIselector and TDOscan fabric routes each have a narrow100ns maxdelay
budget within the500ns half-cycle. UG470 saysTDO is registered on fallingTCK.
These are timing constraints/nominal interface budgets, not an oscilloscope
measurement of the programmer or board. No configuration/eFUSE command is used.
