"""Compare two rustc `--emit=asm` files (.s, AT&T syntax) while ignoring layout.

Companion of crates/xtask/style-check/asmdiff.py, which only parses
llvm-objdump logs. The file is cut into bodies at every non-local label:
functions (with their jump tables), named statics and anonymous constants.
Each body is normalized and hashed; the report lists bodies whose normalized
text exists on one side only. Moving a function between modules or crates,
reordering items and building in another checkout are invisible; a changed
instruction, constant, string or call target is not.

Normalization:
  - mangled Rust symbols are reduced to their final identifier (same rule as
    asmdiff.py); a body's references to itself become <self>
  - .def/.scl/.type/.endef, .file/.loc/.cv_*, .globl, .cfi_*/.seh_* and all
    .debug* sections are dropped; .p2align is kept for data only
  - local labels (.LBB3_7, .LJTI3_0 ...) are renumbered per body in order of
    first appearance
  - `anon.<hash>.<n>` constants are replaced by a hash of their content, so a
    changed constant also flags the functions that use it
  - panic-location data is neutralized: a constant that is a source path
    ("...\\file.rs") becomes SRCPATH, a `Location` {path, line, column} becomes
    PANICLOC, and neither is reported on its own
  - the embedded resident payload (the `SVMRELO1...` blob the native child
    includes from SVMVISOR_RESIDENT_PAYLOAD) becomes PAYLOADBLOB and is not
    reported: its bytes contain absolute source paths, so they differ between
    checkouts of identical source. The payload's code is compared separately,
    by asmdiff.py on the payload disassembly. A changed payload *length* still
    shows up in the function that includes it (efi_main).

usage: python sdiff.py BASE.s NEW.s [--show N] [--dump NAME]
  --show N     list at most N bodies per side (default 40)
  --dump NAME  print the normalized text of every body called NAME, both sides
exit status: 0 when no body differs, 1 otherwise.
"""
import hashlib
import re
import sys
from collections import Counter

LEGACY_HASH = re.compile(r"17h[0-9a-f]{16}E$")
LABEL = re.compile(r'^("[^"]+"|[^\s":]+):\s*$')
SECTION = re.compile(r"^\s+\.section\s+([^,\s]+)")
DIRECTIVE = re.compile(r"^\s+(\.[A-Za-z_0-9$]+)")
DROPPED = re.compile(
    r"^\s+\.(def|scl|type|endef|file|loc|globl|weak|hidden|size|ident|text|data|bss"
    r"|addrsig\w*|cv_\w+|cfi_\w+|seh_\w+)\b"
)
ANON = re.compile(r"(?:\.L)?anon\.[0-9a-f]+\.\d+|\.L__unnamed_\d+")
SYMBOL = re.compile(r'(?:\.L)?anon\.[0-9a-f]+\.\d+|\.L[A-Za-z0-9_$.]+|_R[0-9A-Za-z_$.]+|_ZN[0-9A-Za-z_$.]+')
STRING_DIRECTIVES = (".ascii", ".asciz", ".string")
PAYLOAD_BLOB = ('.asciz "SVMRELO1', '.ascii "SVMRELO1')
SOURCE_PATH = re.compile(r'^\.asciz?\s+".*\.rs"$')


def short(symbol):
    """Reduce a mangled Rust symbol to its final identifier (as asmdiff.py)."""
    if symbol.startswith("_R"):
        names, index = [], 2
        while index < len(symbol):
            char = symbol[index]
            tag = re.match(r"[sBL][0-9A-Za-z]*_", symbol[index:]) if char in "sBL" else None
            const = re.match(r"K[a-z][0-9a-f]*_", symbol[index:]) if char == "K" else None
            if tag or const:
                index += (tag or const).end()
            elif char.isdigit():
                size = re.match(r"\d+", symbol[index:]).group(0)
                index += len(size)
                if symbol[index : index + 1] == "_":
                    index += 1
                names.append(symbol[index : index + int(size)])
                index += int(size)
            else:
                index += 1
        names = [name for name in names if name]
        return names[-1] if names else symbol
    return LEGACY_HASH.sub("", symbol)


class Body:
    def __init__(self, label, section):
        self.label = label
        self.section = section
        self.is_anon = bool(ANON.fullmatch(label))
        self.lines = [f"[{section}]"]
        self.text = None  # normalized, filled by resolve()


