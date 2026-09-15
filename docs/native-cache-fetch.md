# Native guest fetch cache compatibility

The native stopped-guest reader now resolves guest PAT selectors for the root
and each subsequent paging structure. A nonzero PWT/PCD selector is accepted
when its selected PAT entry is WB; selector zero is also checked. With PCIDE
enabled, CR3 low bits remain a PCID. Instruction leaves retain WB and execution
permission checks, while the xAPIC data operand retains its separate UC policy.
The physical reader independently qualifies RAM ownership, monitor exclusion,
host mapping and system memory type before reading.

On CPUID signature 00b40f40 with 48 physical address bits, the shared MTRR
classifier also observes the processor's enabled TOM2 default-WB interval.
Only unmatched complete pages in [4GiB,TOM2) receive that default; matching
variable MTRRs retain precedence. This does not establish that an address is
RAM. Existing RAM-map admission remains required. Other admitted processor
profiles do not read these model-specific registers through this path.

The processor PPR's PAT table reserves UC-minus outside PA2 and PA6. The native
walker conservatively follows that table, including unused slots. This is a
documented restriction on other processor profiles, not a measurement that
other hardware rejects those layouts. Cache-disabled execution and unsupported
cache combinations remain refused without guest-state mutation.

Critical rules were checked using original PDF page images and cross-references.
The implementation reports record document revisions, hashes and page maps:

- [Guest PAT review](../work/native-guest-fetch-cache-2026-09-14/report.md)
- [TOM2 review](../work/native-tom2-default-2026-09-14/review.md)
- [Independent combined review](../work/native-cache-integration-2026-09-14/independent-cache-tom2-review.md)

The latest preceding physical capture reported all CPUs activated and CPU0
stopped before MSR emulation because instruction fetching failed. It did not
retain the rejected predicate, so it does not establish either cache omission
as the cause. See the [capture boundary](../work/native-cache-integration-2026-09-14/capture-analysis.md).

New terminal kind10 preserves a validated software refusal code and canonical48
RIP in the existing bounded record. The decoder distinguishes walker failures
from physical-reader qualification failures. Compound qualification failures
remain compound; the record does not export a physical address or MSR index.
Legacy records keep their meaning, and noncanonical RIP falls back to the
original full-width RIP-only record. Existing raw snapshot collection remains
usable with the updated repository decoder.

No new exits are completed and no rejected instruction advances RIP. The
existing terminal barrier, alias cleanup and stopped-state ownership remain.
Full local tests, linked artifact inspection and disposable emulator results
are retained under `work/native-cache-integration-2026-09-14`; these do not prove
physical Zen5 memory behavior or successful Windows boot. Native timing,
protected-Windows compatibility and Hyper-V/VBS coexistence remain unmeasured.
