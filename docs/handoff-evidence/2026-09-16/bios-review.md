# F7 fixed-MTRR lifecycle: native Binary Ninja review

2026-09-15. This follow-up changes the preferred correction from investigating a new physical writer to capturing the bank at the correct ownership boundary. No original BIOS or existing database was modified; no firmware service, BIOS callback, flash helper or hardware instruction was executed by this analysis.

## Actionable conclusion

**The F7 BIOS contains late BSP-to-AP cache-bank synchronization at ReadyToBoot and ExitBootServices.** The hypervisor's pre-EBS all-core fixed-bank equality requirement can reject a firmware-phase difference before the firmware performs its own scheduled synchronization. The new native evidence also shows a BSP-only routing lock for exactly E0000-E7FFF and adjacent regions, and an earlier AMD AP bootstrap table deliberately uses different fixed values.

Recommended correction, coordinated with the source owner: preserve preparation/topology/high-RAM admission, but acquire a fresh authoritative physical cache bank after the original ExitBootServices returns success and after owned AP bootstrap. Every CPU must publish its fresh observation and park before arm/VMRUN. BSP compares all fresh banks and sibling domains, validates supported active tuples, initializes the existing cache owner, then releases participants. Preserve per-CPU fresh-sample versus actual arm revalidation. A failed final sample/comparison keeps every guest stopped. This changes when evidence becomes authoritative; it does not assume firmware synchronization succeeded or mask a differing bit.

The supplied F7 image's actual execution on the physical board, callback outcomes and exact current banks are not newly measured. The proposed barrier tests those post-firmware conditions at the point where they matter, without introducing a speculative physical routing writer.

## Provenance and tools

Input C:/Users/mato/Documents/svmvisor/docs/vendor/bios-f7/B850AELITEWF7.F7, 33554432 bytes, SHA25641a4a72c5166dc2c2f3ad8db9d8636f5e1a3b83b213375458cf9e9530e9a17b8 (fresh hash). Reused byte-pinned executable extraction indexed in work/native-after-cet-2026-09-15/bios-module-index.json. Local inventory.json records selected module paths, hashes and exploratory literal matches. Those matches alone are not instruction evidence.

Used installed native Binary Ninja6.0.10601 Python API from C:/Users/mato/AppData/Local/Programs/Vector35/BinaryNinja/python. No BN MCP was callable in this agent's inventory. Fresh native analysis completed for four modules; new task-local databases and complete selected-function assembly/HLIL exports are retained here. Addresses are static loaded module VAs, not runtime addresses.

- CpuDxe, FFS e03abadf-e536-4e88-b3a0-b77f78eb34fe, SHA25631749b82088fb940394d8eb3e68034659fd28b55715a8af9f2a621be2f141b16, 76 functions. This is distinct from MP-services CpuDxe596c7c2e.
- LegacyRegion SHA25644b19492 prefix (full hash in inventory.json), 57 functions.
- AmdCcxZen5Pei SHA256d24a691f prefix (full hash in inventory.json), 189 functions.
- AmdCcxZen5Dxe SHA2565701a5174ffd4cbab234e4c1bf4a395f4565d04796a37c85a89b0c9b19005964, 126 functions.

## Exact BIOS synchronization chain

CpuDxe31749b82, complete exports31749b82-hits.txt and31749b82-follow-4022e4.txt:

1. Initialization registers CreateEvent(type201h=EVT_SIGNAL_EXIT_BOOT_SERVICES, TPL8, callback4018FC) at402228-402241.
2. It registers CreateEventEx callbacks401788 for GUID7ce88fb3-4bd7-4679-87a8-a8d8dee50d2b (ReadyToBoot) and401920 for GUID2a571201-4966-47f6-8b86-f31e41f32f10 (LegacyBoot) at402259-4022B2. GUID bytes are exported in guid-region-table.txt.
3. All three callbacks call402ED4. ReadyToBoot401788 additionally saves the raw BSP bank into its CpuS3Resume record and closes the event. ExitBootServices4018FC clears variable pair20E/20F, calls402ED4, then executes CLI. Thus high-bank dormant fields can also legitimately change after pre-EBS capture.
4. Function402ED4 constructs a table of indices200-20F, all eleven fixed indices250/258/259/268-26F, TOP_MEM, TOP_MEM2, SYS_CFG, IORR baseC0010016, MMCONFIG and HWCR (33 entries). It locates EFI_MP_SERVICES_PROTOCOL GUID3fdda605-a76e-4f46-ad29-12f4531b3d08.
5. It calls402E08 locally. APIC_BASE.BSP selects the BSP save branch: expose SYS_CFG18/19, read each table entry into a buffer, clear19, mark buffer ready.
6. It invokes the located protocol's offset10h, StartupAllAPs, with SingleThread=false, WaitEvent=NULL, Timeout=0 and that buffer. On APs402E08 requires ready and calls402D1C. Do not mistake this for StartupThisAP or a BSP-only copy.
7. AP replay402D1C saves IF; CLI helper4010D0; physical CR0.CD=1/NW=0 plus WBINVD helper4011D0; writes physical MTRRdefType=0; sets physical SYS_CFG18/19; replays the full table; clears19; writes MTRRdefType=C00; WBINVD then clears physical CD/NW via4011B0; restores IF. This is actual physical cache-mode maintenance, not virtualization of guestCD.

