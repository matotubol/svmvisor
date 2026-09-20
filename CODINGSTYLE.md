# svmvisor coding style

This codebase should read as if one person wrote all of it in one sitting.
That only happens when every recurring decision has exactly one answer, so
this document does not say "prefer": it says what we do. If a situation is not
covered, find the closest existing code that follows these rules, copy its
shape, and add the missing rule here.

The rules cover **organization, naming and layout**. They say nothing about
what the hypervisor does. Rule IDs (`F2`, `N7`, ...) exist so reviews and
commits can cite them.

Scope: all Rust under `crates/`. The `no_std` crates (`card-abi`,
`card-loader`, `hypervisor`, `launcher`, `resident-payload`,
`memory-attributes`) and the host tools (`xtask`, `rompack`) follow the same
style; rules marked **[no_std]** bind only the former.

Where these rules come from: the habits of a very consistent reference
codebase (BurntSushi's `jiff`), filtered down to what makes sense for a
heap-free, float-free, wire-ABI-heavy hypervisor, and settled toward whatever
form already dominates our own code so the cleanup moves the minority, not the
majority. Where we deliberately differ from `jiff`, it is noted.

---

## 0. The five principles

Everything below is a consequence of these.

1. **One answer.** Two ways to write the same thing is a defect even when both
   are fine. Pick the documented form.
2. **The path names the context, the identifier names the thing.**
   `svm::pause::retry_ready`, not `svm::native_pause::native_pause_retry_ready`.
3. **One home per fact.** A constant, register number, wire layout or helper
   is defined once, in the lowest crate that owns the concept, and imported
   everywhere else.
4. **A file is one responsibility, read top to bottom.** The same section
   order in every file; callers above callees; nothing after the tests.
5. **Hardware-facing and ABI-facing items are conspicuous.** Wire structs,
   exported symbols, `static mut`, asm and register numbers sit in predictable
   places, never buried mid-file.

---

## 1. Formatting (mechanical)

**F1.** `rustfmt` is the only formatter and its output is never hand-adjusted.
`rustfmt.toml` at the repository root is exactly:

```toml
edition = "2024"
max_width = 100
use_small_heuristics = "Max"
```

(Measured on this tree: width 100 grows the code by 1.5% with 33 unfittable
lines; width 79 grows it by 17% and still leaves 1,179 lines over, because of
register and constant names. `jiff` uses 79; we do not.)

**F2.** Every commit is `cargo fmt`-clean. The crate outside the
workspace is formatted explicitly:

```
cargo fmt --all
cargo fmt --manifest-path crates/resident-payload/Cargo.toml
```

**F3.** No hand-compressed code: no `fn f(a:u32,b:u64){`, no several
statements on one line, no missing spaces around operators. If rustfmt would
change it, it is wrong.

**F4.** `#[rustfmt::skip]` is allowed only on a table whose column alignment
carries meaning (register maps, opcode tables), with a one-line comment saying
so.

**F5.** Comments wrap at 100 columns like code. No divider banners
(`// -----`, `// =====`). Structure comes from modules, `impl` blocks and
section order, not from ASCII art.

**F6.** Numeric literals:

| Kind | Form | Example |
|---|---|---|
| Hex digits | lowercase | `0xc001_0131` |
| Hex longer than 4 digits | `_` every 4 digits from the right | `0x000f_ffff_ffff_f000` |
| Structure offsets | zero-padded to 3 digits | `0x070`, `0x3f0` |
| Decimal ≥ 10000 | `_` every 3 digits | `1_000_000` |
| Type suffix | no underscore | `0u64`, `1u32 << 23` |
| Single bits | shift form | `1 << 10`, not `0x400` |
| Multi-bit masks | hex | `0x0fff` |

---

## 2. Crates

**C1.** Dependency direction is fixed: `card-abi` is the leaf (it knows
nothing about UEFI or SVM); `hypervisor` depends on it and knows nothing about
UEFI; `card-loader` depends on `card-abi` only and knows nothing about SVM;
`launcher` depends on `card-abi` and `hypervisor` (and on `memory-attributes`
under its `memory-attribute-*` features); `resident-payload` depends on
`hypervisor`;
`xtask` depends on `card-abi`; `memory-attributes` and `rompack` stand alone.
A crate never reaches into another crate's source tree with `#[path]` or
`include!`. Shared code is shared through a Cargo dependency.

**C2.** `lib.rs` / `main.rs` contain, in this order and nothing else: the
`//!` crate doc, crate attributes, `compile_error!` feature guards, `use`
imports, `pub use` re-exports, `mod` declarations, and (for binaries) the entry point. No types,
no logic.

**C3.** Every `no_std` crate root carries `#![no_std]` and
`#![forbid(unsafe_op_in_unsafe_fn)]`. Lint levels live only in crate roots.
An `#[allow(...)]` anywhere else is on the narrowest item possible and has a
comment on the line above saying why.

**C4.** Cargo features are named `<area>-<noun>` in kebab case, grouped by
area prefix (`card-*` in `card-loader`; `native-*` and `memory-attribute-*` in
`launcher`), each with a `#` comment above it in `Cargo.toml` stating what it
selects. Mutually exclusive features are rejected by a `compile_error!` in the
crate root, never silently resolved.

**C5. [no_std]** No `alloc`, no floating point, no `std` behind a feature.
Anything that needs them belongs in `xtask` or `rompack`.

---

## 3. Modules and files

**M1.** A module with children is `foo/mod.rs`. Never `foo.rs` next to
`foo/`.

**M2.** There are two kinds of directory module, and `mod.rs` differs between
them:

- A **namespace** groups independent modules (`svm/`, `boot/`, `x2avic/`).
  Its `mod.rs` holds only the `//!` doc, `pub use` re-exports and `mod`
  declarations. Code lives in a named sibling file.
- A **unit** is one type or one stateful component split across files
  (`vmcb/`, `runtime/`). Its `mod.rs` holds the shared core the other files
  build on — the primary type with its fields and layout asserts, its
  constructor, the constants and statics the children *share* — and nothing
  else (a constant only one child uses lives in that child). Each
  child file adds one group of behavior (`impl Vmcb { .. }` for x2AVIC, for
  events, ...). Children reach the core through ordinary privacy (a child
  sees its ancestors' private items), so splitting a file into a unit widens
  no visibility.

Either way the directory's public paths are unchanged by a split: what was
`svm::vmcb::ReinjectOutcome` stays that, re-exported by name from `mod.rs`.

A unit is not nested inside another unit whose children call into it: the
inner unit's functions would have to be visible two levels up, which
`pub(super)` cannot express (V2).

Known debt from the 2026-09 splits: some `mod.rs` files carry private `use`
lines, marked with a comment, whose only purpose is to keep `super::…` paths
inside moved function bodies resolving. They go away when those bodies are
brought to I5.

**M3.** `mod` declarations are alphabetical, `pub mod` and `mod` interleaved,
each `#[cfg]` directly above the declaration it gates.

**M4.** A file is one *type family* or one *responsibility*: a primary type
with its companions (`Vmcb`, its intercept enums, `ReinjectOutcome`), or one
job (`dispatch`). Two independent things in one file is a split, whatever the
size. Two prompts to look for a second responsibility: more than ~800 lines
of non-test code, or an `impl` block whose methods fall into groups that never
call each other.

**M5.** A file has to earn its existence. No re-export-only files, no
one-function files: fold them into the file that owns the type they operate
on. A small file is fine when it is a cohesive unit (a wire struct with its
layout asserts).

**M6.** File and module names are singular lowercase nouns naming the primary
type or the job: `vmcb.rs` → `Vmcb`, `dispatch.rs`, `permission_maps.rs` is
plural only because the type family is. Forbidden names: `utils`, `helpers`,
`common`, `misc`, `types`, `prelude`, `core`, `base`, `stuff`.

**M7.** A name is never repeated along its own path. No `svm/native_cache.rs`
next to `svm/native_pause.rs`; no `host/resident/cache_runtime.rs` inside
`runtime`. A prefix shared by every sibling carries no information: delete it
or make it a directory.

**M8.** The module name equals the file name. A file is never mounted under a
different name (`#[path = "physical_boot.rs"] mod physical;` is wrong twice).

**M9.** The same basename means the same role everywhere. `descriptors.rs`
under `arch`, `boot` and `host` is fine because the path disambiguates. A mode
name (`returning`, `resident`) is not a role: the file that is the mode's own
entry keeps the name (`native/returning.rs`), and its supporting parts are
named for what they do (`delivery/child_image.rs`,
`diagnostics/returning_detail.rs`).

**M10.** `#[path]` is permitted for exactly one purpose: selecting between
cfg-gated implementations of one module that expose an identical API:

```rust
#[cfg(feature = "x")]
#[path = "enabled.rs"]
mod inner;
#[cfg(not(feature = "x"))]
#[path = "disabled.rs"]
mod inner;
pub(crate) use self::inner::*;
```

Every other use — mounting binary-only files from `main.rs`, mounting `src/`
files from `tests/`, mounting another crate's file — is debt. Do not add new
ones. Until they are removed, every file move or rename updates all mount
sites in the same commit (see §12).

**M11.** Each source file is compiled into exactly one place in exactly one
crate target. If both the binary and the tests need it, it belongs in the
library.

**M12.** Assembly (`.S`) files live in the crate whose Rust code declares
their symbols, beside the `.rs` file that declares them, sharing its basename
(`runtime.rs` ↔ `runtime.S`).

---

## 4. Layout of a source file

**L1.** Every file uses this order. Sections that do not apply are skipped;
sections never repeat or interleave.

```text
 1. //! module doc
 2. use imports                        (§5)
 3. pub use re-exports, then mod declarations      (mod.rs / crate roots)
 4. constants                          (const)
 5. statics                            (static, static mut, exported symbols)
 6. extern blocks and global_asm!
 7. the primary type
      its layout asserts               (const _: () = { ... };)
      its inherent impl(s)
      its trait impls
 8. companion types, each in the same shape as 7, most important first
 9. error types for this file          (§7)
10. free functions: public first, then private, callers above callees
11. #[cfg(test)] mod tests             — always last; nothing follows it
```

Placement of the things that list does not name:

- One blank line follows the `//!` module doc; inner attributes (`#![cfg(..)]`)
  come next, followed by another blank line.
- `type` aliases and inline modules that hold only constants
  (`pub mod outcome { pub const .. }`) belong to section 4. A `const _` assert
  about constants closes section 4; a `const _` assert about a type follows
  that type (L3). When one assert block covers two structs, it follows the
  second.
- "Public first" in section 10 means `pub`, then `pub(crate)`, then
  `pub(super)`, then private.
- A `#[cfg(test)]` stand-in for an `extern` block stays adjacent to that block
  (section 6), because the two are alternatives (G2); it is not a test module.

**L2.** Constants and statics each form one contiguous block at the top.
Never a `static` between two functions; never a `const` before the imports.
A constant used by one function only may be declared inside that function, as
its first statements.

**L3.** A wire struct's layout asserts follow the struct immediately — before
its `impl` — so the definition and its proof are read together.

**L4.** Inside an `impl` block:

```text
associated constants
constructors          new, empty, from_*, initialize
getters               bare names
predicates            is_*, has_*
setters / mutators    set_*, verbs
conversions           as_*, to_*, into_*
operations            the real work, in lifecycle order
```

`new` is the first function in the block. Always. Private helpers are not
collected at the end: a helper with one caller sits directly below it; a helper
with several callers sits below the last of them.

**L5.** A type with distinct method groups gets one `impl` block per group,
each introduced by a one-line comment naming the group (`// x2AVIC fields.`).
When the groups are large they become files of a directory module, each with
its own `impl Vmcb { ... }` (`vmcb/mod.rs`, `vmcb/x2avic.rs`,
`vmcb/events.rs`).

**L6.** Trait impls follow the inherent impls in this order: `unsafe impl
Send`/`Sync`, `Default`,
`Debug`, `Display`, `PartialEq`/`Eq`, `PartialOrd`/`Ord`, `From`, `TryFrom`,
operators, `Drop`. This holds for error types too (E4).

**L7.** Attribute order on an item: `#[cfg]`, `#[repr]`, `#[derive]`,
`#[unsafe(no_mangle)]`/`#[unsafe(export_name)]`, `#[inline]`/`#[cold]`,
`#[allow]`.

**L8.** Derives are written in this fixed order, omitting what is not
needed: `Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash`.

**L9.** Inside impls, write `Self` for the type (`-> Self`, `Self { .. }`).
(`jiff` spells the concrete name; our code already uses `Self` five to one.)

**L10.** One blank line between items, including between the methods of an
`impl` and between a wire struct and its layout asserts. A run of one-line
`const`, `static` or `mod` declarations stays contiguous. Inside a function, a
blank line only
between phases (gather / validate / commit). No blank line after `{` or
before `}`.

---

## 5. Imports

**I1.** All `use` statements are at the top of the file. None mid-file, none
between functions.

**I2.** Groups in this order, separated by one blank line, each group one
merged nested tree per root, entries alphabetical:

```rust
use core::{
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};

use svmvisor_hypervisor::svm::vmcb::Vmcb;
use uefi_raw::Status;

use crate::{
    arch::x86_64::msr::{self, EFER},
    svm::exit::ExitSnapshot,
};
```

Common path prefixes inside a tree are nested (`arch::x86_64::{apic, msr}`),
never repeated.

1. `core` (then `std`, host tools and tests only)
2. external and sibling crates (`x86_64`, `uefi_raw`, `sha2`, `svmvisor_*`)
3. `crate::`

**I3.** Paths are absolute from `crate::`. `super::` appears in exactly one
place: `use super::*;` as the first line of `mod tests`. A child module names
its parent's private items through `crate::` like anything else.
Exception while M10 debt exists: a file mounted by `#[path]` from more than
one place (the `card-loader` and `launcher` binary-only files and the `src/`
files that `tests/` mount) keeps whatever import paths resolve in every mount;
only the grouping and ordering of its imports is normalized.

**I4.** No glob imports, with two exceptions: `use super::*;` in `mod tests`
(I3), and `use SomeEnum::*;` as the *first statement of a function* whose body
is a `match` over that enum.

**I5.** Items are imported, not spelled out inline. No
`crate::svm::native_cache::owned_msr(index)` in a function body: import
`owned_msr`, or import the module and write `cache::owned_msr(index)`.

**I6.** Functions whose bare name is ambiguous are called module-qualified:
import the module, not the function — `msr::read(EFER)`, `apic::read(..)`,
`irq::capture(..)`. Types and constants are imported by name.

**I7.** Rename on import only to resolve a real collision, and then with the
context as prefix: `use crate::boot::memory::TableStorage as BootTableStorage`.

**I8.** `#[cfg]`-gated imports are separate `use` statements, each with its
own `#[cfg]`, inside the group they belong to. rustfmt decides their position
within the group (F1).

---

## 6. Naming

### 6.1 General

**N1.** Words are spelled out in item names: `config` not `cfg`,
`boot_services` not `bs`, `auxiliary` not `aux`. The exceptions are the
architecture's own vocabulary (N12), which is always abbreviated.

**N2.** A qualifier earns its place only if its opposite exists in the same
namespace. `NativeEfer` is justified only if a non-native `Efer` lives next to
it; otherwise it is `Efer`. The same goes for `Raw`, `Real`, `Physical`,
`Host`, `Guest`.

**N3.** No stutter in functions and constants: they do not repeat their
module's name (`svm::pause::retry_ready`, not `svm::pause::pause_retry_ready`;
`msr::read`, not `msr::read_msr`). Types are the exception, because they are
imported by bare name and must stand alone (N4): `ipi::IpiInventory`.

**N4.** No bare generic nouns as type names. `Error`, `Outcome`, `Report`,
`Refusal`, `Capture`, `State`, `Inventory`, `Prepared` always carry their
subject: `ProbeError`, `DispatchOutcome`, `IpiInventory`. (`jiff` uses bare
`Error` per module; we have 45 `SubjectError` types and 5 bare ones, so the
subject form wins.)

**N5.** Two different types never share a name across modules of one crate
(`TableStorage` ×3, `Translation` ×3 today). Give each its subject.

**N6.** Acronyms are words: `Vmcb`, `Npt`, `Msr`, `Apic`, `X2Avic`, `Ipi`,
`Iopm`, `Msrpm`, `Mtrr`, `Pat`, `Efer`. Never `VMCB` or `NPT` in a type name.

### 6.2 Type suffixes — each word means one thing

| Suffix | Means | Not |
|---|---|---|
| `…Error` | the `enum` a function returns in `Err` | a struct of details |
| `…Failure` | a `struct` of detail accompanying an error: which predicate failed, with which sample | an enum of reasons |
| `…Refusal` | a stopped-guest outcome with a stable wire code | an internal error |
| `…Outcome` | the successful-path result of an operation that can end several ways | an error |
| `…Evidence` | caller-supplied facts given to a validator; not yet trusted | something we measured ourselves |
| `…Observation` | a raw reading we took from hardware or firmware | a classified result |
| `…Snapshot` | a point-in-time copy of one whole structure | a handful of samples |
| `…Report` | what a classifier or admission pass concluded | raw data |
| `…Request` | input to a `prepare`/`validate` step | |
| `…Plan` | a validated decision not yet applied | |
| `Prepared…` / `Validated…` | the proof-carrying result of `prepare_*` / `validate_*` | |
| `…Storage` | caller-owned backing memory handed to a builder | |
| `…Guard` | RAII scope; releases on `Drop` | |
| `…Kind` | a plain discriminant enum | |

A type whose doc comment describes it with a different word from this table
than its suffix is misnamed.

### 6.3 Functions

| Form | Contract |
|---|---|
| `new(..)` | the primary constructor; returns `Self` or `Result<Self, _>` |
| `empty()` | the blank / all-zero value of a table or collection |
| `from_<source>(..)` | constructor from one specific source |
| `initialize(&mut self, ..)` | fills caller-provided storage in place **[no_std]** |
| `field()` | getter; never `get_field()` |
| `get(key)` | keyed lookup returning `Option`, the only use of `get` |
| `set_field(..)` | `&mut self` setter |
| `is_*`, `has_*` | returns `bool`, no side effects |
| `validate_*` | returns `Result`; checks and yields a validated value or `()` |
| `prepare_*` | builds a `Prepared…` without touching hardware |
| `commit_*`, `install_*`, `arm` | the step that touches hardware or publishes state |
| `capture_*`, `observe_*` | reads hardware / firmware into an `…Observation` or `…Snapshot` |
| `read_*`, `write_*` | one access to a register, port, MSR or memory location |
| `read_u16`, `read_u32`, `read_u64` | little-endian decode from a byte slice |
| `handle_*` | handler for one exit / event class |
| `as_*`, `to_*`, `into_*` | borrowed view / computed conversion / consuming conversion |
| `*_detailed` | the twin that also returns the `…Failure` detail |
| `*_unchecked` | the twin that skips validation the caller has already done |

**N7.** Retired spellings: `check_*` → `validate_*`; `valid_*` → `is_valid_*`
(if it returns `bool`) or `validate_*` (if it returns `Result`);
`rdmsr`/`wrmsr` and `read_msr`/`write_msr` → `msr::read`/`msr::write` (N3, U1);
`get16`/`get32`/`get64` → `read_u16`/`read_u32`/`read_u64`.

**N8.** Forbidden suffixes: `_inner`, `_impl`, `_internal`, `_helper`, `_do`,
`_2`. When a public function delegates to a worker, name the worker for the
narrower job it does.

**N9.** Test functions describe the behavior, without a `test_` prefix:
`pause_retries_preserve_guest_state`, `zero_filter_cannot_retry`.

### 6.4 Constants and statics

**N10.** `SCREAMING_SNAKE_CASE`, with the unit as the last word. Sizes are
`_BYTES`, counts are `_COUNT` or the counted noun (`ARENA_PAGES`), maxima are
`MAX_…`. Never `_SIZE` or `_LEN`, which do not say what is being counted.

**N11.** One name per concept, everywhere:

| Concept | Name |
|---|---|
| 4 KiB page size | `PAGE_BYTES` |
| page-table physical-address field | `ADDRESS_MASK` |
| page-table bits | `PRESENT`, `WRITE`, `USER`, `NX` |
| MSR numbers | the AMD manual's name: `EFER`, `VM_CR`, `SYS_CFG`, `HWCR` |
| bits within an MSR | `<MSR>_<BIT>`: `VM_CR_SVMDIS`, `SYS_CFG_MTRR_FIX_DRAM_EN` |
| VMCB field offsets | `<FIELD>_OFFSET`, in `svm::vmcb` |
| SVM exit codes | `EXIT_<NAME>` (`EXIT_MSR`, `EXIT_PAUSE`, `EXIT_NPF`), in `svm::exit` |

**N12.** Exported linker symbols are `svmvisor_<area>_<name>` in lower snake
case. They are ABI, checked by `xtask` audits, and are never renamed as part
of style work.

### 6.5 Locals, parameters and fields

**N13.** A concept has one short name, used for locals, parameters and
fields alike. A second instance gets a qualifying prefix (`host_vmcb`,
`next_vmcb`), never a different word.

| Concept | Name | Never |
|---|---|---|
| a `Vmcb` | `vmcb` | `v`, `control` |
| nested page tables | `npt` | |
| guest / host / generic physical address | `gpa` / `hpa` / `pa`; as a suffix `vmcb_pa` | `physical_address`, `phys`, `addr` |
| virtual address | `va` | |
| MSR index | `msr` | `index` (when it is an MSR) |
| MTRR state | `mtrrs` | `mt` |
| PAT value | `pat` | |
| APIC id | `apic_id` | `id`, `apic` |
| interrupt vector | `vector` | `vec`, `v` |
| exit code / info | `code`, `info1`, `info2` | |
| UEFI boot services | `boot_services` | `bs`, `services` |
| configuration | `config` | `cfg`, `conf` |
| a byte slice | `bytes` | `b`, `buf`, `data` |
| offset into a structure | `offset` | `off`, `o` |
| index into a table | `index` | `i` outside a 3-line loop |
| table slot | `slot` | |
| element count | `count` | `n`, `num`, `len` (unless it *is* `.len()`) |
| length in bytes | `length` | `len`, `size` |
| half-open range | `start`, `end` | `begin`, `first`/`last` for ranges |
| inclusive bounds | `first`, `last` | |
| the value being returned | `result` | `ret`, `res`, `r` |
| firmware status | `status` | `st` |
| an error in scope | `error` | `e`, `err` |
| test expectations | `expected`, `actual` | `want`/`got`, `before`/`after` for non-temporal use |

**N14.** Single-letter names are for closure parameters and loop indices
whose whole scope fits on screen (≤ 3 lines). Nowhere else.

---

## 7. Errors and refusals  [no_std]

`jiff`'s error design (heap-allocated opaque error, context chains, prose
messages) does not apply to us. Ours is:

