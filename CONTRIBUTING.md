# Development rules

The current executable sequence is [docs/first-light-plan.md](docs/first-light-plan.md).
The full [bare-metal roadmap](docs/minimal-baremetal-bringup-roadmap.md) is a
later-gate specification, not the current checklist.

## Source boundaries

- `crates/dxe` owns UEFI allocation, protocols, policy, and lifecycle events.
- `crates/hypervisor` is the `no_std` post-firmware CPU and VM-exit runtime.
- `crates/m0b-probe` is the frozen removable-media inventory application.
- `firmware/squirrel/rtl` owns first-party RTL; every accepted endpoint must be
  completion-only.
- `tools` owns host-side preparation, verification, packaging, and diagnostics.
- Add a directory or module only with its first real implementation and tests.

## Privileged-code rules

- Put privileged and MMIO `unsafe` code in small architecture-specific modules.
  Every public unsafe operation needs a precise `# Safety` contract and a
  normative section or table citation.
- Use validated address and identifier types at trust boundaries. Do not pass
  untyped physical addresses as ordinary integers between modules.
- Keep allocation, formatting, firmware calls, floating point, and unbounded
  work out of persistent host and VM-exit paths.
- Serialize cross-language records field by field and assert every size, offset,
  byte order, and reserved bit in host and RTL tests.
- Cite normative AMD, UEFI, ACPI, PCI, and TCG sources in code. Use EDK II,
  Linux KVM, bhyve, and research hypervisors only as informative cross-checks.
- Pin reference documents and exemplar commits in evidence manifests; a moving
  example never overrides a specification.

## Later introspection boundary

Guest-memory inspection remains a later, separately authorized pipeline:

```text
captured guest CPU state
    -> bounded guest-physical reader
    -> guest page-table walker
    -> exact-build Windows decoder
    -> consistency/snapshot policy
    -> authenticated, versioned client protocol
```

The card, BAR0, option-ROM aperture, and FT601 must never become an arbitrary
host-memory requester or guest-controlled command path. A future transport needs
its own ADR, threat model, authorization, audit, and hard address-range policy.
