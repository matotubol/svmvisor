#!/usr/bin/env python3
"""Explode each docs/*.pdf into docs/<stem>/ so an agent can find the right page without opening the PDF.

Per PDF:
  INDEX.json      small entry point: numbering rules, file map, chapter list (safe to Read whole)
  TOC.jsonl       every outline entry, one JSON object per line -> grep it
  FIGURES.jsonl   figure captions -> page
  TABLES.jsonl    table captions -> page
  PAGES.jsonl     one line per page: sheet numbering, sections on the page, captions
  page_<N>.png    render of physical page N
  page_<N>.txt    text of physical page N (NFKC-normalised so ligatures grep as plain letters)

N and every "page" field are the 1-based physical PDF page (what a viewer's page box shows).
The number printed on the sheet is kept separately as "printed"/"label" and never names a file.

Outline pages are not trusted blindly: each heading is looked up in the page text, moved to the
page that really carries it when the bookmark is off, and marked with how it was confirmed.

Usage:
  python docs/pdf_index.py build [--dpi 120] [--force] [--no-png] [pdf ...]
  python docs/pdf_index.py find <regex> [--doc SUBSTR] [--max N] [--count] [--all]   full text, grouped by section
  python docs/pdf_index.py show <doc> <section regex | 640-645 | printed:582>          dump a section's page text
"""
import argparse
import json
import re
import sys
import unicodedata
from collections import Counter
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import pymupdf

DOCS = Path(__file__).resolve().parent

PAGE_NO = re.compile(r"^(?:page\s+)?(\d{1,4}|[ivxlcdm]{1,8}|[A-Z]{1,4}-\d{1,4})$", re.I)
NUMBERING = re.compile(r"^\s*(?:(?:chapter|appendix|appx\.|ch\.)\s*)?[A-Z]{0,3}-?\d*(?:[.\-:]\d+)*[.:]?\s+", re.I)
CAP_ID = r"(Figure|Fig\.|Table)\s+((?:[A-Z]{1,3}[-.])?\d+(?:[-.]\d+)*)"
CAPTION = re.compile(rf"^{CAP_ID}\s*([.:]?)\s*(.*)", re.S)
CAP_SPLIT = re.compile(r"\n(?=(?:Figure|Fig\.|Table)\s)")
LIST_ENTRY = re.compile(rf"{CAP_ID}[.:]?\s+(.*?)(?:\s*\.){{3,}}\s*([A-Za-z]{{0,4}}-?\d+|[ivxlc]+)\b", re.S)
LEADER = re.compile(r"(?:\.\s?){5,}")
LIST_TITLES = re.compile(r"^(list of (figures|tables)|(table of )?contents|figures|tables)$", re.I)


def clean(s):
    return unicodedata.normalize("NFKC", s).replace(chr(0xAD), "")  # soft hyphen


def squash(s):
    return re.sub(r"[^a-z0-9]", "", s.lower())


def fix_label(s):
    """PyMuPDF leaves UTF-16 label prefixes as '<FEFF0049002D>12'."""
    if not s:
        return None
    return re.sub(r"<FEFF([0-9A-Fa-f]+)>", lambda m: bytes.fromhex(m.group(1)).decode("utf-16-be"), s)


def printed_number(blocks, h):
    """Number printed in the header/footer band, if one stands alone there."""
    for x0, y0, x1, y1, text, *_ in blocks:
        if y1 < h * 0.09 or y0 > h * 0.91:
            for line in text.splitlines():
                m = PAGE_NO.match(line.strip())
                if m:
                    return m.group(1)
    return None


def locate(raw, flat, title, pg):
    """Which page really carries this heading? Returns (page, exact|title_only|moved|unverified)."""
    for key, how in ((squash(title)[:60], "exact"), (squash(NUMBERING.sub("", title, 1))[:60], "title_only")):
        if len(key) < 2:
            continue
        if len(key) < 6:  # short mnemonic headings (ADC, OR): need a whole-word hit
            word = re.escape(title.split()[-1])
            if re.search(rf"(?<![A-Za-z0-9]){word}(?![A-Za-z0-9])", raw[pg]):
                return pg, "exact"
            continue
        if key in flat[pg]:
            return pg, how
        for q in (pg + 1, pg - 1, pg + 2, pg + 3):
            if key in flat.get(q, ""):
                return q, "moved"
    return pg, "unverified"


