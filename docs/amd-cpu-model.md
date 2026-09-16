# AMD guest CPU model

> Historical (2026-09-16): the synthetic QEMU harness and emulator-only APIC models this report relies on were retired; its run commands no longer exist.

Status: implemented bounded CPU-enumeration and extended-state fixture,
2026-09-13. This is a complete CPUID response policy for the selected model,
not implementation of every AMD processor facility or an OS-ready machine.
The guest-model caller is the two-CPU software-emulator fixture. Native DXE
also collects the BSP's identity at firmware image entry for diagnostics.
Windows and the native physical machine have not executed this new guest model.

The user's native-facing CPUID requirement is applied to the admitted model:
the executing AMD processor's actual vendor, signatures and complete brand,
no hypervisor-present feature bit, and no private `40000000h` discovery leaves.
There is no fixed `AMD Processor` replacement brand or synthetic signature.
The historical `SvmVisorTest`
diagnostic policy remains a separate fixture. The new conformance payload uses
VMMCALL as an internal checkpoint; it does not advertise that checkpoint ABI
through native-facing CPUID. Presentation does not establish bare-metal
execution, timing equivalence or invisibility.

## Identity and admission

[cpu_model.rs](../crates/hypervisor/src/svm/cpu_model.rs) owns the pure response
policy. `CpuIdentity` captures vendor, both signatures and all 48 brand bytes
from supplied raw CPUID observations. The actual fixture caller executes the
CPUID instruction directly through Rust intrinsics; no Windows marketing-name
API, WMI lookup or invented processor label participates in this capture.
It checks maximum function numbers before collecting optional identity
leaves. Missing observations remain missing, not fabricated zero leaves.
Validation checks AMD vendor consistency, signature consistency and reserved
signature fields. Family/model/stepping decoding follows the base-family
rules. Brand bytes, including firmware-selected padding or full-width names,
are preserved verbatim.

Native DXE preflight also captures this identity directly during UEFI entry
on the bootstrap processor. It reuses six existing CPUID observations and
adds at most three maximum-leaf-gated brand reads. An out-of-line helper
prints bounded identity diagnostics and returns the original admission
evidence and outcome; identity availability does not change SVM admission.
This is boot-time BSP capture, not native AP enumeration or installation of
the guest model into a resident Windows execution path.

Identity is distinct from the host instruction evidence and runtime execution
contract. Reading a physical Ryzen identity would not authorize exposing
unowned ISA features, 24 logical processors, or unsupported state. The current
caller captures the identity of its actual execution processor: under QEMU,
that is QEMU's CPU identity. It does not inject the development PC's Ryzen
brand/signature into TCG or call that physical execution. A native guest-model
adapter running this same direct CPUID capture on the physical AMD processor
will obtain that processor's native identity; that resident guest-model entry has
not been demonstrated by this fixture.

There is one package, one node, two cores and one thread per core. Guest IDs
are exactly zero and one. The caller constructs the BSP model before guest
entry and requires the AP's independently admitted model to match. Missing,
extra or incompatible execution CPUs are refused. The PC's physical CPU count
and host cache-sharing counts are not forwarded. Cache and TLB geometry are
captured from the execution CPU, as described below.

Admission requires supplied AMD host evidence, valid basic/extended leaf
maxima, the baseline features listed below, and the actual installed
`XstateLayout`. Optional instruction bits are selected from an explicit
allowlist, never copied wholesale. XSAVE/AVX additionally require their host
bits and admitted state layout. The model performs no live host CPUID during
a guest exit.

Guest physical width must equal the observed native MAXPHYADDR, between 32
and 52 bits. Backed RAM can be smaller. Reporting a smaller address width
would require different guest page-walk reserved-bit faults, which NPT range
denial does not implement. Linear width is 48 bits; LA57 is not advertised.
Host-supplied linear width must be at least 48 bits. Unsupported control
settings still require an execution owner: CPUID filtering alone does not
disable a host instruction or control-register capability.

The runtime contract separately selects TSC, RDTSCP and NX. TSC requires host
TSC and the clock owner. RDTSCP requires TSC, host RDTSCP and TSC_AUX ownership.
NX requires host NX and an actual guest EFER.NXE/page-walk owner. The current
fixture selects TSC and capability-gated RDTSCP, and deliberately selects
**NX off** because its guest EFER access contract is incomplete. Core tests
of an admitted NX contract are not execution proof of that missing owner.