**E1.** Errors are plain `Copy` enums named `<Subject>Error`, deriving
`Clone, Copy, Debug, PartialEq, Eq`. One per module that can fail; defined in
the file that returns it (section 9 of the file layout).

**E2.** Variants are nouns or noun phrases stating what is wrong
(`UnsupportedLeaf`, `RangeOverlap`, `ReservedBitsSet`), not verbs, not
`Failed…`, not `Invalid` alone.

**E3.** An error that crosses the wire (journal, terminal, diagnostics) has a
stable numeric code. The mapping lives in one `const fn code(self) -> uN` on
the error type, or one encoder module per wire format — never in scattered
`match`es at call sites. Codes are never renumbered by style work.

**E4.** Crossing a module boundary converts explicitly with a `From` impl
placed directly below the error type. No `map_err(|_| …)` that throws away the
source error when a `From` would keep it.

**E5.** Detail goes in a `…Failure` struct returned by a `*_detailed` twin
(§6.2, §6.3), not in ad-hoc tuple payloads.

---

## 8. Constants, registers and wire layouts  [no_std]

**K1.** No bare hardware numbers in `src/`. MSR numbers, MSR bits, VMCB
offsets, exit codes, CPUID leaves, APIC register offsets and port numbers are
named constants (N11). `vmcb.exit_snapshot().code == 0x77` is
`== EXIT_PAUSE`; `b[0x03c..0x03e]` goes through a `Vmcb` accessor.