def heading_y(page, blocks, title):
    """(heading position as a fraction of page height, True if only header furniture sits above it)."""
    h = page.rect.height
    for probe in (title[:45], NUMBERING.sub("", title, 1)[:45]):
        probe = probe.strip()
        if len(probe) < 3:
            continue
        hits = [r for r in page.search_for(probe) if h * 0.07 < r.y0 < h * 0.93]
        if hits:
            r = min(hits, key=lambda r: r.y0)
            above = sum(len(b[4].strip()) for b in blocks if b[1] > h * 0.07 and b[3] <= r.y0 + 1)
            return round(r.y0 / h, 3), above < 25
    return None, False


def render_range(args):
    pdf, out, start, stop, dpi, force = args
    doc = pymupdf.open(pdf)
    for i in range(start, stop):
        png = Path(out) / f"page_{i + 1}.png"
        if force or not png.exists():
            doc[i].get_pixmap(dpi=dpi).save(png)


def jl(o):
    return json.dumps({k: v for k, v in o.items() if v is not None}, ensure_ascii=False)


def build(pdf, dpi, force, png, pool):
    out = DOCS / pdf.stem
    out.mkdir(exist_ok=True)
    doc = pymupdf.open(pdf)
    count = len(doc)
    jobs = []
    if png:
        jobs = [pool.submit(render_range, (str(pdf), str(out), s, min(s + 25, count), dpi, force))
                for s in range(0, count, 25)]

    pages, raw, flat, blocks_of = [], {}, {}, {}
    for i, page in enumerate(doc):
        n = i + 1
        blocks_of[n] = [b[:4] + (clean(b[4]),) for b in page.get_text("blocks") if b[6] == 0]
        raw[n] = clean(page.get_text())
        flat[n] = squash(raw[n])
        (out / f"page_{n}.txt").write_text(raw[n], encoding="utf-8")
        pages.append({"page": n, "label": fix_label(page.get_label()),
                      "printed": printed_number(blocks_of[n], page.rect.height),
                      "chars": len(raw[n]), "images": len(page.get_images())})

    # outline, with every page checked against the text
    toc = []
    for lv, title, pg in doc.get_toc():
        if not 1 <= pg <= count:
            continue
        title = " ".join(clean(title).split())
        real, how = locate(raw, flat, title, pg)
        y, top = heading_y(doc[real - 1], blocks_of[real], title) if how != "unverified" else (None, False)
        toc.append({"level": lv, "title": title, "page": real, "end_page": None,
                    "bookmark_page": pg if real != pg else None, "check": how, "y": y, "_top": top})
    for a, b in zip(toc, toc[1:]):  # outline order is document order
        if b["page"] < a["page"]:
            b["page"] = a["page"]
    path = []
    for i, e in enumerate(toc):
        nxt = next((t for t in toc[i + 1:] if t["level"] <= e["level"]), None)
        # a following heading at the very top of its page means this section ended the page before
        e["end_page"] = count if nxt is None else max(e["page"], nxt["page"] - 1) if nxt["_top"] else nxt["page"]
        del path[e["level"] - 1:]
        e["path"] = " > ".join(path) or None
        path.append(e["title"])

    # sections per page: whatever runs in from the previous page plus every heading that starts here
    starts = {}
    for e in toc:
        starts.setdefault(e["page"], []).append(e)
    carried = None
    for p in pages:
        here = starts.get(p["page"], [])
        p["sections"] = ([carried] if carried and not (here and here[0]["_top"]) else []) + [e["title"] for e in here]
        if here:
            carried = here[-1]["title"]

    # captions: scan the body, keep the manual's dominant caption style, first occurrence wins
    list_pages = set()
    for e in toc:
        if LIST_TITLES.match(e["title"]) or LIST_TITLES.match(NUMBERING.sub("", e["title"], 1).strip()):
            list_pages.update(range(e["page"], e["end_page"] + 1))
    cands = []
    for n, blocks in blocks_of.items():
        if n in list_pages:
            continue
        for bi, b in enumerate(blocks):
            for para in CAP_SPLIT.split(b[4].strip()):
                m = CAPTION.match(para)
                if m and not LEADER.search(para):
                    title = " ".join(m.group(4).split())
                    if not title and m.group(3) and bi + 1 < len(blocks):  # title sits in the next block
                        title = " ".join(blocks[bi + 1][4].split())
                    cands.append(("Table" if m.group(1) == "Table" else "Figure", m.group(2), m.group(3), title, n))
    caps = {"Figure": [], "Table": []}
    for kind, rows in caps.items():
        style = Counter(c[2] for c in cands if c[0] == kind and c[3][:1].isupper())
        sep = style.most_common(1)[0][0] if style else None
        seen = set()
        for k, num, s, title, n in cands:
            if k == kind and s == sep and num not in seen and title[:1].isupper():
                seen.add(num)
                rows.append({"id": f"{kind} {num}", "title": title[:140], "page": n, "from": None})
    # the manual's own List of Figures/Tables fills in whatever the body scan missed
    by_sheet = {}
    for p in pages:
        for s in (p["printed"], p["label"]):
            if s:
                by_sheet.setdefault(s, p["page"])
    have = {c["id"] for rows in caps.values() for c in rows}
    for m in LIST_ENTRY.finditer("\n".join(raw[n] for n in sorted(list_pages))):
        kind = "Table" if m.group(1) == "Table" else "Figure"
        cid, pg = f"{kind} {m.group(2)}", by_sheet.get(m.group(4))
        if cid not in have and pg:
            have.add(cid)
            caps[kind].append({"id": cid, "title": " ".join(m.group(3).split())[:140], "page": pg, "from": "list"})
    for rows in caps.values():
        rows.sort(key=lambda c: c["page"])
    for p in pages:
        p["captions"] = [c["id"] for rows in caps.values() for c in rows if c["page"] == p["page"]] or None
    for e in toc:
        del e["_top"]

    for name, rows in (("TOC", toc), ("FIGURES", caps["Figure"]), ("TABLES", caps["Table"]), ("PAGES", pages)):
        (out / f"{name}.jsonl").write_text("".join(jl(r) + "\n" for r in rows), encoding="utf-8")

    checks = Counter(e["check"] for e in toc)
    depth = 1 if sum(1 for e in toc if e["level"] <= 2) > 120 else 2
    index = {
        "source": f"../{pdf.name}",
        "title": clean(doc.metadata.get("title") or pdf.stem),
        "page_count": count,
        "dpi": dpi,
        "numbering": "Every 'page' is the 1-based physical PDF page and names page_<N>.png / page_<N>.txt. "
                     "'printed' (header/footer) and 'label' (PDF page label) are the sheet's own numbering; "
                     "to go from a printed number to a file, grep PAGES.jsonl for \"printed\": \"<n>\".",
        "how_to": [
            "heading / register / function by name: grep TOC.jsonl (one entry per line: level, title, page, end_page, path)",
            "figure or table: grep FIGURES.jsonl / TABLES.jsonl",
            "topic across the manual(s): python docs/pdf_index.py find \"<regex>\" [--count] -> hits grouped by section",
            "read a whole section: python docs/pdf_index.py show <doc> \"<title regex>\" (or 640-645, or printed:582 for an 'on page 582' cross-reference)",
            "manuals spell identifiers differently (Fn8000_000A vs Fn8000000A, MSRC001_0114 vs C001_0114h): "
            "search with optional separators, e.g. \"Fn8000_?000A\"",
            "then Read page_<N>.txt, and page_<N>.png when layout matters (bit-field diagrams, tables, figures)",
        ],
        "toc_fields": "page..end_page = inclusive physical range of the section; y = heading position down the page (0 top, 1 bottom); "
                      "check = exact|title_only|moved|unverified (heading text confirmed on that page); "
                      "bookmark_page = what the PDF outline claimed when it was wrong; path = parent headings",
        "files": {"TOC.jsonl": len(toc), "FIGURES.jsonl": len(caps["Figure"]),
                  "TABLES.jsonl": len(caps["Table"]), "PAGES.jsonl": count},
        "toc_check": dict(checks),
    }
    body = ",\n".join(f'  {json.dumps(k)}: {json.dumps(v, ensure_ascii=False)}' for k, v in index.items())
    body += ',\n  "chapters": [\n' + ",\n".join(
        "    " + jl({k: e[k] for k in ("level", "title", "page", "end_page")}) for e in toc if e["level"] <= depth) + "\n  ]"
    (out / "INDEX.json").write_text("{\n" + body + "\n}\n", encoding="utf-8")

    for j in jobs:
        j.result()
    print(f"{pdf.stem}: {count} pages, toc {dict(checks)}, {len(caps['Figure'])} figures, "
          f"{len(caps['Table'])} tables", flush=True)


