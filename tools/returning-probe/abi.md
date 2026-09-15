# Returning probe assembly ABI — emulator only

`probe_entry(Context *)` is a **void** EFI x64 / Microsoft x64 ABI function.

RCX contains a writable, exclusive 560-byte context. Results are written to that

context; RAX is restored to its incoming value and is not a return status. The

outer wrapper preserves all fifteen non-stack GPRs, RSP, RFLAGS, and the caller's

selected extended state. No Rust, firmware, allocator or external C call occurs

between entry and return. Local helper calls are assembly-private interfaces.

There is no SEH unwind path. Only the narrowly allowlisted injected host-fault
checkpoints described below can recover synchronous host exceptions.

This is narrowly qualified by the companion disposable QEMU 10.1 EFI app. It

is not linked into the physical card's installed image or the existing terminal

payload. A successful emulator return does not authorize native execution.

## Preconditions owned by the Rust admission code

All addresses in the context must be identity-mapped, exclusively owned and

valid for the complete operation. VMCB/HSAVE/extra-context pages are 4 KiB aligned;

all extended-state buffers are 64-byte aligned and separately initialized with

zero reserved/header storage. Original, observed, guest and outer-caller areas

must not alias. The guest page tables/NPT/code/stack/descriptors must already be

valid, and guest memory must remain private for this fixed probe.

The application must reject anything outside its single-CPU, inactive-SVM,

known-GIF-set emulator fixture. In particular it must establish:

- Supported SVM/NPT, exact original `VM_HSAVE_PA=0` and `EFER.SVME=0`, and exclusive

  same-CPU ownership; no migration or firmware service while the wrapper runs.

- CR0.EM/TS clear, EFER.FFXSR clear, CR4.OSFXSR set, and an appropriate long-mode

  firmware GDT. The wrapper does not temporarily normalize these controls.

- Either the qualified FXSAVE64 profile (`mask=0`) or original enabled XCR0

  exactly 3/7 with OSXSAVE already enabled and CPUID-enumerated standard layout.

  XCR0 is never rewritten. Reject unsupported supervisor/XSS/XFD state and

  unqualified advanced control modes rather than shrinking the enabled profile.

- No active debug breakpoints/GD, single-step/VM flags, incompatible segments,

  pending asynchronous condition, or unsupported original control state.

- The guest intercepts dangerous instructions, cannot modify the fixed XCR0 or

  host resources, and will perform exactly the included fixed VMMCALL sequence.

These are prerequisites, not proof supplied by `entry.S`. The wrapper relies on

the application for rejection before unsafe instruction entry. Neither GIF

readback nor NMI/SMI/native exception containment is implemented here.

## Context offsets

Every field is an unsigned 64-bit value or pointer. Rust must assert offsets

and total size, independently of this table.

