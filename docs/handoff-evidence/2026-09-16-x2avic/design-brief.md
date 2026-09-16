# Phase B brief: x2APIC/x2AVIC completion with the host IRQ bridge

Date: 2026-09-16. Scratch planning record (work/ is gitignored). Coordinator-authored.
Base: HEAD eafe33a plus the uncommitted Phase A tree (see phase-a-notes.md here).

## User decisions (authoritative)

1. Device interrupts reach x2AVIC guests through the host IRQ bridge
   (`capture_physical_irq`): the host captures physical interrupts and publishes
   them into the owning CPU's AVIC backing page. Windows keeps using the physical
   IOMMU natively. IOMMU GA direct posting is a later batch. (Phase A removed the
   IOMMU XTSup gate, MMIO trap and route table.)
2. Production-uncalled code is removed, not parked. Every new abstraction must
   have a real production caller in this batch.

## Normative inputs

These fact sheets cite rendered page images (hash, PDF page, index, printed page).
Read the relevant sheet before implementing a rule, and cite the manual (not the
sheet) in code comments, e.g. `APM2 rev3.44 Table 16-6 p658`.

- `work/x2avic-manual-2026-09-16/apm-avic/facts.md`: APM2 15.29, Tables B-1/C-1
- `work/x2avic-manual-2026-09-16/apm-x2apic/facts.md`: APM2 chapter 16
- `work/x2avic-manual-2026-09-16/ppr-lapic/facts.md`: PPR 57896 rev3.00 (Family 1Ah Model 44h)
- `work/x2avic-manual-2026-09-16/acpi-madt/facts.md`: ACPI 6.6 MADT and this machine's captured MADT

If a rule you need is not in these sheets, render and read the page images
yourself. Load the `anthropic-skills:pdf` skill first, then use:
`python C:\Users\mato\AppData\Local\Temp\claude\C--Users-mato-Documents-svmvisor\8a332362-e59a-4177-8f90-e764a2cb5b85\scratchpad\manual\find_pages.py <pdf> "<regex>"` (location only), then
`python ...\scratchpad\manual\render_pages.py <pdf> <pages> --out <dir> --split`, and view the PNGs.
Never use extracted text as evidence. Unresolved manual points (U-numbers in the
sheets) must be recorded as explicit decisions, not presented as verified.

## Decisions

### D1. Guest x2APIC interception profile (`Msrpm::configure_native_x2avic`, per-vCPU private MSRPM)

Why: APM2 Table 15-22 (pp566-568) makes SVR/LVT/timer/ESR/ID/LDR writes traps
that happen after the backing write, with RIP already advanced. That is too late
to raise the #GP(0) cases of APM2 Table 16-6 / 16.11.3 (pp657-659). APM2 15.11
(p518) orders the MSRPM intercept before MSR-specific exceptions, and 15.29.10
(p583) orders x2APIC intercept checks before AVIC permission checks. nRIP is
valid for MSR intercepts (15.7.1 p509) but zero for 401h/402h.

- Not intercepted (hardware allowed or accelerated):
  - Read: 802, 803, 808, 80A, 80D, 80F, 810-827, 828, 830, 832-838, 83E.
  - Write: 808 (TPR), 80B (EOI; see D6 for the level exception), 830 (ICR), 83F (SELF IPI).
