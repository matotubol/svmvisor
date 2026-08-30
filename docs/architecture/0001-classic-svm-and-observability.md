# ADR 0001: classic SVM scope and observability claim

- Status: accepted for bring-up
- Date: 2026-08-06
- Applies through: the complete v1 roadmap and any v1 introspection work

## Context

The long-term research goal is to inspect memory belonging to processes in a
controlled Windows guest without installing an in-guest agent. A later client
may consume that data. The immediate problem is smaller: prove that one exact
AMD lab platform can safely support a recoverable classic-SVM hypervisor.

The guest can observe effects of virtualization through architectural details,
timing, firmware and TPM evidence, device topology, and implementation bugs.
Consequently, "the malware cannot tell" is not a testable absolute requirement.

## Decision

1. Version 1 uses classic AMD SVM with VMCBs and NPT. It makes no SEV, SEV-ES,
   SEV-SNP, VMPL, confidential-memory, or confidential-attestation claim.
2. Bring-up uses only trusted EFI applications, WinPE, and a cloned Windows
   installation. Untrusted kernel code is out of scope until the exact frozen
   platform and build pass both the Milestone 11 and Milestone 12 gates.
3. The detection objective is limited to **agentless, out-of-guest observation
   with a minimized and measured guest-visible footprint**. Undetectability is
   neither promised nor used as a pass gate.
4. Process discovery, Windows virtual-address translation, snapshot consistency,
   and the client transport form a later introspection plane. They remain outside
   the VM-exit hot path and are not implemented during hardware bring-up.
5. Every accepted future Squirrel bitstream is a completion-only delivery and
   diagnostic endpoint. The current transitional integration is requester-capable,
   is not accepted, and must not be flashed. No accepted design becomes a
   host-memory requester or general guest-to-host command channel in order to
   export future inspection data. FT601 remains inactive throughout v1. A
   bounded outbound-only transport, if eventually needed, is post-v1 work that
   requires a new ADR, threat model, and milestone sequence.
6. Before privileged implementation, every run is bound to a versioned target
   profile. Unknown platform facts remain blocking unknowns; code must not infer
   that missing evidence is safe.

## Consequences

- The read-only target/evidence prelude is complete and frozen. The current code
  deliverable is the recovery-gated, completion-only, record-only first-light
  path; `VMRUN` remains a later authorization boundary.
- Guest-memory acquisition will later be split into independently testable
  layers: guest-physical access, guest page-table translation, build-bound
  Windows decoding, snapshot policy, and a separate client protocol.
- Windows kernel structure offsets will never be guessed or treated as a stable
  ABI. A decoder must bind to the exact Windows build and matching symbol/PDB
  identity, validate its invariants, and fail closed on mismatch.
- Guest-observability and behavioral-compatibility measurements can be added only
  as explicit tests. They cannot falsify TPM/event logs, attestation, or platform
  identity, and cannot weaken recovery, isolation, or completion-only invariants.

## References and provenance

- Normative: [local AMD64 APM Volume 2 rev. 3.44 PDF](../24593_3.44_APM_Vol2.pdf),
  SHA-256 `3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c`.
- Derivative navigation aid: [local Chapter 5: Page Translation and Protection](../amd64_apm_vol2_markdown/chapters/05-page-translation-and-protection.md),
  especially Sections 5.1 and 5.3.
- Derivative navigation aid: [local Chapter 15: Secure Virtual Machine](../amd64_apm_vol2_markdown/chapters/15-secure-virtual-machine.md),
  especially Sections 15.5, 15.16, and 15.25.
- Derivative transcription aid: [verified local VMCB layout appendix](../Appendix_B_VMCB_Layout_verified.md).
- Normative external, not locally pinned: [UEFI Specification 2.11](https://uefi.org/specs/UEFI/2.11/).
- Informative: [Microsoft symbol documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/symbols)
  and [Microsoft `!vtop` semantics](https://learn.microsoft.com/en-us/windows-hardware/drivers/debuggercmds/-vtop).
- Informative project plan: [bring-up roadmap](../minimal-baremetal-bringup-roadmap.md).