| Offset | Meaning |
| --- | --- |
| 0 | Guest VMCB physical address |
| 8 | Host extra-state VMSAVE page address |
| 16 | Independent observed host-extra VMSAVE page address |
| 24 | Temporary HSAVE physical page address |
| 32 | Inner original/canary-host extended-state buffer |
| 40 | Independent observed host extended-state buffer |
| 48 | Guest extended-state buffer, restored before entry; normal exits save here, post-exit fault profiles preserve this source and save at 536 |
| 56 | Save profile: 0 = FXSAVE64/FXRSTOR64; 3 or 7 = standard XSAVE64/XRSTOR64 mask |
| 64 | Negative-control flags: bit 0 omit first inner host xstate restore; bit 1 corrupt saved inner RBX |
| 72, 80, 88, 96 | Original CR0, CR2, CR3, CR4 |
| 104, 112, 120, 128 | Original EFER, VM_HSAVE_PA, DR7, RFLAGS |
| 136, 144, 152, 160 | Observed CR0, CR2, CR3, CR4 |
| 168, 176, 184, 192 | Observed EFER, VM_HSAVE_PA, DR7, RFLAGS |
| 200, 208 | VMCB EXITCODE and guest RAX |
| 216 | Outer ABI failures: bit 0 nonvolatile GPR mismatch; bit 1 XMM6–15 mismatch; bit 2 returned RFLAGS mismatch; bit 3 upper-YMM mismatch |
| 224, 232 | Original/observed XCR0; zero placeholder in the FX profile |
| 240, 248 | Zero placeholders only: fixture rejects XSAVES/XSS; no XSS MSR read occurs |
| 256 | True outer firmware caller's extended-state buffer |
| 264 | Reserved, unused |
| 272, 280 | Original/observed DR6 |
| 288–399 | Fourteen guest GPR qwords: RCX, RDX, RBX, RBP, RSI, RDI, R8…R15 |
| 400 | Private IDT page address; used only for requested host-fault injection |
| 408–423, 424–439 | Original/observed IDTR: 10-byte SIDT image plus six zero padding bytes each |
| 440 | Host fault kind: 0 none; 1/2 pre-mutation UD/GP; 3 pre-mutation mismatch; 4/5 post-binding UD/GP; 6 post-binding mismatch; 7/8 post-extra-load UD/GP; 9 post-extra-load mismatch; 10/11 post-xstate-load UD/GP; 12 post-xstate-load mismatch; 13/14 post-successful-exit UD/GP; 15 post-successful-exit mismatch |
| 448, 456, 464 | Actual host vector, fault RIP, normalized error code |
| 472 | Accepted recovery count, exactly one for an allowlisted injected fault |
| 480 | Host stage: 0 unarmed, 1 private IDT armed, 2 handler accepted, 3 original IDTR restored, 4 abort cleanup completed |
| 488 | VMRUN instruction attempts: one for guest tests and post-successful-exit host faults, zero for earlier host recovery checkpoints |
| 496, 504 | Actual checkpoint RDMSR observations: EFER and VM_HSAVE_PA; zero for pre-mutation tests |
| 512 | Mutation stage: 0 no SVM mutation, 1 checkpoint fully bound, 2 original HSAVE/GIF/EFER cleanup completed; 3 host-derived guest extra loaded and captured; 4 post-extra-load restoration completed; 5 guest xstate loaded and independently captured; 6 post-xstate-load restoration completed; 7 successful guest exit and captures completed; 8 post-exit restoration completed |
| 520 | Actual checkpoint RFLAGS from PUSHFQ; IF must be clear for mutated-state tests |
| 528 | Separate independent checkpoint-extra VMSAVE page; used for post-extra-load, post-xstate-load and post-successful-exit tests |
| 536 | Separate independent checkpoint xstate page; post-xstate-load and post-successful-exit tests |
| 544 | Assembly-captured guest RIP after VMRUN return; zero before entry |
| 552 | Post-exit capture stage: 0 absent, 1 guest GPR/extra/xstate captures completed |

Saved control observations describe the actual caller values. The inner

original extended-state image intentionally contains the outer wrapper's

nonvolatile XMM canaries. The separate buffer at offset 256 retains the real

firmware caller image for final restoration. Do not compare those two buffers

as if they were the same capture.

## Entry, observation and return order

The outer wrapper immediately saves RFLAGS and all GPRs. It reserves 32 bytes

of Microsoft ABI shadow space plus an eight-byte context slot, keeping the

stack 16-byte aligned before calling the inner wrapper. Its flags slot is

160 bytes above that local RSP. It saves actual caller extended state before

executing any SIMD instruction, then seeds all eight nonvolatile GPRs and

XMM6–15 with distinct fixed canaries. It restores incoming flags immediately

before calling `probe_inner`.

The inner wrapper saves all GPRs/flags and retains one context pointer. Its

flags slot is at RSP+128. It independently observes original CR0/CR2/CR3/CR4,

EFER/HSAVE/DR6/DR7 and applicable XCR0, then captures the canary-host xstate.

It executes CLI, enables SVME, and executes CLGI. It uses VMSAVE into a distinct

host-extra page before installing the temporary HSAVE binding and VMLOADing

that fixed guest's extra state. The saved host-extra page is never interpreted

as the opaque hardware HSAVE page.

