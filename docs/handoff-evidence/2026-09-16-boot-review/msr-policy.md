# Native MSR/cache policy review - 2026-09-16

## Result

No production behavior was changed. The claims identify genuine unsupported execution, but they do not establish that the current Windows boot hits those paths. Removing the controls or replacing every refusal with a guessed #GP would weaken correctness. The existing CPU-local HWCR30 fix was preserved. No image was flashed or activated.

| Claim | Verified implementation | Review finding |
| --- | --- | --- |
| 254 protected MSRs have no completion handler | `Msrpm::native_boot` intercepts both directions of C0010100..C00101FF except OSVW C0010140/141. The native MSR fallback stops F104 with RCX. | True as a compatibility limitation. This is a range of 254 indices, not 254 demonstrated implemented Windows-required registers. Actual native use can reach the stop. Some indices contain real monitor/SMM/secure controls; the map also covers reserved indices. |
| SYS_CFG20/21/22 writes are refused | The cache owner permits19 always and18 only in replay phases2/3; changed20/21/22 are Unsupported. | These are real RW routing/type controls. Unsupported is an explicit monitor stop, not a claim that valid input architecturally faults. Passing them through can change physical typing/routing of retained monitor memory. |
| MTRRdefType11:10 changes are refused | E0 permits clearing only11 from admitted default with guest CD1/NW0 and paired entry. E1 restores admitted default. Idle identical writes complete. Other valid default/FE changes stop. | Not blanket refusal: the inspected E0/E1 replay is implemented. Arbitrary FE/default changes remain unsupported and cannot be declared safely supported by silently changing only software state. |
| Variable/fixed MTRR writes are refused | `CacheCoreState::write` accepts valid changes in phases2/3. Outside replay, a changed value is unsupported. Final E1 requires original effective bank; both-invalid variable pairs may differ. | Blanket claim false. New final cache maps are unsupported, while coordinated restore is supported. Fixed visibility and invalid type/reserved inputs have explicit #GP handling. |
| CR0 writes in the cache window lack a handler | Cache E0 selects denied-low-memory NPT and enables CR0 guard; E1 restores root and clears guard. Such exits reach terminal native fallback. | Deliberate documented restriction of the bounded replay. Removing it permits leaving CD1/NW0 while logical and physical banks differ. Arbitrary cache-window CR0 writes require a new reviewed owner; no boot evidence establishes this need. |

## Reachability and stopped-state semantics

`runtime.rs` installs the native map and both-direction cache inventory before guest entry. At MSR exit it routes owned cache registers to `cache_runtime::handle` before generic handling. Therefore these are live native policies, not emulator-only paths.

The cache handler validates instruction boundary, CPL, TF, and continuation before guest completion. Invalid CPL and recognized malformed owned-register inputs queue checked #GP without advancing RIP. Supported reads return zero-extended EDX:EAX; supported writes complete only at a validated continuation. Ordinary unsupported writes retain instruction completion state and stop. E0 can already have installed the protective NPT/CR0 guard before a peer/barrier failure. E1 departure failures can occur after local commit, as the existing contract explicitly documents; neither is a blanket rollback promise.

The generic protected-range fallback does not inject #GP. APM MSR interception occurs after exceptions common to all MSRs and before MSR-specific checks, including unimplemented/reserved/password conditions. Consequently an MSR exit alone proves neither that access would succeed nor that #GP is correct. Unknown protected accesses must remain explicitly unsupported until a register-specific policy is justified.

The protected range includes VM_CR at C0010114 (host INIT interception/SVM disable/lock), VM_HSAVE_PA at C0010117, SVM lock key at C0010118, AVIC doorbell at C001011B and secure-guest controls. This inventory is sufficient to reject blanket passthrough; it is not an exhaustive support classification of all 254 indices. OSVW already has native access.

## Primary manual image review

Read requirements: AGENTS.md, handoff 2026-09-16, README, CONTRIBUTING, hypervisor and DXE guides, malware-analysis direction, and PDF skill. Text extraction located pages only. The following complete page images were read using `view_image`, rendered at1.4x with PyMuPDF in `msr-images/`. PDF page numbers below are one-based; indices zero-based.

