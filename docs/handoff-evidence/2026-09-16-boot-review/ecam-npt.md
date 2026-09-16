# ECAM and GPA aperture review - 2026-09-16

Scope: user claims 2 and 3 only. Read-only code review, rendered AMD manual review,
existing focused tests and current Windows device-resource observation. No source
change, no programming, no activation, and no new physical guest boot.

## Claim 2: CPL0 ECAM writes necessarily have NPF.US=0 - contradicted

AMD APM Volume 2, publication 24593 revision 3.44, March 2026;
SHA256 3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c.
Applicable to the ordinary, unencrypted native SVM/NPT profile. Rendered images
were viewed directly from docs/24593_3.44_APM_Vol2.pdf at 1.4x, stored in ecam-pages/.

Reference chain visually read in full:
- Section 15.25.1, printed 547-549 / PDF indices 608-610: two translations,
  guest-physical inputs to NPT, and separate guest and nested roots.
- Sections 15.25.3-15.25.5, printed 550-551 / indices 611-612: NPT uses host
  paging mode. Guest-page-table accesses are user writes at nested level (subject
  to ROGPT). Crucially the next paragraph expressly says the walk for the final
  guest page itself is ALWAYS treated as a user access at the nested page-table
  level, with data read/write or code read selected by the guest access.
- Section 15.25.6, printed 551-552 / indices 612-613: NPF error fields,
  EXITINFO1[33:32] distinguishes final GPA versus guest-page-table translation.
  US is nested-level access, not a copy of guest CPL. The user's reading stops
  one paragraph too late and misses the explicit final-page rule on p551.
- Section 15.25.7, printed 552-553 / indices 613-614: guest and nested write
  permissions combine; guest CR0.WP cannot bypass a read-only nested page.
  Guest accesses require user-permitted NPT pages even for supervisor guests.
- p552 references 15.36.10. Printed 607-609 / indices 668-670 were inspected:
  RMP/VMPL checks apply when SEV-SNP is enabled globally. The native admission
  rejects active encryption rather than implementing those alternate semantics;
  this review does not assert correctness for SEV-SNP or shadow-stack overrides.
  The optional decode-assist reference is not used: this path retries hardware,
  and does not decode or emulate the guest instruction.

Source: runtime.rs handle_diagnostic_ecam currently requires
`exit.info1 & 0x1f == 7`: present protection fault, write, nested user, no reserved
bit fault, no instruction fetch. That qualification is consistent with the
reviewed ordinary NPT rule for kernel data stores. DO NOT remove the US check or
replace it with a guest-CPL rule to fix the claimed bug. No runtime patch needed.

Existing lifetime policy: each private NPT initially protects the admitted ECAM
aperture. On a qualified write the owner takes the shared publication guard,
permanently revokes diagnostic publication, restores ECAM write permissions in
this CPU's tables and requests a full nested-TLB flush. It returns without
changing RIP/GPR/event state; hardware retries the same guest store. Other CPUs
retain their local trap until they take this same path. endpoint() remains
available after shared revocation, so those later traps can still be recognized.
The saved card transport may stop producing records at this point; that is an
observation limitation, not proof the guest stopped. This is not a proof of all
ECAM handling or every supported event-overlap case.

## Claim 3: identity aperture is only 40 bits - true limitation, no shown blocker

IdentityNpt::new uses min(physical_bits,40) and deliberately maps at most 1 TiB.
The 48-bit CPU capability establishes encoding width; it does not assert that
this platform uses memory or BAR addresses across all 48 bits. Above-4G devices
can remain inside the 1-TiB aperture. The firmware-map owner and final activation
validate the complete supplied map at min(physical_bits,40) and refuse any
out-of-range descriptor before running. MMIO omitted from that map or reassigned
later above the limit remains unsupported and can cause a terminal NPF.

Read-only current Windows resource evidence is saved in
current-windows-memory-resources.json. Host VOJTECH, Ryzen 9 9900X, processor ID
178BFBFF00B40F40, BIOS F7. All 36 Win32_DeviceMemoryAddress records end below
0x10000000000. Highest endpoint is 0xFC1FFFFFFF (range starts 0xFC10000000),
which is below 1 TiB despite being well above 4 GiB. This snapshot is not an
exhaustive physical aperture survey or a guarantee of next-boot placement.
It supplies no current device-resource evidence for the alleged >1-TiB blocker.

No speculative expansion made: the existing fixed eight-table identity storage
and stopped low-MTRR copy rely on this bound. A direct full48-bit map with 1-GiB
leaves needs 512 PDPTs plus root and split tables, requiring a reviewed resident
layout/allocation redesign. Changing just min(40) would be incorrect.

## Executed validation

- cargo test --locked -p svmvisor-hypervisor --test identity_npt: 10 passed.
  Includes every admitted width32..52, exact aperture edge, exclusion boundaries,
  ECAM permission restoration preserving LAPIC holes/neighbors, and transactional
  refusal on corrupt tables.
- cargo test --locked -p svmvisor-dxe --features native-preflight --test native_resident_memory:
  5 passed. Includes any firmware descriptor >=1TiB refusing before NPT mutation,
  monitor ownership, and final-rebuild LAPIC protection.

No new tests added to mirror unchanged implementation. No physical timing,
Windows guest boot, Hyper-V/VBS/HVCI compatibility, or containment measurement was
performed. Existing tests establish serialized table/admission behavior, not CPU
TLB behavior or physical ECAM accesses. The already flashed HWCR image is unchanged
by this review and remains awaiting its first boot.
