# docs/ — reference manuals, indexed for agents

Do not open the PDFs. Every PDF here has a sibling folder with its pages already split out,
an index of where everything is, and a search tool on top. Cite what you read as
`<folder> p<N>`.

| Folder | Manual | Use it for |
|---|---|---|
| `24593_3.44_APM_Vol2` | AMD64 APM Vol 2: System Programming | SVM, VMCB, AVIC/x2AVIC, NPT, APIC/x2APIC, paging, MSRs, exit codes |
| `24594_3.37_APM_Vol3` | AMD64 APM Vol 3: Instructions | instruction reference (VMRUN, WRMSR, …), CPUID leaves (Appendix E) |
| `57896-3.00_PPR` | PPR, Family 1Ah Model 44h B0 | this CPU's actual MSRs, CPUID values, reset values, register bit fields |
| `48882-3.11` | AMD IOMMU specification | device table, interrupt remapping, guest vAPIC (GA), IVRS |
| `ACPI_Spec_6.6` | ACPI 6.6 | MADT, tables, AML |
| `UEFI_PI_Spec_1_10` | UEFI Platform Initialization 1.10 | PEI/DXE/SMM, HOBs, firmware volumes, `EFI_MP_SERVICES_PROTOCOL` (AP enumeration/startup) |
| `pg054-7series-pcie-2022-12-23` | Xilinx 7-series PCIe block (PG054) | the FPGA card's PCIe core: ports, config space, interrupts |

`INDEX.json` in this directory is the machine-readable version of that table.

**Scope:** these manuals are the only trustworthy material outside `crates/`. For how the
hypervisor actually works, read and search `crates/` only — notes, old docs and other
directories elsewhere in the repo contain stale and wrong references. `docs/vendor/` is
third-party source, not project documentation.

## The one rule about page numbers

`N` in `page_<N>.png` / `page_<N>.txt`, and every `"page"` field in every index file, is the
**1-based physical PDF page** (what a PDF viewer shows in its page box).

The number *printed on the sheet* is usually different (APM Vol 2 physical page 644 is printed
"582"). It is recorded as `printed` / `label` in `PAGES.jsonl` and never names a file. When the
manual text says "see page 582", that is a printed number — convert it:

```bash
python docs/pdf_index.py show 24593 printed:582
```

Page references in the comments under `crates/` (e.g. "APM p582", "pp643-644") are **printed**
numbers too. Resolve them with `printed:` before reading, or you will land in the wrong section.

## What is in each folder

| File | What | How to use |
|---|---|---|
| `INDEX.json` | small entry point: numbering rules, file counts, chapter list with page ranges | Read it whole |
| `TOC.jsonl` | every outline entry: `level, title, page, end_page, path, check, y` | **grep**, never read whole |
| `FIGURES.jsonl` | figure captions → page | grep |
| `TABLES.jsonl` | table captions → page | grep |
| `PAGES.jsonl` | one line per page: `printed`, `label`, `sections` on the page, `captions` | grep for `"page": 644,` or `"printed": "582"` |
| `page_<N>.txt` | text of page N (ligatures normalised, so plain-letter grep works) | Read |
| `page_<N>.png` | 120 dpi render of page N | Read when layout matters (see below) |

`page..end_page` in `TOC.jsonl` is the inclusive physical range of that section.
`check` says how the page was confirmed against the page text: `exact` and `title_only` are
trustworthy; `moved` means the PDF's bookmark was wrong and `page` is already the corrected one
(`bookmark_page` keeps the original); `unverified` (3 entries in total) means look before you cite.

## How to look things up

Run the tool from the repo root. On Windows set `PYTHONIOENCODING=utf-8` first.

**A named heading, register, MSR, CPUID leaf, instruction or function** — grep the outline:

```bash
grep -i "VM_HSAVE_PA" docs/24593_3.44_APM_Vol2/TOC.jsonl
```

**A figure or table** — grep `FIGURES.jsonl` / `TABLES.jsonl` by caption words or by id (`"Table B-1"`).

**A topic, field or signal name that may only appear inside body text or a table** — full-text
search, grouped by the section each hit falls in:

```bash
python docs/pdf_index.py find "x2AVIC" --count
```

- `--count` gives sections and pages only; start with it, a broad term can produce a lot of output.
- `--doc SUBSTR` restricts to one manual (`--doc PPR`, `--doc 48882`).
- `--max N` sets snippets per section (one per page). `--all` includes contents, lists, index and revision history, which are skipped by default.
- Hit counts read `632x22` = 22 hits on page 632. The densest section is usually the one you want.

**Read a whole section** instead of page by page:

```bash
python docs/pdf_index.py show 24593 "^15\.29\.10 x2AVIC"
```

`show <doc> <what>` takes a section-title regex (must match exactly one entry; otherwise it lists
the candidates), a physical range `640-645`, or `printed:582`.

### Spelling differs between manuals

The same thing is written differently in each manual, so make separators optional:

| Thing | APM | PPR |
|---|---|---|
| CPUID leaf | `Fn8000_000A` | `CPUID_Fn8000000A_EDX` |
| MSR | `VM_CR MSR (C001_0114h)` | `MSRC001_0114 [...] (Core::X86::Msr::VM_CR)` |

```bash
python docs/pdf_index.py find "Fn8000_?000A" --count
python docs/pdf_index.py find "C001_?011B"
```

Searching an MSR or CPUID number across *all* manuals is worth doing: the APM gives the
architecture, the PPR gives what this CPU really implements, and they sometimes disagree.

## When to open the PNG

`page_<N>.txt` is reliable for prose and headings. It is **not** reliable for:

- **bit-field figures** (VMCB fields, table-entry formats, IRTE layouts): the text is a flat
  stream of numbers and labels that cannot be decoded;
- **tables that continue across pages**: rows can interleave with surrounding body text, and a
  row can land on the following page after the next heading;
- tables in general come out one cell per line — readable, but check the PNG if column
  alignment decides the meaning.

Read `page_<N>.png` in those cases. 120 dpi is enough to read every field.

**Rule: text to find and to spell, image to understand.**

1. Locate the page with the index or `find` (text only — images cannot be searched).
2. Read `page_<N>.txt` for prose, and copy identifiers, hex values, offsets and bit ranges
   from it: the text is character-exact, a vision read of an image can confuse `8`/`B`, `0`/`O`.
3. If the page has a figure or table, or you are implementing from it (register layout,
   structure format, VMCB fields, exit-code table): **always read `page_<N>.png` too** and
   cross-check. Trust the image for structure (which bits belong to which field, which row a
   cell is in), trust the text for spelling.
4. If text and image disagree on a value, say so instead of picking one.

`PAGES.jsonl` tells you in advance which pages need the image: a `"captions"` entry means a
figure or table starts on that page.

## Habits that keep answers correct

- Cite `<folder> p<N>` with the physical page you actually read, not a page the index pointed at.
- Prefer the PPR for anything value-specific to this CPU (reset values, which bits exist); prefer
  the APM for architectural behaviour. If they differ, report both.
- A section's `end_page` is inclusive and the next section may begin mid-page on it.
- Revision-history and contents hits are noise; the `find` default already hides them.

## Rebuilding

After adding or replacing a PDF in `docs/`:

```bash
python docs/pdf_index.py build
```

Needs `pip install pymupdf`. Existing PNGs are kept (`--force` re-renders, `--dpi` changes the
resolution, `--no-png` rebuilds only indexes and text). The PNGs are git-ignored (~900 MB);
everything else in the folders is small and can be committed.
