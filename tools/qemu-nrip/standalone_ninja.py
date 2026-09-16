"""Freeze a generated Ninja graph when optional symlink postconf cannot finish.

Compilation and link rules are copied verbatim. Only the self-regeneration edge
is omitted; this is deliberately a frozen build, not an incremental Meson tree.
"""
import hashlib
import json
import pathlib

root = pathlib.Path(__file__).resolve().parent
original = root / 'build/build.ninja'
data = original.read_bytes()
lines = data.splitlines(keepends=True)
indices = [i for i, line in enumerate(lines) if line.startswith(b'build build.ninja: REGENERATE_BUILD ')]
if len(indices) != 1:
    raise RuntimeError('Expected exactly one Ninja self-regeneration edge')
start = indices[0]
end = start + 1
while end < len(lines) and lines[end].startswith(b' '):
    end += 1
removed = lines[start:end]
if len(removed) != 2 or removed[1].strip() != b'pool = console':
    raise RuntimeError('Unexpected regeneration edge bindings')
frozen = b''.join(lines[:start] + lines[end:])
(root / 'build/build-standalone.ninja').write_bytes(frozen)
(root / 'standalone-ninja-manifest.json').write_text(json.dumps({
    'originalSha256': hashlib.sha256(data).hexdigest(),
    'frozenSha256': hashlib.sha256(frozen).hexdigest(),
    'omittedRule': b''.join(removed).decode().rstrip(),
    'reason': 'Only optional QEMU symlink postconf failed (WinError1314); no source/build command modification.',
}, indent=2) + '\n')