## Function and subfunction inventory

Numbers below are hexadecimal. Basic maximum is `0000000D`. Extended maximum
is `80000021` when the source enumerates that metadata function, otherwise
`8000001E`. Scalar functions ignore stale/nonzero ECX. Indexed functions
interpret the full subfunction input without wrapping to a supported entry.
All unlisted, reserved, future and out-of-range function numbers return four
zeros. There is no Intel-style fallback to the highest basic function.

| Function | Implemented result or explicit absence |
| --- | --- |
| `00000000` | Maximum basic function and AMD vendor in EBX/EDX/ECX order |
| `00000001` | Captured native signature, per-CPU guest initial ID, two logical processors, selected instruction features; dynamic OSXSAVE |
| `00000002`–`00000004` | Zero; Intel descriptor/serial/deterministic-cache interfaces are absent; AMD cache enumeration uses extended functions |
| `00000005` | Zero; MONITOR/MWAIT and their scheduling/intercept contract are absent |
| `00000006` | Zero; thermal, power and always-running APIC-timer claims have no admitted owner |
| `00000007:0` | Maximum subfunction zero; explicit EBX/ECX/EDX instruction allowlists |
| `00000007:n`, `n > 0` | Four zeros |
| `00000008`–`0000000A`, `0000000C` | Zero; reserved/basic performance-monitoring interfaces are not exposed |
| `0000000B:0` | Thread level: shift zero, one thread, type 1, current virtual ID |
| `0000000B:1` | Package level: shift one, two processors, type 2, current virtual ID |
| `0000000B:n`, `n >= 2` | Terminal type/count/shift zero; ECX retains `n & FFh`, EDX retains current virtual ID |
| `0000000D` | XSAVE state inventory described below, or all zero in the FX profile |
| `0000000E` and higher basic functions | Zero and outside maximum; PQoS monitoring/allocation, trace, frequency, newer topology and other interfaces are not part of this model |
| `40000000`–`4FFFFFFF` | Four zeros, including former diagnostic discovery function numbers |
| `80000000` | Maximum extended function and repeated AMD vendor |
| `80000001` | Captured extended signature, selected AMD extended features, source-gated topology extensions and CmpLegacy |
| `80000002`–`80000004` | Complete 48-byte processor-name area |
| `80000005` | Actual native L1 cache/TLB observation, preserved verbatim |
| `80000006` | Actual native L2/L3 cache and L2 TLB observation, preserved verbatim |
| `80000007` | Zero; no invariant-TSC, RAS, thermal, P-state, boost, energy or frequency-feedback interfaces |
| `80000008` | Native physical/48-bit linear widths; guest-physical-width field zero aliases physical width; two threads/package and one APIC-ID bit; host-gated CLZERO in EBX, other feature fields zero |
| `80000009` | Reserved, zero |
| `8000000A` | Zero; no nested SVM revision, ASIDs or virtualization features |
| `8000000B`–`80000018` | Reserved/unsupported, zero |
| `80000019` | Zero; 1 GiB-page/TLB support is not advertised |
| `8000001A` | Zero; no physical execution-datapath/performance hints |
| `8000001B` | Zero; no instruction-based sampling or IBS MSRs |
| `8000001C` | Zero; no lightweight profiling/context owner |
| `8000001D:n` | Native deterministic cache descriptors when host maximum and TopologyExtensions enumerate them; only sharing counts are projected onto the two-core guest topology |
| `8000001D:n`, after source terminator | Four zeros; capture requires a null entry within an eight-subfunction budget, with no silent truncation |
| `8000001E` | When source topology extensions are admitted: core ID zero/one, one thread/core, one node; extended APIC ID zero under the APIC-absent policy; otherwise all zero |
| `8000001F` | Zero; SME/SEV/SEV-ES/SNP and encrypted-memory ownership absent |
| `80000020` | Zero; platform QoS ownership absent |
| `80000021` | If source-enumerated: EAX bit 14 preserves the native L2-TLB size multiplier needed to interpret `80000006`; EBX preserves native MicrocodePatchSize/RapSize metadata; all other fields clear. Otherwise outside maximum and zero |
| `80000022` | Outside maximum, zero; architectural performance-monitoring/LBR owners absent |
| `80000023` | Outside maximum, zero; multi-key encryption absent |
| `80000024`–`80000025` | Outside maximum, zero |
| `80000026` | Outside maximum, zero; newer heterogeneous/complex topology is unnecessary for the selected two-core hierarchy |
| Every other namespace/input | Four zeros, including `C0000000` and `FFFFFFFF` |

