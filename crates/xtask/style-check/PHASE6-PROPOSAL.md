# Phase 6b rename proposal

Generated at the end of phase 6a from the tree at that commit; every `file:line` is a definition site in
that tree. Nothing here has been applied. Approve or strike each line; a struck line needs no reason.

Conventions:

- *uses* is the number of identifier occurrences under `crates/` outside comments, not counting the
  definition. `≈n` means the name is shared by several definitions and the count is limited to the
  defining file plus files that also name the defining module.
- Paths are relative to `crates/`.
- A proposal in parentheses is not a rename (keep, merge, doc fix).
- Every rename below is a pure identifier rename (CODINGSTYLE §14). Type names reach `Debug` output;
  no `src/` code formats these types onto a wire, but tests that compare `{:?}` strings would need the
  new spelling.
- Phase 6a finding that bears on all of this: in the release payload a function rename is invisible in
  `payload.bin`/`driver.efi` (byte-identical), but `asmdiff.py` reports a renamed function that is a
  payload symbol with internal jumps (its own name is part of its jump targets).

## A. The `Native` qualifier (N2)

In this tree `native` means "the real-hardware boot profile", as opposed to the synthetic fixture
guest. N2 keeps a qualifier only when the unqualified counterpart lives in the same namespace. The
*counterpart* column names it when there is one.

### A1. `Native*` types, `native_*` functions, `NATIVE_*` constants