**K2.** In `tests/`, literal values taken from the manual are *allowed and
encouraged*: a conformance test that writes `0x070` cross-checks the constant
instead of restating it.

**K3.** Homes, one each (principle 3):

| What | Home |
|---|---|
| MSR numbers and their bits | `hypervisor::arch::x86_64::msr` |
| APIC / x2APIC registers | `hypervisor::arch::x86_64::apic` |
| page size, address mask, page-table bits | `hypervisor::memory::address`. A `u64` view is derived from it (`PAGE_BYTES as u64`), never a second literal |
| memory-map descriptor bound `MAX_DESCRIPTORS` | `hypervisor::boot::memory` |
| MP Services status bits | `launcher::native::admission::cpu` |
| VMCB offsets | `hypervisor::svm::vmcb`, `pub(crate)`; other modules use `Vmcb` accessors, not offsets |
| exit codes | `hypervisor::svm::exit` |
| resident bridge ABI | `hypervisor::host::resident` |
| card loader<->child contract (boot options, native result, journal record, terminal endpoint) | `card-abi` |
| card image format (the 128-byte envelope: sizes, magics, flags, field offsets, PE policy limits, and the parser) | `card-abi::envelope` |
| `SVMRELO1` relocatable package (arena size, handoff offset, parser and relocator) | `card-abi::package` |