It restores guest xstate and sets fixed guest GPR values, then performs one

VMRUN. Immediately after VMEXIT, fourteen pushes capture guest GPRs before

scratch use; their frame is copied to the context. The retained context lies

at RSP+112 during those pushes. Guest RAX comes from the VMCB, not the host's

VMRUN RAX. VMSAVE captures the guest's additional state, then XSAVE/FXSAVE

captures guest vectors before any host SIMD work.

VMLOAD restores the host-extra page. A second VMSAVE into the observed page

provides independent hardware evidence. The inner host xstate is restored,

independently saved into the observed xstate buffer, and restored again so even

the omission fixture is repaired before firmware can resume. CR2, DR6, DR7 and

original HSAVE binding are restored explicitly. CR2 is not assumed to be

restored automatically by VMEXIT. CR0/CR3/CR4 were not changed in software;

observations after VMEXIT check their restored values.

With host context restored and IF still clear, STGI restores the fixture's known

original GIF. Original EFER is restored only after STGI while the instruction is

still legal. Control/MSR readbacks are retained. Original flags are restored and

independently read back, then only integer pops and RET remain in the inner

wrapper. The outer wrapper captures actual returned RFLAGS immediately after CALL, before

any flag-changing instruction, and overwrites offset 192 with this independent

observation. It checks those flags and actual returned GPR/vector canaries, records

failures, restores the actual firmware caller xstate as its last SIMD/FPU

operation, restores GPRs/flags/stack and returns.

## Fixed guest and negative controls

`guest_code_start` through `guest_code_end` delimit a position-independent blob.

It initializes x87 and loads one value, clears XMM0/XMM15 and (only when original

XCR0 enables AVX) YMM15, writes `0x51554d5552455455` to `[RSP-16]`, and invokes

VMMCALL with that same RAX. `guest_vmmcall` identifies the expected intercepted

RIP. A trailing UD2 prevents accidental continuation from appearing successful.

The ordinary guest GPR seeds are also retained independently on exit.

Separate `guest_ud_start/fault/end` and `guest_pf_start/fault/end` blobs write

the same pre-fault marker to `[RSP-24]`, then execute UD2 or a nonpresent linear

`0x200000` read at the named fault label. Each has a later `[RSP-16]` write that

must remain unexecuted. These are intercepted guest exceptions, not a host

exception recovery mechanism.

Negative flag 1 deliberately skips the *first* inner host xstate restore, so

the independent observed buffer captures the guest's changed state. The correct

original image is then restored unconditionally before STGI and firmware return.

Negative flag 2 zeros the inner saved RBX at RSP+96, causing a real return-value

canary mismatch in the outer caller. The outer saved true firmware RBX is still

restored. These omissions must be reported as expected failures, never as clean

passes. No negative fixture intentionally returns corrupted state to firmware.

## Limits of the evidence

Byte comparisons of whole save buffers are conservative fixture checks, not a

general XSAVE architectural equivalence relation. Reserved/unwritten bytes,

MXCSR_MASK, inactive component payloads, and conditional x87 FIP/FDP/FOP require

separate native qualification. The current AMD reference documents conditional

exception-pointer behavior; these tests do not establish native preservation

of unmasked exceptions or every processor state component.

VMRUN/VMEXIT can reload descriptors from the firmware GDT. A fault in that

transition may cause shutdown. There is no general recoverable host fault path,

full NMI/SMI stress qualification, arbitrary OS continuation, multicore support,

or physical device execution here. The runner's bounded emulator process and

failure logs contain test failures; they are not a native recovery mechanism.

Architectural references are the pinned AMD APM vol.2 rev.3.44, §15.5.2

(VMSAVE/VMLOAD), §15.6 (VMEXIT), §15.17 (GIF), §15.25 (CR2), and chapters 11/18

(extended-state save/restore), as collected in

`docs/reversible-svm-probe-plan.md`.

## Explicit enclosing XSAVE fixture

`fixture_entry(Fixture *)` in `src/fixture.S` is a separate void EFI x64 assembly

