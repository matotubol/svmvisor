# Current source investigation handoff

Stopped at the user's request to commit current work and transfer to another AI.
No production source edits, builds or hardware operations were performed in
this investigation. `source-before` preserves the four inspected cache/runtime/
diagnostic source files and their SHA256 manifest.

## Confirmed physical boundary

Exact flashed candidate `34f1d747f8cb47689ba4a06a8646c621`, combined SHA256
`903eb3f397cbd34390bec6e250490fa815de029c5c821cdf07cdf44e4ec9510b`;
FPGA/ROM IDs `27ddef70674522cc` / `f2e3abb0b289c024`, boot ID `1875948883`.

The decoded header reports stage5/all24 CPUs activated before loader return.
BSP first-fault bank32 is event3, sequence47302, guest RIP
`fffff8077db5fd56`, exit code `0x7c`, reason `0xf400`, detail `0x10`,
exit count26051. Bank0 is a later terminal-barrier checkpoint, sequence47303:
expected and initially acknowledged masks `0x00ffffff`, current acknowledged
mask1, owner1, outcome0. These records do not contain the failed MSR index,
requested value, previous value or cache-owner phase.

The post-EBS admission correction is therefore physically past its previous
failure boundary on this image. This does not establish Windows boot success.

## Exact source implication

`crates/hypervisor/src/host/resident/cache_runtime.rs` emits detail16 only after
the ordinary `CacheCoreState::write` path returns `CacheWriteError::Unsupported`.
It is not the E0/E1 transition refusal path and not the invalid-write `Fault`
branch, which injects guest #GP.

The current `crates/hypervisor/src/svm/native_cache.rs` implementation can return
Unsupported from that call for these distinct reasons:

1. The index is not provided by the owner read function.
2. SYS_CFG changes a field outside visibility bit19, plus bit18 only while
   owner phase is2 or3.
3. A fixed-MTRR merged value changes outside phase2/3.
4. A variable-MTRR value changes outside phase2/3.
5. Another owned field differs from its current value (routing, HWCR,
   MMCONFIG, and default-type fallback).

Consequently, reason/detail alone cannot identify a specific register or justify
a semantic change. No branch was weakened or new physical writer introduced.

## Next investigation

Map the reported RIP against the exact retained Windows image using its proven
KASLR base or corroborated instruction offsets; architecture reviewer was
examining retained BN6 data. The available typed analysis database is
`work/native-raw-result-2026-09-15/windows-review/analysis-copy.bndb` and the
prior read-only tracing helper is `work/native-fixed-mtrr-2026-09-15/windows-trace.py`.
Do not assume that a low-offset match alone proves the module/base or MSR.

Once the actual operation and operands are established, review their semantics
against the original AMD tables and current ownership model. If current evidence
cannot establish operands, preserve them at the actual refusal through the
existing checked diagnostic transport rather than guessing a register. Such a
diagnostic change has not been implemented here.

Preserve the exact physical capture under `capture`, earlier delivery reports,
and pre-change source snapshots. The committed implementation's complete build,
execution and offline packaging evidence remains under
`work/native-fixed-mtrr-2026-09-15`.