| current | proposed | counterpart | defined at | uses | why |
|---|---|---|---|---|---|
| `NativeResult` | `ChildResult` | none (bare `Result` is core's) | dxe/src/diagnostics/native_result.rs:13 | 121 | module doc: result ABI of the returning PE child |
| `NativeProbe` | `ProbeReader` | none | dxe/src/memory_attributes/probe.rs:533 | 7 | doc: "temporary reader"; bare `Probe` is a generic noun (N4) |
| `NativeBoundary` | `EntryBoundary` | none | dxe/src/native/admission/boundary.rs:19 | 69 | module doc: image-entry observation and return boundary; wire struct, layout untouched |
| `NativeSnapshot` | `ControlStateObservation` | none | dxe/src/native/admission/snapshot.rs:13 | 29 | module doc calls it "host observations" of selected registers, not a whole structure (§6.2); see D |
| `NativeTransition` | `TransitionStorage` | none | dxe/src/native/transition/state.rs:180 | 25 | doc: "Storage only" (§6.2 `…Storage`) |
| `native_paging_config` | `paging_config` | none | dxe/src/native/resident/launch.rs:25 | 9 | N2; module path already says resident launch |
| `NativeEncryptionPlan` | `EncryptionPlan` | none | hypervisor/src/arch/x86_64/encryption.rs:20 | 11 | N2 |
| `NATIVE_BOOTSTRAP_ACK` | `BOOTSTRAP_ACK` | none | hypervisor/src/guest/continuation.rs:23 | 4 | already imported as `BOOTSTRAP_ACK` in host/resident/mod.rs (I7 alias disappears) — `BOOTSTRAP_ACK` already appears in 1 user file(s), e.g. hypervisor/src/host/resident/mod.rs |
| `NativeBootstrapAck` | `BootstrapAck` | none | hypervisor/src/guest/continuation.rs:137 | 12 | its error is already `BootstrapAckError` |
| `NativeContinuationRequest` | (keep) | `IntegerContinuation` family, same file | hypervisor/src/guest/continuation.rs:90 | 7 | the synthetic continuation lives beside it |
| `NativeContinuationError` | (keep) | `ContinuationError`, same file | hypervisor/src/guest/continuation.rs:244 | 40 | counterpart exists (N2 satisfied) |
| `native_cr4_supported` | (keep) | none, but belongs to the `Native*` family above | hypervisor/src/guest/continuation.rs:415 | 4 | follows the family decision |
| `native_mtrrs` | `capture_mtrrs` | none | hypervisor/src/host/resident/runtime/guest_reader.rs:197 | 4 | reads the MTRR MSRs (§6.3 `capture_*`); bare `mtrrs` is the N13 local name and would shadow it |
| `native_fixed_page_is_wb` | `fixed_page_is_wb` | none | hypervisor/src/host/resident/runtime/guest_reader.rs:208 | 5 | N2 |
| `native_fixed_page_is_wb` | `fixed_page_is_wb` | none | hypervisor/src/memory/mtrrs.rs:73 | 5 | N2; method on `Mtrrs`, distinct from the free function above |
| `native_topology` | `capture_topology` | none | hypervisor/src/svm/cache/mod.rs:729 | 1 | reads CPUID (§6.3); bare `topology` is a local in arm.rs and cache/mod.rs |
| `native_topology_detailed` | `capture_topology_detailed` | none | hypervisor/src/svm/cache/mod.rs:732 | 3 | twin of the above |
| `native_boot_cpuid` | `boot_cpuid` | none | hypervisor/src/svm/cpu_model.rs:591 | 11 | N2 |
| `NATIVE_EFER_MASK` | `EFER_MASK` | none | hypervisor/src/svm/dispatch.rs:19 | 2 | N2 |
| `NATIVE_VM_CR_VALUE` | `VM_CR_VALUE` | none | hypervisor/src/svm/dispatch.rs:26 | 5 | N2 |
| `NativeEfer` | `Efer` | none | hypervisor/src/svm/dispatch.rs:49 | 47 | the guide's own N2 example |
| `NativeEferError` | `EferError` | none | hypervisor/src/svm/dispatch.rs:156 | 45 | N2 |
| `NativeVmCrError` | `VmCrError` | none | hypervisor/src/svm/dispatch.rs:175 | 5 | alias of the above |
| `NativeMsrOutcome` | `MsrOutcome` | none (`DispatchOutcome` is the synthetic dispatcher's) | hypervisor/src/svm/dispatch.rs:139 | 59 | N2 |
| `native_cpuid_mode` | `cpuid_mode` | none | hypervisor/src/svm/dispatch.rs:440 | 3 | N2 |
| `native_startup_instruction_mode` | `startup_instruction_mode` | none | hypervisor/src/svm/dispatch.rs:473 | 6 | N2 |
| `native_boot` | `boot_profile` | none | hypervisor/src/svm/permission_maps.rs:75 | 17 | constructor of the boot permission maps; bare `boot` is a module name at its import sites |
| `native_pause_retry_ready` | `pause_retry_ready` | none | hypervisor/src/svm/vmcb/control.rs:221 | 5 | the guide's own §0.2 example |
| `NATIVE_CONTROL` | `X2AVIC_CONTROL` | none | hypervisor/src/svm/x2avic/profile.rs:14 | 18 | bare `CONTROL` says nothing at an import site |
| `NativeX2AvicProfile` | `X2AvicProfile` | none | hypervisor/src/svm/x2avic/profile.rs:38 | 44 | N2 |
| `NativeIcr` | `StartupIcr` | none | hypervisor/src/svm/x2avic/startup.rs:30 | 36 | bare `Icr` is a helper type in tests/svm_x2avic_manual_conformance.rs that imports this one |
| `NativeIcrError` | `StartupIcrError` | none | hypervisor/src/svm/x2avic/startup.rs:698 | 50 | follows `StartupIcr` |
| `NativeStartupMailbox` | `StartupMailbox` | none | hypervisor/src/svm/x2avic/startup.rs:315 | 53 | N2 |
| `NativeStartupTarget` | `StartupTarget` | none | hypervisor/src/svm/x2avic/startup.rs:450 | 23 | N2 |
| `NativeStartupState` | `StartupState` | none | hypervisor/src/svm/x2avic/startup.rs:588 | 31 | N2 |
| `NativeStartupCommand` | `StartupCommand` | none | hypervisor/src/svm/x2avic/startup.rs:594 | 60 | N2 |
| `NativeStartupEffect` | `StartupEffect` | none | hypervisor/src/svm/x2avic/startup.rs:623 | 34 | N2 |
| `NativeRouteGuard` | `RouteGuard` | none | hypervisor/src/svm/x2avic/startup.rs:525 | 6 | N2 |
| `NativeRouteFailure` | `RouteFailure` | none | hypervisor/src/svm/x2avic/startup.rs:659 | 7 | N2 |
| `NativeRoutePredicate` | `RoutePredicate` | none | hypervisor/src/svm/x2avic/startup.rs:672 | 9 | N2; wire codes untouched |
| `NativeRouteRecipient` | `RouteRecipient` | none | hypervisor/src/svm/x2avic/startup.rs:690 | 6 | N2 |
| `NativeDestinationCommit` | `DestinationCommit` | none | hypervisor/src/svm/x2avic/startup.rs:560 | 3 | N2 |
| `NativeDestinationMode` | `DestinationMode` | none | hypervisor/src/svm/x2avic/startup.rs:634 | 13 | N2; wire encoding untouched |
| `NativeDestinationCause` | `DestinationCause` | none | hypervisor/src/svm/x2avic/startup.rs:650 | 18 | N2 |

The `native_*_inner` workers of dispatch.rs are in section E. Exported symbols `svmvisor_native_*` are ABI
(N12) and are not listed.

### A2. `native` in the middle of a name

Same rule, mechanical proposal: drop the word when the shorter name is free in the crate.

| current | proposed | defined at | uses | why |
|---|---|---|---|---|
| `PreparedNativeContinuation` | `PreparedContinuation` | hypervisor/src/guest/continuation.rs:110 | 5 | N2 |
| `prepare_native` | `prepare` | hypervisor/src/guest/continuation.rs:277 | 16 | N2 — shorter name already defined at hypervisor/src/boot/probe.rs:41, hypervisor/src/host/resident/runtime/arm.rs:58, hypervisor/src/host/resident/runtime/diagnostics.rs:93: keep or pick another |
| `prepare_native_with_efer` | `prepare_with_efer` | hypervisor/src/guest/continuation.rs:290 | 6 | N2 |
| `read_native_apic` | `read_apic` | hypervisor/src/host/resident/runtime/debug.rs:19 | 7 | N2 |
| `notify_native_startup` | `notify_startup` | hypervisor/src/host/resident/runtime/startup.rs:383 | 3 | N2 |
| `send_native_notification` | `send_notification` | hypervisor/src/host/resident/runtime/startup.rs:389 | 3 | N2 |
| `admit_native` | `admit` | hypervisor/src/svm/dispatch.rs:67 | 17 | N2 — shorter name already defined at hypervisor/src/arch/x86_64/clock.rs:24, hypervisor/src/svm/cache/survey.rs:43, hypervisor/src/svm/cpu_model.rs:78: keep or pick another |
| `handle_native_efer` | `handle_efer` | hypervisor/src/svm/dispatch.rs:182 | 37 | N2 |
| `handle_native_efer_with_nrip` | `handle_efer_with_nrip` | hypervisor/src/svm/dispatch.rs:196 | 6 | N2 |
| `handle_native_vmcr` | `handle_vmcr` | hypervisor/src/svm/dispatch.rs:206 | 10 | N2 |
| `handle_native_vmcr_with_nrip` | `handle_vmcr_with_nrip` | hypervisor/src/svm/dispatch.rs:218 | 6 | N2 |
| `handle_native_cpuid` | `handle_cpuid` | hypervisor/src/svm/dispatch.rs:232 | 5 | N2 |
| `handle_native_startup_cpuid` | `handle_startup_cpuid` | hypervisor/src/svm/dispatch.rs:246 | 4 | N2 |
| `handle_native_cpuid_with_nrip` | `handle_cpuid_with_nrip` | hypervisor/src/svm/dispatch.rs:279 | 11 | N2 |
| `validate_native_msr_boundary` | `validate_msr_boundary` | hypervisor/src/svm/dispatch.rs:424 | 4 | N2 |
| `validate_native_interrupt_profile` | `validate_interrupt_profile` | hypervisor/src/svm/dispatch.rs:651 | 2 | N2 |
| `configure_native_x2avic` | `configure_x2avic` | hypervisor/src/svm/permission_maps.rs:129 | 15 | N2 |
| `apply_native_continuation` | `apply_continuation` | hypervisor/src/svm/vmcb/continuation.rs:58 | 1 | N2 |
| `initialize_native_cet_msrs` | `initialize_cet_msrs` | hypervisor/src/svm/vmcb/continuation.rs:122 | 3 | N2 |
| `commit_native_efer` | `commit_efer` | hypervisor/src/svm/vmcb/continuation.rs:151 | 2 | N2 |
| `complete_native_instruction_state` | `complete_instruction_state` | hypervisor/src/svm/vmcb/continuation.rs:160 | 10 | N2 |
| `configure_native_boot_intercepts` | `configure_boot_intercepts` | hypervisor/src/svm/vmcb/control.rs:128 | 4 | N2 |
| `enable_native_nested_paging` | `enable_nested_paging` | hypervisor/src/svm/vmcb/control.rs:151 | 4 | N2 |
| `configure_native_pause_filter` | `configure_pause_filter` | hypervisor/src/svm/vmcb/control.rs:173 | 4 | N2 |
| `enable_native_x2avic` | `enable_x2avic` | hypervisor/src/svm/vmcb/mod.rs:206 | 11 | N2 |
| `validate_native_x2avic` | `validate_x2avic` | hypervisor/src/svm/vmcb/mod.rs:242 | 16 | N2 — shorter name already defined at hypervisor/src/svm/x2avic/startup.rs:462: keep or pick another |
| `validate_native_x2avic_controls` | `validate_x2avic_controls` | hypervisor/src/svm/vmcb/mod.rs:258 | 5 | N2 |
| `queue_native_x2avic_general_protection` | `queue_x2avic_general_protection` | hypervisor/src/svm/vmcb/mod.rs:323 | 4 | N2 |
| `queue_native_general_protection` | `queue_general_protection` | hypervisor/src/svm/vmcb/mod.rs:361 | 4 | N2 |

## B. Bare generic type names (N4)

| current | proposed | defined at | uses | why |
|---|---|---|---|---|
| `State` | `ChildImageState` | dxe/src/delivery/child_image.rs:145 | ≈21 | owner of the loaded child image |
| `Prepared` | `PreparedFixture` | dxe/src/fixtures/transition.rs:63 | ≈2 | binary-only fixture |
| `Observation` | `FixtureObservation` | dxe/src/fixtures/transition.rs:69 | ≈2 | binary-only fixture |
| `Range` | `PolicyRange` | dxe/src/memory_attributes/firmware.rs:55 | ≈20 | doc: "explicitly declared policy interval" |
| `Slot` | `ProbeSlot` | dxe/src/memory_attributes/probe.rs:421 | ≈6 | function-local helper |
| `Adapter` | `AttributeAdapter` | dxe/src/memory_attributes/provider.rs:24 | ≈9 | three-slot protocol interface; also section C |
| `Adapter` | `JournalIoAdapter` | dxe/src/diagnostics/journal.rs:17 | ≈2 | function-local `JournalIo` shim; also section C |
| `Range` | `MtrrRange` | dxe/src/native/admission/cache/write_back.rs:36 | ≈6 | a decoded variable-MTRR interval |
| `Slot` | `RendezvousSlot` | dxe/src/native/admission/cache_rendezvous/mod.rs:145 | ≈11 | per-AP rendezvous cell |
| `Record` | `CpuRecord` | dxe/src/native/admission/cpu.rs:465 | ≈9 | per-CPU admission record |
| `Report` | `PreflightReport` | dxe/src/native/admission/preflight.rs:11 | ≈2 | N4 names it explicitly |
| `Outcome` | `PreflightOutcome` | dxe/src/native/admission/preflight.rs:20 | ≈18 | N4 names it explicitly |
| `Prepared` | `PreparedBootHandoff` | dxe/src/native/resident/activation/boot_handoff.rs:20 | ≈3 | §6.2 `Prepared…` |
| `Prepared` | `PreparedCardBoot` | dxe/src/native/resident/activation/card_boot.rs:34 | ≈3 | §6.2 `Prepared…` |
| `Prepared` | `PreparedArena` | dxe/src/native/resources/arena.rs:63 | ≈10 | §6.2 `Prepared…` |
| `Record` | `AllocationRecord` | dxe/src/native/resident/allocation.rs:478 | ≈4 | test double's call log (inside `mod tests`) |
| `Error` | `BootstrapPagingError` | dxe/src/native/resident/bootstrap_paging.rs:87 | ≈18 | E1 |
| `Inventory` | `ProcessorInventory` | dxe/src/native/resident/processors.rs:30 | ≈5 | N4 names it explicitly |
| `Capture` | `ProcessorCapture` | dxe/src/native/resident/processors.rs:75 | ≈3 | N4 names it explicitly |
| `Error` | `ProcessorError` | dxe/src/native/resident/processors.rs:84 | ≈25 | E1 |
| `Context` | `ProbeContext` | hypervisor/src/boot/probe.rs:372 | ≈7 | N4 |
| `Stage` | `ProbeStage` | hypervisor/src/boot/probe.rs:380 | ≈43 | N4 |
| `Outcome` | `ProbeOutcome` | hypervisor/src/boot/probe.rs:400 | ≈15 | N4 |
| `Error` | `ProbeError` | hypervisor/src/boot/probe.rs:412 | ≈97 | the guide's own N4 example |
| `State` | `RuntimeState` | hypervisor/src/host/resident/runtime/mod.rs:228 | ≈45 | per-CPU resident runtime state |
| `Layout` | `MemoryLayout` | hypervisor/src/memory/layout.rs:20 | ≈31 | N4 |
| `Region` | `LayoutRegion` | hypervisor/src/memory/layout.rs:79 | ≈5 | N4 |
| `Inventory` | `IpiInventory` | hypervisor/src/svm/x2avic/ipi.rs:37 | ≈7 | the guide's own N3/N4 example |
| `Capture` | `IrqCapture` | hypervisor/src/svm/x2avic/irq.rs:199 | ≈48 | N4 |
| `Access` | `McaxAccess` | hypervisor/src/svm/mcax.rs:29 | ≈18 | doc: what the runtime does for one intercepted MCAX access |
| `Error` | `X2AvicError` | hypervisor/src/svm/x2avic/profile.rs:82 | ≈42 | E1; re-exported as `svm::x2avic::Error` |
| `Refusal` | `RegisterRefusal` | hypervisor/src/svm/x2avic/registers.rs:329 | ≈59 | N4; wire codes untouched |
| `Config` | `AttributeConfig` | memory-attributes/src/lib.rs:19 | ≈16 | N4 |
| `Error` | `AttributeError` | memory-attributes/src/lib.rs:100 | ≈106 | E1 |
| `Entry` | `TableEntry` | memory-attributes/src/x86.rs:66 | ≈16 | N4 |
| `Access` | `BudgetedAccess` | memory-attributes/src/x86.rs:30 | ≈7 | memory reader that charges a step budget |
| `Header` | `CardHeader` | xtask/src/card.rs:30 | ≈5 | N4 |

Test-only `State`/`Context`/`Storage` helpers in `crates/*/tests/**` and `tables/tests/lookup.rs` are left to the
test-naming pass.

## C. Different items sharing one name within a crate (N5)

### C1. Types

| current | proposed | defined at | uses | why |
|---|---|---|---|---|
| `TableStorage` | `GuestTableStorage` | hypervisor/src/guest/pages.rs:185 | ≈31 | guest page tables |
| `TableStorage` | `NptTableStorage` | hypervisor/src/memory/npt/table.rs:13 | ≈55 | nested page tables |
| `TableView` | `GuestTableView` | hypervisor/src/guest/pages.rs:187 | ≈2 |  |
| `TableView` | `NptTableView` | hypervisor/src/memory/npt/table.rs:15 | ≈8 |  |
| `Translation` | `GuestTranslation` | hypervisor/src/guest/pages.rs:193 | ≈9 | guest-virtual walk |
| `Translation` | `HostTranslation` | hypervisor/src/host/paging.rs:24 | ≈12 | host CR3 walk |
| `Translation` | `NptTranslation` | hypervisor/src/memory/npt/synthetic.rs:258 | ≈3 | `IdentityTranslation` already carries its subject |
| `PagePermissions` | `GuestPagePermissions` | hypervisor/src/guest/pages.rs:200 | ≈10 |  |
| `PagePermissions` | `NptPermissions` | hypervisor/src/memory/npt/synthetic.rs:266 | ≈12 |  |
| `Error` | `ProbeError` | hypervisor/src/boot/probe.rs:412 | ≈97 | = section B |
| `Error` | `X2AvicError` | hypervisor/src/svm/x2avic/profile.rs:82 | ≈42 | = section B |
| `CaptureError` | `CacheCaptureError` | dxe/src/native/admission/cache/mod.rs:99 | ≈41 |  |
| `CaptureError` | `SnapshotCaptureError` | dxe/src/native/admission/snapshot.rs:58 | ≈36 | follows the section A/D name of `NativeSnapshot` |
| `CpuArchProtocol` | (merge into one definition, K5) | dxe/src/memory_attributes/firmware.rs:256 | ≈14 | same PI protocol ABI declared twice |
| `CpuArchProtocol` | (merge; else `ProbeCpuArchProtocol`) | dxe/src/memory_attributes/probe.rs:119 | ≈11 | layouts must be compared first |
| `InterruptScope` | (merge; §6.2 says `InterruptGuard`) | dxe/src/memory_attributes/f7.rs:386 | ≈3 | RAII scope declared twice |
| `InterruptScope` | (merge; §6.2 says `InterruptGuard`) | dxe/src/memory_attributes/native.rs:140 | ≈3 |  |
| `RegistrationState` | `ProbeRegistrationState` | dxe/src/memory_attributes/probe.rs:180 | ≈19 |  |
| `RegistrationState` | (keep) | dxe/src/memory_attributes/registration.rs:91 | ≈22 | the module's primary state |
| `Error` | `BootstrapPagingError` | dxe/src/native/resident/bootstrap_paging.rs:87 | ≈18 | = section B |
| `Error` | `ProcessorError` | dxe/src/native/resident/processors.rs:84 | ≈25 | = section B |

`Prepared` ×4, `Range` ×2, `Record` ×2, `Slot` ×2 and `Adapter` ×2 in dxe are resolved by their section B names.

### C2. Constants

| current | proposed | defined at | uses | why |
|---|---|---|---|---|
| `ABI_VERSION` | (one home if one concept, K3) | dxe/src/native/admission/boundary.rs:9 | ≈10 | = `1`; identical value in every copy |
| `ABI_VERSION` | (one home if one concept, K3) | dxe/src/native/admission/cache/mod.rs:14 | ≈3 | = `1`; identical value in every copy |
| `ABI_VERSION` | (one home if one concept, K3) | dxe/src/native/transition/state.rs:8 | ≈4 | = `1`; identical value in every copy |
| `ARENA_BYTES` | (subject prefix) | dxe/src/native/resources/arena.rs:30 | ≈36 | = `(ARENA_PAGES * 4096) as u64`; values differ between copies |
| `ARENA_BYTES` | (subject prefix) | dxe/src/native/resources/guest.rs:27 | ≈17 | = `ARENA_PAGES * 4096`; values differ between copies |
| `ARENA_PAGES` | (subject prefix) | dxe/src/native/resident/allocation.rs:30 | ≈10 | = `ARENA_BYTES / PAGE_BYTES as usize`; values differ between copies |
| `ARENA_PAGES` | (subject prefix) | dxe/src/native/resources/arena.rs:29 | ≈6 | = `33`; values differ between copies |
| `ARENA_PAGES` | (subject prefix) | dxe/src/native/resources/guest.rs:26 | ≈2 | = `33`; values differ between copies |
| `COOKIE` | (subject prefix) | dxe/src/native/resident/activation/mod.rs:83 | ≈5 | = `0x53564d52`; values differ between copies |
| `COOKIE` | (subject prefix) | dxe/src/native/resources/guest.rs:28 | ≈15 | = `0x53564d4e41544956`; values differ between copies |
| `EMPTY` | (subject prefix) | dxe/src/native/admission/cache_rendezvous/mod.rs:32 | ≈3 | = `0`; values differ between copies |
| `EMPTY` | (subject prefix) | dxe/src/native/resident/activation/mod.rs:81 | ≈1 | = `MemoryDescriptor { memory_type: 0, physi`; values differ between copies |
| `FORBIDDEN_CR4` | (one home if one concept, K3) | dxe/src/memory_attributes/f7.rs:20 | ≈1 | = `(1 << 12) \| (1 << 17) \| (1 << 21) \| (1 <`; identical value in every copy |
| `FORBIDDEN_CR4` | (one home if one concept, K3) | dxe/src/memory_attributes/probe.rs:23 | ≈1 | = `(1 << 12) \| (1 << 17) \| (1 << 21) \| (1 <`; identical value in every copy |
| `HEADER_BYTES` | (one home if one concept, K3) | dxe/src/delivery/card.rs:7 | ≈9 | = `128`; identical value in every copy |
| `HEADER_BYTES` | (one home if one concept, K3) | dxe/src/delivery/child_image.rs:15 | ≈6 | = `128`; identical value in every copy |
| `LOW_CANONICAL_END` | (one home if one concept, K3) | dxe/src/memory_attributes/f7.rs:18 | ≈2 | = `1 << 47`; identical value in every copy |
| `LOW_CANONICAL_END` | (one home if one concept, K3) | dxe/src/memory_attributes/probe.rs:19 | ≈7 | = `1 << 47`; identical value in every copy |
| `REQUIRED_CR0` | (one home if one concept, K3) | dxe/src/memory_attributes/f7.rs:19 | ≈2 | = `(1 << 31) \| (1 << 16) \| 1`; identical value in every copy |
| `REQUIRED_CR0` | (one home if one concept, K3) | dxe/src/memory_attributes/probe.rs:22 | ≈2 | = `(1 << 31) \| (1 << 16) \| 1`; identical value in every copy |
| `SLOT_BYTES` | (one home if one concept, K3) | dxe/src/delivery/card.rs:8 | ≈9 | = `0x100000`; identical value in every copy |
| `SLOT_BYTES` | (one home if one concept, K3) | dxe/src/delivery/child_image.rs:16 | ≈7 | = `0x100000`; identical value in every copy |
| `XSTATE_CAPACITY` | (one home if one concept, K3) | dxe/src/native/admission/boundary.rs:12 | ≈2 | = `1024`; identical value in every copy |
| `XSTATE_CAPACITY` | (one home if one concept, K3) | dxe/src/native/transition/state.rs:10 | ≈0 | = `1024`; identical value in every copy |
| `AVX` | (one home if one concept, K3) | hypervisor/src/boot/xstate.rs:15 | ≈4 | = `1 << 28`; identical value in every copy |
| `AVX` | (one home if one concept, K3) | hypervisor/src/svm/cpu_model.rs:41 | ≈2 | = `1 << 28`; identical value in every copy |
| `LONG_MODE` | (one home if one concept, K3) | hypervisor/src/svm/cpu_model.rs:45 | ≈2 | = `1 << 29`; identical value in every copy |
| `LONG_MODE` | (one home if one concept, K3) | hypervisor/src/svm/emulation.rs:18 | ≈1 | = `1 << 29`; identical value in every copy |
| `MSR` | (one home if one concept, K3) | hypervisor/src/svm/cpu_model.rs:29 | ≈3 | = `1 << 5`; identical value in every copy |
| `MSR` | (one home if one concept, K3) | hypervisor/src/svm/emulation.rs:15 | ≈1 | = `1 << 5`; identical value in every copy |
| `NX` | (subject prefix) | hypervisor/src/memory/address.rs:11 | ≈12 | = `1 << 63`; values differ between copies |
| `NX` | (subject prefix) | hypervisor/src/svm/cpu_model.rs:43 | ≈2 | = `1 << 20`; values differ between copies |
| `OSXSAVE` | (subject prefix) | hypervisor/src/boot/xstate.rs:13 | ≈2 | = `1 << 18`; values differ between copies |
| `OSXSAVE` | (subject prefix) | hypervisor/src/svm/cpu_model.rs:40 | ≈1 | = `1 << 27`; values differ between copies |
| `PAE` | (one home if one concept, K3) | hypervisor/src/svm/cpu_model.rs:30 | ≈2 | = `1 << 6`; identical value in every copy |
| `PAE` | (one home if one concept, K3) | hypervisor/src/svm/emulation.rs:16 | ≈1 | = `1 << 6`; identical value in every copy |
| `TABLE_COUNT` | `GUEST_TABLE_COUNT` | hypervisor/src/guest/pages.rs:18 | ≈8 | = `4`; values differ between copies |
| `TABLE_COUNT` | `NPT_TABLE_COUNT` | hypervisor/src/memory/npt/table.rs:8 | ≈59 | = `8`; values differ between copies |
| `XSAVE` | (one home if one concept, K3) | hypervisor/src/boot/xstate.rs:12 | ≈1 | = `1 << 26`; identical value in every copy |
| `XSAVE` | (one home if one concept, K3) | hypervisor/src/svm/cpu_model.rs:39 | ≈2 | = `1 << 26`; identical value in every copy |

Not listed: 12 x2APIC register names (`APR`, `EOI`, `ESR`, `ICR`, `ID`, `LDR`, …) that are a
register offset in `arch::x86_64::apic` and the matching MSR number in `svm::x2avic::registers`; both are
always used module-qualified or file-locally. Owner may still want `registers::*` to carry `_MSR`.

Only top-level constants are compared; constants inside inline namespace modules (`state::msr`, `state::outcome`, …)
and associated constants are distinguished by their path.

Copies with an identical value are K3 de-duplication candidates rather than renames; several are the
feature-disjoint or binary-only duplicates K3 tolerates until the M10 debt is paid. Copies whose values
differ are different concepts and need a subject.

### C3. Across crates (dxe imports hypervisor)

| current | proposed | defined at | uses | why |
|---|---|---|---|---|
| `State` | (subject prefix on the dxe side) | dxe/src/delivery/child_image.rs:145 | ≈21 | also hypervisor/src/host/resident/runtime/mod.rs:228 |
| `JournalIo` | (subject prefix on the dxe side) | dxe/src/diagnostics/journal.rs:5 | ≈29 | also hypervisor/src/host/resident/terminal/journal.rs:6 |
| `Dispatch` | (subject prefix on the dxe side) | dxe/src/native/admission/cpu.rs:471 | ≈12 | also hypervisor/src/host/resident/abi.rs:177 |
| `Outcome` | (subject prefix on the dxe side) | dxe/src/native/admission/preflight.rs:20 | ≈18 | also hypervisor/src/boot/probe.rs:400 |
| `Error` | (subject prefix on the dxe side) | dxe/src/native/resident/bootstrap_paging.rs:87 | ≈18 | also hypervisor/src/svm/x2avic/profile.rs:82 |
| `Inventory` | (subject prefix on the dxe side) | dxe/src/native/resident/processors.rs:30 | ≈5 | also hypervisor/src/svm/x2avic/ipi.rs:37 |
| `Capture` | (subject prefix on the dxe side) | dxe/src/native/resident/processors.rs:75 | ≈3 | also hypervisor/src/svm/x2avic/irq.rs:199 |
| `Error` | (subject prefix on the dxe side) | dxe/src/native/resident/processors.rs:84 | ≈25 | also hypervisor/src/svm/x2avic/profile.rs:82 |
| `TableStorage` | (subject prefix on the dxe side) | dxe/src/native/resources/tables/mod.rs:234 | ≈17 | also hypervisor/src/memory/npt/table.rs:13 |
| `LayoutError` | (subject prefix on the dxe side) | firmware-handoff/src/layout.rs:127 | ≈45 | also hypervisor/src/memory/layout.rs:128 |

## D. Suffix does not match the doc comment (§6.2)

Only types whose own doc (or module doc) uses a different §6.2 word, or whose shape contradicts the suffix.

| current | proposed | defined at | uses | why |
|---|---|---|---|---|
| `CacheSnapshot` | `CacheControlObservation` | dxe/src/native/admission/cache/mod.rs:42 | 65 | doc: "Raw integer observations"; hypervisor already owns `CacheObservation`, so the bare form is taken |
| `CacheCapture` | `CacheSnapshot` | hypervisor/src/svm/cache/mod.rs:316 | 16 | doc: "Immutable pre-guest evidence" that we sample ourselves; `Capture` is not a §6.2 suffix. Needs the line above first |
| `HostCacheEvidence` | (keep; doc says "snapshot") | hypervisor/src/svm/cpu_model.rs:371 | 8 | it is caller-supplied to admission, so `Evidence` is right and the doc word is wrong |
| `HostCpuEvidence` | (keep; doc says "Raw observations") | hypervisor/src/svm/cpu_model.rs:344 | 6 | same reasoning |
| `F7Failure` | `F7Error` | dxe/src/memory_attributes/f7.rs:56 | 56 | an enum of reasons returned in `Err`; §6.2 reserves `Failure` for a struct of detail |
| `FetchReadFailure` | `FetchReadError` | hypervisor/src/host/resident/terminal/refusal.rs:7 | 11 | enum of stable codes returned in `Err` by the guest reader |
| `AttributeLookupFailure` | `AttributeLookupError` | dxe/src/native/resources/tables/mod.rs:371 | 27 | an enum returned in `Err`; its doc calls it "the immediate LocateProtocol observation" |
| `NativeSnapshot` | `ControlStateObservation` | dxe/src/native/admission/snapshot.rs:13 | 29 | module doc: "host observations"; a handful of registers, not one whole structure (= section A) |
| `TableSnapshot` | `DescriptorTableObservation` | dxe/src/native/admission/snapshot.rs:40 | 13 | GDTR/IDTR base and limit samples |
| `CaptureRefusal` | `RegisterCaptureFailure` | hypervisor/src/svm/x2avic/registers.rs:472 | 15 | a struct of detail (`msr`, `value`), not a wire-coded outcome |
| `RawVmexitCapture` | `RawVmexitObservation` | hypervisor/src/host/resident/runtime/mod.rs:286 | 28 | `Capture` is not a §6.2 suffix; raw samples written by runtime.S. Wire struct, layout untouched |
| `ApCapture` | `ApObservation` | firmware-handoff/src/smp.rs:51 | 3 | `Capture` is not a §6.2 suffix |
| `OwnershipRecord` | `ValidatedOwnership` | hypervisor/src/boot/ownership.rs:54 | 53 | doc: "Validated view into owned handoff bytes" |
| `ExitSnapshot` | (keep; doc says "Caller-supplied") | hypervisor/src/svm/exit.rs:14 | 61 | a copy of the VMCB exit fields, so `Snapshot` is right |

## E. Forbidden function suffixes (N8) and `*_detailed` twins (§6.3)

### E1. `_inner` workers

| current | proposed | defined at | uses | why |
|---|---|---|---|---|
| `execute_inner` | `execute_with_options` | dxe/src/delivery/child_image.rs:418 | 3 | `execute`, `execute_resident` and `execute_resident_dev` differ only in the load options they pass |
| `install_inner` | `install_with_boot_services` | dxe/src/native/resident/activation/install.rs:115 | 1 | `install` resolves the boot-services pointer and the card; the worker does the installation |
| `svmvisor_resident_callback_inner` | (keep: exported symbol, N12) | dxe/src/native/resident/activation/callback.rs:46 | 0 | called from bridge.S |
| `svmvisor_boot_inner` | (keep: exported symbol, N12) | dxe/src/native/resident/activation/boot_handoff.rs:89 | 0 | called from boot.S |
| `svmvisor_native_efi_main_inner` | (keep: exported symbol, N12) | dxe/src/main.rs:119 | 0 | called from boundary.S |
| `initialize_inner` | `initialize_profile` | dxe/src/native/resources/guest.rs:333 | 2 | the public initializers choose single- or multi-exit; the worker takes the flag |
| `native_efer_inner` | `handle_efer_at` | hypervisor/src/svm/dispatch.rs:494 | 2 | worker of `handle_native_efer` and `…_with_nrip`: takes the optional next RIP |
| `native_vmcr_inner` | `handle_vmcr_at` | hypervisor/src/svm/dispatch.rs:572 | 2 | same shape |
| `native_cpuid_inner` | `handle_cpuid_at` | hypervisor/src/svm/dispatch.rs:617 | 3 | same shape |

### E2. `*_detailed` twins

| function | returns | defined at | uses | verdict |
|---|---|---|---|---|
| `validate_observation_detailed` | `Result<(), F7Failure>` | dxe/src/memory_attributes/f7.rs:120 | 5 | returns `F7Failure`, which section D calls an error enum: if D is approved the twin is the only function and loses `_detailed` |
| `validate_capabilities_detailed` | `Result<(), F7Failure>` | dxe/src/memory_attributes/f7.rs:167 | 9 | returns `F7Failure`, which section D calls an error enum: if D is approved the twin is the only function and loses `_detailed` |
| `new_detailed` | `Result<Self, F7Failure>` | dxe/src/memory_attributes/f7.rs:267 | 2 | returns `F7Failure`, which section D calls an error enum: if D is approved the twin is the only function and loses `_detailed` |
| `get_detailed` | `Result<u64, F7Failure>` | dxe/src/memory_attributes/f7.rs:302 | 2 | returns `F7Failure`, which section D calls an error enum: if D is approved the twin is the only function and loses `_detailed` |
| `finish_handoff_detailed` | `Result<(), F7Failure>` | dxe/src/memory_attributes/f7.rs:321 | 2 | returns `F7Failure`, which section D calls an error enum: if D is approved the twin is the only function and loses `_detailed` |
| `cache_observation_detailed` | `Result< CacheObservation, CacheAdmissionFailure, >` | dxe/src/native/resident/activation/capture.rs:179 | 2 | returns the `…Failure` detail: conforms |
| `prepare_resource_ranges_detailed` | `Result<PreparedTables<'a>, TableFailure>` | dxe/src/native/resources/tables/preparation.rs:120 | 9 | returns the `…Failure` detail: conforms |
| `retain_detailed` | `Result<(), RetainError>` | firmware-handoff/src/ownership.rs:49 | 6 | does not return a `…Failure`: rename or change the twin |
| `capture_detailed` | `Result<Self, CacheAdmissionFailure>` | hypervisor/src/svm/cache/mod.rs:94 | 2 | returns the `…Failure` detail: conforms |
| `agrees_with_bsp_detailed` | `Result<(), (usize, CacheAdmissionFailure)>` | hypervisor/src/svm/cache/mod.rs:365 | 3 | returns the `…Failure` detail: conforms |
| `domain_mask_detailed` | `Result<u32, (usize, CacheAdmissionFailure)>` | hypervisor/src/svm/cache/mod.rs:392 | 3 | returns the `…Failure` detail: conforms |
| `initialize_detailed` | `Result<(), (usize, CacheAdmissionFailure)>` | hypervisor/src/svm/cache/mod.rs:687 | 2 | returns the `…Failure` detail: conforms |
| `native_topology_detailed` | `Result<[u32; 4], CacheAdmissionFailure>` | hypervisor/src/svm/cache/mod.rs:732 | 3 | returns the `…Failure` detail: conforms |

## F. Constants (N10, N11, K3)

### F1. `_SIZE` / `_LEN` and page-size names

| current | proposed | defined at | uses | why |
|---|---|---|---|---|
| `HANDOFF_SIZE` | `HANDOFF_BYTES` | hypervisor/src/boot/handoff.rs:15 | ≈11 | N10 |
| `PAGE_SIZE` | (drop; write `PAGE_BYTES as u64`) | hypervisor/src/memory/layout.rs:17 | ≈44 | K3: a `u64` view is derived, not named twice. Alternative one-word answer: `PAGE_BYTES_U64` |
| `PAGE_BYTES` | (same decision as `layout::PAGE_SIZE`) | dxe/src/native/resident/allocation.rs:29 | ≈15 | a `u64` constant shadowing the `usize` `address::PAGE_BYTES` under the same name |
| `PAGE` | (same decision as `layout::PAGE_SIZE`) | dxe/src/native/resident/bootstrap_paging.rs:11 | ≈7 | third spelling of the `u64` view |
| `PAGE_MASK` | `PHYSICAL_PAGE_MASK` | dxe/src/native/admission/cache/mod.rs:19 | ≈11 | `PHYSICAL_MASK & !0xfff`: a width-dependent MTRR mask, not the page-table `ADDRESS_MASK` |
| `PAGE_SIZE` | `PAGE_BYTES` | memory-attributes/src/lib.rs:16 | ≈156 | N11; standalone crate keeps its own copy (K3) |
| `ADDRESS_FIELD` | `ADDRESS_MASK` | memory-attributes/src/x86.rs:26 | ≈3 | N11; standalone crate keeps its own copy (K3) |
| `EFI_ROM_HEADER_SIZE` | `EFI_ROM_HEADER_BYTES` | rompack/src/lib.rs:8 | ≈2 | N10 |
| `PCI_DATA_STRUCTURE_SIZE` | `PCI_DATA_STRUCTURE_BYTES` | rompack/src/lib.rs:9 | ≈3 | N10 |
| `HEADER_SIZE` | `HEADER_BYTES` | rompack/src/lib.rs:11 | ≈1 | N10 |
| `MAX_ROM_SIZE` | `MAX_ROM_BYTES` | rompack/src/lib.rs:13 | ≈1 | N10 |

### F2. dxe `MULTI_*` duplicates of `native::transition::state::multi` / `::outcome`

| current | proposed | defined at | uses | why |
|---|---|---|---|---|
| `MULTI_CPUID_RIP` | (import `state::multi::CPUID_RIP`) | dxe/src/native/resources/guest.rs:42 | 2 | `0x1086` vs `0x1086`; K3 — guest.rs is mounted by main.rs and by tests/native_guest_resources.rs: the path must resolve in every mount |
| `MULTI_QUERY_RIP` | (import `state::multi::QUERY_RIP`) | dxe/src/native/resources/guest.rs:43 | 2 | `0x10bf` vs `0x10bf`; K3 — guest.rs is mounted by main.rs and by tests/native_guest_resources.rs: the path must resolve in every mount |
| `MULTI_STOP_RIP` | (import `state::multi::STOP_RIP`) | dxe/src/native/resources/guest.rs:44 | 8 | `0x10ff` vs `0x10ff`; K3 — guest.rs is mounted by main.rs and by tests/native_guest_resources.rs: the path must resolve in every mount |
| `MULTI_FAIL_RIP` | (import `state::multi::FAIL_UD2_RIP`) | dxe/src/native/resources/guest.rs:45 | 1 | `0x1102` vs `0x1102`; K3 — guest.rs is mounted by main.rs and by tests/native_guest_resources.rs: the path must resolve in every mount |
| `MULTI_CPUID_LEAVES` | (import `state::multi::CPUID_LEAVES`) | dxe/src/native/resources/guest.rs:46 | 2 | `[0, 1, 0x40000000, 0x40000001, 0x8` vs `[0, 1, 0x4000_0000, 0x4000_0001, 0`; K3 — guest.rs is mounted by main.rs and by tests/native_guest_resources.rs: the path must resolve in every mount |
| `MULTI_EXIT_OUTCOME` | (import `state::outcome::MULTI_EXIT`) | dxe/src/diagnostics/native_result.rs:8 | 4 | `12` vs `12`; K3 — `diagnostics` is compiled in every feature set, `native::transition::state` only under native features: the one home has to be the `diagnostics` side |
| `MULTI_EXIT_ENTRIES` | (import `state::multi::EXPECTED_EXITS`) | dxe/src/diagnostics/native_result.rs:9 | 6 | `65` vs `2 * ROUNDS + 1`; K3 — `diagnostics` is compiled in every feature set, `native::transition::state` only under native features: the one home has to be the `diagnostics` side |

`MULTI_GUEST_BYTES`, `MULTI_CPUID_OUTPUTS`, `MULTI_FINAL_GPRS` and `MULTI_CODE` have no twin in `state::multi`; if the
duplicates above go, these four belong in `state::multi` too, without the `MULTI_` stutter (N3).

### F3. VMCB field offsets without `_OFFSET` (N11)

| current | proposed | defined at | uses | why |
|---|---|---|---|---|
| `INTERCEPT_MISC1` | `INTERCEPT_MISC1_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:28 | ≈15 | N11 `<FIELD>_OFFSET` |
| `INTERCEPT_MISC2` | `INTERCEPT_MISC2_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:29 | ≈13 | N11 `<FIELD>_OFFSET` |
| `IOPM_BASE` | `IOPM_BASE_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:30 | ≈3 | N11 `<FIELD>_OFFSET` |
| `MSRPM_BASE` | `MSRPM_BASE_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:31 | ≈3 | N11 `<FIELD>_OFFSET` |
| `TSC_OFFSET` | `TSC_OFFSET_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:32 | ≈7 | N11 `<FIELD>_OFFSET`; `TSC_OFFSET_OFFSET` reads badly: `TSC_OFFSET_FIELD_OFFSET`? |
| `GUEST_ASID` | `GUEST_ASID_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:33 | ≈4 | N11 `<FIELD>_OFFSET` |
| `VIRTUAL_INTERRUPT_CONTROL` | `VIRTUAL_INTERRUPT_CONTROL_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:34 | ≈10 | N11 `<FIELD>_OFFSET` |
| `NESTED_CR3` | `NESTED_CR3_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:44 | ≈3 | N11 `<FIELD>_OFFSET` |
| `CLEAN_BITS` | `CLEAN_BITS_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:45 | ≈7 | N11 `<FIELD>_OFFSET` |
| `GUEST_EFER` | `GUEST_EFER_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:46 | ≈6 | N11 `<FIELD>_OFFSET` |
| `GUEST_CR4` | `GUEST_CR4_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:47 | ≈5 | N11 `<FIELD>_OFFSET` |
| `GUEST_CR3` | `GUEST_CR3_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:48 | ≈5 | N11 `<FIELD>_OFFSET` |
| `GUEST_CR0` | `GUEST_CR0_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:49 | ≈5 | N11 `<FIELD>_OFFSET` |
| `GUEST_RFLAGS` | `GUEST_RFLAGS_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:50 | ≈6 | N11 `<FIELD>_OFFSET` |
| `GUEST_RIP` | `GUEST_RIP_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:51 | ≈7 | N11 `<FIELD>_OFFSET` |
| `GUEST_RSP` | `GUEST_RSP_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:52 | ≈5 | N11 `<FIELD>_OFFSET` |
| `GUEST_S_CET` | `GUEST_S_CET_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:53 | ≈2 | N11 `<FIELD>_OFFSET` |
| `GUEST_ISST_ADDR` | `GUEST_ISST_ADDR_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:54 | ≈2 | N11 `<FIELD>_OFFSET` |
| `GUEST_RAX` | `GUEST_RAX_OFFSET` | hypervisor/src/svm/vmcb/mod.rs:55 | ≈7 | N11 `<FIELD>_OFFSET` |

### F4. Other non-N11 spellings

| current | proposed | defined at | uses | why |
|---|---|---|---|---|
| `ID_MSR` | (keep) | hypervisor/src/arch/x86_64/apic.rs:97 | ≈3 | MSR number of the x2APIC `ID` register; the suffix separates it from the register offset `ID` |
| `ICR_MSR` | (keep) | hypervisor/src/arch/x86_64/apic.rs:98 | ≈15 | same |
| `TIMER_CURRENT_COUNT_MSR` | (keep) | hypervisor/src/arch/x86_64/apic.rs:99 | ≈2 | same |
| `SELF_IPI_MSR` | `SELF_IPI` | hypervisor/src/arch/x86_64/apic.rs:101 | ≈5 | no register-offset twin exists, so the suffix is not needed (N11: the manual's name) — name already defined at hypervisor/src/svm/x2avic/registers.rs:55 |
| `X2APIC_MSR_FIRST` | (keep) | hypervisor/src/arch/x86_64/apic.rs:93 | ≈7 | inclusive bounds already use N13 `first`/`last` |
| `IDENTITY_TSC_RATIO` | `TSC_RATIO_IDENTITY` | hypervisor/src/arch/x86_64/clock.rs:8 | ≈3 | N11 `<MSR>_<VALUE>` now that `TSC_RATIO` lives in `msr` |
| `XSTATE_CAPACITY` | `XSTATE_CAPACITY_BYTES` | dxe/src/native/admission/boundary.rs:12 | ≈2 | N10: unit last |

## G. Field names against N13

Grouped by struct. **wire** marks a `#[repr(C)]` struct: renaming a field does not move it, but assembly,
`offset_of!` asserts and any `.S` comment that names the field must follow.

| struct | field | proposed | defined at | uses |
|---|---|---|---|---|
| `ResidentBootOptions` **wire** | `size: u32` | `length` | dxe/src/diagnostics/resident_boot.rs:10 | ≈2 in file |
| `Fixture` | `services: &'a BootServices` | `boot_services` | dxe/src/fixtures/transition.rs:33 | ≈8 in file |
| `BootServicesDatabase` | `services: &'fw BootServices` | `boot_services` | dxe/src/memory_attributes/registration.rs:39 | ≈5 in file |
| `NativeBoundary` **wire** | `xstate_size: u64` | `xstate_length` | dxe/src/native/admission/boundary.rs:24 | ≈3 in file |
| `NativeBoundary` **wire** | `avx_size: u32` | `avx_length` | dxe/src/native/admission/boundary.rs:59 | ≈1 in file |
| `PageMapping` | `physical_page: u64` | `page_pa` | dxe/src/native/admission/cache/write_back.rs:29 | ≈2 in file |
| `PreparedCacheRendezvous` | `services: &'a BootServices` | `boot_services` | dxe/src/native/admission/cache_rendezvous/mod.rs:51 | ≈4 in file |
| `PreparedCpus` | `services: &'a BootServices` | `boot_services` | dxe/src/native/admission/cpu.rs:31 | ≈21 in file |
| `TplScope` | `services: &'a BootServices` | `boot_services` | dxe/src/native/admission/cpu.rs:482 | ≈21 in file |
| `MemoryMapSnapshot` | `services: &'a BootServices` | `boot_services` | dxe/src/native/admission/memory.rs:27 | ≈6 in file |
| `MapMetadata` | `descriptor_size: usize` | (keep: UEFI's own `DescriptorSize`) | dxe/src/native/admission/memory.rs:75 | ≈9 in file |
| `LowAllocation` | `bs: &'a BootServices` | `boot_services` | dxe/src/native/resident/activation/physical_boot.rs:80 | ≈9 in file |
| `OwnedPages` | `services: S` | (keep: a generic page provider, not UEFI boot services) | dxe/src/native/resident/allocation.rs:181 | ≈18 in file |
| `Arena` | `services: &'a BootServices` | `boot_services` | dxe/src/native/resources/arena.rs:35 | ≈12 in file |
| `PreparedTables` | `services: &'a BootServices` | `boot_services` | dxe/src/native/resources/tables/mod.rs:57 | ≈1 in file |
| `EntryObservation` | `address: u64` | `pa` | dxe/src/native/resources/tables/mod.rs:264 | ≈5 in file |
| `LeafObservation` | `physical_page: u64` | `page_pa` | dxe/src/native/resources/tables/mod.rs:298 | ≈2 in file |
| `TablePageObservation` | `physical_page: u64` | `page_pa` | dxe/src/native/resources/tables/mod.rs:324 | ≈2 in file |
| `XstateLayout` | `size: usize` | `length` | hypervisor/src/arch/x86_64/xstate.rs:20 | ≈10 in file |
| `XstateCapabilities` | `enabled_size: u32` | `enabled_length` | hypervisor/src/arch/x86_64/xstate.rs:218 | ≈0 in file |
| `XstateCapabilities` | `max_size: u32` | `max_length` | hypervisor/src/arch/x86_64/xstate.rs:219 | ≈1 in file |
| `XstateCapabilities` | `avx_size: u32` | `avx_length` | hypervisor/src/arch/x86_64/xstate.rs:220 | ≈2 in file |
| `ValidatedMemoryMap` | `physical_end: u64` | `end_pa` | hypervisor/src/boot/memory.rs:16 | ≈4 in file |
| `PermittedRange` | `physical_start: u64` | `start_pa` | hypervisor/src/boot/memory.rs:159 | ≈9 in file |
| `MemoryDescriptor` | `physical_start: u64` | `start_pa` | hypervisor/src/boot/memory.rs:175 | ≈9 in file |
| `Translation` | `physical_address: u64` | `pa` | hypervisor/src/host/paging.rs:25 | ≈1 in file |
| `GuestReader` | `mt: crate::memory::mtrrs::Mtrrs` | `mtrrs` | hypervisor/src/host/resident/runtime/guest_reader.rs:33 | ≈5 in file |
| `PhysicalRange` | `len: u64` | `length` | hypervisor/src/memory/address.rs:77 | ≈7 in file |
| `Region` | `len: u64` | `length` | hypervisor/src/memory/layout.rs:82 | ≈7 in file |
| `TableView` | `physical_address: u64` | `pa` | hypervisor/src/memory/npt/table.rs:16 | ≈0 in file |
| `ResumeCandidate` | `address: u64` | (`gpa` or `va`: owner to say which it is) | hypervisor/src/svm/exit.rs:308 | ≈18 in file |
| `NestedPageFault` | `guest_physical_address: u64` | `gpa` | hypervisor/src/svm/exit.rs:326 | ≈3 in file |

`physical_bits`, `physical_address_bits` and `sys_cfg` fields are architecture vocabulary (N1 exception) and are not
listed. Local variables that N13 also covers but phase 6a left alone are in section I.

## H. `pub` items nothing outside their crate names (later, feature-aware narrowing)

Name search only: the identifier occurs in no file outside the item's own target. It has not been compiled;
in phase 6a the same search gave 143 hypervisor candidates, of which the compiler and the binary comparison let 73
become `pub(crate)`; the other 70 stayed `pub` (dead-code warnings in some feature set, `pub use` re-exports, or a
change in generated code). Another 15 were narrowed although their name also occurs outside the crate, because the
outside occurrence is a different item; a name search cannot find those.

### dxe library: 111 candidates in 21 files

- dxe/src/delivery/child_image.rs: `ImageKind`:126, `parse_pe`:240
- dxe/src/diagnostics/returning_detail.rs: `DETAIL`:9
- dxe/src/diagnostics/trace.rs: `TRACE_DETAIL`:9
- dxe/src/memory_attributes/f7.rs: `finish_handoff`:317
- dxe/src/memory_attributes/firmware.rs: `into_parts`:115, `SetMemoryAttributesFn`:247
- dxe/src/memory_attributes/native.rs: `TransientTableReader`:19, `last_error`:48
- dxe/src/memory_attributes/probe.rs: `PAGE_FAULT_VECTOR`:15, `MAX_PROBE_READS`:17, `SystemContext`:107, `InterruptHandler`:113, `RegisterInterruptHandler`:114, `NativeProbe`:533, `try_close`:657, `registration_state`:669
- dxe/src/memory_attributes/registration.rs: `BootServicesDatabase`:38
- dxe/src/native/admission/boundary.rs: `XSTATE_CAPACITY`:12
- dxe/src/native/admission/cache/mod.rs: `MAX_VARIABLE_MTRRS`:16, `CONTROLS`:25, `ADDRESS_ENCRYPTION`:26, `ROUTING`:27, `MTRRS`:28, `capture_into`:119
- dxe/src/native/admission/cache/write_back.rs: `WriteBackReport`:14, `leaf_pat_index`:83
- dxe/src/native/admission/cpu.rs: `PreparedCpus`:30, `processor_information`:47, `CpuReport`:397, `PreparedScopeCompletion`:410
- dxe/src/native/admission/memory.rs: `MapMetadata`:73, `StorageRange`:82
- dxe/src/native/admission/preflight.rs: `Report`:11
- dxe/src/native/resident/allocation.rs: `RuntimeArena`:38, `PublishedArena`:138, `AllocationError`:287
- dxe/src/native/resident/bootstrap_paging.rs: `TABLE_PAGES`:10
- dxe/src/native/resident/callback.rs: `CallbackPrepared`:48
- dxe/src/native/resident/memory.rs: `validate_runtime_coverage`:55
- dxe/src/native/resident/processors.rs: `Processor`:50
- dxe/src/native/transition/canary.rs: `CANARY_BYTES`:11, `CANARY_ALIGNMENT`:12, `MAX_SHIM_STACK_BYTES`:13, `GPRS`:16, `RFLAGS`:17, `RSP`:18, `X87_ENVIRONMENT`:19, `X87_PAYLOAD`:20, `MXCSR`:21, `XMM`:22, `YMM_UPPER`:23, `XSTATE_CONTROLS`:24, `SETUP`:25
- dxe/src/native/transition/state.rs: `XSTATE_CAPACITY`:10, `VM_HSAVE_PA`:16, `DEBUGCTL`:17, `DEBUG_EXTN_CTL`:18, `FS_BASE`:19, `GS_BASE`:20, `KERNEL_GS_BASE`:21, `XSS`:22, `DEBUG_REGISTERS`:28, `SVM_MSRS`:29, `DEBUGCTL`:31, `DEBUG_EXTN`:32, `XSS`:35, `SEGMENT_BASES`:36, `EXIT_FIELDS`:41, `GPRS`:42, `EXTRA`:43, `XSTATE`:44, `NOT_ENTERED`:51, `ORIGINAL_CAPTURED`:52, `SVME_ENABLED`:53, `GIF_CLEARED`:54, `HOST_EXTRA_SAVED`:55, `HSAVE_BOUND`:56, `GUEST_EXTRA_LOADED`:57, `VMRUN_ATTEMPTED`:58, `VMEXIT_CAPTURED`:59, `HOST_EXTRA_RESTORED`:60, `HOST_SCALARS_RESTORED`:61, `EVENTS_RELEASED`:62, `EFER_RESTORED`:63, `RESTORED_OBSERVED`:64, `RETURNING`:65, `NOT_RUN`:69, `GUEST_EXCEPTION`:75, `INVALID_GUEST`:76, `GUARDED_HOST_FAULT`:78, `INTR`:81, `RESTORATION_FAILED`:83, `FAIL_UD2_RIP`:103, `CPUID_INPUT_RBX`:107, `PHASE_CPUID`:144, `PHASE_QUERY`:145, `PHASE_STOP`:146, `COUNTS`:148, `SITE`:149, `OPERAND`:150, `GPR`:151, `STACK_FLAGS`:152, `PENDING_EVENT`:153, `EXIT`:155, `HARD_CAP`:156, `INPUTS`:164, `ORIGINAL`:165, `RESTORED`:166, `GUEST`:167, `DescriptorTableImage`:300

### dxe binary-only files (mounted by `main.rs`, M10 debt)

A binary has no outside: every `pub` here can be `pub(crate)` once the mounts are gone. Counts of `pub` items:

- dxe/src/fixtures/transition.rs: 1
- dxe/src/main.rs: 2
- dxe/src/native/resident/activation/callback.rs: 1
- dxe/src/native/resources/arena.rs: 4
- dxe/src/native/resources/cache.rs: 5
- dxe/src/native/resources/guest.rs: 25
- dxe/src/native/resources/image.rs: 5
- dxe/src/native/resources/tables/mod.rs: 28
- dxe/src/native/resources/tables/preparation.rs: 5

### firmware-handoff library: 2 candidates in 1 files

- firmware-handoff/src/lib.rs: `run_initialized`:62, `panic_fail`:239

### memory-attributes library: 0 candidates in 0 files


### rompack library: 1 candidates in 1 files

- rompack/src/lib.rs: `RomError`:67

### resident-payload library: 0 candidates in 0 files


xtask is a binary; its `pub` items are all candidates and are not listed.

## I. Skipped in phase 6a (Part 1), with the reason

### I1. Visibility (group 1): hypervisor items still `pub` although nothing outside the crate names them

| what | where (hypervisor/src) | why it stayed `pub` |
|---|---|---|
| `TSC_AUX`, `TSC_RATIO` (were `TSC_AUX_MSR`, `TSC_RATIO_MSR`) | arch/x86_64/msr.rs | no user at all, tests included: `pub(crate)` raises `dead_code` |
| `XSAVE_HEADER_OFFSET`, `XstateArea::as_mut_ptr` | arch/x86_64/xstate.rs | no user at all |
| `OwnershipRecord::arena` | boot/ownership.rs | no user at all |
| `PhysicalRange::is_empty`, `Region::is_empty` | memory/address.rs, memory/layout.rs | no user at all |
| `PreparedIo::width` | svm/diagnostic_config.rs | no user at all |
| `Vmcb::enable_physical_interrupt_virtualization` | svm/vmcb/interrupt.rs | no user at all |
| `CacheCapture::agrees_with_bsp`, `CacheOwner::initialize` | svm/cache/mod.rs | used only by in-crate unit tests (the `_detailed` twin is what the runtime calls) |
| `Vmcb::tsc_offset`, `Vmcb::set_tsc_offset_zero` | svm/vmcb/control.rs | used only by in-crate unit tests |
| `HostX2Apic`, `HostX2Apic::new`, `ring_avic_doorbell` | arch/x86_64/apic.rs | used only by the `resident-runtime` feature: dead in the default-feature build |
| `HWCR_MC_STATUS_WR_EN`, `HWCR_IO_CFG_GP_FAULT` | arch/x86_64/msr.rs | same |
| `TerminalControl::{all_acknowledged, diagnostic_snapshot}` | host/resident/terminal/control.rs | same |
| `DeferredFault::{capture, pending, published, failed, status}` | host/resident/terminal/journal.rs | same |
| `LowMemoryNptStorage::{empty, prepare}` | memory/npt/identity.rs | same |
| `CacheObservation::{capture, same_physical_state}`, `CacheCapture::domain_mask`, `CacheCoreState::{enter, leave, depart, write}`, `CacheWriteError`, `native_topology`, `owned_msr` | svm/cache/mod.rs | same |
| `PreparedIo::input` | svm/diagnostic_config.rs | same |
| `mcax::Access`, `mcax::plan` | svm/mcax.rs | same |
| `Vmcb::{guest_cr3, set_cache_cr0_guard, consume_tlb_flush_after_exit, reinject_interrupted_delivery}` | svm/vmcb/ | same |
| `written_command` | svm/x2avic/ipi.rs | same |
| `CacheCoreState`, `NativeRouteRecipient`, `NativeDestinationCause` | svm/cache/mod.rs, svm/x2avic/startup.rs | type of a `pub` field of a type that must stay `pub`: `private_interfaces` warning |
| 26 items re-exported by name: `DiagnosticGuard`, `cpu_mask`, `DeferredFault`, `FetchReadFailure`, `IrqSite`, `StartupStage`, `X2AvicStop` and 14 refusal-word functions (`*_failure`, `*_refusal`, `*_error_code`, `profile_mismatch_at_entry`); `LowMemoryNptStorage`, `IdentityTranslation`; `HwcrError`, `access_hwcr`; `ReinjectOutcome` | host/resident/terminal/mod.rs, memory/npt/mod.rs, svm/cache/mod.rs, svm/vmcb/mod.rs | a `pub use` cannot re-export a `pub(crate)` item; splitting the re-export into `pub(crate) use` was tried and warns (`unused_imports`, `dead_code`, `private_interfaces`) in the default-feature build because the only in-crate user is the feature-gated runtime |
| `CacheObservation::restored_mtrrs`, `CacheCapture::complete`, `CacheCoreState::read`, `NativeStartupTarget::validate_x2avic` | svm/cache/mod.rs, svm/x2avic/startup.rs | no warning, but `pub(crate)` lets LLVM inline them into their single caller: `payload.bin` and `driver.efi` change (`restored_mtrrs` disappears into `leave`, +3 instructions overall) |

### I2. Function verbs (group 2)

| what | where | why skipped |
|---|---|---|
| `check_exit_event` | hypervisor/src/host/resident/runtime/exit.rs | returns `bool` but takes `&mut State, &mut Vmcb`, stops the guest and re-injects events: `is_*` promises no side effects and `validate_*` promises a `Result`. Candidates: `settle_exit_event`, `handle_exit_event` |
| `check_boundary` | hypervisor/tests/svm_x2avic_manual_conformance.rs | a test helper that asserts and returns `()`; tests were out of scope. Candidate: `assert_boundary` |
| `valid_page` | hypervisor/tests/boot_ownership.rs | builds a valid page (adjective, not a predicate): not an N7 case |

### I3. Constants (group 3)

| what | where | why skipped |
|---|---|---|
| `ADDRESS_FIELD`, `ADDRESS` | memory-attributes/src/x86.rs, memory-attributes/tests/conformance.rs | same value and meaning as `ADDRESS_MASK`, but the crate is standalone (C1) and cannot import it; the rename is in F1 |
| CODINGSTYLE K3 still says "the TSC MSRs in `clock.rs` are still to move in" | CODINGSTYLE.md | the guide is not edited by style work; the parenthesis is now stale |

### I4. Locals and parameters (group 4)

Renamed: `mt`→`mtrrs` (3 files), `cfg`→`config` (3), `bs`→`boot_services` (5), `b`→`bytes` (3). Everything else:

| what | where | why skipped |
|---|---|---|
| `mt`, `cfg` | dxe/src/native/resident/activation/{boot_handoff, callback, card_boot, install, physical_boot}.rs | every one of these functions calls the free functions `mtrrs(..)` and `config(..)`: a local of the same name would shadow the function it is initialized from (`let mtrrs = mtrrs(..)` compiles, later calls would not). Several also contain `asm!`. Rename the functions first (`capture_mtrrs`, `paging_config`), then the locals |
| `mt` | hypervisor/src/host/resident/runtime/guest_reader.rs | it is the struct field `GuestReader::mt` (section G); the local in `new` feeds the field-init shorthand |
| `cfg` | hypervisor/src/host/resident/runtime/diagnostics.rs (`checked_endpoint_locked`) | it is a PCI configuration-space address, not a configuration: needs its own word (`config_base`?), and `read_cfg` beside it |
| `bs` | dxe/src/native/resident/activation/physical_boot.rs | `LowAllocation { bs, .. }` field shorthand; the field goes first (section G) |
| `b`, `o` | dxe/src/delivery/child_image.rs `read_u16/32/64` | the body destructures `&[a, b, ..]`, rebinding `b` as one byte inside the same function; a rename has to choose new pattern names too |
| `b` | dxe native/resident/callback.rs, activation/callback.rs, launch.rs | a `&NativeBoundary`, not bytes: `boundary` |
| `b` | dxe delivery/card.rs `nibble(b: u8)`, hypervisor svm/emulation.rs `cpuid` | one byte / the second vendor word, not a byte slice |
| `v` | everywhere in `src/` | no `v` is a `Vmcb`. What remains: ≤3-line closures (N14 allows), `let (r, v) = pending(e)` in terminal/refusal.rs, and `let v: u8` in runtime/diagnostics.rs `handle_io`, which is an `asm!` operand and must never be renamed mechanically |
| `e` | hypervisor host/resident/terminal/refusal.rs (`instruction(e)`, `pending(e)`, `msr_failure_context`) | the only error bindings longer than three lines; the file is skipped as a whole because `msr_failure_context` already binds `error` and rebinds `e` in nested arms |
| `e` | hypervisor memory/npt/identity.rs | page-table *entries* and range *ends*, not errors: `entry`, `end` |
| `e` | dxe firmware/driver.rs, firmware-handoff smp.rs, hypervisor svm/syscfg.rs | one-line `Err(e) => return e` arms and `map_err(\|e\| ..)` closures: N14 allows them |
| `r` | dxe admission/preflight.rs, native/entry.rs (CPUID registers), resident/allocation.rs tests (a `RefCell` guard), hypervisor svm/vmcb/continuation.rs (`&prepared.request`), terminal/refusal.rs and runtime/msr.rs (a refusal reason) | none is "the value being returned": each needs its own word (`registers`, `record`, `request`, `reason`) |
| `st`, `err`, `ret`, `res` | — | do not occur as locals in `src/` |
| all locals in `crates/*/tests/**` | — | out of scope for phase 6a |

## Open questions for the owner

1. Section A: drop `Native`/`native_` wherever no counterpart exists? (yes / no)
2. `guest::continuation`, where `ContinuationError` does exist beside `NativeContinuationError`: keep the `Native*` family, or rename the synthetic side to `Integer*` and drop `Native`? (keep / rename)
3. The `u64` page size (`layout::PAGE_SIZE`, dxe `PAGE_BYTES: u64`, `PAGE`): write `PAGE_BYTES as u64` at each use, or one named view? (inline / named)
4. Enums returned in `Err` but called `…Failure` (`F7Failure`, `FetchReadFailure`, `AttributeLookupFailure`): rename to `…Error`? (yes / no)
5. `…Capture` as a type suffix (`CacheCapture`, `RawVmexitCapture`, `ApCapture`, `irq::Capture`): add it to §6.2, or rename them away? (add / rename)
6. `services: &BootServices` fields (8 structs): rename to `boot_services`? (yes / no)
7. Items only the feature-gated runtime uses stay `pub`, because `pub(crate)` warns in the default-feature build: accept that, or make the default build include the runtime's users later? (accept / later)
8. Four functions stay `pub` only because narrowing them changes the payload's machine code (section I1, last row): keep them `pub`, or narrow them in a commit that is allowed a code change and gets a boot test? (keep / narrow)