boundary enclosing a complete EFI-ABI callback. It exists because the fixed

OVMF boot environment can have CR4.OSXSAVE clear. The native

`FirmwareXstatePlan` continues to reject that original environment; acceptance

inside the configured callback describes **the emulator fixture**, not proof

that firmware originally supported that XCR0 profile. The fixed FX path still

calls the existing probe directly.

The preflight `cpu::fixture_supported(mask)` performs read-only admission after

exact TCG/AMD guards. It checks XSAVE support, selected component layout,

unsupported XSAVES/supervisor/dynamic features, and the admitted original control

state. When original OSXSAVE is already enabled it also rejects an unsupported

or non-subset original XCR0 immediately. When OSXSAVE is off, only the enclosing

assembly temporarily enables it to discover XCR0; no separate Rust save/restore

helpers are used. A native-policy hypervisor-refusal test remains a rejection-

ordering test using the actual hypervisor-present bit and illustrative other

capability fields, not a complete runtime capability inventory.

The fixture object contains twenty qwords (160 bytes):

| Offset | Meaning |
| --- | --- |
| 0, 8 | EFI callback address and opaque argument pointer |
| 16 | Requested fixture XCR0, exactly 3 or 7 |
| 24, 32 | Original FXSAVE64 buffer and original standard XSAVE64 buffer |
| 40, 48 | Independent observed FXSAVE64 and XSAVE64 buffers |
| 56 | Status: 0 callback completed and restoration path ran; 1 original profile refused before callback |
| 64, 72 | Original CR4 and actual original XCR0 |
| 80, 88 | Observed restored CR4 and XCR0 |
| 96, 104 | Original and restored/readback RFLAGS |
| 112 | Callback ABI failure bits: 0 GPR, 1 XMM6–15, 2 callback CR4/XCR0 mismatch |
| 120, 128 | Original/observed CR0 |
| 136, 144 | Original/observed EFER |
| 152 | Reserved |

Allocate and clear four separately owned 4 KiB, 64-byte-aligned buffers before

entry. Each callback must return normally, including expected negative tests;

result reporting through emulator exit or firmware-service checks occurs only

after this outer fixture returns. There is no unwind path if a callback traps

or diverges.

The fixture saves all GPRs/flags and always FXSAVEs the true caller before any

SIMD. This is necessary even if original XCR0 is 1: firmware can have live legacy

XMM state that an XSAVE mask of 1 would not preserve. It enables only OSXSAVE,

reads actual original XCR0, and accepts original masks 1/3/7 only when they are

subsets of the requested fixture. Otherwise it restores original CR4 without

issuing XSETBV, changing SIMD state, or calling the callback. It never silently

reduces preexisting enabled components.

For an accepted original profile, it XSAVEs every original enabled component

before setting target XCR0. It then seeds nonvolatile GPR/XMM canaries and calls

the complete Rust callback with original IF/DF semantics. The callback operates

with the explicitly configured profile and must preserve the EFI nonvolatile

register contract. Arbitrary Rust code cannot retain live variables across a

separate stale-state restoration call because restoration occurs only as the

last part of this enclosing assembly call.

After callback return, canaries and configured CR4/XCR0 are checked. The fixture

restores original XCR0, XRSTORs original enabled extended state, and FXRSTORs the

original legacy image, including SSE when original XCR0 bit 1 was clear. It

independently XSAVEs/FXSAVEs into the observed buffers. A final FXRSTOR is the

last SIMD/FPU operation; original CR4, flags and GPRs are restored before RET.

Original/observed XSAVE comparisons must use **original** XCR0, including 1,

with the available component layout. Independent legacy comparisons use the

FXSAVE images. Zero padding is not a substitute for semantic component checks.

For target 7, `probe_entry` seeds all sixteen YMM registers with a 32-byte canary

whose upper halves are nonzero. The inner independent save verifies their

preservation, and the outer wrapper additionally checks returned upper halves

before restoring the true caller. Upper YMM registers are not assumed to be

nonvolatile across the complete Rust callback: the fixture saves/restores the

