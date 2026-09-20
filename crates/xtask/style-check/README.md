# style-check

The gate for organization-only changes (CODINGSTYLE.md sections 13 and 14): a
change that claims to move, rename or re-home code without altering what it
does has to pass all of this.

Run everything from the repository root, in Git Bash. Python 3 is required.

| Command | What it does |
|---|---|
| `bash crates/xtask/style-check/verify.sh quick` | `cargo fmt --check` for all crates, then all 7 test configurations. About two minutes warm. |
| `bash crates/xtask/style-check/verify.sh full` | `quick`, plus the UEFI driver build, `cargo xtask resident`, and a comparison of the resident payload's disassembly against the baseline. |
| `bash crates/xtask/style-check/verify.sh baseline [REV]` | Builds the baseline disassembly from `REV` (default `a77f713`, the last commit before the style cleanup) in a temporary worktree and stores it in `target/style-baseline/`. Needed once per checkout; `target/` is not versioned. |
| `python crates/xtask/style-check/tokenbag.py BASE_REV NEW_REV` | Token-level difference of all Rust under `crates/` between two revisions, ignoring layout. |

Expected results for a pure move, rename or reorder:

- `verify.sh` ends with `VERIFY: OK`; the test counts are unchanged; the
  warning count stays at 2 (one unused import in
  `hypervisor/tests/svm_x2avic.rs`).
- The disassembly comparison prints `bodies only in base: 0   only in new: 0`.
  `asmdiff.py` hashes each function body with addresses removed and symbol
  names reduced to their final identifier, so moving a function between
  modules, reordering items and building in another checkout are invisible;
  a changed instruction, constant or call target is not.
- `tokenbag.py` reports only plumbing: `use`/`mod`/`pub(super)` tokens, path
  identifiers, layout punctuation, and the old and new spelling of whatever
  was renamed, in equal numbers. A literal, an operator or an unrelated
  identifier in its output is a finding.

Limits:

- The disassembly comparison covers the **resident payload** (the hypervisor
  core as linked by `cargo xtask resident`). `card-loader` and `launcher` driver
  code is not in it. For a change to either, build the affected UEFI feature
  sets with `--emit=asm` before and after and compare per-function bodies.
- A constant whose value changes but is never used by the payload is caught by
  `tokenbag.py` and the tests, not by the disassembly.
- None of this replaces booting the card.