## Feature accounting

Every feature bit not included below is clear. A clear bit represents an
explicit exclusion from this selected model; it does not establish that the
underlying hardware traps or rejects the corresponding instruction.

| Feature group | Exact exposure and reason |
| --- | --- |
| Baseline `1.EDX` | FPU, MSR, PAE, CMPXCHG8B, CMOV/FCOMI/FCMOV, MMX, FXSR, SSE and SSE2 are required host features; guest integer, VMCB and eager legacy state are owned |
| Clock `1.EDX`, `80000001.EDX` | TSC and RDTSCP follow separate host and runtime admission; no invariant-rate or frequency claim |
| Cache operation `1.EDX` | CLFLUSH only when the host exposes it with 64-byte lines; EBX then reports eight quadwords |
| Topology `1.EDX`, `80000001.ECX` | HTT means more than one logical processor in the package; CmpLegacy specifies separate cores; TopologyExtensions is enabled only with source enumeration of that interface and a valid bounded native cache snapshot |
| Optional `1.ECX` | Host-gated SSE3, PCLMULQDQ, SSSE3, CMPXCHG16B, SSE4.1, SSE4.2, MOVBE, POPCNT, AES and RDRAND use admitted integer/legacy SIMD state |
| XSAVE `1.ECX` | XSAVE follows the installed XSAVE layout; OSXSAVE follows stopped guest CR4.OSXSAVE; AVX follows admitted component 2 |
| AVX-dependent instructions | FMA, F16C, XOP and FMA4 require their host bits and admitted AVX state; AVX2 additionally requires `7:0.EBX[5]`; GFNI, VAES and VPCLMULQDQ require `7:0.ECX[8:10]` and AVX state; no AVX-512 prerequisites are advertised |
| Optional `7:0.EBX` | Host-gated BMI1, BMI2, ERMS, RDSEED, ADX and SHA use admitted integer/SIMD state |
| Optional `7:0.ECX/EDX` | RDPID requires host ECX bit 22 and the admitted RDTSCP/TSC_AUX owner; FSRM requires host EDX bit 4 |
| Cache operations | CLFLUSHOPT and CLWB require their host `7:0.EBX[23:24]` bits and admitted 64-byte CLFLUSH; CLZERO requires `80000008.EBX[0]` and the same line contract |
| Optional `80000001.ECX` | Host-gated LAHF/SAHF in long mode, ABM, SSE4A, PREFETCH/PREFETCHW and TBM use existing state |
| Extended EDX | AMD duplicate FPU/TSC/MSR/PAE/CX8/CMOV/MMX/FXSR bits agree with basic EDX; long mode is required; NX follows separate admission; MMX extensions and 3DNow use host bits and owned legacy state; 3DNow extensions additionally require base 3DNow |
| APIC/x2APIC and timer features | Clear. Existing timer/ICR/IRQ/startup fixtures provide only a partial controller; they do not certify every APIC register, LVT, route or transition |
| VME, debugging extensions, MCE/MCA, PSE/PSE36, PGE | Clear. General legacy-mode, debug, machine-check and optional paging-control contracts are not admitted by this fixture |
| SYSENTER/SYSEXIT, SYSCALL/SYSRET, PAT, MTRR, FFXSR | Clear. Relevant MSRs, modes and restoration/fault behavior need their own general guest access policy; switching VMCB auxiliary fields alone is insufficient |
| PCID/INVPCID, SMEP/SMAP, PKU/OSPKE, LA57, FSGSBASE, UMIP | Clear. These add paging/control/MSR or mode dependencies outside this admitted execution profile |
| MONITOR/MWAIT, MONITORX/MWAITX and scheduling facilities | Clear. No general wakeup, power-state or instruction-completion owner |
| SVM, SEV/SME and nested Hyper-V | Clear. No nested virtualization, VTL or confidential-memory architecture is implemented |
| XSAVEOPT/XSAVEC/XSAVES, XGETBV with ECX=1, XSS | Clear. Only standard-format XSAVE/XRSTOR and XCR0 are admitted; optimized/compacted/supervisor-state contracts are absent |
| AVX-512, CET/shadow stacks and other additional xstate | Clear. No component storage, switching or dependent control/MSR ownership |
| PMU, IBS, LBR, performance/TSC-size extensions, power/thermal facilities | Clear. No counter ownership, interrupt delivery, calibration or corresponding MSR model |
| IBRS/IBPB/STIBP/SSBD and related mitigation guarantees | Clear. Host claims cannot replace guest mitigation MSR semantics or execution guarantees |
| MOVDIRI/MOVDIR64B and newer data movement | Clear pending their specific guest-memory/fault conformance; ordinary integer ownership alone is not treated as sufficient proof for these operations |
| Other vendor extensions and all remaining defined/reserved bits | Clear under the selected allowlist. RDPRU, WBNOINVD, extended speculation/prefetch hints, newer invalidation, additional cache-management and other unlisted facilities are not part of the admitted model; no generic host feature register is forwarded. `80000021` keeps the cache-format and software-size metadata described above, without enabling microcode writes or mitigation MSRs |