true caller's enabled state explicitly, whereas its callback ABI canary checks

cover the Microsoft ABI's required low XMM6–15 halves.

The SSE/AVX omission tests skip the first inner host restore and must fail its

independent observed-state comparison; actual caller state is still repaired.

All other limitations above remain, including original exception-pointer

qualification, no general host-fault recovery beyond the exact checkpoints
below, and no native launch readiness claim.

## Bounded synchronous host-fault recovery

The `host-ud` and `host-gp` emulator profiles intentionally take real host #UD

and #GP exceptions at named assembly instruction addresses. This window is

**after** original/canary-host xstate capture but **before** any SVME, HSAVE,

VMLOAD/VMSAVE, GIF, or guest-entry mutation. It returns by the pre-entry abort

path with zero VMRUN attempts. It does not qualify faults during VMRUN, VMEXIT,

MSR/descriptor transitions, or arbitrary instructions. Successful host recovery

sessions must not be reported as SVM entries.

The Rust adapter allocates a separate 4 KiB private IDT and fills all 256

interrupt gates with the current firmware CS selector, IST zero and type 0x8e.

Only vectors 6 and 13 target their specific assembly stubs; every other vector

targets the terminal unexpected-fault stub. No firmware service or compiler

callback executes while this IDT is installed. CLI excludes maskable interrupts

from this short window; NMI/SMI/general asynchronous containment is not claimed.

The assembly saves the original IDTR with SIDT and arms stage 1 immediately

before LIDT. Host UD executes UD2 at `host_fault_ud_pc`. Host GP reads through RAX
at noncanonical 48-bit address `0x0000800000000000` at `host_fault_gp_pc`;
LA57 is rejected by admission and the operand uses DS rather than SS. AMD APM
vol.2 section 8.2.14, Table 8-6 specifies #GP(0) for this access. No control
register write or memory access completes. In the QEMU 10.1 smoke test, returned
CR2 was observed to contain that address; the precise cause was not established.
Abort cleanup explicitly restores original CR2, DR6 and DR7 before independent
readback. This is not a claim that architectural #GP updates CR2. It does not assume that
the injected #GP leaves fault/debug state unchanged. There is no fall-through success path if either instruction
unexpectedly completes.

AMD64 exception frames contain SS/RSP even at unchanged privilege. The UD stub

adds a synthetic zero error code; the GP stub retains the CPU-pushed error code.

Both add a vector slot and enter a compiler-free common handler, which saves

its only scratch registers RAX/RDX. The resulting offsets are vector +16,

error +24 and RIP +32. IRETQ restores the hardware-saved stack after the handler

removes its scratch and normalized vector/error slots. Hardware-added RF in a

fault frame is not assumed to equal the incoming firmware flags.

The handler requires host stage 1, no previous recovery, zero entry attempts, the

exact requested vector and fixed instruction RIP, and error code zero. It

records actual vector/RIP/error, sets recovery count 1 and stage 2, and changes

only the hardware return RIP to `host_fault_recovered_pc`. That continuation

restores the original IDTR before any common cleanup, independently reads it

back, restores and recaptures host xstate, and enters the original control/

flag/ABI readback path. It sets stage 4 only after reaching that abort cleanup.

SVME/HSAVE/GIF and guest memory were not touched, so this path does not manufacture

VMLOAD/VMSAVE restoration evidence. Rust independently SIDTs before and after

the returned assembly call and checks its own original table against both

assembly observations before freeing the private IDT.

`host-fault-mismatch` executes the same UD2 at the correct fixed site while the

allowlist expects vector 13. Its vector check must fail before recovery. The

unexpected handler prints exactly `FAIL host-fault-unexpected`, exits QEMU using

debug-exit value 17 (process exit status 35), and halts if the emulator does not

terminate. It never resumes firmware or reports a recovered state, normal SVM

entry, BootServices success, or PASS. This test exercises the reject path; it

is not restoration evidence.

