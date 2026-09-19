"""Review aid: token-level difference between two revisions, ignoring layout.

Tokenizes every .rs file under crates/ at both revisions (comments kept as
single tokens, whitespace dropped) and compares the multisets. Moving code
between files, reordering items and rustfmt re-wrapping leave no difference.
What remains is what was genuinely added or removed: `use` paths, `mod`
lines, `pub(super)`, attributes, comments -- or a changed literal, operator
or identifier, which is what this tool exists to catch.

Tokens that only ever come from imports/visibility/module plumbing are
summarized; everything else is listed.

usage: python tokenbag.py BASE_REV NEW_REV
"""
import re
import subprocess
import sys
from collections import Counter

TOKEN = re.compile(
    r"""//[^\n]*|/\*.*?\*/                       # comments
      |r\#*".*?"\#*                              # raw strings
      |b?"(?:\\.|[^"\\])*"                       # string literals
      |b?'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}|.)|[^'\\])'   # char / byte literals
      |'[A-Za-z_]\w*                             # lifetimes and labels
      |0x[0-9a-fA-F_]+\w*|\d[\d_]*(?:\.\d[\d_]*)?\w*   # numbers
      |[A-Za-z_]\w*!?                            # identifiers, macros
      |::|->|=>|==|!=|<=|>=|&&|\|\||<<=|>>=|<<|>>|\+=|-=|\*=|/=|%=|\^=|&=|\|=|\.\.=|\.\.\.|\.\.
      |[^\s]""",
    re.S | re.X,
)
# Trailing commas come and go with rustfmt wrapping; braces/parens with `use` trees.
LAYOUT = {",", "{", "}", "(", ")", ";", "::"}
PLUMBING = {"use", "pub", "super", "crate", "self", "mod", "as", "*"}


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True, encoding="utf-8",
                          errors="replace", check=True).stdout


def bag(rev):
    tokens = Counter()
    for path in git("ls-tree", "-r", "--name-only", rev, "crates").splitlines():
        if path.endswith(".rs"):
            text = git("show", f"{rev}:{path}")
            tokens.update(re.sub(r"\s+", " ", t) for t in TOKEN.findall(text))
    return tokens


def main():
    base, new = bag(sys.argv[1]), bag(sys.argv[2])
    removed, added = base - new, new - base
    print(f"tokens: {sum(base.values())} -> {sum(new.values())}")
    for label, side in (("REMOVED", removed), ("ADDED", added)):
        layout = sum(c for t, c in side.items() if t in LAYOUT)
        plumbing = {t: c for t, c in side.items() if t in PLUMBING}
        rest = {t: c for t, c in side.items() if t not in LAYOUT and t not in PLUMBING}
        idents = {t: c for t, c in rest.items() if re.match(r"^[A-Za-z_]\w*!?$", t)}
        other = {t: c for t, c in rest.items() if t not in idents}
        print(f"\n{label}: layout punctuation {layout}; plumbing {plumbing}")
        print(f"  identifiers ({sum(idents.values())}): "
              + ", ".join(f"{t}x{c}" for t, c in sorted(idents.items(), key=lambda kv: -kv[1])[:60]))
        print(f"  literals / operators / comments / attributes ({sum(other.values())}):")
        for token, count in sorted(other.items(), key=lambda kv: -kv[1])[:40]:
            print(f"    {count}x {token[:140]}")


if __name__ == "__main__":
    main()
