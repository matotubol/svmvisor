"""Compile actual backend C bodies with bounded CPU/memory/unwind stubs.

This verifies field publication and policy, not TCG translation or IDT delivery.
The separate emulator fixture is required for execution evidence.
"""
from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import subprocess


def body(text, signature):
    start = text.index(signature)
    end = text.index('{', start) + 1
    depth = 1
    while depth:
        depth += (text[end] == '{') - (text[end] == '}')
        end += 1
    return text[start:end] + '\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--cc', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    source, out = args.source.resolve(), args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    names = ['target/i386/tcg/system/svm_helper.c', 'target/i386/svm.h',
             'target/i386/cpu.h', 'target/i386/tcg/translate.c',
             'target/i386/tcg/emit.c.inc', 'target/i386/tcg/excp_helper.c']
    texts = {n: (source / n).read_text() for n in names}
    svm = texts[names[0]]
    actual = body(svm, 'static uint64_t svm_next_rip(')
    actual += body(svm, 'void cpu_vmexit(')
    start = svm.index('        case SVM_EVTINJ_TYPE_EXEPT:')
    end = svm.index('        default:', start)
    actual += ('void inject(CPUX86State *env, unsigned kind, unsigned vector) {\n'
               'CPUState *cs=env_cpu(env); unsigned event_inj_err=0;\n'
               'switch (kind) {\n' + svm[start:end] + 'default: abort(); } }\n')
    defines = []
    for line in texts['target/i386/cpu.h'].splitlines():
        if re.match(r'#define (HF_CS(?:32|64)_(?:MASK|SHIFT)|CPUID_SVM_NRIPSAVE)\b', line):
            defines.append(line)
    (out / 'defines.inc').write_text('\n'.join(defines) + '\n')
    (out / 'svm.h').write_text(texts['target/i386/svm.h'])
    (out / 'actual.inc').write_text(actual)
    test = Path(__file__).with_name('test_nrip.c')
    (out / 'test.c').write_bytes(test.read_bytes())
    cc = args.cc.resolve()
    env = os.environ.copy()
    env['PATH'] = str(cc.parent) + os.pathsep + env['PATH']
    command = [str(cc), '-std=gnu11', '-O2', '-Wall', '-Wextra',
               '-Wno-unused-parameter', str(out / 'test.c'), '-o', str(out / 'test.exe')]
    with (out / 'compile.log').open('w') as log:
        subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
    result = subprocess.run([str(out / 'test.exe')], env=env, capture_output=True, text=True)
    (out / 'result.log').write_text(result.stdout + result.stderr)
    digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
    manifest = {'scope': __doc__, 'sources': {n: digest(source / n) for n in names},
                'test_sha256': digest(test), 'actual_sha256': digest(out / 'actual.inc'),
                'compiler_sha256': digest(cc), 'command': command,
                'exit_code': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}
    (out / 'result.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(result.stdout, end='')
    result.check_returncode()


if __name__ == '__main__':
    main()