The additional `armed-host-ud` and `armed-host-gp` profiles use a second, distinct
checkpoint. The assembly first executes CLI, enables SVME, executes CLGI,
VMSAVEs original host extra state, and writes the owned temporary HSAVE binding.
It records actual EFER/HSAVE RDMSR results and PUSHFQ flags before setting
mutation stage 1 and installing the private IDT. The fixed fault instructions
are `host_armed_ud_pc` (UD2) and `host_armed_gp_pc` (the same noncanonical DS
read described above). No guest VMLOAD, guest xstate restore, or VMRUN occurs.

This allowlist additionally requires mutation stage 1, checkpoint EFER exactly
`original EFER | 0x1000`, checkpoint HSAVE equal to the owned temporary page,
and checkpoint IF clear. Rust independently requires that the temporary page
differs from original HSAVE. The `armed-host-fault-mismatch` profile executes
UD2 at the correct armed UD site but expects vector 13, exercising the terminal
vector-rejection branch after the binding has actually changed. A terminal
mismatch does not restore firmware and is never restoration evidence.

After an accepted post-binding fault, the common continuation first restores
and reads back original IDTR. It VMSAVEs independently observed host extra state;
no host VMLOAD is necessary because guest extra state was never loaded. It
restores, independently captures, and repairs host xstate and restores original
CR2/DR6/DR7. Then it writes original HSAVE, executes STGI while SVME remains set
and IF remains clear, and restores original EFER. Only after these operations
does it set mutation stage 2 and enter shared independent control/flag/ABI
readback. Final host stage 4 and recovery count 1 must accompany mutation stage
2, original EFER/HSAVE equality, unchanged selected VMSAVE fields and zero VMRUN
attempts. STGI completion is instruction-progress evidence under the known-GIF
fixture assumption; GIF is not independently readable here.

The private IDT covers only the established injection window. Partial SVM setup
and cleanup exceptions are outside this recovery contract. In particular, this
is not recovery from arbitrary faults after enabling SVM, from guest VMLOAD,
or from VMRUN/VMEXIT. There are no firmware calls during setup, injection, or
cleanup. Restoring the original IDTR before STGI is part of the required order.

The normal SVM route without an injected post-exit fault never installs this private IDT. Its attempt counter is

incremented immediately before the VMRUN instruction with guest R11 preserved.

The host-abort route never reaches that increment. Existing guest #UD/#PF tests

remain guest exceptions intercepted through the VMCB; they are distinct from

these actual host IDT events.

The protocol relies on a known valid host stack and mapped context at the exact

injection sites. It provides no arbitrary stack unwinding, IST-based recovery,

double-fault recovery, or recovery from failure while loading/restoring IDTR.

The native physical card and native launch gates are unchanged.


## Recovery after a completed host-derived guest extra-state load

The `loaded-host-ud`, `loaded-host-gp` and `loaded-host-fault-mismatch` profiles
add a third exact checkpoint. After saving the original host extra state and
binding the temporary HSAVE page, assembly copies only VMSAVE-defined byte ranges
`0x440..0x460`, `0x470..0x480`, `0x490..0x4a0`, and `0x600..0x640` from the actual
host capture into the guest VMCB. It changes only LSTAR at `0x608` to
`0x12345000` and CSTAR at `0x610` to `0x23456000`. These canonical syscall targets
are never executed. Rust requires each to differ from its original host value.
This deliberately preserves host FS/GS, TR/LDTR and other extra state while
handling host exceptions; it does not test loading arbitrary guest descriptors
into an active host fault environment.

VMLOAD installs this host-derived synthetic guest extra state. A VMSAVE into
the separate page at context offset 528 captures the actual loaded fields,
independently of both the source VMCB and final restored-host observation.
Rust requires both changed fields to match their selected values, and requires
all other selected fields to match the original host. Actual EFER, HSAVE and
RFLAGS observations then precede mutation stage 3 and private-IDT installation.
The fixed fault sites are `host_loaded_ud_pc` and `host_loaded_gp_pc`, using the
same UD2/noncanonical DS read injectors as earlier profiles. The allowlist
requires exact kind, stage 3, vector, instruction RIP, zero error, no previous
recovery and zero VMRUN attempts, plus exact checkpoint EFER/HSAVE and IF clear.
Kind 9 executes UD2 but expects vector 13 and must terminate without recovery.