The BIOS API return is not independently checked here against physical execution; the code path and arguments are proven. Its Timeout0 infinite wait is BIOS behavior, not a recommendation to introduce unbounded waits in our runtime.

## Why the fixed banks can differ earlier

CpuDxe initialization finally calls4019A8 at4022E4. Under its Setup-variable flags, it calls402B3C on E0000/8000, E8000/8000 and F0000/10000 with route mode0, then requests CPU-architecture attribute1000h for those ranges. Function402B3C's complete implementation directly modifies the current CPU fixed bank: per-byte mask18h, replacement10h, preserving MemType bits. No MP dispatch occurs in that function or its4019A8 wrapper. The table maps E0000-E7FFF to26C exactly (guid-region-table.txt). With existing type5, this produces15 and clears the observed differing WrDram bit.

Separately, AMD Zen5 PEI function9B35351 saves BSP MSRs into an AP bootstrap table but explicitly replaces entries26C-26F with1818181818181818. The bootstrap target9B343AC sets physical19, writes each table index/value, clears19, sets18/20, then enables caching and continues. Complete controlling function and bounded exact-address stub assembly are retained in d24a691f-follow-9b35351.txt and zen5-ap-stub.txt. A later type-only fixed write with19hidden could transform18 to1D, while the BSP-only route lock yields15. That last runtime sequence is an inference; no exact physical origin of1D is asserted solely from this static trace.

Together, these actual writers and late synchronization establish that immutable pre-EBS fixed-bank equality is not an appropriate unqualified post-firmware baseline.

## Normative cross-check and alternative physical-writer disposition

Original AMD tables and hashes remain in ../manual-review/report.md. Their actual constraints remain: bit3 is routing,1D active extended WP is reserved, and WP does not suppress writes. The BIOS implementation does not override those rules.

PI1.10 original PDF SHA256ed35ab171e8aa66514e2f04013faf7912098b960bdf614f973a8b9d4a5ff09ea: visually read full StartupAllAPs description/table including continuations PDF366-369 (printedII-130-133), and LegacyBoot GUID page262/II-26. MP consumers own the correctness of BSP/AP concurrent work; the protocol does not create an arbitrary quiescent execution environment. Blocking dispatch includes all enabled APs and waits for return; nonblocking dispatch is unavailable after ReadyToBoot. A fresh callback alone does not exclude SMM or guarantee all sibling cache modes. Earlier original-page lifecycle review at work/native-percpu/pi-1.10-review.md records firmware ownership through EBS and nondeterministic notification order.

UEFI2.11 original PDF SHA256a64b8e442004b91becc3de9afaf8ca61b259a9a3b436accb6b3711ab5400cee9: rendered PDF229/printed145 confirms the ReadyToBoot GUID. Current post-successful-EBS ownership design does not invoke MP Services after firmware exit.

The F7 LegacyRegion/CpuDxe route-writing idiom uses physicalCD+WBINVD+MTRRdisable/re-enable, corroborating the Windows physical sequence found independently by the source peer. AmdCcxZen5Dxe has narrower physical route updates to259 in SMM aperture helpers405818/405894, but those have special SMM controls and surrounding MMIO handshakes. They do not establish a portable cache-on26C update protocol.

We therefore do not need to close a new physical normalizer for this correction. The source's after-EBS fresh capture/barrier approach preserves physical ownership and enforces the already-required consistency before guest execution. It is subject to actual implementation review and executable validation. If firmware's final bank still fails, the barrier must report that actual bank and stop rather than silently normalize it.