A crate that depends on `hypervisor` imports these; it never re-declares
them. A deliberately standalone crate (`card-loader`, `memory-attributes`,
`rompack`) may keep its own copy, with a comment naming the authoritative one.

Same value is not same concept: `APIC_BASE_ADDRESS` and the AVIC pointer
masks equal `ADDRESS_MASK` numerically and stay separate. A constant whose only
candidate homes are feature-disjoint or binary-only mounts (`ARENA_PAGES`)
stays duplicated, guarded by a `const _` equality assert, until the M10 debt
is paid.

**K4.** A wire struct — anything assembly, firmware, the card or another
binary reads — is `#[repr(C)]` (plus `align` where required), has fixed-width
integer fields only, and is followed immediately by a `const _: () = { … };`
block asserting `size_of`, `align_of` and every `offset_of!` (L3).

**K5.** Wire structs that form one ABI live together in one dedicated file
named for the ABI, not next to the first function that happened to use them.

---

## 9. `unsafe`, assembly and statics  [no_std]

**U1.** Privileged or architectural instructions are wrapped once, in
`hypervisor::arch::x86_64` (`msr::read`, `msr::write`, `cpuid`, port I/O,
`invlpga`, ...). Everything else calls the wrapper. Inline `asm!` outside
`arch::x86_64` is limited to world-switch and entry sequences that cannot be
a function call. The standalone `card-loader` does not depend on `hypervisor`:
its architectural instructions live in its `firmware::{cpu, pci_io}`.

