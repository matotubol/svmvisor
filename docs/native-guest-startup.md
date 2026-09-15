# Native guest INIT/SIPI restart — 2026-09-14

The opt-in diagnostic driver now preempts already-running AP guests, applies
guest INIT on the destination CPU, waits for guest SIPI, and executes the supplied
real16 → protected32 → long64 continuation. Every AP restarts twice on the
2-, 24- and 32-CPU pinned emulator profiles. The private resident hosts survive
both generations. These are actual concurrent native resident runtimes, not
the earlier cooperative synthetic CPU fixture.

This is a restricted post-EBS diagnostic interrupt profile. It does not yet
integrate an unmodified Windows loader or normal Windows interrupt traffic.
No physical launch, firmware programming, reboot, Windows/ESP modification or
protection change was performed. Earlier physical evidence applies only to its
exact older returning image.

## Ownership and source changes

`svm::ipi` owns a 64-byte, cache-line-aligned mailbox per admitted CPU. Readiness
is published by the destination after its actual native bootstrap ACK. A bounded
four-entry FIFO preserves accepted INIT/SIPI ordering across producers and the
sole destination consumer. A producer validates the stopped instruction, state,
destination and queue before publishing; publication is the final fallible
step. It then unconditionally issues a host notification and completes the
source instruction. No physical INIT/SIPI is sent to an active resident host.
Concurrent producer/consumer tests exercise 4,096 accepted requests without loss
or per-producer reordering. A full or contended queue refuses before completion.

The destination alone owns its VMCB, register frame and lifecycle. There is no
Cold relabeling: Running → AwaitSipi → Running is explicit, and repeated INIT is
supported. INIT resets the specified architectural CPU state while preserving
INIT-retained debug/auxiliary registers and CR0.CD/NW. Extended state stays live
on its owning physical CPU; the persistent host remains integer-only. SIPI
sets CS.base to vector << 12 and IP to zero. Duplicate SIPI does not restart a
runnable target. AwaitSipi uses bounded stopped-host polling, not guest execution
at the reset vector or physical host HLT.

Directory ABI version **4** adds the explicit startup-profile argument to
ArmRuntime. BridgeContext is **128 bytes**, with the host-interrupt flag at
offset112. The shared page at pool base + FE000h is mapped RW/NX at the matching
local slot alias in every host root. Every guest NPT excludes the whole monitor
pool. DXE checks the shared translation and keeps private payload memory below
that page; version3 and overlapping directories are rejected. All mailbox
construction occurs before any resident activation.

The explicit consumer reserves host vector F1 and masks its LAPIC sources after
successful EBS return. The runtime admits the supported nonextended x2APIC
layout, zero guest interrupt state and masked inactive LVT entries. The private
F1 gate checks the actual ISR bit before EOI. Host IF=1 with GIF clear at entry,
physical INTR interception and V_INTR_MASKING permit notification even while
the guest executes CLI. Host dispatch opens GIF only in the bounded private
acknowledgment window. Requests arriving after the final queue drain still
have an unconditional physical notification.

Guest SVR/ICR readback is owned separately from the physical host notification
state. INIT reports guest SVR=FFh, ICR=0 and TPR=0 while the host keeps its own
LAPIC notification path enabled. Source programming that could reclaim or mask
the host notification is intercepted and refused. Pending guest interrupts,
extended LAPIC configurations and other physical interrupt sources remain
unsupported. Quiescent INIT admission checks read-only LVT delivery/remote-IRR
status as well as the masked bit; a host-only F1 arrival is preserved across
the reset preparation race.

Native EFER and instruction fetch have separate opt-in startup support for
legacy unpaged execution. Fetch uses CS-relative addresses, checks the entire
opcode and following IP against segment bounds, and qualifies low RAM with
fixed-MTRR precedence. Legacy paging and IP wrap remain refused. EFER keeps its
logical SVME bit separate from SVM backing and validates hardware-derived LMA
after the guest changes CR0. Failed preparation does not update that logical
observation. The ordinary native profile retains its previous mode restrictions.

