"""Compare two llvm-objdump disassembly logs while ignoring layout.

Each function body is normalized (addresses, rip-relative displacements and
symbol offsets removed; mangled Rust names reduced to their final path
segment) and hashed. The report lists functions whose normalized body exists
on only one side. Moves, renames of modules and reordering do not show up;
changed instructions do.

usage: python asmdiff.py BASE.log NEW.log [--show N]
"""
import hashlib
import re
import sys
from collections import Counter

HEADER = re.compile(r"^[0-9a-f]{16} <(.+)>:$")
LEGACY_HASH = re.compile(r"17h[0-9a-f]{16}E$")


def short(symbol):
    """Reduce a mangled Rust symbol to something stable across module moves."""
    if symbol.startswith("_R"):
        # v0 mangling: keep the identifier segments, drop crate hashes/paths.
        # The crate disambiguator (`Cs<base62>_`) is derived from the workspace
        # path, so it differs between git worktrees of identical source.
        # Identifiers are length-prefixed, so they must be scanned, not regexed:
        # a greedy match swallows the rest of the symbol. Base-62 tags
        # (`s0_` disambiguators, `B5_` back references, `L1_` lifetimes) contain
        # digits that are not identifier lengths and are skipped.
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
        # The final identifier survives a move between modules; the path does not.
        return names[-1] if names else symbol
    return LEGACY_HASH.sub("", symbol)


def normalize(line, own):
    line = line.split("\t", 1)[1] if "\t" in line else line
    line = re.sub(r"#.*$", "", line)                       # objdump comments
    # A jump inside a function is printed as <own_name+offset>; keeping the name
    # would flag every renamed function that has internal branches.
    line = re.sub(
        r"<([^>+]+)(\+0x[0-9a-f]+)?>",
        lambda m: "<self>" if m.group(1) == own else "<" + short(m.group(1)) + ">",
        line,
    )
    line = re.sub(r"-?0x[0-9a-f]+\(%rip\)", "REL(%rip)", line)
    line = re.sub(r"\b0x[0-9a-f]{5,}\b(?= <)", "ADDR", line)  # branch targets
    return line.strip()


def functions(path):
    result, name, body, own = [], None, [], None
    with open(path, encoding="utf-8", errors="replace") as handle:
        for raw in handle:
            raw = raw.rstrip("\n")
            header = HEADER.match(raw)
            if header:
                if name is not None:
                    result.append((name, body))
                own = header.group(1)
                name, body = short(own), []
            elif name is not None and raw.startswith(" "):
                text = normalize(raw, own)
                if text and text != "int3" and not text.startswith("nop"):
                    body.append(text)
        if name is not None:
            result.append((name, body))
    return result


def digest(body):
    return hashlib.sha256("\n".join(body).encode()).hexdigest()[:16]


def main():
    base, new = functions(sys.argv[1]), functions(sys.argv[2])
    show = int(sys.argv[sys.argv.index("--show") + 1]) if "--show" in sys.argv else 40
    base_count = Counter(digest(body) for _, body in base)
    new_count = Counter(digest(body) for _, body in new)
    only_base = [(n, b) for n, b in base if new_count[digest(b)] < base_count[digest(b)]]
    only_new = [(n, b) for n, b in new if base_count[digest(b)] < new_count[digest(b)]]
    instructions = lambda items: sum(len(body) for _, body in items)
    print(f"base: {len(base)} functions, {instructions(base)} instructions")
    print(f"new:  {len(new)} functions, {instructions(new)} instructions")
    print(f"bodies only in base: {len(only_base)}   only in new: {len(only_new)}")
    for label, items in (("base", only_base), ("new", only_new)):
        for name, body in items[:show]:
            print(f"  {label}: {name} ({len(body)} instructions)")
    return 1 if only_base or only_new else 0


if __name__ == "__main__":
    sys.exit(main())
