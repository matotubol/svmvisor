"""Strict offline audit of the bounded I/O execution trace; no emulator launch."""
import json
import re
import statistics
import sys
from pathlib import Path


def expected_cases():
    for round_id in range(8):
        yield (round_id, 0x3ff, 1, False, False, False, True, 1)
        yield (round_id, 0x3ff, 1, True, False, False, True, 1)
        yield (round_id, 0x3ff, 1, False, False, False, False, 1)
        yield (round_id, 0x3ff, 1, True, False, False, False, 1)
        for width in (1, 2, 4):
            for port in (0, 7, 0x1234, 0xfffc, 0xfffd, 0xfffe, 0xffff):
                for direction in (False, True):
                    yield (round_id, port, width, direction, False, False, False, 2 if width == 2 else 1)
            for direction in (False, True):
                yield (round_id, 0x80, width, direction, False, False, False, 3 if width == 2 else 2)
        for port, width in ((7, 2), (7, 4), (0x7fff, 2), (0x7fff, 4),
                            (0xffff, 2), (0xffff, 4), (0xfffe, 4), (0xfffd, 4)):
            for direction in (False, True):
                yield (round_id, port, width, direction, False, False, False, 2 if width == 2 else 1)
        for direction, rep, width, length in ((False, False, 1, 1), (True, False, 1, 1),
                (False, True, 1, 2), (True, True, 1, 2), (False, True, 2, 3), (True, True, 4, 2)):
            yield (round_id, 0x3ff, width, direction, True, rep, False, length)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def audit(record):
    audit_environment(record)
    if record['missingEndpoint'] or record['bypassBoundary']:
        return audit_negative(record)
    require(record['status'] == 'passed' and record['exitCode'] == 33 and not record['timedOut']
            and not record['stderr'] and not record['missingEndpoint'] and not record['bypassBoundary'], 'run outcome')
    require(record['qemuSha256'].lower() == 'c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047', 'backend hash')
    trace = record['trace']
    require(record['profile'] in ('Avx', 'Sse', 'Fx'), 'xstate profile')
    profile_marker = {'Avx': 'xsave-avx', 'Sse': 'xsave-sse', 'Fx': 'fxsave'}[record['profile']]
    require(trace.splitlines().count('xstate-profile=' + profile_marker) == 1, 'executed xstate profile')
    cpu = 'max,svm=on,hypervisor=off' + (',rdtscp=off' if record['rdtscpDisabled'] else '')
    cpu += {'Avx': '', 'Sse': ',avx=off,avx2=off', 'Fx': ',xsave=off,avx=off,avx2=off'}[record['profile']]
    require(record['cpu'] == cpu and record['arguments'][record['arguments'].index('-cpu') + 1] == cpu, 'CPU profile')
    for probe in ('IO-PROBE first=0000000000000036', 'IO-PROBE second=0000000000000069'):
        require(trace.splitlines().count(probe) == 1, 'endpoint control probe')
    require(not re.search(r'^(FAIL|GAP|REFUSE) ', trace, re.M), 'failure trace')
    for marker in ('PASS io-boundary stopped-in-out-live-endpoint-pending-preserved', 'PASS rust-dispatch'):
        require(trace.splitlines().count(marker) == 1, 'final marker')
    rows = []
    current = None
    cases = None
    for line in trace.splitlines():
        if not line.startswith('IO '):
            continue
        if line == 'IO case-pass':
            require(current is not None, 'orphan case marker')
            rows.append(current)
            current = None
            continue
        match = re.fullmatch(r'IO ([a-z0-9-]+)=([0-9a-f]{16})', line)
        require(match is not None, 'malformed metric')
        key, value = match[1], int(match[2], 16)
        if key == 'id':
            require(current is None and cases is None, 'unfinished/late case')
            current = {}
        if key == 'cases':
            require(current is None and cases is None, 'duplicate summary')
            cases = value
        else:
            require(current is not None and key not in current, 'orphan/duplicate metric')
            current[key] = value
    expected = list(expected_cases())
    require(current is None and len(rows) == cases == len(expected) == 592, 'case count')
    base_fields = set('id code info1 info2 rip rax flags marker rcx rdx rsi rdi rsp endpoint-before endpoint-after pending-before pending-after entry-ticks'.split())
    for index, (row, spec) in enumerate(zip(rows, expected)):
        round_id, port, width, input_, string, rep, allowed, length = spec
        require(set(row) == base_fields | (set() if allowed else {'service-ticks'}), 'metric fields')
        require(row['rcx'] == 3 and row['rdx'] == port and row['rsi'] == row['rdi'] == 0x8000 and row['rsp'] == 0x9000, 'retained registers')
        require(row['id'] == index and row['flags'] == 0x8d7 and row['entry-ticks'] > 0, 'id/flags/ticks')
        require(row['pending-before'] == row['pending-after'] == 0x6101060100, 'pending IRQ')
        before = 0x69 ^ round_id
        require(row['endpoint-before'] == before, 'endpoint setup')
        require(row['endpoint-after'] == (0xa5 if allowed and not input_ else before), 'endpoint side effect')
        rax = 0x123456789abcdea5
        require(row['rax'] == ((rax & ~255) | before if allowed and input_ else rax), 'destination RAX')
        require(row['marker'] == (0xfeedface01234567 if allowed else 15), 'post-instruction marker')
        require(row['code'] == (0x81 if allowed else 0x7b), 'exit code')
        require(row['rip'] == (0x1000 + length + 21 if allowed else 0x1000), 'stopped RIP')
        if not allowed:
            info = (port << 16) | (width << 4) | int(input_) | (int(string) << 2) | (int(rep) << 3)
            # The pinned backend omits ASIZE/SEG. This exact backend contract
            # is evidence of its limitation, not a claim those fields are zero on AMD.
            require(row['info1'] == info and row['info2'] == 0x1000 + length, 'IOIO metadata')
            require(row['service-ticks'] > 0, 'service timing')
    def distribution(values):
        values = sorted(values)
        return dict(samples=len(values), minimum=values[0], median=statistics.median(values),
                    p95=values[int((len(values)-1)*.95)], maximum=values[-1])
    return dict(cases=cases, allowed=16, refused=576, telemetryLoss=0,
                entryTicks=distribution([r['entry-ticks'] for r in rows]),
                refusalServiceTicks=distribution([r['service-ticks'] for r in rows if 'service-ticks' in r]))