## Validation

Frozen evidence belongs under `work/native-guest-startup/summary.json`; build
manifests identify exact source, binaries and complete linked-code audits.
Failed attempts are retained separately. Host tests pass **365 core + 282 DXE =
647**. Diagnostic payload audit covers **8,703 instructions**; production audit
covers **7,385**, with no unowned FP/SIMD/xstate instructions or undefined symbols.
The production image is built and audited only.

Final executed guest-startup driver SHA256:
`98c2022be8bbe64cebd957876019219e130393e27d85e5b5229eefbe8bc3ca8a`.
All ten final scenarios pass. The three 2/24/32-CPU positive runs establish
110 executed guest AP restarts in total; the reserved-F1 refusal run adds two
successful restarts before its intentional terminal stop.

For each final 2/24/32-CPU run, every AP has its own retained low startup page.
The guest itself records reset GPR/control/RFLAGS values, logical EFER, APIC
readback, a resident CPUID witness, and independent real/long-mode generation
counters. It then increments a progress word in a loop with no intentional VM
exits. The second INIT therefore requires actual running-target preemption.
The parser requires each AP's two generation results, exact INIT/SIPI counts,
actual physical-INTR records, full initial activation and successful process
exit. GetTime and identity SetVirtualAddressMap remain exercised before restart.

Additional final scenarios cover reserved-F1 write refusal after successful
restarts, the prior ordinary SMP activation profile, ordinary single-CPU runtime
services on both previous and corrected backends, and unsupported CPU/mode
admission. A terminal refusal requires the exact stopped reason and absence of
post-write continuation, followed by watchdog termination; timeout alone is
never a passing result.

The backend is unchanged from the previous activation batch:
QEMU SHA256 `07c6409a119ea0d48e880fe55e0ad004812aeff6242823e9975287a8f3147451`.
The ordinary single-CPU regression also uses predecessor SHA256
`c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047`.
OVMF SHA256 remains
`33090cc07675baa5190d9f1e84bf5176b33bcbfa9bacac522961150cdb6dbb2a`.

Fresh output example:

```powershell
python tools/native-resident/build.py --output work/new-startup-build --test-output --guest-startup
python tools/native-resident/run.py --output work/new-startup-run --driver work/new-startup-build/driver.efi --features guest-startup,virtual-map --cpus 24 --init-preserving-backend --timeout 60
```

## Remaining Windows-first work

Normal native IRQ routing and xAPIC startup are still needed. This diagnostic
reserves F1, refuses unsupported guest APIC accesses and does not provide general
INIT handling with pending device interrupts. Self, logical and broadcast
startup destinations, external INIT, CPU offline/rebind and unrestricted reset
remain unsupported. These limits must be resolved or explicitly admitted before
using the mechanism in an ordinary Windows boot.

The loader still explicitly calls the post-successful-EBS entry. Bootstrap low
LoaderCode pages and original firmware paging structures remain retained by the
diagnostic consumer until reset. Normal loader memory lifetime and nonidentity
runtime mapping, actual-machine CPU/encryption/routing admission, and protected
Windows compatibility remain open.

No native timing baseline or calibrated startup/notification latency was
measured. TLB_CONTROL still flushes every entry. There is no live AP xstate,
debug, PAT or cache-retention sentinel test; pure state tests and the existing
integer-only boundary support only their stated contracts. Malware-analysis
features and general device emulation remain deferred.

Primary reference: AMD APM volume2 publication24593 rev3.44, March2026,
Tables14-1/2, 16-2/4 and §§14.6,15.13.1,15.16,15.27.8,16.11/16.13.
Reviewed library PDF SHA256:
`3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
The duplicate-SIPI bounded policy retains the earlier MPspec1.4 AppendixB.4.2
cross-check; the cited AMD startup paragraph does not explicitly settle it.