def catalogue():
    docs = []
    for idx in sorted(DOCS.glob("*/INDEX.json")):
        j = json.loads(idx.read_text(encoding="utf-8"))
        docs.append({"dir": idx.parent.name, "title": j["title"], "page_count": j["page_count"],
                     "index": f"{idx.parent.name}/INDEX.json"})
    top = {"about": "One folder per PDF. Read <dir>/INDEX.json first; it explains the other files. "
                    "Cross-manual topic search: python docs/pdf_index.py find \"<regex>\"",
           "docs": docs}
    (DOCS / "INDEX.json").write_text(json.dumps(top, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


NOISE = re.compile(r"^((table of )?contents|list of (figures|tables|chapters)|figures|tables|index|revision history)$", re.I)


def load(d, name):
    return [json.loads(l) for l in (d / f"{name}.jsonl").read_text(encoding="utf-8").splitlines()]


def find(pattern, only, limit, everything, count_only):
    rx = re.compile(pattern, re.I)
    searched = found = 0
    for idx in sorted(DOCS.glob("*/INDEX.json")):
        d = idx.parent
        if only.lower() not in d.name.lower():
            continue
        searched += 1
        toc = load(d, "TOC")
        noise = set()
        for e in toc:
            if NOISE.match(NUMBERING.sub("", e["title"], 1).strip()) or NOISE.match(e["title"]):
                noise.update(range(e["page"], e["end_page"] + 1))
        starts = {}
        for e in toc:
            starts.setdefault(e["page"], []).append(e)
        count = json.loads(idx.read_text(encoding="utf-8"))["page_count"]
        groups, carried, total, skipped = {}, None, 0, 0
        for n in range(1, count + 1):
            text = " ".join((d / f"page_{n}.txt").read_text(encoding="utf-8").split())
            # where each heading that starts on this page sits in the text, so a hit lands in the right section
            marks, pos = [], 0
            for e in starts.get(n, []):
                at = text.lower().find(e["title"].lower()[:50], pos)
                if at < 0:
                    at = text.lower().find(NUMBERING.sub("", e["title"], 1).lower()[:50], pos)
                pos = at if at >= 0 else pos
                marks.append((pos, e))
            for m in rx.finditer(text):
                if n in noise and not everything:
                    skipped += 1
                    continue
                sec = carried
                for at, e in marks:
                    if at <= m.start():
                        sec = e
                key = (sec["page"], sec["end_page"], sec["title"]) if sec else (0, 0, "(before first heading)")
                groups.setdefault(key, []).append((n, text[max(0, m.start() - 60):m.end() + 80]))
                total += 1
            if marks:
                carried = marks[-1][1]
        if not groups and not skipped:
            continue
        found += 1
        note = f" (+{skipped} in contents/lists/index/revision history, --all shows them)" if skipped else ""
        print(f"\n## {d.name}: {total} hits{note}")
        for (pg, end, title), hits in sorted(groups.items()):
            by_page = Counter(n for n, _ in hits)
            where = " ".join(f"{n}x{c}" if c > 1 else str(n) for n, c in sorted(by_page.items()))
            print(f"  [{pg}-{end}] {title[:90]}  -> pages {where}")
            if count_only:
                continue
            shown = set()
            for n, snip in hits:  # one snippet per page
                if n not in shown and len(shown) < limit:
                    shown.add(n)
                    print(f"      p{n}: ...{snip}...")
    if not found:
        print(f"no hits for {pattern!r} in {searched} manual(s); try optional separators (C001_?011B) or a shorter term")


def show(only, what):
    """Print the text of a section (title regex), a page range (640-645) or a printed page (printed:582)."""
    d = next((i.parent for i in sorted(DOCS.glob("*/INDEX.json")) if only.lower() in i.parent.name.lower()), None)
    if d is None:
        sys.exit(f"no indexed doc matches {only!r}")
    if what.startswith("printed:"):
        hits = [p["page"] for p in load(d, "PAGES") if what[8:] in (p.get("printed"), p.get("label"))]
        if not hits:
            sys.exit("no page carries that printed number")
        lo = hi = hits[0]
    elif re.fullmatch(r"\d+(-\d+)?", what):
        lo, _, hi = what.partition("-")
        lo, hi = int(lo), int(hi or lo)
    else:
        rx = re.compile(what, re.I)
        secs = [e for e in load(d, "TOC") if rx.search(e["title"])]
        if len(secs) != 1:
            for e in secs[:40]:
                print(f"  [{e['page']}-{e['end_page']}] {e['title']}")
            sys.exit(f"{len(secs)} sections match; narrow the regex or pass a page range")
        lo, hi = secs[0]["page"], secs[0]["end_page"]
        print(f"# {secs[0]['title']}")
    for n in range(lo, hi + 1):
        f = d / f"page_{n}.txt"
        if f.exists():
            print(f"\n===== {d.name} page {n} (page_{n}.png) =====\n{f.read_text(encoding='utf-8')}")


def main():
    if len(sys.argv) > 1 and sys.argv[1] not in ("build", "find", "show", "-h", "--help"):
        sys.argv.insert(1, "build")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd")
    b = sub.add_parser("build")
    b.add_argument("pdfs", nargs="*", type=Path)
    b.add_argument("--dpi", type=int, default=120)
    b.add_argument("--force", action="store_true", help="re-render existing PNGs")
    b.add_argument("--no-png", action="store_true", help="rebuild indexes and text only")
    f = sub.add_parser("find")
    f.add_argument("pattern")
    f.add_argument("--doc", default="", help="only folders whose name contains this")
    f.add_argument("--max", type=int, default=2, help="snippets shown per section (one per page)")
    f.add_argument("--all", action="store_true", help="include contents, lists, index and revision history")
    f.add_argument("--count", action="store_true", help="sections and pages only, no snippets")
    s = sub.add_parser("show")
    s.add_argument("doc", help="substring of the doc folder name")
    s.add_argument("what", help="section title regex | 640-645 | printed:582")
    a = ap.parse_args()
    if a.cmd == "find":
        return find(a.pattern, a.doc, a.max, a.all, a.count)
    if a.cmd == "show":
        return show(a.doc, a.what)
    pdfs = [p.resolve() for p in getattr(a, "pdfs", [])] or sorted(DOCS.glob("*.pdf"))
    with ProcessPoolExecutor() as pool:
        for pdf in pdfs:
            build(pdf, getattr(a, "dpi", 120), getattr(a, "force", False), not getattr(a, "no_png", False), pool)
    catalogue()


if __name__ == "__main__":
    sys.exit(main())
