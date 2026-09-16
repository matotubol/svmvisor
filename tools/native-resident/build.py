"""Build a raw resident payload, audit its complete linked code, then its DXE shim.
No firmware execution. Output directories are fresh and retain failure logs.
"""
import argparse, hashlib, json, os, re, shutil, subprocess, sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]

def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()
def sources():
    paths = {ROOT/name for name in ('Cargo.toml', 'Cargo.lock', '.cargo/config.toml',
        'AGENTS.md', 'README.md', 'CONTRIBUTING.md', 'docs/malware-analysis-direction.md',
        'tools/synthetic-harness/package-relocations.py')}
    for directory in ('crates/hypervisor', 'crates/dxe', 'crates/memory-attributes',
                      'tools/native-resident', 'tools/synthetic-harness/firmware-handoff'):
        paths.update(p for p in (ROOT/directory).rglob('*') if p.is_file()
            and not {'target', '__pycache__'}.intersection(p.relative_to(ROOT/directory).parts))
    return {str(p.relative_to(ROOT)).replace('\\', '/'): sha(p) for p in sorted(paths)}
def command(args,out,name,env=None):
    executable=shutil.which(str(args[0]))
    if not executable: raise RuntimeError(f'missing tool {args[0]}')
    result=subprocess.run([executable,*map(str,args[1:])],cwd=ROOT,env=env,text=True,
        stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
    (out/(name+'.log')).write_text(result.stdout,encoding='utf-8')
    if result.returncode: raise RuntimeError(f'{name} exit {result.returncode}\n{result.stdout[-6000:]}')
    return result.stdout

def audit_debug_reset(text):
    """Permit only the exact successful guest-INIT DR0-3 clearing helper."""
    owner = None
    body = []
    symbol = 'svmvisor_resident_reset_guest_debug'
    for line in text.splitlines():
        label = re.fullmatch(r'\s*[0-9a-f]+ <([^>]+)>:', line)
        if label:
            owner = label[1]
            continue
        instruction = re.fullmatch(r'\s*[0-9a-f]+:\s+(.+)', line)
        if not instruction:
            continue
        value = re.sub(r'\s+', ' ', instruction[1]).strip()
        if re.search(r'%dr[0-7]\b', value) and owner != symbol:
            raise RuntimeError('unowned debug-register instruction: '+value)
        if owner == symbol:
            # Alignment padding after RET is not part of the helper body.
            if body and body[-1] == 'retq':
                if re.search(r'%dr[0-7]\b', value):
                    raise RuntimeError('debug instruction after reset helper return')
                continue
            body.append(value)
    expected = ['xorl %eax, %eax', *[f'movq %rax, %dr{i}' for i in range(4)], 'retq']
    if body != expected:
        raise RuntimeError('guest debug reset helper differs from audited zero-only sequence: '+repr(body))
    return {'symbol': symbol, 'zeroed_live_registers': ['DR0', 'DR1', 'DR2', 'DR3'],
            'other_debug_register_instructions': 0}

def audit_host_fault(text):
    """Check all vector frames and the bounded first-record capture ABI."""
    bodies = {}
    owner = None
    for line in text.splitlines():
        label = re.fullmatch(r'\s*[0-9a-f]+ <([^>]+)>:', line)
        if label:
            owner = label[1]
            bodies[owner] = []
        instruction = re.fullmatch(r'\s*[0-9a-f]+:\s+(.+)', line)
        if instruction and owner:
            bodies[owner].append(re.sub(r'\s+', ' ', instruction[1]).strip())
    errors = {8, 10, 11, 12, 13, 14, 17, 21, 29, 30}
    for vector in range(256):
        body = bodies.get(f'svmvisor_resident_fault_{vector}')
        if vector == 255 and body is None:
            body = bodies.get('svmvisor_resident_fault')
        pushes = ([] if vector in errors else ['pushq $0x0']) + [f'pushq $0x{vector:x}']
        if body is None or body[:-1] != pushes or not re.fullmatch(
                r'jmp 0x[0-9a-f]+ <svmvisor_resident_fault_common>', body[-1]):
            raise RuntimeError(f'host fault vector{vector} frame mismatch: {body!r}')
    common = bodies.get('svmvisor_resident_fault_common', [])
    checks = ['cli', 'clgi', 'cld', 'movb $0x1, %al', 'testb %al, %al',
        'movq %cr2, %r8', 'movq %cr3, %r9', 'movq %rsp, %rsi',
        'movl $0x7, %ecx', 'rep movsq (%rsi), %es:(%rdi)',
        'movq %r8, (%rdi)', 'movq %r9, 0x8(%rdi)', 'movq %r8, %rsi',
        'movq %r9, %rdx', 'andq $-0x10, %rsp']
    if any(item not in common for item in checks) or len(common) != 20:
        raise RuntimeError('host fault capture sequence mismatch: '+repr(common))
    if not ('xchgb %al,' in common[4] and 'svmvisor_resident_fault_latched' in common[4]
            and common[6].endswith('<svmvisor_resident_fault_stop>')
            and all('svmvisor_resident_fault_record' in common[i] for i in (10, 15))
            and common[-1].endswith('<svmvisor_resident_host_fault>')):
        raise RuntimeError('host fault latch/record/callback targets differ')
    enter = bodies.get('svmvisor_resident_enter', [])
    # Far-return local label may split the symbol in some objdump versions.
    ordered = '\n'.join(text.splitlines())
    begin = ordered.index('<svmvisor_resident_enter>:')
    end = ordered.index('<svmvisor_resident_vmrun>:')
    setup = ordered[begin:end]
    if setup.index('ltr') > setup.index('lidt'):
        raise RuntimeError('private IDT selected before private TSS')
    # Returning IRQ gates (irq.S): every vector 16-255 except 18 (#MC) takes
    # a window interrupt without an error code and otherwise falls through,
    # frame unchanged, to its own fault stub. Gate 30 is reached only from
    # the #SX gate when the first stack word is not the INIT error code 1.
    rip = r'-?0x[0-9a-f]+\(%rip\) # 0x[0-9a-f]+'
    def returning(name):
        # Alignment padding after IRETQ (before the next object) is not part
        # of the gate; anything else after it is.
        body = bodies.get(name)
        if body is None or 'iretq' not in body:
            return body
        end = body.index('iretq') + 1
        if not all(re.fullmatch(r'int3|nop[lw]?(?: .*)?', line) for line in body[end:]):
            return body
        return body[:end]
    gates = [vector for vector in range(16, 256) if vector != 18]
    for vector in gates:
        expected = [r'clgi',
            rf'cmpl \$0x1, {rip} <svmvisor_resident_irq_window>',
            rf'jne 0x[0-9a-f]+ <svmvisor_resident_fault_{vector}>',
            rf'movl \$0x0, {rip} <svmvisor_resident_irq_window>',
            rf'movl \$0x{vector:x}, {rip} <svmvisor_resident_irq_vector>',
            r'andq \$-0x201, 0x10\(%rsp\)(?: # imm = 0xFDFF)?',
            r'iretq']
        body = returning(f'svmvisor_resident_irq_{vector}')
        if body is None or len(body) != len(expected) or not all(
                re.fullmatch(pattern, line) for pattern, line in zip(expected, body)):
            raise RuntimeError(f'IRQ gate vector{vector} differs from the window check: {body!r}')
    if 'svmvisor_resident_irq_18' in bodies:
        raise RuntimeError('vector 18 must stay the terminal #MC stub')
    sx = returning('svmvisor_resident_sx') or []
    expected = [r'clgi', r'cmpq \$0x1, \(%rsp\)', r'jne 0x[0-9a-f]+ <svmvisor_resident_irq_30>',
        r'lock', rf'incq {rip} <svmvisor_resident_init_acks>', r'addq \$0x8, %rsp', r'iretq']
    if len(sx) != len(expected) or not all(re.fullmatch(p, line) for p, line in zip(expected, sx)):
        raise RuntimeError('unexpected #SX bypasses the window check and host fault reporter: '+repr(sx))
    return {'vectors': 256, 'hardware_error_vectors': sorted(errors),
        'copied_frame_qwords': 7, 'copied_control_registers': ['CR2', 'CR3'],
        'private_tss_loaded_before_idt': True, 'recursive_callback_blocked': True,
        'irq_window_gates': len(gates), 'irq_window_gate_vectors': [16, 255],
        'terminal_only_vectors_below_32': [vector for vector in range(32) if vector < 16 or vector == 18],
        'sx_non_init_path': ['svmvisor_resident_irq_30', 'svmvisor_resident_fault_30']}

def build(out,low_runtime=False):
    """Build the one supported native profile: SMP guest startup behind the
    successful-EBS interposer. `low_runtime` adds the platform-specific
    below-1GiB runtime allocation."""
    out.mkdir(parents=True,exist_ok=False)
    manifest=sources()
    (out/'source-manifest.json').write_text(json.dumps(manifest,indent=2),encoding='utf-8')
    for name in manifest:
        destination=out/'source'/name; destination.parent.mkdir(parents=True,exist_ok=True)
        shutil.copyfile(ROOT/name,destination)
        if sha(destination)!=manifest[name]: raise RuntimeError('source changed during snapshot: '+name)
    target=ROOT/'target/native-resident-cargo'
    args=['cargo','rustc','--manifest-path',ROOT/'tools/native-resident/payload/Cargo.toml',
          '--target','x86_64-unknown-none','--target-dir',target,'--release']
    command(args+['--','-C','relocation-model=static','-C','code-model=small'],out,'payload-cargo')
    library=target/'x86_64-unknown-none/release/libsvmvisor_resident_payload.a'
    shutil.copyfile(library,out/'payload.a')
    for name,source in [('runtime',ROOT/'crates/dxe/src/native/resident/runtime.S'),
                        ('irq',ROOT/'crates/dxe/src/native/resident/irq.S'),
                        ('fault',ROOT/'tools/native-resident/fault.S')]:
        command(['clang','--target=x86_64-unknown-none','-c',source,'-o',out/(name+'.o')],out,name+'-compile')
    command(['ld.lld','-m','elf_x86_64','--gc-sections','--emit-relocs','-T',ROOT/'tools/native-resident/payload.ld',
             out/'runtime.o',out/'irq.o',out/'fault.o',out/'payload.a','-o',out/'payload.elf'],out,'payload-link')
    command(['llvm-objcopy','-O','binary',out/'payload.elf',out/'payload.bin'],out,'payload-flat')
    command([sys.executable,ROOT/'tools/synthetic-harness/package-relocations.py','--elf',out/'payload.elf',
             '--image',out/'payload.bin','--output',out/'payload.reloc'],out,'payload-relocations')
    undefined=command(['llvm-nm','--undefined-only',out/'payload.elf'],out,'undefined')
    if undefined.strip(): raise RuntimeError('resident payload has external symbol dependencies')
    text=command(['llvm-objdump','-d','--no-show-raw-insn',out/'payload.elf'],out,'disassembly')
    debug_reset_audit = audit_debug_reset(text)
    host_fault_audit = audit_host_fault(text)
    count=0
    for line in text.splitlines():
        match=re.fullmatch(r'\s*[0-9a-f]+:\s+(.+)',line)
        if not match: continue
        instruction=match[1]; mnemonic=instruction.split()[0]; count+=1
        if re.search(r'%(?:[xyz]mm|mm|st)[0-9(]',instruction) or mnemonic.startswith(('f','v','xsave','xrstor','xsetbv')):
            if mnemonic not in ('vmrun','vmload','vmsave'): raise RuntimeError('unowned extended-state instruction: '+instruction)
    if not count: raise RuntimeError('empty disassembly')
    env=os.environ.copy(); env['SVMVISOR_RESIDENT_PAYLOAD']=str(out/'payload.reloc')
    feature='native-resident-boot' + (',native-resident-low-runtime' if low_runtime else '')
    command(['cargo','build','--locked','-p','svmvisor-dxe','--target','x86_64-unknown-uefi',
             '--target-dir',target,'--release','--features',feature],out,'dxe-cargo',env)
    shutil.copyfile(target/'x86_64-unknown-uefi/release/svmvisor-dxe.efi',out/'driver.efi')
    pe=(out/'driver.efi').read_bytes(); import struct
    offset=struct.unpack_from('<I',pe,0x3c)[0]
    if struct.unpack_from('<H',pe,offset+24+68)[0]!=12: raise RuntimeError('shim is not EFI runtime driver')
    obj = out/'physical-audit.obj'
    command(['clang','--target=x86_64-pc-windows-msvc','-c',
        ROOT/'crates/dxe/src/native/resident/physical.S','-o',obj],out,'physical-audit-compile')
    listing = command(['llvm-objdump','-t','-r',obj],out,'physical-audit-relocations')
    import struct
    data = obj.read_bytes()
    section_count = struct.unpack_from('<H',data,2)[0]
    optional_size = struct.unpack_from('<H',data,16)[0]
    copied_sections = []
    for i in range(section_count):
        offset = 20 + optional_size + i * 40
        name = data[offset:offset+8].rstrip(b'\0')
        if name == b'.rdata':
            relocations = struct.unpack_from('<H',data,offset+32)[0]
            if relocations: raise RuntimeError('copied AP code has COFF relocations')
            copied_sections.append(i+1)
    def symbol(name):
        match = re.search(r'\(sec\s+(\d+)\).*?0x([0-9a-f]+) '+name+r'$', listing, re.M)
        if not match: raise RuntimeError('missing AP audit symbol '+name)
        return int(match[1]), int(match[2],16)
    section,start = symbol('svmvisor_ap_wait')
    end_section,end = symbol('svmvisor_ap_wait_end')
    if section not in copied_sections or end_section != section or not 0 < end-start <= 3840:
        raise RuntimeError('invalid copied AP wait span')
    bootstrap_audit = {'copied_wait_bytes':end-start,'copied_section_relocations':0}
    command(['clang','--target=x86_64-pc-windows-msvc','-c',
        ROOT/'crates/dxe/src/native/resident/boot.S','-o',out/'boot-audit.obj'],out,'boot-audit-compile')
    command(['llvm-objdump','-d','-r',out/'boot-audit.obj'],out,'boot-audit-disassembly')
    if sources()!=manifest: raise RuntimeError('source changed during build; retry from a stable snapshot')
    # verify-resident-build.py requires these fixed profile fields.
    result={'test_output':False,'payload_only':False,'boot':True,'low_runtime':low_runtime,'linked_instruction_count':count,
            'bootstrap_audit':bootstrap_audit,'debug_reset_audit':debug_reset_audit,'host_fault_audit':host_fault_audit,
            'no_fp_simd_xstate_instructions':True,'undefined_symbols':0,
            'artifacts':{p.name:sha(p) for p in out.iterdir() if p.is_file()}}
    (out/'summary.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
    print(json.dumps({k:v for k,v in result.items() if k!='artifacts'},indent=2))

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True)
    p.add_argument('--boot',action='store_true',help='the only profile; accepted for existing command lines')
    p.add_argument('--low-runtime',action='store_true',help='platform-specific runtime allocation below 1GiB')
    a=p.parse_args();build(a.output.resolve(),a.low_runtime)