def audit_negative(record):
    trace = record['trace']
    require(record['status'] == 'passed' and record['exitCode'] == 35 and not record['timedOut']
            and not record['stderr'], 'negative outcome')
    require(record['qemuSha256'].lower() == 'c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047', 'negative backend')
    require(trace.splitlines().count('FAIL rust-dispatch') == 1 and 'PASS io-boundary' not in trace
            and 'PASS rust-dispatch' not in trace, 'negative terminal marker')
    if record['missingEndpoint']:
        require(not record['bypassBoundary'] and 'IO id=' not in trace, 'missing endpoint scope')
        require(trace.splitlines().count('IO-PROBE first=00000000000000ff') == 1 and
                trace.splitlines().count('REFUSE io-endpoint first-probe') == 1 and
                'IO-PROBE second=' not in trace, 'absent endpoint witness')
        return {'expectedRefusal': 'missing-endpoint', 'guestEntries': 0}
    require(trace.splitlines().count('IO case-pass') == 2, 'bypass completed control count')
    for probe in ('IO-PROBE first=0000000000000036', 'IO-PROBE second=0000000000000069'):
        require(trace.splitlines().count(probe) == 1, 'negative live endpoint probe')
    require(re.findall(r'^IO id=([0-9a-f]{16})$', trace, re.M) ==
            ['0000000000000000', '0000000000000001', '0000000000000002'], 'bypass entry order')
    # Validate the complete negative trace by replacing only the expected
    # assertion stop with a local three-row check (never relabel as positive).
    blocks = trace.split('IO id=')[1:]
    for index, block in enumerate(blocks):
        pairs = re.findall(r'^IO ([a-z0-9-]+)=([0-9a-f]{16})$', block, re.M)
        require(len(pairs) == 17 and len(dict(pairs)) == len(pairs), 'negative metric fields')
        for line in block.splitlines():
            if line.startswith('IO '):
                require(line == 'IO case-pass' or re.fullmatch(r'IO ([a-z0-9-]+)=([0-9a-f]{16})', line), 'negative malformed metric')
        row = {k: int(v, 16) for k, v in pairs}
        require(set(row) == set('code info1 info2 rip rax flags marker rcx rdx rsi rdi rsp endpoint-before endpoint-after pending-before pending-after entry-ticks'.split()), 'negative exact fields')
        require(row['entry-ticks'] > 0, 'negative entry timing')
        require(row['code'] == 0x81 and row['rip'] == 0x1016 and row['marker'] == 0xfeedface01234567,
                'bypass instruction completion')
        require(row['flags'] == 0x8d7 and row['rcx'] == 3 and row['rdx'] == 0x3ff and
                row['rsi'] == row['rdi'] == 0x8000 and row['rsp'] == 0x9000, 'negative registers')
        require(row['pending-before'] == row['pending-after'] == 0x6101060100, 'negative pending')
        require(row['endpoint-before'] == 0x69 and row['endpoint-after'] == (0x69 if index == 1 else 0xa5),
                'bypass changed endpoint witness')
        require(row['rax'] == (0x123456789abcde69 if index == 1 else 0x123456789abcdea5), 'negative RAX')
    return {'expectedRefusal': 'boundary-bypass-witness', 'guestEntries': 3, 'completedControls': 2}


def audit_environment(record):
    trace = record['trace']
    require(record['profile'] in ('Avx', 'Sse', 'Fx'), 'environment profile')
    marker = {'Avx': 'xsave-avx', 'Sse': 'xsave-sse', 'Fx': 'fxsave'}[record['profile']]
    require(trace.splitlines().count('xstate-profile=' + marker) == 1, 'environment executed profile')
    require(re.fullmatch('[0-9a-fA-F]{64}', record['imageSha256']), 'image hash format')
    cpu = 'max,svm=on,hypervisor=off' + (',rdtscp=off' if record['rdtscpDisabled'] else '')
    cpu += {'Avx': '', 'Sse': ',avx=off,avx2=off', 'Fx': ',xsave=off,avx=off,avx2=off'}[record['profile']]
    args = record['arguments']
    require(record['cpu'] == cpu, 'environment CPU')
    for flag, value in (('-machine','pc-i440fx-10.1'),('-accel','tcg,thread=single'),('-cpu',cpu),
            ('-m','64M'),('-smp','1'),('-display','none'),('-monitor','none'),('-serial','none'),('-nic','none')):
        require(args.count(flag) == 1 and args[args.index(flag)+1] == value, 'environment ' + flag)
    endpoint = 'isa-serial,chardev=io-witness,iobase=0x3f8,irq=4'
    require(args.count(endpoint) == (0 if record['missingEndpoint'] else 1), 'endpoint argument')


if __name__ == '__main__':
    print(json.dumps(audit(json.loads(Path(sys.argv[1]).read_text(encoding='utf-8-sig'))), indent=2))
