# Fixed multi-exit TCG integration fixture

This fixture boots the actual DXE PE and its linked native transition assembly in
QEMU 10.1.0 TCG. Its explicit TCG capability/provider fixture is separate from
native admission. Passing here does not claim native SVM success.

The production constructor places a 1,792-byte fixed program and constants in
one existing read/execute guest page. It makes 32 rounds, each containing CPUID
and QUERY VMMCALL, followed by one STOP VMMCALL: exactly 65 entries/exits and
64 resumptions. The 8 CPUID leaves are 0, 1, 0x40000000, 0x40000001, 0x80000000,
0x80000001, 0xdeadbeef and 0x40000002, each executed four times. Responses use
`crates/hypervisor/src/svm/emulation.rs` exactly; QUERY full RAX=0 returns ABI1 and
STOP full RAX=1 terminates without resume.

`guest.S` independently assembles the frozen `guest.bin`. A host test compares
all 1,792 constructor bytes with that binary and compares all CPUID table rows
with the core emulation implementation. CPUID input RAX/RCX/RBX has poisoned
upper halves. The guest compares full 64-bit RAX/RBX/RCX/RDX outputs, detecting
failure to zero-extend. RBP/RSI/RDI/R8..R14 carry distinct full-width sentinels;
R15 is the round number. A guest-produced payload in the existing writable page
records completed rounds at GPA0x8000 after both results validate and a final
cookie at GPA0x8008 only after all 32 rounds. The independent adapter requires
both payload values, final registers, counters and full restoration captures.

Run from the worktree, for example:

```powershell
./tools/native-transition-multi-test/run.ps1 -Case Success -Profile BoundaryAvx
```

Cases:

* Success: all 65 actual exits, 32/32 CPUID/query, 64 resumes, payload32/cookie,
  final STOP, successful scalar/xstate/extra-state restoration and direct canary.
* Unexpected: test-only QUERY instruction replaced by intercepted HLT after the
  first successful CPUID/resume; requires two exits and clean abnormal return.
* Mismatch: test-only QUERY opcode changed to 1<<32, retaining low EAX=0; requires
  rejection after two exits, proving full-width opcode comparison.
* BadMode: changes mode to3 after the otherwise valid fixture is constructed;
  requires refusal before any VMRUN and independent canary/outer preservation.

All negative mutations are guarded by native-transition-test-derived features;
that feature is incompatible with native-returning. Nothing in this runner
programs or reads physical hardware. Legacy one-entry, bind-only and event
fixtures remain available through their original runners.
