# Memory Attribute Protocol component

This `no_std` crate implements Get, Set and Clear protection semantics over an
independently qualified backend. `crates/dxe/src/memory_attributes.rs` exposes the
standard EFI function table through the default-off `memory-attribute-provider`
feature. No protocol is installed automatically and no native admission rule is
changed.

The interface is defined by [UEFI 2.11 section 37.7.1](https://uefi.org/specs/UEFI/2.11/37_Secure_Technologies.html#efi-memory-attribute-protocol).
The compatibility reference is EDK2 commit
`82cfea329cc2214df006edc067ed852f4d86a314`, specifically
`UefiCpuPkg/CpuDxe/CpuPageTable.c` and
`MdePkg/Include/Protocol/MemoryAttribute.h`.

`Provider<M>` implements `Attributes` using `M: Memory`. The backend must supply
safe physical table reads and isolated transactions with rollback and all-CPU
publication. The complete requirements are on the `Memory` trait; implementing
it with unchecked physical-pointer dereferences would not satisfy them. The host
tests supply an owned in-memory backend and an independent observation walker.

## Implemented profile

- Four-level, unencrypted x86 identity mappings, physical width 32 through 52,
  below the low canonical limit, and 4-KiB-aligned nonempty ranges.
- RP, XP and RO, including all seven nonzero combinations. Set adds restrictions;
  Clear removes requested restrictions. Mixed Get returns `NoMapping`.
- Nonzero nonpresent mappings retain their metadata and support Clear(RP).
- Whole 2-MiB/1-GiB leaves can be edited directly. Partial changes split only
  when necessary and preserve physical offsets, PAT, cache, U/S, A/D, global and
  software bits.
- Parent restrictions contribute to Get. Partial Clear transfers lifted parent
  restrictions to siblings so permissions outside the requested range survive.
- Invalid ranges and missing mappings fail before updates. An update failure
  aborts staged changes and allocations. No-op changes allocate nothing.

## Deliberate differences from literal EDK2 copying

EDK2's validation order is retained, including its invalid-parameter status for
zero/unknown update masks and unsupported status for misalignment. These details
are an implementation compatibility choice, not all prescribed by UEFI.

This implementation checks unsigned range overflow (`InvalidParameter`), rejects
nonidentity/unsupported encodings, includes parent permissions, preserves split
flags, and rejects XP changes when NXE is inactive. These avoid legacy reference
assumptions or deficiencies. It scans the entire requested range before returning
mixed-permission `NoMapping`, so a later malformed/missing entry can take priority.

Every call has a 131,072-entry-operation resource budget. Exceeding it returns
`OutOfResources`, rolling back an update. Five-level tables, encryption/shared
bits and protection keys are outside this profile. Zero parent entries are
absent; a zero final PTE is accepted only at physical address zero. Updates that
would collapse a retained nonleaf reference into raw zero return `Unsupported`.
There is no large-page coalescing.

The ABI adapter rejects null/unaligned pointers, leaves Get output unchanged on
failure, and returns `AccessDenied` on concurrent/reentrant use of one instance.
Other invalid raw pointers remain a caller-contract violation. Keep the adapter
pinned and alive, obtain `This` using `protocol_ptr()`, and quiesce callers before
unregistration/destruction. A backend must not panic across the EFI ABI.

## Verification

From the workspace root:

```text
cargo test -p svmvisor-memory-attributes
cargo test -p svmvisor-dxe --lib --test memory_attributes --features memory-attribute-provider
cargo check -p svmvisor-memory-attributes -p svmvisor-dxe --lib --features memory-attribute-provider --target x86_64-unknown-uefi
```

These validate software behavior and UEFI compilation. They do not establish a
safe live firmware backend, CPU synchronization, protocol installation or a
successful hypervisor boot.
