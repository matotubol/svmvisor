"""Read-only decoder for a captured ACPI MADT ("APIC") table plus the FADT
fields that the MADT section cross-references (SCI_INT, APIC flags).

Layouts come from ACPI Specification Release 6.6, as read from rendered pages
of docs/ACPI_Spec_6.6.pdf
(SHA256 8c7542dd4de974ae47bba71bb0336637fe1e3838daad7692370ab4cf218efd35):
  MADT header/flags/types   Tables 5.19-5.21   PDF 204-206 (printed 133-135)
  Local APIC + flags        Tables 5.22-5.23   PDF 207     (printed 136)
  I/O APIC                  Table 5.24         PDF 208     (printed 137)
  ISO + MPS INTI flags      Tables 5.25-5.26   PDF 209     (printed 138)
  NMI Source, LAPIC NMI     Tables 5.27-5.28   PDF 210     (printed 139)
  LAPIC Address Override    Table 5.29         PDF 211     (printed 140)
  Local x2APIC, x2APIC NMI  Tables 5.34-5.35   PDF 214     (printed 143)
  FADT header/SCI_INT       Table 5.9          PDF 182-183 (printed 111-112)
  FADT IAPC_BOOT_ARCH/Flags Table 5.9          PDF 187     (printed 116)
  FADT flag bits 18-21      Table 5.10         PDF 193-194 (printed 122-123)

The script only reads the files named on the command line. It never calls a
firmware-table API and never touches hardware.
"""
import argparse
import hashlib
import json
import pathlib
import struct
import sys

TYPE_NAMES = {
    0x00: 'Processor Local APIC',
    0x01: 'I/O APIC',
    0x02: 'Interrupt Source Override',
    0x03: 'Non-maskable Interrupt (NMI) Source',
    0x04: 'Local APIC NMI',
    0x05: 'Local APIC Address Override',
    0x06: 'I/O SAPIC',
    0x07: 'Local SAPIC',
    0x08: 'Platform Interrupt Sources',
    0x09: 'Processor Local x2APIC',
    0x0A: 'Local x2APIC NMI',
    0x0B: 'GIC CPU Interface (GICC)',
    0x0C: 'GIC Distributor (GICD)',
    0x0D: 'GIC MSI Frame',
    0x0E: 'GIC Redistributor (GICR)',
    0x0F: 'GIC Interrupt Translation Service (ITS)',
    0x10: 'Multiprocessor Wakeup',
    0x11: 'CORE PIC', 0x12: 'LIO PIC', 0x13: 'HT PIC', 0x14: 'EIO PIC',
    0x15: 'MSI PIC', 0x16: 'BIO PIC', 0x17: 'LPC PIC',
    0x18: 'RISC-V RINTC', 0x19: 'RISC-V IMSIC', 0x1A: 'RISC-V APLIC',
    0x1B: 'RISC-V PLIC',
}
# Fixed lengths printed in the 6.6 tables for the structures decoded here.
FIXED_LENGTH = {0x00: 8, 0x01: 12, 0x02: 10, 0x03: 8, 0x04: 6, 0x05: 12,
                0x09: 16, 0x0A: 12}
POLARITY = {0: 'bus-conformant (00)', 1: 'active-high (01)',
            2: 'RESERVED (10)', 3: 'active-low (11)'}
TRIGGER = {0: 'bus-conformant (00)', 1: 'edge (01)',
           2: 'RESERVED (10)', 3: 'level (11)'}
FADT_FLAG_NAMES = {18: 'FORCE_APIC_CLUSTER_MODEL',
                   19: 'FORCE_APIC_PHYSICAL_DESTINATION_MODE',
                   20: 'HW_REDUCED_ACPI',
                   21: 'LOW_POWER_S0_IDLE_CAPABLE'}


class DecodeError(Exception):
    pass


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def text(raw):
    return raw.decode('ascii', 'replace').rstrip('\x00')