**U2.** `static mut` and exported statics appear only in section 5 of the
file layout (L1), as one block, each with a comment stating its single writer
and who reads it. A file with many of them is a `state.rs`/`statics.rs` of its
directory module, not a feature file with statics sprinkled through it.

**U3.** `extern` blocks and `global_asm!` form section 6 of the layout, one
block per file, with a comment naming the `.S` file or linker script that
defines the symbols.

**U4.** `#[unsafe(no_mangle)]` items are declared in the file that owns the
corresponding state, in the statics block (data) or as the last public
function (entry points). Their names follow N12.

**U5.** New or changed `unsafe` blocks carry a `// SAFETY:` comment directly
above, stating the invariant relied on. `unsafe fn` documents the caller's
obligation under `# Safety`. (Existing blocks are backfilled deliberately, by
someone who has verified the invariant — not mechanically.)

**U6.** `.unwrap()` / `.expect()` outside tests is preceded by a comment
saying why it cannot fail, or replaced by the error path. Slice-to-array
conversions of fixed ranges (`b[0x00c..0x010].try_into().unwrap()`) go through
the `read_u32`-style helpers instead.

---

## 10. Visibility

**V1.** Private by default. Then `pub(crate)`. `pub` only for items named
from outside the crate — by another crate or by `tests/`. An item nothing
outside the crate names is not `pub`. Two exceptions, both measured: an item
stays `pub` when `pub(crate)` would make it warn as dead code in some feature
configuration (items only the `resident-runtime` feature uses), and when
narrowing it changes the payload's machine code (visibility changes linkage,
and LLVM may then inline the function) — that is a code change and needs a
boot test, not a style commit.