- AMD PPR57896 rev3.00, August28 2024, Family1Ah Model44h B0. Local `docs/57896-3.00_PPR.pdf`, SHA256 `643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5`.
- AMD APM Volume2 publication24593 rev3.44, March2026. Local `docs/24593_3.44_APM_Vol2.pdf`, SHA256 `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.

| Reference chain | Printed page | PDF page / index | Bounded conclusion |
| --- | --- | --- | --- |
| APM15.11 complete table15-8 and exception ordering, continued exit-info paragraph | 518-519 | 580-581 /579-580 | Permission-map ranges/access order and direction encoding. Reserved/out-of-map interception does not supply exact guest-fault semantics. |
| PPR SYS_CFG table -> variable/fixed controls and default register | 202 ->126-130,173 | Same pages /201 ->125-129,172 | Bits20/21/22 affect routing/default type;19 affects fixed attribute access and is per-thread;18 controls extended attributes. Default11/10 and variable/fixed fields are RW. |
| PPR MTRR default -> variable base general MTRR information and full fixed64K table | 173 ->126-130 | Same pages /172 ->125-129 | Default E0 yields UC memory behavior; FE ignored while E0. Variable masks include valid bit. Fixed type inputs have explicit reserved-value errors. Complete fixed64K table read across four pages, including continued cells. |
| APM7.8.6/.7 -> complete Table7-12 and both footnotes | 231-232 |293-294 /292-293 | Cross-processor memory-type consistency and alias/cache/TLB transition constraints make arbitrary physical updates unsafe without an owner. |
| PPR VM_CR, VM_HSAVE_PA, lock key, AVIC, virtual TOM, Secure AVIC, OSVW tables |215-217 |Same pages /214-216 | Concrete protected-control counterexamples refute blanket pass-through; OSVW is RW hardware state. |

Scope qualification: the source profile gates physical signature0x00b40f40 and48-bit physical width; the PPR's product header agrees. This review does not newly measure physical CPUID or verify all other model applicability. No control from the protected region was newly enabled. VM_CR's delegated APM enabling-SVM/lock semantics, secure-guest branches, and OSVW revision-guide interpretation are not needed to establish that blanket access is unsafe and were not approved for new emulation. The full fixed4K/16K register table families and arbitrary new routing configurations are not independently reverified here; no new support relies on them. Prior replay proof remains a separate historical evidence set.

## Actual validation and evidence limits

- `cargo test --locked -p svmvisor-hypervisor --lib native_cache`:15 passed, including paired generation reuse, fixed visibility, final-bank refusal, disabled-slot equivalence, fresh capture and current HWCR owner tests.
- `cargo test --locked -p svmvisor-hypervisor --test native_boot_policy msrpm_passes_apic_and_native_msrs_but_protects_monitor_controls`:1 passed. This test exhaustively checks all256 protected-region indices and the2 OSVW exemptions.
- Read-only inspection of actual native dispatch, permission-map installation, cache handling, and existing cache-owner contract. No new test was added merely to duplicate the existing exhaustive map test.

No emulator run, physical cache/MSR write, exit latency measurement, counter/timing baseline, flash or reboot occurred in this review. The latest physical snapshot remains the predecessor image's stopped cache write; it lacks its MSR operands. The retained-kernel HWCR30 attribution remains strong static evidence, not recovered runtime proof. The new HWCR image was flashed previously but has not been activated. Nothing in this review proves Windows boot, protected Windows/Hyper-V/VBS coexistence, arbitrary cache-map support, malware containment or timing transparency.

## Recommendation

Keep stable physical cache ownership and exact unsupported outcomes. Do not add a generic physical MSR read/write fallback or blanket #GP conversion. Use the already improved operand diagnostics on an explicitly authorized future exact-image physical boot to identify any next compatibility gap. Then review that concrete MSR/CR0 operation and implement the smallest faithful owner without weakening resident memory or stopped-state invariants. SVM instruction and SHUTDOWN policy are reviewed by the VMCB/dispatch owner in the same batch, not approved by this MSR report.