def parse(path):
    """Cut the file into bodies of raw (whitespace-squeezed) lines."""
    bodies, current = [], None
    section, pending, skipping = ".text", [], False
    with open(path, encoding="utf-8", errors="replace") as handle:
        for raw in handle:
            raw = raw.rstrip("\r\n")
            if not raw.strip():
                continue
            entered = SECTION.match(raw)
            if entered:
                section = entered.group(1)
                skipping = section.startswith(".debug")
                pending = [("section", section)]
                continue
            if skipping or DROPPED.match(raw) or raw.startswith("@feat.00"):
                continue
            label = LABEL.match(raw)
            if label:
                name = label.group(1).strip('"')
                if re.match(r"\.Lfunc_(begin|end)\d+$", name):
                    continue
                if not name.startswith(".L") or ANON.fullmatch(name):
                    current = Body(name, section)
                    bodies.append(current)
                    # alignment written before the label belongs to this body
                    current.lines += [text for kind, text in pending if kind == "align"]
                    pending = []
                    continue
            if current is None:
                current = Body("<preamble>", section)
                bodies.append(current)
            text = " ".join(raw.split())
            if text.startswith(".p2align") or text.startswith(".balign") or text.startswith(".align"):
                if section.startswith(".text"):
                    continue
                pending.append(("align", text))
                continue
            for kind, value in pending:  # a section change inside one body (jump table)
                current.lines.append(f"[{value}]" if kind == "section" else value)
            pending = []
            current.lines.append(text)
    return bodies


def resolve(bodies):
    """Normalize every body; anonymous constants are inlined as content hashes."""
    anon = {body.label: body for body in bodies if body.is_anon}
    active = set()

    def normalized(body):
        if body.text is not None:
            return body.text
        if body.label in active:  # reference cycle between constants
            return "CYCLE"
        active.add(body.label)
        local = {}

        def replace(match):
            token = match.group(0)
            if token == body.label:
                return "<self>"
            if token in anon:
                return "DATA:" + tag(anon[token])
            if token.startswith(".L"):
                return local.setdefault(token, f"L{len(local)}")
            return short(token)

        out = []
        for line in body.lines:
            if line.startswith(STRING_DIRECTIVES):
                out.append(line)
            else:
                out.append(SYMBOL.sub(replace, line))
        active.discard(body.label)
        body.text = "\n".join(out)
        return body.text

    def tag(body):
        text = normalized(body)
        data = [line for line in text.split("\n") if not line.startswith(("[", ".p2align"))]
        if len(data) == 1 and SOURCE_PATH.match(data[0]):
            return "SRCPATH"
        if len(data) == 1 and data[0].startswith(PAYLOAD_BLOB):
            return "PAYLOADBLOB"
        if len(data) == 2 and data[0] == ".quad DATA:SRCPATH" and data[1].startswith(".asci"):
            return "PANICLOC"
        return hashlib.sha256(text.encode()).hexdigest()[:16]

    result = []
    for body in bodies:
        text = normalized(body)
        if len(body.lines) == 1:  # nothing but the section marker
            continue
        if body.is_anon:
            kind = tag(body)
            if kind in ("SRCPATH", "PANICLOC", "PAYLOADBLOB"):
                continue
            preview = next((l for l in text.split("\n") if not l.startswith(("[", ".p2align"))), "")
            name = "anon " + preview[:48]
        else:
            name = short(body.label)
        result.append((name, text, hashlib.sha256(text.encode()).hexdigest()[:16]))
    return result


def main():
    arguments = sys.argv[1:]
    option = lambda flag, default: (
        arguments[arguments.index(flag) + 1] if flag in arguments else default
    )
    show, dump = int(option("--show", 40)), option("--dump", None)
    base, new = resolve(parse(arguments[0])), resolve(parse(arguments[1]))
    if dump is not None:
        for label, items in (("base", base), ("new", new)):
            for name, text, digest in items:
                if name == dump:
                    print(f"=== {label}: {name} [{digest}]\n{text}\n")
        return 0
    base_count = Counter(digest for _, _, digest in base)
    new_count = Counter(digest for _, _, digest in new)
    only_base = [(n, t) for n, t, d in base if new_count[d] < base_count[d]]
    only_new = [(n, t) for n, t, d in new if base_count[d] < new_count[d]]
    lines = lambda items: sum(text.count("\n") for _, text, *_ in items)
    print(f"base: {len(base)} bodies, {lines(base)} lines")
    print(f"new:  {len(new)} bodies, {lines(new)} lines")
    print(f"bodies only in base: {len(only_base)}   only in new: {len(only_new)}")
    for label, items in (("base", only_base), ("new", only_new)):
        for name, text in items[:show]:
            print(f"  {label}: {name} ({text.count(chr(10))} lines)")
    return 1 if only_base or only_new else 0


if __name__ == "__main__":
    sys.exit(main())