**V2.** `pub(super)` has exactly one use: an item — function, method, type,
constant or field — shared between sibling files of one directory module (M2),
or between a child file and the directory's moved-out tests:
`runtime/exit.rs` calling a function in `runtime/irq.rs`, `npt/identity.rs`
using the entry bits in `npt/table.rs`. The directory's `mod.rs` keeps its
child modules private. `pub(in …)` is not used.

**V3.** Struct fields are private, with getters, unless the struct is a wire
struct (K4) or a plain evidence/observation record (§6.2) whose fields *are*
its interface — then all fields are `pub` and there are no getters. Never a
mix.

**V4.** Glob re-exports (`pub use provider::*`) are not used. Re-export by
name. In a module that only a binary mounts (M10 debt), a by-name re-export
that some feature sets do not use carries `#[allow(unused_imports)]` with a
comment saying so.

---

## 11. `cfg` and features

**G1.** `#[cfg]` goes on items — functions, modules, fields, `use` lines,
match arms — not inside expressions. `cfg!()` is not used in `src/`.

**G2.** When a function differs by feature, write two whole functions, each
under its own `#[cfg]`, adjacent, the enabled one first. When a module
differs, use the alternates pattern (M10).

**G3.** A `#[cfg]` predicate repeated more than three times in a file gets a
gated submodule instead.