RDRAND and RDSEED keep their native carry-flag success/failure behavior;
enumeration does not guarantee success on each invocation or deterministic
random replay. ERMS/FSRM describe the admitted native string engine and do
not promise end-to-end timing equivalence through guest exits. CLWB and other
cache operations do not by themselves guarantee persistence across power
loss. These instruction flags rely on supplied native execution capability
and state ownership. The CPUID fixture does not separately execute every
advertised optional opcode, measure its performance, or prove all of its
memory-fault combinations.

## Cache and TLB interpretation

There is no fixed cache/TLB geometry table in the implementation. The caller
captures native functions `80000005` and `80000006`, then captures indexed
`8000001D` only when the source maximum and TopologyExtensions permit it.
The eight-entry owned array includes its null terminator; unterminated input,
missing required descriptors and AMD-reserved deterministic-cache bits are
refused. An associativity value of 9, which delegates interpretation to the
deterministic cache function, cannot be admitted without its referenced cache.

Scalar cache/TLB leaves retain their actual source values. Deterministic
descriptor types, levels, ways, partitions, line sizes, set counts and cache
policy bits are retained. Only the number of processors sharing a cache is
projected: one guest CPU for L1/L2 under this two-separate-core contract, and
the smaller of the native sharing count and two for L3. This is a guest
topology projection, not physical cache partitioning or isolation. Unsupported
cache levels/descriptor formats are refused rather than guessed.

For processors with the newer `L2TlbSizeX32` format, the native bit in
`80000021.EAX[14]` is preserved so the OS correctly multiplies the raw TLB-size
fields by 32. The function's native microcode-patch-size and return-address-
predictor-size metadata is retained in EBX; replacing those fields with zero
could change software's interpretation. Other function-21 behavior and
mitigation flags remain filtered. If the source does not enumerate function
21, the guest extended maximum remains 1E.

Native CPUID geometry is still enumeration evidence, not a measurement of
cache-miss timing or execution equivalence. An emulator's native CPUID
describes its emulated CPU model. The generic archived AMD associativity
table and modern source-format discriminators explain the captured fields;
the current product-specific PPR's reset values are not substituted for them.

## Extended-state and exit contract

The three actual fixture profiles are FX (FXSAVE/FXRSTOR, no XSAVE exposure),
SSE (XSAVE-supported XCR0 mask `3`) and AVX (mask `7`). With XSAVE, guest masks
`1` and `3` are valid in both profiles; `7` is additionally valid with AVX.
The x87 bit is mandatory, AVX requires SSE, and unowned high bits are refused.
The original fixed-host-layout validator remains separate from the dynamic
guest-XCR0 validator.

Leaf `D:0` reports the admitted mask in EAX, current required standard save
area size in EBX, and maximum admitted standard size in ECX. Current size is
576 bytes without enabled AVX and the admitted AVX end offset with AVX.
Leaf `D:2` reports 256 bytes, the admitted standard offset, and retained
component alignment flags. Other components are zero. None of those layout
answers is derived from the host's current OS-enabled XCR0 after admission.
FX mode returns zero for every leaf-D subfunction.