## Required implementation review

- Every AP and BSP contributes one current observation after the successful originalEBS return; no guest is released before complete validation and owner initialization.
- Callback, code, stack, page tables and shared state remain retained and owned; no firmware services are used in the new AP phase.
- Release/acquire publication prevents stale or partial records; duplicate/missing/wrong-CPU publication fails boundedly.
- Domain identity and active-bank equality are retained, with separate disabled-variable dormant-field rule and full active tuple validation.
- Current versus fresh baseline drift check remains at arm, and no cache-owned write mutates physical routing.
- Collection/admission failure or timeout leaves all guests stopped; EBS failure/retry does not prematurely consume this one-shot transition. A later activation failure can follow an earlier AP's bootstrap guest completion, as in the existing serial activation flow.

No new timing, physical boot, protected-Windows compatibility or containment result is claimed. New local artifact analysis only; production implementation is owned by the source agent.

## Independent implementation review

Reviewed the source owner's current `physical_boot.rs` collection/admission functions and `start` dispatch, `activation.rs` callback placement, and `native_cache.rs` gate/capture/tuple validation. The physical INIT/SIPI broadcast occurs once, before the two serial release passes. First release samples and parks each AP after its CPU and high-memory closure checks. The BSP consumes the release/acquire sample publications, compares the complete banks and sibling domains, initializes the cache owner, and only then publishes admission and second releases. No physical fixed-route writer is introduced. The existing actual-arm comparison remains exact against the new fresh baseline.

The capture has one serial writer at a time, and each complete bank is ordered before its sample bit. The source's active tuple allow-list matches complete Table7-13. Its separate rejection of SYS_CFG18=0 while E/FE are active is an explicit restriction of the bounded native bootstrap profile; that configuration is not itself described as AMD-reserved. Dormant fixed type bytes are not misclassified as active solely because their raw contents are retained.

An intermediate loop placement would have repeated physical INIT/SIPI; the source owner corrected it before this review disposition. Executable survey validation is being completed by the source owner. This review establishes the source ordering and manual interpretation, not physical firmware callback execution or a measured boot result.

## Final bounded review, 2026-09-16

Reviewed the three final execution summaries and complete retained traces in `../execution/{pass,mismatch,drift}-final-run/summary.json`, the execution review and linked survey audit. No blocking finding remains. The positive run samples the actual late-EBS BSP disabled-variable sentinel `0x12345006`, then AP slots3,2,1, and publishes admission before any arm; four guest acknowledgements follow. The mismatch run has all four samples and activation refusal48 with no admission, arm, entry or acknowledgement. The drift run admits the full capture, then the first AP arm returns12 with no guest entry or acknowledgement. Negative-run timeouts are intentional stopped-state harness termination and are not alone treated as success.

Reviewed the final fixture compile guards: modeled AMD-only observations and stage1 drift injection are inside `native-cache-survey-fixture`; production uses the exact native capture path. Independently hashed the final production EFI: `87953f4584a32d67cf7fe54e8a2a9ded6ccf14fe0a2f71a672841a1d3167d04c`. The execution peer's byte-identical relink, closure call resolution, production fixture absence, and conservative 6176-byte new-path stack bound are recorded in its linked audit; those are linked-code evidence, not hardware execution.

The new `card_boot::activation_failure(48)` publication reuses the existing serialized BSP publisher after the survey resolves. It revalidates current UC config/BAR mappings, the target CPU and MMIO-config register, PCI identity/command/BAR, FPGA/ROM identifiers and boot ID before committing the full USER3 record, then emits the ordinary stage80 activation-failure header. It uses retained numeric state and no expired `Prepared` options pointer or firmware service. Validation failure permanently stops diagnostic accesses for that boot.

The decoder binds this record only to an encoding-valid phase19/stage80/failure48 header, same boot and FPGA/ROM identities, event12, and cache operation4/6/7/8. Conflicting matching full records remain an explicit unresolved result; absent full evidence is reported as unavailable. This does not promote a survey refusal into a runtime-activation claim.

The executed AMD hidden attributes, topology and negative injections remain explicit models. Actual F7 synchronization, physical AMD cache coherence and Windows boot remain unmeasured. The final source correction preserves that distinction and introduces no physical routing normalizer.
