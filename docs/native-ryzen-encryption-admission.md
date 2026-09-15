# Native Ryzen encryption admission

The resident admission path distinguishes encryption capability from enabled
address-encryption modes. This closes a source-level refusal of the existing
Ryzen 9 9900X's advertised SME capability; it is not physical admission or Windows
boot evidence. The nonzero-leaf profile is deliberately restricted to signature
`00b40f40` (Family 1Ah Model 44h B0), 48 physical address bits and C-bit 51.
Other nonzero encryption profiles remain unsupported pending their PPR review.

`arch::x86_64::encryption::NativeEncryptionPlan` is the shared pure owner used by
DXE admission and resident per-CPU capture. It validates CPUID before identifying
the MSRs the caller may read. SYS_CFG evidence is mandatory for the reviewed
SME-capable target. SEV_STATUS is requested only if CPUID advertises SEV; its
absence otherwise must not provoke an unsupported RDMSR. No missing observation
is interpreted as zero. An absent or all-zero encryption leaf remains usable
for legacy native profiles, but is rejected as contradictory on the reviewed
target, whose PPR specifies SME as present.

SYS_CFG bits 23 through 26 must be clear: SME, SNP, VMPL and host multi-key
encryption are unsupported enabled modes. Unknown SYS_CFG bits and nonzero
SEV_STATUS also refuse admission. This does not disable any protection. A
disabled target keeps the original CPUID physical width: the advertised address
reduction applies when encryption is enabled. The C-bit is nevertheless retained
in the existing unencrypted AddressPolicy, rather than claiming the CPU has no
encryption address bit. PPR allows reduction values 0 through 6 and fixes C-bit
to 51; malformed evidence refuses before any control read.

The native MSRPM now intercepts SYS_CFG writes. The existing stopped-MSR
dispatcher refuses the entire write transaction, including writes which would
leave encryption disabled: its other cache/routing controls do not have an
emulation owner. Refusal performs no hardware WRMSR, no RIP advance, no register
mutation and no fabricated guest #GP. SYS_CFG reads remain native. Native CPUID
already hides encryption/host-multi-key capability leaves from the guest. These
facts preserve the disabled-mode admission invariant during guest execution;
they do not establish protection against firmware/SMM changing controls.

The new pure tests cover advertised-but-disabled SME, every unsupported enable
bit, missing and unexpected MSR evidence, SEV gating, reserved control bits,
legacy absent leaves, malformed/unreviewed profiles and unreduced disabled
width. The native policy test checks the exact SYS_CFG write-intercept bit and
unchanged stopped state for harmless and enabling writes. These tests were
authored here; the coordinating implementation run records their execution.
No build, emulator, physical execution or timing measurement was performed by
this admission subtask. Existing physical results belong to their tested image.
Windows, Hyper-V/nested SVM, VBS/HVCI and other protected configurations retain
their existing unsupported/untested status. This is trusted first-boot work,
not a malware containment or analysis-coverage claim.

## Independent xAPIC startup obstacle

Encryption correction does not close the existing physical xAPIC routing gate.
PPR APIC410 ExtApicIdEn resets on INIT, so following SIPIs compare only the low
four ID bits and destination Fh broadcasts. Setting that control before INIT
or in the trampoline cannot ensure unique SIPI routing to IDs 15 or above.
A source-supported alternative is to establish x2APIC on every physical target
before INIT and use x2APIC ICR writes: APM2 16.10 states that INIT preserves
APIC_BASE AE/EXTD. Such a change needs explicit per-CPU promotion, verification,
firmware ownership and failure handling. This subtask does not implement that
promotion or weaken the current refusal.

## References and applicability

- AMD PPR 57896 rev. 3.00, 28 August 2024, Family 1Ah Model 44h B0:
  CPUID 8000001F pp.111–112, SYS_CFG p.202, APIC400/410 p.64, and APIC
  enumeration §2.1.11.2.1.3. Local library file
  `C:/Users/mato/Documents/svmvisor/docs/57896-3.00_PPR.pdf`, SHA256
  `643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5`.
  Read via the supplied page-marked `work/native-cache-ppr.txt` extract.
- AMD APM volume 2, publication 24593 rev. 3.44, March 2026: §7.10.1–2
  capability, address reduction and enablement; §7.10.9 SME-MK; §15.34.10
  SEV_STATUS availability; §16.9.1/.10 APIC mode transitions and INIT retention.
  Local library `24593_3.44_APM_Vol2.pdf`, SHA256
  `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
  Read via `work/qemu-corrections/amd-apm-vol2.txt`.

The official AMD document URLs could not be fetched during this subtask; the
local pinned documents above provide the reviewed normative contents.