- Intercepted before access (VMEXIT_MSR 7Ch with nRIP), emulated by the new register owner:
  - Read: 800-801, 804-807, 809 (APR, emulated), 80B (#GP), 80C, 80E, 829-82F, 831,
    839 (current count, emulated), 83A-83D, 83F (#GP), 840-8FF (#GP).
  - Write: every MSR in 800-8FF except 808, 830, 83F, and 80B while no level source is held.
  - APIC_BASE (1Bh): read and write.

### D2. Emulated register semantics (apm-x2apic Q1 table, ppr-lapic section 3)

- #GP(0), with no side effects and RIP unchanged:
  - any unlisted MSR;
  - a write to 802;
  - a read of 83F;
  - a non-zero write to 80B or 828;
  - any reserved bit set, including bits 63:32 of every non-ICR register.
- Also #GP(0), each an explicit U-decision to record:
  - Writes to the RO registers 803, 809, 80A, 80D, 810-827, 839 (U1). The APM says
    only "RO"; the PPR says Error-on-write.
  - A read of 80B (U2). The APM says write-only; the PPR says Error-on-read.
  - Any access to 840-8FF (U10). The guest version register is 0x0005_0010, so bit 31
    is 0 and there is no AMD extended space. `cpu_model::native_boot_cpuid` already
    hides CPUID 8000_0001 ECX[3] ExtApicSpace, and the PPR's physical DirectedEoiSupport
    has no enable bit (PPR U2), so guest bit 24 = 0 is correct.
- APR read (809), emulated from the backing page (APM2 16.6.x pp646-647):
  - Take the highest priority class among TPR, highest ISR and highest IRR.
  - Sub-priority equals TPR's when APR equals the TPR class, otherwise 0.
- ESR write of 0: store 0. The virtual error state is always empty; virtual APIC
  error generation is not modeled (U15/U16 decision).
- SVR (80F):
  - Bits 63:10 set: #GP. Bit 12 is reserved (APM Fig 16-17 p641; PPR p57/p176).
  - Otherwise store the value in the backing SVR.
  - If bit 8 = 0, force the mask bit into all six backing LVTs and all mirrored
    physical LVTs (APM p629/p641; PPR p57: "All LVT entry mask bits are set and cannot
    be cleared"). On re-enable the masks stay set until the guest rewrites them (U13 decision).
- LVT writes, reserved masks (#GP if any is set):
  - Timer 832: 63:18, 15:13, 11:8. Bit 17 is the mode (periodic/one-shot). There is
    no TSC-deadline mode (APM U23; PPR D1 follows the x2APIC table).
  - Thermal 833, perf 834, error 837: 63:17, 15:13, 11.
  - LINT0 835 and LINT1 836: 63:17, 13, 11. Bit 15 is the trigger mode.
  - DS (bit 12) and remote IRR (bit 14) are read-only. Ignore them on write and store
    them as 0 (APM U14; PPR "writes are ignored").
  - While virtual SVR bit 8 = 0, force bit 16 in the stored value.
  - Message type (bits 10:8) for thermal/perf/error/LINT: 000 fixed, 010 SMI,
    100 NMI, 111 ExtINT. Error, thermal and perf allow only fixed, SMI and NMI
    (APM Table 16-1 p628). Any other encoding is a stopped unsupported refusal
    (U3: the manual does not say fault versus ignore).
- Initial count 838: bits 63:32 set is #GP. Otherwise store and mirror.
- Divide 83E: bits 63:4 or bit 2 set is #GP. Otherwise store and mirror (Table 16-3 p638).
- Current-count read 839: return the physical 839 value, i.e. the mirrored timer.
  PPR p43: counts in steps of 1-8.

### D3. Physical LVT and timer mirroring

The physical LAPIC stays host-owned; guest-visible values live in the backing page.
"Mirror" means writing the validated guest value to the same physical MSR, with
the D2 read-only bits cleared.

- Force the physical mask when virtual SVR bit 8 = 0, or when the entry is an
  unmasked fixed type with a vector below 16. The latter is an illegal-vector APIC
  error (APM p635), not a #GP. The virtual APIC never delivers it; record that
  virtual ESR is not modeled.
- Per register:
  - Timer: mirror.
  - Thermal, perf, error:
    - Masked: mirror.
    - Fixed: mirror. The IRQ bridge captures the physical interrupt at the guest's
      vector and publishes it.
    - NMI: mirror. Physical NMIs are not intercepted and reach the guest in guest mode.
      Record the existing limitation that an NMI arriving during a host GIF window
      is terminal.
    - Unmasked SMI: stopped refusal. With HWCR SmmLock set, SMIs are not intercepted
      (PPR p204).
  - LINT0 and LINT1:
    - Masked: mirror.
    - Unmasked NMI: mirror. The captured MADT has exactly one Local APIC NMI entry
      (UID 0xFF, LINT1, active-high, edge), so Windows is expected to program it on
      every CPU.
    - Unmasked fixed: mirror. A level trigger sets physical TMR, so the bridge holds
      it as a level source.
    - Unmasked ExtINT: stopped refusal. The vector comes from the 8259 and sets no
      physical ISR, so the bridge's ISR check cannot own it.
    - Unmasked SMI: stopped refusal.
  - Initial count and divide: mirror exactly.
  - SVR: the physical SVR stays host-owned at 0x1FF. On a virtual software-disable,
    write masked values to all mirrored physical LVTs.

### D4. APIC_BASE (1Bh) shadow

Sources: apm-x2apic Q2 (Fig 16-2 p630, Table 16-5 p655, Fig 16-32 p656); ppr Q8 (p121).

- Read: return the shadow, i.e. the physical value captured at arm, which must be
  enabled x2APIC at FEE00000h.
- Write:
  - #GP if any of bits 63:52, bits above the admitted physical width, bit 9, or
    bits 7:0 are set.
  - BSC (bit 8) is read-only; ignore the written bit (U7 decision).
  - AE:EXTD = 11 with an unchanged base: complete as a no-op.
  - 01, or 10 from 11: #GP.
  - 00 from 11: the APM allows it, but the exclusive profile does not support it.
    Stop with the requested value (the existing 0xf511 style) and record it as a
    documented deviation.
  - 11 with a different base: stopped unsupported refusal (U7).

### D5. IPIs

- Hardware delivers fixed edge IPIs (physical, logical, self and broadcast
  shorthands) and SELF IPI.
- AVIC_INCOMPLETE_IPI (401h) is a trap: the ICR write is complete, RIP has already
  advanced, and nRIP must not be used.
  - EXITINFO1 = ICR (destination 63:32, low 31:0).
  - EXITINFO2: ID in 63:32, index in 11:0.
- ICR reserved bits (31:20, 17:16, 13:12) or reserved x2APIC message types (1, 3, 7):
  #GP is impossible after completion, so stop with the ICR value.
- Handling by ID (APM Tables 15-25..27 pp580-581; step order 15.29.6.1 pp576-577):
  - **ID 0:**
    - Message type 5 (INIT) or 6 (STARTUP): existing `route_x2avic_startup` mailbox.
      Only shorthand 00 or 11 is allowed (Table 16-4 p644).
    - Fixed with level trigger, NMI, SMI, lowest priority, ExtINT: explicit stopped
      unsupported refusal carrying the ICR. An NMI-IPI stop is useful bugcheck evidence.
    - Fixed edge: software fan-out.
  - **ID 1 (target not running):** hardware already set IRR everywhere and doorbelled
    the running targets. This runtime never clears IsRunning, so ID 1 means
    inconsistent table state: stopped refusal. Never republish, since that would
    double-deliver.
  - **ID 2 (invalid target):** hardware wrote nothing. Informative cross-check: Linux
    KVM avic.c, "falls over if _any_ targets are invalid". Do a full software fan-out.
  - **ID 3 (invalid backing page):** stopped refusal.
  - **ID 4 (vector below 16):** drop the IPI and record it. Virtual send-illegal-vector
    ESR is not modeled; informative KVM also drops.
  - **ID above 4:** stopped refusal.
- Software fan-out (fixed, edge):
  - A vector below 16 is dropped, as for ID 4.
  - Target set over the admitted inventory:
    - Shorthand 01: self. 10: all. 11: all except self.
    - Shorthand 00, physical mode: FFFFFFFF means all; otherwise ID == destination.
    - Shorthand 00, logical mode: FFFFFFFF means all; otherwise
      `(dest >> 16) == (ldr(id) >> 16) && (dest & ldr(id) & 0xffff) != 0`, where
      `ldr(id) = ((id >> 4) << 16) | (1 << (id & 15))` (APM p662 and p574).
  - An empty target set is dropped; send-accept error is not modeled (record it).
  - For each target, publish into its backing page (atomic IRR set, TMR cleared)
    through the D7 alias. A MixedTrigger result is a stopped refusal.
  - Ring the doorbell (D8) for every remote target. Do not ring for self; the next
    VMRUN evaluates IRR (p579).

### D6. EOI and level sources

- Edge EOIs stay accelerated.
- While the local `PhysicalIrqLedger` holds any level source, intercept EOI (80B)
  writes in this vCPU's MSRPM and emulate each one:
  1. A non-zero value is #GP.
  2. Clear the highest backing ISR bit and recompute PPR (`eoi_stopped`).
  3. If that vector is a held level source, mark it guest-completed, drain the
     physical EOIs, then clear its TMR bit unless it is pending again.
  4. Complete with nRIP.
- When the ledger becomes empty, restore EOI acceleration. Only the owning CPU
  changes its private MSRPM, and only while its guest is stopped.
- The 402h level-EOI exit stays as a fallback. Table 15-22 calls it a trap, 15.29.9.2
  calls it a fault (U11). EXITINFO2[7:0] carries the vector.
  1. If that ISR bit is still set and is the highest, do a software EOI; if it is
     already clear, leave ISR alone.
  2. If the ledger holds the vector, complete it and drain. Otherwise (stale TMR),
     clear TMR unless the vector is pending.
  3. Never advance RIP.
- Any other 402h exit means a profile mismatch: stopped refusal carrying EXITINFO.
- When a level source completes, also clear its now-stale TMR bit.

### D7. Remote backing aliases in each private host root

- New layout constant in `host/resident.rs`:
  `X2AVIC_BACKING_ALIASES_OFFSET = X2AVIC_TABLE_OFFSET - MAX_RESIDENT_CPUS * 4096` (0xd4000).
- Page `offset + s * 4096` maps slot s's backing page RW/NX for every existing slot.
  Pages for nonexistent slots stay absent.
- The image must end at or below `base + 0xd4000`:
  - runtime `prepare` bound;
  - DXE launch/activation bounds;
  - `payload.ld`: `ASSERT(image_bss_end <= 0x1d4000, "...")` with an accurate message.
  - The Phase A image ends at 0x17a000, so it fits.
- Backing PA of slot s = `pool_base + s * 1 MiB + (own backing VA - own image base)`.
  The linked offset is the same in every slot.
- DXE checks:
  - every directory's `avic_backing` matches that formula;
  - every alias PTE in every root is correct (existing `check_alias`);
  - bump `DIRECTORY_VERSION`.
- Runtime accessor: `remote_backing(slot)`, valid only after arm and for slot < count.

### D8. AVIC doorbell

- MSR C001_011Bh, write-only.
  - APM2 15.29.8.2 / Fig 15-22 p579: bits 7:0 = physical APIC ID, bits 63:8 MBZ.
  - PPR p216: bits 31:0 = ApicId, bits 63:32 reserved.
  - The two conflict. A value of 254 or less satisfies both.
- Enable condition: CPUID 8000_000A EDX[13], already admitted. Never read the MSR.
- Admission: every host APIC ID must be 254 or less. IDs of 255 or less were already
  required; 255 is excluded because the x2AVIC table entry 255 broadcast reservation
  is unresolved (APM U7).
- A doorbell to a core in host mode is undefined in the APM (U5). This design already
  depends on it being harmless, because hardware doorbells IsRunning targets.
  Informative: Linux KVM avic.c ("the spurious one is harmless"). Record it.
- Add an `AVIC_DOORBELL` constant in `arch::x86_64::msr` and one host primitive
  with a `# Safety` contract.

### D9. Guest INIT (replaces the startup stage-14 stop in `service_startup`)

**Preparation** (fallible, no side effects):
- `validate_x2avic` returns Init.
- Read-only check of the backing ID and version.
- The physical ISR contains only vectors held by the ledger, i.e. `next_eoi`
  would not fail.
- The EFER owner is startup-owned.
- Route and cache leases as today.

**Commit order:**
1. Reset the physical timer and mirrored LVTs:
   - timer LVT 0x10000, initial count 0, divide 0;
   - thermal, perf, LINT0, LINT1, error LVTs 0x10000.
2. Retire held level sources: mark every held vector guest-completed and drain the
   physical EOIs, bounded. A failure here is a terminal stop, because side effects
   have already happened.
3. Stop intercepting EOI; the ledger is now empty.
4. `BackingPage::reset_after_init_stopped()`.
   - Check its list against apm-x2apic Q3: TPR, APR and PPR 0; SVR FF; ISR, TMR and
     IRR 0; ESR 0; ICR 0; LVTs 0x10000; counts 0; divide 0; ID and version preserved.
   - LDR takes the derived value (U5 decision): APM p661 says LDR is initialized
     whenever x2APIC mode is enabled, and it stays enabled.
5. `NativeStartupTarget::apply_x2avic(Init)`. Confirm that V_TPR ends at 0 and the
   VMCB clean bits are invalidated.
6. `NativeEfer::reset_after_init()`.
7. Destination commit with `NativeDestinationCause::GuestInit`; the mode stays X2Apic.
8. `svmvisor_resident_reset_guest_debug()`. `build.py`'s audit requires this exact
   out-of-line helper to exist and be reachable.
9. `mailbox.complete(command)`.

**Also:**
- The APIC_BASE shadow is unchanged: INIT preserves AE, EXTD and base (APM p657).
- Record this deviation: a physical interrupt already pending in physical IRR at INIT
  is captured after the next VMRUN and published into the freshly reset page, where
  real INIT would have discarded it.
- AwaitSipi still polls on the host and never enters the guest.

### D10. Stays explicitly unsupported

Each of these is a stopped refusal with evidence, and must be documented:
- NMI, SMI, lowest-priority, ExtINT and level IPIs;
- guest APIC disable or relocation;
- unmasked SMI or ExtINT LVTs, and reserved LVT message types;
- host-mode NMIs during host GIF windows (existing terminal behavior);
- virtual ESR error generation;
- IOMMU GA posting;
- the HLT wake dependency: guest HLT runs natively with IsRunning=1, and waking relies
  on doorbell/INTR delivery to a halted guest (APM U16; informative KVM disable-HLT-exits
  plus AVIC).

## File ownership in the shared checkout (no worktrees)

- **B-core:**
  - `crates/hypervisor/src/svm/x2avic/**`
  - `crates/hypervisor/src/svm/permission_maps.rs`
  - `crates/hypervisor/src/arch/x86_64/apic.rs` and `msr.rs`
  - tests: `crates/hypervisor/tests/{x2avic*.rs, permission_maps.rs}` and new test files.
  - Keep every API that `runtime.rs` currently calls source-compatible (add, don't
    change), so `--features resident-runtime` keeps compiling for the other agents.
- **B-layout:**
  - `crates/hypervisor/src/host/resident.rs` (layout constants, directory, `valid_pool_slot`)
  - the `prepare()` function only in `crates/hypervisor/src/host/resident/runtime.rs`
  - `crates/dxe/src/native/resident/**`
  - `tools/native-resident/payload.ld`, plus `build.py` only if bounds live there
  - related DXE and layout tests.
- **B-integration** (after both): the rest of `runtime.rs`, `runtime.S`/`irq.S` if
  needed, `build.py` audits, removal of APIs superseded by the new owners.
- **Nobody in Phase B edits `docs/` or README files.** A later docs agent does.
- Use a private cargo target directory per agent (`--target-dir target/<agent>`) to
  avoid lock contention. A compile error in a file you do not own is the other
  agent's work in progress: wait and retry. Do not edit it.