def header(data, signature):
    if len(data) < 36:
        raise DecodeError(f'{signature}: file shorter than 36-byte header')
    sig, length, revision, checksum = struct.unpack_from('<4sIBB', data, 0)
    if sig != signature.encode():
        raise DecodeError(f'signature {sig!r} != {signature!r}')
    if length != len(data):
        raise DecodeError(f'{signature}: header length {length} != file size {len(data)}')
    total = sum(data) & 0xFF
    if total != 0:
        raise DecodeError(f'{signature}: checksum sum is 0x{total:02x}, not zero')
    return {
        'signature': signature,
        'length': length,
        'revision': revision,
        'checksum_byte': f'0x{checksum:02x}',
        'checksum_ok': True,
        'oem_id': text(data[10:16]),
        'oem_table_id': text(data[16:24]),
        'oem_revision': f'0x{struct.unpack_from("<I", data, 24)[0]:08x}',
        'creator_id': text(data[28:32]),
        'creator_revision': f'0x{struct.unpack_from("<I", data, 32)[0]:08x}',
    }


def inti(flags, where, problems):
    polarity = flags & 0x3
    trigger = (flags >> 2) & 0x3
    if flags >> 4:
        problems.append(f'{where}: MPS INTI reserved bits 15:4 nonzero (0x{flags:04x})')
    if polarity == 2 or trigger == 2:
        problems.append(f'{where}: reserved polarity/trigger encoding (0x{flags:04x})')
    return {'raw': f'0x{flags:04x}', 'polarity': POLARITY[polarity],
            'trigger': TRIGGER[trigger]}


def cpu_flags(flags, where, problems):
    enabled = bool(flags & 1)
    online_capable = bool(flags & 2)
    if flags & ~0x3:
        problems.append(f'{where}: Local APIC flags reserved bits nonzero (0x{flags:08x})')
    if enabled and online_capable:
        problems.append(f'{where}: Online Capable set while Enabled set (must be zero)')
    if enabled:
        state = 'enabled (ready for use)'
    elif online_capable:
        state = 'disabled, online-capable (may be enabled at OS runtime)'
    else:
        state = 'unusable (Enabled=0, OnlineCapable=0): OSPM shall ignore'
    return {'raw': f'0x{flags:08x}', 'enabled': enabled,
            'online_capable': online_capable, 'state': state}


def decode_madt(data):
    hdr = header(data, 'APIC')
    if len(data) < 44:
        raise DecodeError('APIC: shorter than 44-byte MADT fixed part')
    lapic_address, flags = struct.unpack_from('<II', data, 36)
    problems = []
    if flags & ~1:
        problems.append(f'MADT flags reserved bits nonzero (0x{flags:08x})')
    madt = dict(hdr)
    madt['spec_revision_in_acpi_6_6'] = 7
    madt['local_interrupt_controller_address'] = f'0x{lapic_address:08x}'
    madt['flags'] = {'raw': f'0x{flags:08x}', 'PCAT_COMPAT': bool(flags & 1)}
    entries = []
    offset = 44
    while offset < len(data):
        if offset + 2 > len(data):
            raise DecodeError(f'truncated structure header at offset {offset}')
        kind, length = data[offset], data[offset + 1]
        if length < 2 or offset + length > len(data):
            raise DecodeError(f'bad structure length {length} at offset {offset}')
        body = data[offset:offset + length]
        where = f'offset {offset} type 0x{kind:02x}'
        entry = {'index': len(entries), 'offset': offset, 'type': kind,
                 'type_name': TYPE_NAMES.get(kind, 'reserved/OEM'),
                 'length': length, 'raw': body.hex()}
        expected = FIXED_LENGTH.get(kind)
        if expected is not None and length != expected:
            raise DecodeError(f'{where}: length {length} != 6.6 length {expected}')
        if kind == 0x00:
            uid, apic_id, fl = struct.unpack_from('<BBI', body, 2)
            entry.update(acpi_processor_uid=uid, apic_id=apic_id,
                         flags=cpu_flags(fl, where, problems))
        elif kind == 0x01:
            ioapic_id, reserved, address, gsi_base = struct.unpack_from('<BBII', body, 2)
            if reserved:
                problems.append(f'{where}: I/O APIC reserved byte nonzero')
            entry.update(io_apic_id=ioapic_id, io_apic_id_hex=f'0x{ioapic_id:02x}',
                         address=f'0x{address:08x}', gsi_base=gsi_base)
        elif kind == 0x02:
            bus, source, gsi, fl = struct.unpack_from('<BBIH', body, 2)
            if bus != 0:
                problems.append(f'{where}: ISO bus {bus} is not 0 (ISA)')
            entry.update(bus=bus, source_irq=source, gsi=gsi,
                         identity_mapped=(source == gsi),
                         flags=inti(fl, where, problems))
        elif kind == 0x03:
            fl, gsi = struct.unpack_from('<HI', body, 2)
            entry.update(gsi=gsi, flags=inti(fl, where, problems))
        elif kind == 0x04:
            uid, fl, lint = struct.unpack_from('<BHB', body, 2)
            entry.update(acpi_processor_uid=uid,
                         applies_to_all_processors=(uid == 0xFF),
                         lint=lint, flags=inti(fl, where, problems))
        elif kind == 0x05:
            reserved, address = struct.unpack_from('<HQ', body, 2)
            if reserved:
                problems.append(f'{where}: LAPIC address override reserved nonzero')
            entry.update(local_apic_address=f'0x{address:016x}')
        elif kind == 0x09:
            reserved, x2apic_id, fl, uid = struct.unpack_from('<HIII', body, 2)
            if reserved:
                problems.append(f'{where}: x2APIC reserved nonzero')
            entry.update(x2apic_id=x2apic_id, acpi_processor_uid=uid,
                         flags=cpu_flags(fl, where, problems))
        elif kind == 0x0A:
            fl, uid, lint = struct.unpack_from('<HIB', body, 2)
            if any(body[9:12]):
                problems.append(f'{where}: x2APIC NMI reserved bytes nonzero')
            entry.update(acpi_processor_uid=uid,
                         applies_to_all_processors=(uid == 0xFFFFFFFF),
                         lint=lint, flags=inti(fl, where, problems))
        else:
            problems.append(f'{where}: structure type not decoded by this x86 decoder')
        entries.append(entry)
        offset += length
    if offset != len(data):
        raise DecodeError(f'structure walk ended at {offset}, table length {len(data)}')
    madt['entries'] = entries
    madt['summary'] = summarize(entries, problems)
    madt['problems'] = problems
    return madt


