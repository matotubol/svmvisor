# Independent IO/startup review - 2026-09-16 (second batch)

Scope: dirty native_diagnostic_config.rs, diagnostic_runtime.rs, ipi.rs and
native_guest_startup.rs tests, with native runtime caller inspection. Read-only
review; no source edits. No blocking correctness finding in this bounded change.

## Reviewed behavior

- Scalar IO owner admits the complete byte-span overlap with CF8-CFF, including
  operations beginning before CF8. It preserves original starting port, width
  and value. OUT revokes diagnostic publication first except the ordinary CF8
  DWORD selector; IN completion preserves high bits for AL/AX and zero-extends
  EAX. String/REP and non-CPL0/non-long64 remain explicit unsupported stops.
- Live owning-CPU HWCR20 is read before revoke or physical IN/OUT. When set,
  the owner queues #GP(0), keeps faulting RIP/GPR/RFLAGS and marks pending_fault.
  This prevents forwarding an access which would #GP inside the monitor.
  The helper does not manufacture an exception for an unsupported policy case.
- INIT/SIPI shorthand11 ignores DM/destination and selects every owned identity
  except the source. All recipients must remain admitted/ready because physical
  notification broadcasts. The complete ordered slice binds one route gate.
- Every selected FIFO is read and capacity-checked before any queue store.
  Early refusal leaves queues, source VMCB/GPRs and ICR shadow unchanged.
  Native runtime producers hold that gate; service_startup holds it across
  target apply and queue completion. Its earlier peek is observational and does
  not apply state before the gate. Publication releases the guard before kick.
  Source instruction/ICR completion follows successful publication and kick.
- Scalar IO and guest startup remain native forwarding/target-owned reset,
  without new shadow owners, unbounded allocation or remote VMCB mutation.

## Manual verification

Read full rendered pages, including tables/notes, from local supplied PNGs:
- APM2 publication24593 rev3.44 March2026, SHA256
  3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c.
  Section15.10.1-.3 printed516-517/PDFidx577-578: IOPM checks every operand byte;
  IOPL/TSS/VM86 exceptions precede interception, others follow; EXITINFO2 provides
  next RIP. REP/string crossreference is outside admitted scalar scope.
  Section16.5 and Table16-4 printed643-644/PDFidx704-705: INIT vector0, shorthand
  and DM rules, valid INIT/SIPI all-excluding-self destination. Complete merged
  trigger/level rows and don't-care note inspected.
- PPR57896 rev3.00 Aug28 2024, Family1Ah Model44h B0, SHA256
  643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5.
  HWCR complete register table printed203-204/PDFidx202-203: bit20 #GP applies
  to scalar IO if any byte is in CF8-CFF; does not apply string/REP.
  Followed its IO-trap reference to full printed208/PDFidx207 (rendered here as
  pages/ppr-207-review.png): GP priority over SMI IO trap confirmed, including
  IO_TRAP_CTL_STS.IoTrapEn definition. No IO-trap/SMM emulation added or claimed.

## Validation executed

cargo test --locked -p svmvisor-hypervisor with selected test targets:
- native_diagnostic_config:6 passed.
- native_guest_startup:11 passed.
- native_destination_routing:11 passed.
- native_init_deassert_independent:3 passed.
- native_xapic_startup:9 passed.
Total40 passed. Includes new busy-last-target broadcast refusal with no partial
publication, successful INIT/SIPI broadcasts, ignored destination/DM and source
exclusion; existing route-lock and unchanged-state refusal checks remain green.

## Limits / nonblocking follow-up

- New broadcast test exercises x2APIC adapter; direct shorthand11 through xAPIC
  and 32-recipient boundary are not explicitly tested by these selected suites.
- Generic NativeStartupMailbox publish/complete APIs support standalone lock-free
  fixture transport. They do not enforce the route gate. Current native callers
  are correct, but any future native caller must join the same gate or staged
  whole-queue stores could overwrite a concurrent ungated update. Do not claim
  broadcast atomicity against arbitrary generic API callers.
- handle_native_x2apic_startup_access's introductory doc comment still says only
  explicit physical remote assignments; update it to name shorthand11 too.
- Pure tests do not establish real guest IOIO overlap exits, silicon HWCR20
  exception priority, physical AP startup/reset behavior, or Windows boot.
  No timing, Hyper-V/VBS/HVCI or sandbox containment measurement was performed.
  Physical CF9 forwarding may reset the machine exactly as the guest requested;
  no hardware reset or programming was executed in this review.