The bridge restores the complete owned guest xstate before installing its
current guest XCR0. On exit, it restores the full admitted host XCR0 before
saving every guest component, then restores host and caller state. Disabled
components survive guest-mask changes. This matters because legacy SSE
instructions can still modify XMM state when XCR0.SSE is clear. The fixture
executes XMM15 changes in that state and checks both guest and host canaries.

CPUID intercept `72h` uses stopped VMCB RAX and the saved GPR frame's RCX low
32 bits, the owned model, virtual ID, stopped CR4 and owned XCR0. The dispatcher
checks state and instruction continuation before changing four output
registers; outputs zero-extend to 64 bits. Successful completion advances RIP
exactly past CPUID and preserves RFLAGS and pending-event state. Rejected
bytes/state/continuation leave stopped state unchanged. Shutdown is handled
before reading undefined saved guest fields. No guest pointers, allocation,
firmware calls, logging or unbounded work are introduced in CPUID service.

XSETBV intercept `8Dh` has a checked three-byte continuation. The pure
validator distinguishes #UD eligibility (XSAVE/CR4.OSXSAVE unavailable) from
#GP eligibility (privilege, selector or XCR0 mask invalid). The caller commits
only a validated owned-XCR0 update and continuation; native guest XGETBV then
checks the installed value. **Invalid XSETBV currently stops unchanged in
this fixture. It does not yet inject and return through a guest #GP/#UD
handler.** Pure fault classification and unchanged refusal are not complete
architectural fault delivery or OS compatibility.

## Validation and remaining scope

The model has host tests for captured vendor/signature/raw-brand consistency,
family/model decoding, missing identity evidence, topology, native cache
geometry and format-metadata preservation, bounded cache refusal, cache
arithmetic, full reserved/unsupported ranges, feature dependencies,
native-width admission, scalar/indexed behavior, dynamic OSXSAVE/XCR0 and
retained xstate component metadata. Dispatcher tests cover completion and
unchanged failures. The real two-CPU fixture executes intercepted CPUID,
checks resulting guest GPRs at a subsequent checkpoint, exercises supported
XSETBV/XGETBV transitions and unchanged invalid-mask refusals, and verifies
private guest/host xstate on both CPUs. Its exact build, image/backend hashes,
run counts and outcomes belong to
[final-validation/summary.json](../work/amd-cpu-model/final-validation/summary.json).
Do not substitute planned counts or prior checkpoints for that evidence.
The runner is [run-amd-cpu.ps1](../tools/synthetic-harness/run-amd-cpu.ps1),
with Avx/Sse/Fx, RDTSCP absence and single-CPU refusal configurations.

No native timing distribution, observation overhead, cache fidelity, Windows
boot, Hyper-V/VBS/HVCI/PatchGuard/Secure Boot compatibility, device/DMA
containment or malware-analysis completeness is established by this batch.
No binary inherits the old physical returning-probe evidence. General MSRs,
APIC, fault reflection, control registers, memory/platform admission and the
other gates in [OS boot readiness](os-boot-readiness.md) remain work. A complete
selected CPUID table does not mean the user's broader request for every CPU
facility has been achieved.

## References and review

- AMD APM volume 2, publication 24593, revision 3.44, March 2026; sections
  11.4/11.5, chapter 15 and Appendix B. Local SHA256:
  `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
- AMD PPR 57896 revision 3.00, 2024-08-28, Family 1Ah Model 44h Revision B0;
  section 2.1.12, especially vendor/features, xstate, cache and topology
  definitions. Local SHA256:
  `643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5`.
  This product applicability does not identify the current execution host.
- [AMD CPUID specification 25481 revision 2.34](https://www.amd.com/content/dam/amd/en/documents/archived-tech-docs/design-guides/25481.pdf),
  September 2010, pp. 22-25 and Table 4 for classic cache/TLB encodings.
  Modern facilities use the current PPR/APM rather than the archived list.
- [Independent review](../work/amd-cpu-model/independent-review.md) records
  implementation findings and their disposition separately from execution.