def summarize(entries, problems):
    counts = {}
    for e in entries:
        key = f"0x{e['type']:02x} {e['type_name']}"
        counts[key] = counts.get(key, 0) + 1
    lapic = [e for e in entries if e['type'] == 0x00]
    x2apic = [e for e in entries if e['type'] == 0x09]
    cpus = lapic + x2apic
    enabled = [e for e in cpus if e['flags']['enabled']]
    online = [e for e in cpus if not e['flags']['enabled'] and e['flags']['online_capable']]
    unusable = [e for e in cpus if not e['flags']['enabled'] and not e['flags']['online_capable']]
    ids = [e.get('apic_id', e.get('x2apic_id')) for e in enabled]
    if len(set(ids)) != len(ids):
        problems.append('duplicate APIC IDs among enabled processor entries')
    uids = [e['acpi_processor_uid'] for e in cpus]
    if len(set(uids)) != len(uids):
        problems.append('duplicate ACPI processor UIDs among processor entries')
    ioapics = [e for e in entries if e['type'] == 0x01]
    if len({e['io_apic_id'] for e in ioapics}) != len(ioapics):
        problems.append('duplicate I/O APIC IDs')
    if len({e['address'] for e in ioapics}) != len(ioapics):
        problems.append('duplicate I/O APIC addresses (spec: each resides at a unique address)')
    if sum(1 for e in entries if e['type'] == 0x05) > 1:
        problems.append('more than one Local APIC Address Override (spec allows only one)')
    nmi = [e for e in entries if e['type'] in (0x04, 0x0A)]
    any_ge_255 = any(i >= 255 for i in ids)
    if any_ge_255 and any(e['type'] == 0x04 for e in nmi):
        problems.append('IDs >= 255 present but type 4 NMI used; 6.6 5.2.12.13 requires type 0xA for ALL')
    return {
        'structure_counts': counts,
        'processor_entry_types_present': sorted({f"0x{e['type']:02x}" for e in cpus}),
        'processor_entries_total': len(cpus),
        'enabled_processors': len(enabled),
        'online_capable_processors': len(online),
        'unusable_processor_entries': len(unusable),
        'unusable_processor_uids': [e['acpi_processor_uid'] for e in unusable],
        'enabled_apic_ids_in_table_order': ids,
        'enabled_apic_ids_sorted': sorted(ids),
        'enabled_uid_to_apic_id': {e['acpi_processor_uid']: e.get('apic_id', e.get('x2apic_id')) for e in enabled},
        'first_processor_entry_apic_id': ids[0] if ids else None,
        'max_enabled_apic_id': max(ids) if ids else None,
        'any_enabled_id_ge_255': any_ge_255,
        'nmi_entries': [{'type': f"0x{e['type']:02x}", 'uid': e['acpi_processor_uid'],
                         'all_processors': e['applies_to_all_processors'],
                         'lint': e['lint'], 'flags': e['flags']} for e in nmi],
        'io_apics': [{'id': e['io_apic_id_hex'], 'address': e['address'],
                      'gsi_base': e['gsi_base']} for e in ioapics],
        'interrupt_source_overrides': [{'source_irq': e['source_irq'], 'gsi': e['gsi'],
                                        'flags': e['flags']} for e in entries if e['type'] == 0x02],
        'nmi_sources': [{'gsi': e['gsi'], 'flags': e['flags']} for e in entries if e['type'] == 0x03],
    }