After accepted recovery, the continuation restores and observes original IDTR,
VMLOADs the original host extra state, and independently VMSAVEs the restored
host into its distinct observed page. It restores and observes host xstate,
restores CR2/DR6/DR7 and original HSAVE, executes STGI with IF clear and SVME
still enabled, and restores original EFER. Mutation stage 4 is written only
after these cleanup instructions. Existing stage 1-to-2 binding-only cleanup
retains its independent VMSAVE without an unnecessary host VMLOAD. The earlier
unmutated fault path also remains unchanged.

No guest xstate restoration or VMRUN occurs in this new checkpoint. This tests
faults after a completed VMLOAD and checkpoint capture, not faults in VMLOAD,
VMSAVE, partial setup, cleanup, guest xstate restoration or the VMRUN/VMEXIT
boundary. It retains the prior stack, asynchronous-event and emulator-only
limitations. No physical launch readiness or new flash candidate is implied.


## Recovery after completed guest xstate restoration

The `xstate-host-ud`, `xstate-host-gp` and `xstate-host-fault-mismatch`
profiles extend the host-derived extra-state fixture with a fourth exact
checkpoint. Kinds 10/11/12 reuse the defined extra-state copy, canonical syscall
target canaries, VMLOAD and independent checkpoint VMSAVE. They then restore the
separate guest xstate image at context offset 48 with FXRSTOR64 or XRSTOR64 and
immediately save actual live state to the separately allocated page at offset
536. Between restoration and that capture there are only integer operations and
assembly-private helper calls; no Rust, firmware, SIMD or FPU work intervenes.
Neither the guest source image nor the original host snapshot is overwritten.

The guest image contains non-initial XMM15 qwords `0x13579bdf2468ace0` and
`0x0fedcba987654321` in every profile. The AVX profile additionally contains
upper-YMM15 qwords `0x1122334455667788` and `0x8877665544332211` and marks SSE/AVX
components present in XSTATE_BV. Rust requires each checkpoint canary to equal
its expected value, differ from the original host, and return exactly to the
original host value in the independent restored observation. Whole selected
component semantic comparisons additionally require checkpoint/source equality
and restored/original equality. These checks qualify the selected vector state;
they do not claim all advanced xstate components or pending x87 exceptions.

Only after the restore, independent save, EFER/HSAVE observations and PUSHFQ does
assembly set mutation stage 5 and install the private IDT. Fixed fault sites
`host_xstate_ud_pc` and `host_xstate_gp_pc` use the prior real UD2 and noncanonical
DS-read injectors. The handler requires exact kind, stage 5, vector, fault RIP,
zero error, zero previous recoveries and zero VMRUN attempts, plus the actual
checkpoint control checks. Its body uses only integer registers and IRETQ.
Kind 12 executes UD2 but expects vector 13 and must terminate without recovery.

Accepted recovery restores and observes original IDTR, VMLOADs original host
extra state and independently VMSAVEs it, then restores, independently captures
and repairs original host xstate. Original CR2/DR6/DR7 and HSAVE follow, then STGI
with IF clear and SVME enabled, then original EFER. Mutation stage 6 is written
only after those operations. The previous stage 3-to-4 extra-state path and
stage 1-to-2 binding path retain their existing behavior.

This checkpoint still executes zero VMRUN instructions. Its private IDT is not
installed during guest XRSTOR/FXRSTOR or the independent checkpoint save, so
faults in those instructions remain outside this contract. Partial setup,
cleanup, arbitrary exceptions, VM entry/exit, NMI/SMI, physical SVM and Windows
guest execution remain unqualified. This is the completed guest-state window
before entry, not general entry-boundary fault containment.


## Recovery after a successful guest exit

