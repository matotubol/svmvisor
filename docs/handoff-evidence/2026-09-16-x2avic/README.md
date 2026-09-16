# x2APIC/x2AVIC completion batch — portable evidence (2026-09-16)

Text evidence behind [the completion record](../../x2avic-completion-2026-09-16.md).
The rendered manual page images the reviews relied on (about 1,000 PNG files)
remain local under `work/x2avic-manual-2026-09-16/` and
`work/x2avic-batch-2026-09-16/`; they are not committed. Paths inside these
files that point at `work/` refer to those local artifacts.

| File | What it is |
| --- | --- |
| `manual/apm2-avic-facts.md` | APM Vol.2 rev3.44 section 15.29 (AVIC/x2AVIC), Tables B-1/C-1, read from rendered pages; SHA256, PDF/printed pages, unresolved points |
| `manual/apm2-x2apic-facts.md` | APM Vol.2 rev3.44 chapter 16 (local APIC/x2APIC), including the Table 16-6 access and #GP matrix |
| `manual/ppr57896-lapic-facts.md` | PPR 57896 rev3.00 (Family 1Ah Model 44h B0): LAPIC/x2APIC registers, CPUID, AVIC doorbell MSR, topology |
| `manual/acpi-madt-facts.md` | ACPI 6.6 MADT rules and the decoded MADT captured from the target machine |
| `manual/decode_madt.py`, `manual/madt-decoded.txt` | Read-only MADT decoder and its output for that capture |
| `design-brief.md` | Coordinator design decisions D1-D10 and file ownership used by the implementation agents |
| `integration-and-fix-notes.md` | Runtime integration record, stop/arm codes, the review-fix pass (F1-F13) and the validation actually run |
| `code-review.md` | Independent correctness/concurrency/cleanup review of the batch (findings M1-M3, L1-L7, N1, T1) |
| `independent-test-notes.md` | Notes of the manual-derived conformance tests (`crates/hypervisor/tests/x2avic_manual_conformance.rs`) |

Scope limits: these files record host-side reviews, host tests and linked-image
audits. No image from this batch was flashed or executed natively. A separate
rendered-page manual-conformance review was stopped before it wrote its report;
its partial page renders remain local and it produced no findings file.