---

## 12. Tests

**T1.** Unit tests of private behavior: one `#[cfg(test)] mod tests` per
file, named exactly `tests`, the last item in the file, starting with
`use super::*;`. Not `lookup_tests`, not three modules.

**T2.** When a file's inline tests pass ~300 lines, `foo.rs` becomes the
unit directory `foo/` (M2): the tests move to `foo/tests.rs` — or, when there
are several test modules, to `foo/tests/<topic>.rs` with a declarations-only
`foo/tests/mod.rs`. `#[cfg(test)] mod tests;` is the last `mod` declaration of
`foo/mod.rs`, separated from the others by a blank line (rustfmt sorts a run
of `mod` lines alphabetically and would otherwise move it). A source file is
never more than half tests. Tests that leave an inline module lose one indent
level, so rustfmt may re-wrap them: they stay identical token for token, not
byte for byte.

**T3.** Integration tests (`tests/*.rs`) exercise the public API. A test file
is named for the module path under test, joined with `_`, with an optional
topic: `svm_vmcb.rs`, `svm_dispatch_efer.rs`, `svm_x2avic_ipi.rs`. The file
name alone says which module broke.

**T4.** Helpers come first inside a test module, then the `#[test]`
functions. In `tests/*.rs` the order is: inner attributes, `#[path]` mounts
and their inline substitute modules, imports, constants and statics, then
helper types and functions as one section (callers above callees, a type
staying with its impls), then the tests. A helper used by several integration test files lives in
`tests/support/mod.rs`.

