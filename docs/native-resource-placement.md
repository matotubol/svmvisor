# Resident resource preparation on the first-boot platform

The first physical resident candidate reached its Rust entry, but returned
EFI_OUT_OF_RESOURCES before arming the ExitBootServices hook. USER2 snapshot
`target/firmware/squirrel/snapshots/7078a0b4dafb465a9b50a19fc984b755`
matches FPGA968cbbd8710939f4 and ROMf56dfa81fa7121ea. The user reported a normal
Windows desktop; that boot did not activate this resident hypervisor.

The old allocator requests RuntimeServicesCode with AnyPages, then rejects a
pool ending above1GiB. Its small-code-model relocations, host roots and bounded
NPT exclusion depend on this limit. A2GiB emulator run of the previous diagnostic
build reproduced `Iabcde[Address]F[OUT_OF_RESOURCES]`. Its generic rejection
fixture expected a different status and therefore reports failure; this retained
run is a reproduction, not a passing boot test. The physical record cannot yet
prove which resource operation failed.

The explicit `--boot --low-runtime` platform profile requests the full runtime
reservation with AllocateMaxAddress=0x3fffffff. On24CPUs it requests26MiB, trims
alignment slack and retains24MiB. It verifies the complete returned reservation
and the existing slot/map/access admission. Firmware failure propagates; there
is no memory-type substitution or unchecked physical-address fallback. Generic
builds retain AnyPages. UEFI2.11 §7.2.1 printed155 specifically qualifies its
AnyPages requirement to drivers not targeted for a specific implementation.
This opt-in is for the existing-machine experiment, not general portability.

Boot-options version2 keeps the128byte ABI and adds preparation stage, reason,
underlying status and address. Version1 input remains accepted without changing
its reserved fields. Parent journal phase0x14/detail8 exports the failure stage,
reason, ordinary EFI status and a48bit address; values not representable use the
existing full-width status record. Successful activation still uses phase0x13,
stage5. Neither a successful flash nor stage5 alone proves a Windows desktop.

No VM-exit semantics, runtime timing policy or guest protections change in this
batch. Hyper-V/VBS, containment and timing fidelity remain outside the measured
first-boot result. Current evidence lives under
`work/native-resource-fix-2026-09-14`; older candidate evidence remains unchanged.