The `post-exit-host-ud`, `post-exit-host-gp` and
`post-exit-host-fault-mismatch` profiles use kinds 13/14/15. They execute exactly
one VMRUN per session using the existing normal VMMCALL guest. Unlike the earlier
zero-entry loaded-state fixtures, they retain the prepared, validated guest
TR/LDTR fields. Copying firmware TR/LDTR into the VMCB could invalidate guest
entry and is deliberately excluded. Only host FS/GS and syscall/SYSENTER fields
are copied, with the same canonical LSTAR/CSTAR canaries as above.

The original firmware IDT remains installed throughout guest VMLOAD, xstate
restoration, VMRUN, VMEXIT and immediate capture. Assembly first pushes all
fourteen live guest GPRs, recovers context R10 from the known stack frame, and
copies that frame into the context. VMSAVE captures guest extra state into the
VMCB and independent page at offset 528. The VMCB EXITCODE, guest RAX and RIP are
captured at offsets 200, 208 and 544. The independent xstate save goes to offset
536; the initial guest image at offset 48 is preserved for this profile. Only
integer operations and VMSAVE intervene before the xstate capture. No host
VMLOAD, host xstate restore, compiler callback or vector work occurs first.

Capture stage 552 becomes 1 after all captures. Exactly one attempted entry and
EXITCODE 0x81 (VMMCALL) are required before assembly observes checkpoint controls
and sets mutation stage 7. INVALID entry and intercepted guest exceptions cannot
arm this window. Returned-session Rust checks additionally require the normal
guest VMMCALL RIP, RAX/stack sentinel, full expected guest GPRs and changed captured
state. The normal guest zeros XMM15 and, for AVX, upper YMM15 and executes FLD1;
these actual captured values are checked against guest behavior and independent
original/restored host observations. Source image bytes alone are not evidence
of guest execution. The two syscall-target captures must equal their canaries,
differ from original host values and restore exactly. Guest extra captures
retain valid guest TR/LDTR rather than claiming those fields equal host state.

Only then does assembly install the private IDT and execute the actual UD2 at
`host_post_exit_ud_pc` or noncanonical DS read at `host_post_exit_gp_pc`.
The integer-only handler requires host stage 1, mutation stage 7, capture stage
1, exactly one VMRUN attempt, exit 0x81, zero previous recovery, checkpoint
EFER/HSAVE equality and IF clear, plus exact kind/vector/RIP/error-zero matching.
Context R10 has already been restored before this private IDT becomes live.

Kind 15 executes the exact post-exit UD2 while the contract expects GP. Only after
all common post-exit guards and the actual vector 6, exact UD site and zero error
checks does it print `FAIL post-exit-fault-mismatch` and terminate with exit 35.
Other unexpected faults retain `FAIL host-fault-unexpected`. The unique negative
marker therefore proves this specific post-exit rejection; it never claims
recovery or host restoration and never reaches firmware or a PASS marker.

An accepted fault resumes the common continuation: restore and independently
observe original IDTR, VMLOAD original host extra state, VMSAVE restored extra
state, restore/capture/repair host xstate, restore CR2/DR6/DR7 and original HSAVE,
execute STGI while IF is clear and SVME remains enabled, then restore original
EFER. Only afterward is mutation stage 8 set. Independent control, flags, ABI,
IDTR and post-return Boot Services observations remain required.

Pinned AMD APM Volume 2 revision 3.44 section 15.6 describes automatic host CS,
SS, DS and ES reload and host consistency checks at VMEXIT. Segment reload
exceptions and invalid host consistency cause shutdown; this private IDT cannot
recover them. Host segments must use the GDT since guest LDTR survives exit.
This fixture uses same-CPL interrupt gates with IST zero and an already valid
host stack, so the deliberate faults do not use the still-active guest TSS.
It does not qualify task switches, other privilege levels, asynchronous events,
NMI/SMI, arbitrary descriptors, VMRUN instruction faults, guest-entry rejection,
VMEXIT transition failures, capture/setup/cleanup faults or native execution.
Opaque HSAVE contents are neither interpreted nor changed. No physical card,
flash or Windows launch readiness follows from this bounded emulator result.