def decode_fadt(data):
    hdr = header(data, 'FACP')
    fadt = dict(hdr)
    fadt['fadt_major_version_offset8'] = data[8]
    if len(data) > 131:
        fadt['fadt_minor_version_offset131'] = data[131]
    if len(data) < 116:
        raise DecodeError('FACP too short for Flags at offset 112')
    fadt['firmware_ctrl_offset36'] = f'0x{struct.unpack_from("<I", data, 36)[0]:08x}'
    fadt['dsdt_offset40'] = f'0x{struct.unpack_from("<I", data, 40)[0]:08x}'
    fadt['preferred_pm_profile_offset45'] = data[45]
    fadt['sci_int_offset46'] = struct.unpack_from('<H', data, 46)[0]
    fadt['iapc_boot_arch_offset109'] = f'0x{struct.unpack_from("<H", data, 109)[0]:04x}'
    flags = struct.unpack_from('<I', data, 112)[0]
    fadt['flags_offset112'] = {'raw': f'0x{flags:08x}'}
    for bit, name in FADT_FLAG_NAMES.items():
        fadt['flags_offset112'][f'bit{bit}_{name}'] = bool(flags >> bit & 1)
    return fadt


def manifest_record(manifest, path, digest):
    if manifest is None:
        return None
    for record in manifest.get('tables', []):
        if pathlib.Path(record.get('path', '')).name == path.name:
            return {'manifest_sha256': record.get('sha256'),
                    'manifest_bytes': record.get('bytes'),
                    'sha256_matches_manifest': record.get('sha256') == digest,
                    'captured_utc': manifest.get('captured_utc'),
                    'source': manifest.get('source'),
                    'board': manifest.get('board')}
    return {'note': 'file not listed in manifest'}


