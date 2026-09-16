"""Fresh Windows TCG build from the prepared, pinned NRIP source tree.

The predecessor runtime supplies only DLLs and firmware. Never modifies the
predecessor source/runtime or writes to the archived checkout.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
WORK = ROOT / 'work/qemu-nrip'
PRIOR = ROOT / 'work/qemu-init-sx/build-attempt-03'
PRIOR_SHA = '677158d2f10933bfc8770e3741a3c6ebf33466d1f7f71fee87e6aec3e009b240'


def sha(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def tree(path):
    return {p.relative_to(path).as_posix(): sha(p)
            for p in sorted(path.rglob('*')) if p.is_file()}


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n', encoding='utf-8')


def msys(path):
    value = path.resolve().as_posix()
    if len(value) < 3 or value[1:3] != ':/':
        raise ValueError('Expected an absolute Windows drive path')
    return '/' + value[0].lower() + value[2:]


def build(out):
    source = WORK / 'source'
    toolchain = WORK / 'toolchain'
    previous_exe = PRIOR / 'runtime/bin/qemu-system-x86_64.exe'
    if sha(previous_exe) != PRIOR_SHA:
        raise RuntimeError('Predecessor executable pin mismatch')
    out.mkdir(parents=True, exist_ok=False)
    before = tree(source)
    previous = json.loads((WORK / 'predecessor-source-manifest.json').read_text())
    if previous.keys() - before.keys():
        raise RuntimeError('Unexpected deleted predecessor source files')
    save(out / 'source-manifest.json', before)
    changed = {name: value for name, value in before.items()
               if value != previous.get(name)}
    for name, expected in changed.items():
        archived = out / 'source-changed' / name
        archived.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source / name, archived)
        if sha(archived) != expected:
            raise RuntimeError('Source changed during archival: ' + name)
    save(out / 'changed-source-manifest.json', changed)
    script = (HERE / 'build.sh').read_text(encoding='utf-8')
    script = script.replace('@SOURCE@', msys(source)).replace('@TOOLCHAIN@', msys(toolchain))
    (out / 'build.sh').write_text(script, encoding='utf-8', newline='\n')
    shutil.copyfile(HERE / 'standalone_ninja.py', out / 'standalone_ninja.py')
    with (out / 'build.log').open('w', encoding='utf-8') as log:
        subprocess.run(['C:/Program Files/Git/bin/bash.exe', str(out / 'build.sh')],
                       cwd=ROOT, check=True, stdout=log, stderr=subprocess.STDOUT)
    if tree(source) != before:
        raise RuntimeError('Source changed during build; do not use this result')
    executable = out / 'build/qemu-system-x86_64.exe'
    runtime = out / 'runtime'
    shutil.copytree(PRIOR / 'runtime', runtime)
    shutil.copyfile(executable, runtime / 'bin/qemu-system-x86_64.exe')
    manifest = {
        'build': 'full fresh configure/compile/link; inherited runtime DLLs and firmware only',
        'qemu_base_revision': 'f8b2f64e2336a28bf0d50b6ef8a7d8c013e9bcf3',
        'changed_source': list(changed),
        'source_manifest_sha256': sha(out / 'source-manifest.json'),
        'changed_source_archive': 'source-changed',
        'changed_source_manifest_sha256': sha(out / 'changed-source-manifest.json'),
        'predecessor_source_manifest_sha256': sha(WORK / 'predecessor-source-manifest.json'),
        'predecessor_executable_sha256': PRIOR_SHA,
        'executable_sha256': sha(executable),
        'build_tools': {p.name: sha(p) for p in [HERE / 'build.py', HERE / 'build.sh', HERE / 'standalone_ninja.py']},
        'toolchain': {name: sha(toolchain / 'mingw64/bin' / name)
                      for name in ['gcc.exe', 'ld.exe', 'ninja.exe', 'python.exe']},
        'runtime_files': tree(runtime),
    }
    save(out / 'build-manifest.json', manifest)
    print('Built', executable, 'SHA256', manifest['executable_sha256'], flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    build(parser.parse_args().output.resolve())