**T5.** Test-only constructors on real types are `#[cfg(test)] pub(crate) fn
fixture(..)`. Test doubles for hardware are traits implemented in the test,
not alternate source files mounted over the real module.

**T6.** Tests follow every formatting and naming rule. They are not a place
for compressed code.

---

## 13. Verifying a change

A style change is correct when all of this passes. `bash
crates/xtask/style-check/verify.sh full` runs every command below and the
disassembly comparison (see its README); each command was verified on
2026-09-19:

```
cargo fmt --all -- --check
cargo test --workspace
cargo test -p svmvisor-hypervisor --lib --features resident-runtime
cargo test -p svmvisor-hypervisor --lib --features resident-runtime-test
cargo test -p svmvisor-launcher --features native-returning
cargo test -p svmvisor-launcher --features native-resident-boot
cargo test -p svmvisor-launcher --features native-transition-multi-exit
cargo test -p svmvisor-launcher --features memory-attribute-probe
cargo test -p svmvisor-card-loader --features card-resident-dev-loader
cargo test -p svmvisor-card-loader --features card-resident-loader
cargo build-card-loader --features card-resident-dev-loader
cargo xtask resident --output target/native-resident/<fresh-dir> --low-runtime
```

Notes:

- The hypervisor `resident-runtime` features only link with `--lib`; the
  integration tests cannot resolve the runtime's assembly symbols.
- `--all-features` never works: several features are mutually exclusive.
- `cargo xtask resident` is the only check that compiles the real payload,
  assembles the `.S` files and runs the relocation, no-FP and symbol audits.
  It is reproducible: two builds of the same tree give byte-identical
  `payload.bin` and `driver.efi`. For a change that claims to move or rename
  without altering logic, diff `disassembly.log` against a build of the parent
  commit; the expected difference is panic-location data (file paths and line
  numbers) and symbol order, not instructions. Compare function bodies with
  symbol names reduced to their final identifier: mangled names embed the
  module path and a per-checkout crate hash, so a raw diff flags every moved
  function and every git worktree.
- That comparison covers the resident payload only; `card-loader` and
  `launcher` code is not in it. For a change to either, build the affected UEFI feature
  sets with `--emit=asm` before and after and compare per-function bodies the
  same way.

A file move or rename must update, in the same commit, every place that
names the path: `#[path]` mounts (`card-loader/src/main.rs`,
`card-loader/tests/*.rs`, `launcher/src/main.rs`,
`launcher/src/native/resident/activation/mod.rs`, `launcher/tests/*.rs`),
the `.S` table in
`launcher/build.rs`, and the source paths in `xtask/src/resident.rs`.

---

## 14. What style work may and may not touch

Style work changes where code lives and what it is called. It never changes
what the code does.

| Allowed | Not allowed |
|---|---|
| `cargo fmt` | changing any expression, condition, constant value or control flow |
| moving items between files and modules | reordering statements inside a function |
| reordering items within a file (L1, L4) | splitting or merging functions |
| renaming files, modules, types, functions, constants, locals (a longer name may re-wrap a line and move panic line numbers in the binaries; instructions stay identical) | renaming exported symbols (N12), wire codes, Cargo features, `.S` labels |
| fixing imports and visibility | editing `///` and `//!` doc text (update a renamed identifier inside it, nothing else) |
| replacing a literal with a named constant **of the identical value** | deleting code, including code that becomes visibly dead |
| replacing a duplicate constant with an import of the one home (K3) | changing `#[repr]`, field order, field types, derives that affect layout or ABI |
| | adding or removing `#[inline]`, `#[cold]`, `#[no_mangle]` |
| | touching `.S` files, `payload.ld`, anything outside `crates/` except `rustfmt.toml` |

One kind of change per commit — format, or move, or rename — never mixed, so
every diff can be reviewed as "is this really only a move". If following a
rule would require a logic change (the 440-line `install_inner`, the 350-line
`callback`), leave the code alone and record it; long functions are a design
task, not a cleanup task.