def ivrs_ioapic_handles(path):
    if path is None:
        return None
    data = json.loads(pathlib.Path(path).read_text(encoding='utf-8'))
    handles = set()
    for block in data.get('blocks', []):
        for dev in block.get('devices', []):
            if dev.get('variety') == 'ioapic':
                handles.add((dev['handle'], dev.get('source_device_id')))
    return {'source': str(path), 'source_sha256_field': data.get('source_sha256'),
            'ioapic_handles': [{'handle': f'0x{h:02x}', 'source_device_id': f'0x{s:04x}'}
                               for h, s in sorted(handles)]}


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('madt')
    parser.add_argument('--fadt')
    parser.add_argument('--manifest')
    parser.add_argument('--ivrs-admission')
    parser.add_argument('--json', required=True)
    args = parser.parse_args()

    manifest = None
    if args.manifest:
        manifest = json.loads(pathlib.Path(args.manifest).read_text(encoding='utf-8-sig'))
    report = {'decoder': 'decode_madt.py (read-only, ACPI 6.6 layouts)'}
    madt_path = pathlib.Path(args.madt)
    madt_bytes = madt_path.read_bytes()
    report['madt_file'] = {'path': str(madt_path), 'bytes': len(madt_bytes),
                           'sha256': sha256(madt_bytes),
                           'provenance': manifest_record(manifest, madt_path, sha256(madt_bytes))}
    report['madt'] = decode_madt(madt_bytes)
    if args.fadt:
        fadt_path = pathlib.Path(args.fadt)
        fadt_bytes = fadt_path.read_bytes()
        report['fadt_file'] = {'path': str(fadt_path), 'bytes': len(fadt_bytes),
                               'sha256': sha256(fadt_bytes),
                               'provenance': manifest_record(manifest, fadt_path, sha256(fadt_bytes))}
        report['fadt'] = decode_fadt(fadt_bytes)
    ivrs = ivrs_ioapic_handles(args.ivrs_admission)
    if ivrs is not None:
        madt_ids = {e['io_apic_id'] for e in report['madt']['entries'] if e['type'] == 0x01}
        ivrs_ids = {int(h['handle'], 16) for h in ivrs['ioapic_handles']}
        ivrs['madt_ioapic_ids_equal_ivrs_ioapic_handles'] = madt_ids == ivrs_ids
        report['ivrs_cross_check'] = ivrs
    pathlib.Path(args.json).write_text(json.dumps(report, indent=2), encoding='utf-8')

    m = report['madt']
    s = m['summary']
    print(f"MADT {madt_path.name}: {len(madt_bytes)} bytes sha256={report['madt_file']['sha256']}")
    print(f"  checksum ok, revision {m['revision']} (ACPI 6.6 Table 5.19 lists 7), "
          f"OEM {m['oem_id']!r}/{m['oem_table_id']!r} rev {m['oem_revision']}, "
          f"creator {m['creator_id']!r} {m['creator_revision']}")
    print(f"  LAPIC address {m['local_interrupt_controller_address']}, flags {m['flags']['raw']} "
          f"PCAT_COMPAT={int(m['flags']['PCAT_COMPAT'])}")
    for e in m['entries']:
        line = f"  [{e['index']:2}] off {e['offset']:3} type 0x{e['type']:02x} len {e['length']:2} {e['type_name']}"
        if e['type'] == 0x00:
            line += f": UID {e['acpi_processor_uid']} APIC ID {e['apic_id']} (0x{e['apic_id']:02x}) flags {e['flags']['raw']} {e['flags']['state']}"
        elif e['type'] == 0x09:
            line += f": UID {e['acpi_processor_uid']} x2APIC ID {e['x2apic_id']} flags {e['flags']['raw']} {e['flags']['state']}"
        elif e['type'] in (0x04, 0x0A):
            line += (f": UID 0x{e['acpi_processor_uid']:x} (all={e['applies_to_all_processors']}) "
                     f"LINT{e['lint']} {e['flags']['raw']} {e['flags']['polarity']} {e['flags']['trigger']}")
        elif e['type'] == 0x01:
            line += f": ID {e['io_apic_id_hex']} addr {e['address']} GSI base {e['gsi_base']}"
        elif e['type'] == 0x02:
            line += (f": bus {e['bus']} IRQ {e['source_irq']} -> GSI {e['gsi']} "
                     f"{e['flags']['raw']} {e['flags']['polarity']} {e['flags']['trigger']}")
        elif e['type'] == 0x03:
            line += f": GSI {e['gsi']} {e['flags']['raw']} {e['flags']['polarity']} {e['flags']['trigger']}"
        elif e['type'] == 0x05:
            line += f": {e['local_apic_address']}"
        print(line)
    print('  summary:', json.dumps({k: s[k] for k in (
        'structure_counts', 'processor_entry_types_present', 'enabled_processors',
        'online_capable_processors', 'unusable_processor_entries',
        'enabled_apic_ids_sorted', 'max_enabled_apic_id', 'any_enabled_id_ge_255')}))
    print('  problems:', m['problems'] or 'none')
    if 'fadt' in report:
        f = report['fadt']
        print(f"FADT {pathlib.Path(args.fadt).name}: {f['length']} bytes sha256={report['fadt_file']['sha256']} "
              f"checksum ok, version {f['fadt_major_version_offset8']}.{f.get('fadt_minor_version_offset131')}, "
              f"SCI_INT {f['sci_int_offset46']}, IAPC_BOOT_ARCH {f['iapc_boot_arch_offset109']}, "
              f"Flags {json.dumps(f['flags_offset112'])}, DSDT {f['dsdt_offset40']}")
    if ivrs is not None:
        print('IVRS cross-check:', json.dumps(ivrs))
    for key in ('madt_file', 'fadt_file'):
        prov = report.get(key, {}).get('provenance')
        if prov:
            print(f'{key} provenance:', json.dumps(prov))
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except DecodeError as error:
        print(f'DECODE ERROR: {error}', file=sys.stderr)
        sys.exit(2)
