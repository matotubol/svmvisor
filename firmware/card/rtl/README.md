# First-party RTL

The new completion-only path consists of `svmvisor_endpoint.sv`,
`svmvisor_bypass.sv`, `svmvisor_completer.sv`, `svmvisor_tx_guard.sv`, and
`svmvisor_pcie_pkg.sv`. Build it with `../build-endpoint.ps1`; run its focused
simulations with `../test-endpoint.ps1`. It does not use the older PCILeech stream
interface below. See [the endpoint implementation notes](../../../docs/completion-only-endpoint.md).

`svmvisor_option_rom.sv` is the read-only 4 KiB data source for Expansion ROM
BAR slot 6. `svmvisor_tlps128_bar_rdengine.sv` is the matching read completer;
it preserves First/Last DWORD byte enables so firmware byte probes receive the
correct Completion Byte Count and Lower Address. The build overlays both into
the pinned upstream board controller.

The ROM has the same two-clock response latency as the upstream BAR blocks.
`tools/rompack` writes its `$readmemh` image as little-endian numeric DWORDs;
the completer converts those DWORDs to PCIe wire byte order. Unused addresses
are padded with `0xff`.

Do not copy the complete upstream PCILeech source tree into this directory.
Board support is pinned by `../config.psd1` and fetched into `target/`;
this directory is reserved for the small amount of code owned by svmvisor.

## Candidate card-flash payload transport

`PAYLOAD_ENABLED=1` adds a **read-only 1 MiB BAR1**. BAR0 and the completion
TX guard remain the sole diagnostic/register and user-TX paths. BAR1 low 20
address bits select bytes in fixed flash range `[0x00400000, 0x00500000)`;
there is no writable flash address register or host-selected SPI command.
The PCI core owns BAR-base matching; its 1 MiB alignment means only those
low address bits are an offset. Read requests may not cross a 4 KiB boundary.
At most 8 DWORDs are accepted per payload request; larger valid requests get
Unsupported Request completions without touching flash. Posted BAR1 writes
are discarded and cannot modify flash or originate completions.

`svmvisor_payload_spi.sv` emits only IS25LP256D **13h** normal reads with a
four-byte address, independent of the flash's EXTADD setting. Each read transfers
one DWORD in 72 serial clocks. SPI mode 0 runs at 15.625 MHz from the 62.5 MHz
PCIe user clock, with bytes converted into little-endian DWORDs. Two ASYNC_REG stages synchronize STARTUPE2 EOS; all SPI activity remains reset
until End Of Startup. Four subsequent CE#-high serial clocks cover STARTUPE2's
initial three swallowed clocks. Reset
and timeout cancel a transaction and repeat priming. The top forces CE# high
immediately on physical PERST# or bypass, even if the user clock has stopped.

The completer waits at most 384 user clocks per word, including request readiness.
A timeout or transport error latches RX fault, cancels as applicable, and fills
the failed word and remaining request with erased `ffffffff` data. An electrically
absent flash also reads as erased data; software must validate the package header
and hash before treating it as a payload. No SPI presence or authentication claim
is made. The full 8-DWORD request is simulation-checked below 40 us with an 8 ns
flash data-output delay; arbitrary downstream PCIe backpressure is outside that
service bound. CE# stays asserted for 32 ns after the final falling drive edge. DXE uses DWORD PCI I/O reads. Limiting the *whole* request avoids
assuming that intermediate completions restart a requester's completion timer.

`ROM_BYTES` independently supports power-of-two ROMs from 8192 through 131072
bytes. The default is 8192 and payload support is disabled by default. ROM
high address bits are retained separately from the page-local TX-guard token.
The integrated test uses 65536 bytes and distinguishes upper-page ROM contents.
The Tcl builder accepts optional sixth/seventh arguments `payload_enabled`
and `rom_bytes`, and fails candidate synthesis if the SCK timing clock is absent.

Configuration pin functions come from AMD Vivado 2026.1's xc7a35tfgg484-2
package database: T19 FCS_B, P22 D00_MOSI, R22 D01_DIN, P21 D02, R21 D03;
CCLK L12 uses STARTUPE2. These are dedicated configuration functions, not
unverified spare board GPIOs. The generated clock is located at USRCCLKO,
including clock-to-Q and the actual routed net. AMD DS181 specifies another
0.5..6.7 ns through STARTUPE2 for this -2 part. MISO input max is 16.7 ns
(6.7 primitive + 2 board round trip + 8 flash tV); min is conservatively 0.5 ns.
The board budget assumes 0..1 ns per PCB leg and is **not measured**.

MISO is captured on the falling user-clock edge, enabled only when SCK is high
and half_cycle is zero: exactly one sample per serial bit, 40 ns after the
preceding falling SCK drive edge. Narrow setup3/hold2 multicycles apply only
to the flash input -> incoming[0] capture path, excluding two disabled edges.
MOSI changes only with falling serial drive edges (32 ns after a rising edge).
Its narrow hold1/start exception permits only a 16 ns launch shift, remaining
stricter than that phase-qualified change. Output delays are max3.5/min-9.7 ns,
including datasheet setup/hold and earliest/latest STARTUP propagation. The
conditional clock and I/O constraints live in endpoint.tcl, applied after
synthesis; XDC supplies static pad mappings. Physical SPI signal quality and
the assumed PCB delay budget remain unmeasured.

References:
- [ISSI IS25LP256D/IS25WP256D datasheet](https://www.issi.com/WW/pdf/25LP-WP256D_128D.pdf), retrieved version Rev.A 2017, sections 8.3 and 9.6.
- [AMD DS181 Artix-7 switching characteristics](https://docs.amd.com/api/khub/documents/iAkxxTOk96ANLJqYf2hgrQ/content), Table 66 TUSRCCLKO.
- [AMD STARTUPE2 primitive](https://docs.amd.com/r/en-US/ug953-vivado-7series-libraries/STARTUPE2).
- [AMD UG470 configuration guide](https://docs.amd.com/api/khub/documents/FOs3lXmlcWxBhTIFxVKyGA/content), first three USRCCLKO clocks.

Tests remain under `rtl/tb/`: the independent flash model accepts only 13h within
the slot and implements edge-based serial data with worst-case output delay.
The test also delays the clock seen by flash 12.8 ns, and adds 5.5 ns return
path delay beyond its 8 ns tV; SI/CE setup/hold and exactly32 capture assertions
check the phase contract. An initial 31.25 MHz candidate was rejected in review
because its SCK-Q clock reference omitted a measured 4.124 ns route and the
specified STARTUP delay; its positive STA summary is not valid proof.
`payload_spi` covers endpoints, bytes, alignment, priming, cancellation, and reset;
`payload_completer` covers real framing/guarded replies, zero requester identity,
latency, ROM enlargement, posted writes, oversized UR, aperture boundaries,
erased flash, lost/error responses, and transaction reset.

The [per-CPU diagnostic transport](../../../docs/native-percpu-diagnostic-transport.md) adds independent retained progress and first-fault banks through read-only USER3, preserving the completion-only endpoint boundary.
