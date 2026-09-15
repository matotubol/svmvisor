"""Run only the generated EFI continuation fixture on the pinned TCG backend.
No physical disks, networking, firmware programming, or Windows image.
"""
import argparse, hashlib, json, os, re, shutil, subprocess, sys
from pathlib import Path
from build import ROOT, command, sha

def run(out,driver,features,cpus,memory,cpu_extra,timeout,init_preserving_backend=False,init_sx_backend=None,cache_survey=None):
    selected=set(features.split(','))
    if cache_survey:
        if cpus < 2: raise ValueError('survey fixture requires at least two CPUs')
        selected.add('cache-survey')
        features=','.join(sorted(selected - {''}))
    if 'cache-survey' in selected: selected.add('loader-new-root')
    cache_cases = selected & {'guest-cache-low','guest-cache-cr0','guest-cache-init'}
    if len(cache_cases) > 1: raise ValueError('select one cache refusal per run')
    if cache_cases: selected.add('guest-cache')
    if 'guest-cache' in selected: selected.add('loader-new-root')
    if 'guest-paging-avl' in selected: selected.add('loader-new-root')
    if 'guest-vmcr' in selected: selected.add('loader-new-root')
    if 'guest-apic-pke' in selected: selected.update({'loader-new-root', 'guest-xapic'})
    if 'guest-apic-contract' in selected: selected.add('loader-new-root')
    if 'loader-controls' in selected: selected.add('loader-fsgsbase')
    if 'loader-fsgsbase' in selected: selected.add('loader-new-root')
    if 'loader-new-root' in selected: selected.add('loader-boot')
    if 'loader-boot' in selected: selected.update({'guest-startup','virtual-map'})
    if 'guest-xapic-upgrade' in selected: selected.add('guest-xapic')
    if 'guest-xapic' in selected: selected.add('guest-startup')
    if 'guest-irq-reset-refusal' in selected: selected.add('guest-irq-wakeup')
    if 'guest-terminal-await' in selected: selected.add('guest-startup')
    if 'guest-irq-level' in selected: selected.add('guest-irq-wakeup')
    if 'guest-irq-wakeup' in selected: selected.add('guest-startup')
    negative=selected & {'smp-reject-init','smp-reject-sipi','smp-reject-remote-init','smp-reject-apic-mode'}
    if len(negative)>1: raise ValueError('select exactly one terminal SMP refusal per run')
    out.mkdir(parents=True,exist_ok=False)
    qemu=ROOT/'work/qemu-smp-ignne/build-final/runtime/bin/qemu-system-x86_64.exe'
    qemu_hash='c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047'
    if init_preserving_backend:
        qemu=ROOT/'work/qemu-init-preservation/build-attempt-01/runtime/bin/qemu-system-x86_64.exe'
        qemu_hash='07c6409a119ea0d48e880fe55e0ad004812aeff6242823e9975287a8f3147451'
    if init_sx_backend:
        if init_preserving_backend: raise ValueError('select one backend')
        backend_manifest=json.loads((init_sx_backend/'build-manifest.json').read_text())
        qemu=init_sx_backend/'runtime/bin/qemu-system-x86_64.exe'
        qemu_hash=backend_manifest['executable_sha256']
        shutil.copyfile(init_sx_backend/'build-manifest.json',out/'backend-manifest.json')
    qemu_img=ROOT/'target/synthetic-tools/qemu-10.1.0/qemu-img.exe'
    firmware=qemu.parent/'share/edk2-x86_64-code.fd'
    template=qemu.parent/'share/edk2-i386-vars.fd'
    for path,expected in [(qemu,qemu_hash),
        (firmware,'33090cc07675baa5190d9f1e84bf5176b33bcbfa9bacac522961150cdb6dbb2a'),
        (template,'5d2ac383371b408398accee7ec27c8c09ea5b74a0de0ceea6513388b15be5d1e')]:
        if sha(path)!=expected: raise RuntimeError('backend/firmware pin mismatch: '+str(path))
    shutil.copyfile(driver,out/'driver.efi')
    env=os.environ.copy();env['SVMVISOR_RESIDENT_DRIVER']=str(out/'driver.efi')
    target=ROOT/'target/native-resident-fixture-cargo'
    args=['cargo','build','--manifest-path',ROOT/'tools/native-resident/fixture/Cargo.toml',
          '--target','x86_64-unknown-uefi','--target-dir',target,'--release']
    if features: args+=['--features',features]
    command(args,out,'fixture-build',env)
    boot=out/'esp/EFI/BOOT';boot.mkdir(parents=True)
    shutil.copyfile(target/'x86_64-unknown-uefi/release/BOOTX64.efi',boot/'BOOTX64.EFI')
    command([qemu_img,'convert','-f','vvfat','-O','raw','fat:'+str(out/'esp'),out/'esp.img'],out,'esp-convert')
    shutil.copyfile(template,out/'vars.fd')
    cpu='max,svm=on,hypervisor=off'+(','+cpu_extra if cpu_extra else '')
    args=[str(qemu),'-machine','pc-q35-10.1,accel=tcg','-cpu',cpu,'-m',memory,
          '-smp',str(cpus),'-drive','if=pflash,format=raw,readonly=on,file='+str(firmware),
          '-drive','if=pflash,format=raw,file='+str(out/'vars.fd'),
          '-drive','format=raw,snapshot=on,file='+str(out/'esp.img'),
          '-no-reboot','-display','none','-monitor','none','-serial','none','-nic','none',
          '-debugcon','file:'+str(out/'debug.log'),'-device','isa-debug-exit,iobase=0xf4,iosize=0x04']
    (out/'command.json').write_text(json.dumps(args,indent=2),encoding='utf-8')
    with (out/'stdout.log').open('w') as stdout,(out/'stderr.log').open('w') as stderr:
        process=subprocess.Popen(args,cwd=ROOT,stdout=stdout,stderr=stderr,
                                 creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
        timed_out=False
        try: code=process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out=True;process.kill();code=process.wait()
    trace=(out/'debug.log').read_text(errors='replace') if (out/'debug.log').exists() else ''
    required=['resident-ack','native-fixture-after-ready','native-fixture-cpu-and-abi-witness-pass',
              'native-fixture-post-ready-allocator','native-fixture-after-ebs','PASS native-resident-callback-ebs']
    if 'virtual-map' in features: required+=['native-fixture-after-virtual-map']
    passed=code==33 and not timed_out and all(x in trace for x in required) and 'resident-stop' not in trace
    if 'expect-rejection' in features:
        passed=(code==33 and not timed_out and 'PASS native-resident-rejection' in trace
            and 'native-fixture-refusal-cpu-witness' in trace and 'resident-ack' not in trace
            and 'native-fixture-driver-registered' not in trace)
    if 'smp-prepare' in features:
        preparation=re.search(r'\("native-smp-prepared", (\d+), (\d+), (\d+), (\d+), (\d+)\)',trace)
        retained=[(int(a,16),int(b,16)) for a,b in re.findall(r'native-retained-range ([0-9a-f]{16}) ([0-9a-f]{16})',trace)]
        coverage=False
        if preparation:
            observed,callbacks,bsp,pool,size=map(int,preparation.groups())
            cursor=pool
            for start,length in sorted(retained):
                if start<=cursor<start+length: cursor=min(pool+size,start+length)
            coverage=(cursor==pool+size and observed==cpus and callbacks==cpus-1 and size==cpus*0x100000)
        passed=(code==33 and not timed_out and 'PASS native-smp-preparation-ebs' in trace
            and 'native-smp-preparation-native-witness' in trace
            and 'native-smp-prepared' in trace and 'resident-ack' not in trace
            and trace.count('private-slot')==cpus and coverage)
    if 'smp-activate' in selected or 'guest-startup' in selected or negative:
        activation=re.search(r'native-smp-activation-installed count=([0-9a-f]{16}) pool=([0-9a-f]{16}) bytes=([0-9a-f]{16})',trace)
        completed=re.search(r'native-smp-activation-all-guests mask=([0-9a-f]{16})',trace)
        common=(activation is not None and completed is not None
            and int(activation[1],16)==cpus and int(activation[3],16)==cpus*0x100000
            and int(completed[1],16)==(1<<cpus)-1
            and trace.count('resident-ack\n')==cpus
            and all(marker in trace for marker in [
                'native-smp-activation-after-ebs','native-smp-activation-pool-retained',
                'native-smp-activation-return status=0000000000000000',
                'native-smp-repeat-refused',
                'native-smp-activation-runtime-time'])
            and ('guest-startup' in selected or 'native-smp-native-icr-forward-readback' in trace)
            and ('virtual-map' not in selected or 'native-fixture-after-virtual-map' in trace)
            and 'FAIL ' not in trace)
        if 'guest-startup' in selected:
            witnessed=re.findall(r'native-guest-restart-pass cpu=([0-9a-f]{16}) generation=([0-9a-f]{16})',trace)
            expected=[(cpu,generation) for cpu in range(1,cpus) for generation in (1,2)]
            witnessed=[(int(cpu,16),int(generation,16)) for cpu,generation in witnessed]
            debug_witnessed=[(int(cpu,16),int(generation,16)) for cpu,generation in re.findall(
                r'native-guest-debug-reset-pass cpu=([0-9a-f]{16}) generation=([0-9a-f]{16})',trace)]
            deassert_witnessed=[(int(cpu,16),int(generation,16)) for cpu,generation in re.findall(
                r'native-guest-init-deassert-pass cpu=([0-9a-f]{16}) generation=([0-9a-f]{16})',trace)]
            init_cpus=[int(cpu,16) for cpu in re.findall(r'resident-guest-init cpu=([0-9a-f]{16})',trace)]
            sipi_cpus=[int(cpu,16) for cpu in re.findall(r'resident-guest-sipi cpu=([0-9a-f]{16})',trace)]
            intr_cpus=[int(cpu,16) for cpu in re.findall(r'resident-physical-init cpu=([0-9a-f]{16})',trace)]
            ready_cpus=[int(cpu,16) for cpu in re.findall(r'resident-init-redirect-ready cpu=([0-9a-f]{16})',trace)]
            resettable_cpus=[int(cpu,16) for cpu in re.findall(r'native-guest-resettable-state-pass cpu=([0-9a-f]{16})',trace)]
            passed=(common and code==33 and not timed_out
                and 'PASS native-guest-startup-repeat' in trace and 'resident-stop' not in trace
                and witnessed==expected and debug_witnessed==expected and deassert_witnessed==expected
                and resettable_cpus==list(range(1,cpus))
                and init_cpus==[cpu for cpu,generation in expected]
                and sipi_cpus==init_cpus
                and sorted(ready_cpus)==list(range(cpus))
                and all(intr_cpus.count(cpu)>=2 for cpu in range(1,cpus)))
            if 'guest-xapic' in selected:
                passed=passed and all(marker in trace for marker in [
                    'native-fixture-initial-xapic', 'native-xapic-ldr-dfr-readback',
                    'native-xapic-four-npf-witness',
                    'PASS native-xapic-mmio-startup'])
            if 'guest-xapic-upgrade' in selected:
                passed=passed and 'PASS native-xapic-to-x2apic' in trace
            if 'guest-irq-wakeup' in selected:
                blocked=re.search(r'resident-physical-init cpu=0000000000000001[^\n]*tpr=00000000000000f0 irr-f1=0000000000020000 isr-f1=0000000000000000 tpr-after=00000000000000f0 irr-f1-after=0000000000020000 isr-f1-after=0000000000000000',trace)
                held=re.search(r'resident-physical-init cpu=0000000000000001[^\n]*tpr=0000000000000000 irr-f1=0000000000000000 isr-f1=0000000000020000 tpr-after=0000000000000000 irr-f1-after=0000000000000000 isr-f1-after=0000000000020000',trace)
                passed=(passed and blocked is not None and held is not None
                    and 'PASS native-irq-wakeup-priority-isr-disabled' in trace)
                if 'guest-irq-level' in selected:
                    passed=passed and 'PASS native-level-irq-remote-irr-eoi' in trace
                if 'guest-irq-reset-refusal' in selected:
                    passed=(common and timed_out and witnessed==expected and debug_witnessed==expected
                        and blocked is not None and held is not None
                        and init_cpus==[cpu for cpu,generation in expected] and sipi_cpus==init_cpus
                        and 'native-guest-reset-with-held-irq' in trace
                        and 'native-guest-held-irq-svr-disabled' in trace
                        and re.search(r'resident-stop code=0000000000000063 rip=[0-9a-f]{16} info1=0000000000bcf10c info2=0000000100020000',trace) is not None
                        and 'PASS native-guest-startup-repeat' not in trace)
            if 'guest-terminal-await' in selected:
                passed=(common and timed_out and witnessed==expected and debug_witnessed==expected
                    and 'native-terminal-before-msr-refusal' in trace
                    and re.search(r'resident-stop code=000000000000007c rip=[0-9a-f]{16} info1=000000000000f104 info2=00000000c0010100',trace) is not None
                    and 'native-terminal-msr-returned' not in trace
                    and 'PASS native-guest-startup-repeat' not in trace
                    and 'resident-terminal await-sipi-peer=0000000000000001' in trace)
            if selected & {'guest-terminal-await', 'guest-irq-reset-refusal'}:
                barriers=re.findall(r'resident-terminal barrier=complete owner=([0-9a-f]{16}) count=([0-9a-f]{16}) card=disabled',trace)
                passed=(passed and len(barriers)==1 and int(barriers[0][0],16)<cpus
                    and int(barriers[0][1],16)==cpus and 'barrier=incomplete' not in trace)
        elif negative:
            case=next(iter(negative)).removeprefix('smp-reject-')
            reason=3 if case=='apic-mode' else 5 if case=='remote-init' else 1
            msr=0x1b if case=='apic-mode' else 0x830
            refusal=f'resident-native-icr-refused reason={reason:016x} msr={msr:016x}'
            passed=(common and timed_out and refusal in trace
                and f'native-smp-before-rejected-{case}' in trace
                and 'resident-stop code=000000000000007c' in trace
                and 'native-smp-unowned-write-returned' not in trace
                and 'PASS native-smp-activation-ebs' not in trace)
        else:
            passed=(common and code==33 and not timed_out
                and 'PASS native-smp-activation-ebs' in trace and 'resident-stop' not in trace)
        if 'guest-cache' in selected:
            if cache_cases:
                case=next(iter(cache_cases)).removeprefix('guest-cache-')
                expected_code={'low':0x400,'cr0':0x10,'init':0x63}[case]
                passed=(common and timed_out and witnessed==expected and debug_witnessed==expected
                    and 'native-cache-paired-before' in trace and 'native-cache-negative-'+case in trace
                    and re.search(r'resident-stop code='+f'{expected_code:016x}'+r' rip=[0-9a-f]{16}', trace) is not None
                    and 'PASS native-cache-paired-physical-stable' not in trace and 'cache-'+case+'-returned' not in trace)
                if case == 'low': passed = passed and 'native-cache-fixture-low-npf gpa=0000000000080000' in trace
                barriers=re.findall(r'resident-terminal barrier=complete owner=([0-9a-f]{16}) count=([0-9a-f]{16}) card=disabled',trace)
                passed=passed and len(barriers)==1 and int(barriers[0][1],16)==cpus
            else:
                passed=passed and 'PASS native-cache-paired-physical-stable' in trace
    if 'loader-boot' in selected:
        saved_icr = re.search(r'\("native-bsp-initial-icr", (\d+)\)', trace)
        resumed_icr = re.search(r'native-loader-bsp-icr before=([0-9a-f]{16}) after=([0-9a-f]{16})', trace)
        passed = passed and saved_icr is not None and resumed_icr is not None
        if saved_icr and resumed_icr:
            passed = passed and int(saved_icr[1]) == int(resumed_icr[2], 16)
        roots = re.search(r'native-loader-roots obsolete=([0-9a-f]{16}) new=([0-9a-f]{16})',trace)
        owned = [(int(slot),int(root)) for slot,root in re.findall(
            r'\("native-ap-owned-root", (\d+), (\d+)\)',trace)]
        passed = passed and roots is not None and [slot for slot,root in owned] == list(range(1,cpus))
        if roots:
            passed = passed and all(root not in (int(roots[1],16),int(roots[2],16)) for slot,root in owned)
        passed = passed and all(x in trace for x in ['native-loader-failed-ebs-inactive',
            'native-loader-obsolete-root-reclaimed', 'native-loader-nonidentity-runtime-no-alias',
            'native-loader-table-crc-pass', 'native-loader-success-cpu-abi-pass',
            'native-loader-after-all-ebs-events'])
        if 'loader-fsgsbase' in selected:
            combined = 'loader-controls' in selected
            label = 'controls' if combined else 'fsgsbase'
            controls = re.search(r'native-loader-'+label+r'-pass cr3=([0-9a-f]{16}) cr4=([0-9a-f]{16})', trace)
            passed = passed and controls is not None and 'native-loader-fsgsbase-exit-pass' in trace
            if controls and roots:
                passed = passed and int(controls[1],16) == (int(roots[2],16) | (0x18 if combined else 0))
                passed = passed and int(controls[2],16) & 0x30000 == (0x30000 if combined else 0x10000)
    if 'guest-paging-avl' in selected:
        passed = passed and all(marker in trace for marker in [
            'native-guest-paging-avl-before', 'native-guest-paging-avl-pass'])
    if 'guest-vmcr' in selected:
        passed = passed and all(marker in trace for marker in [
            'native-vmcr-before', 'native-vmcr-readonly-pass', 'native-vmcr-gp-retry-pass'])
    if 'guest-apic-pke' in selected:
        passed = passed and all(marker in trace for marker in [
            'native-apic-pke-before', 'native-apic-pke-all-keys-pass'])
    if 'guest-apic-contract' in selected:
        passed = passed and all(marker in trace for marker in [
            'native-apic-contract-before', 'native-apic-contract-visibility-and-gp-pass',
            'native-apic-contract-after-restart-pass'])
    survey_evidence = None
    if cache_survey:
        samples = [(int(slot), int(default), int(var7), int(fixed7)) for slot,default,var7,fixed7 in
            re.findall(r'\("cache-survey-sample", (\d+), (\d+), (\d+), (\d+)\)', trace)]
        expected_order = [0, *range(cpus-1, 0, -1)]
        admitted = f'("cache-survey-admitted", {cpus})' in trace
        fresh = (len(samples) == cpus and [x[0] for x in samples] == expected_order
            and samples[0][2] == 0x12345006
            and 'native-cache-fixture-before-driver var7=' in trace
            and 'native-cache-fixture-late-ebs var7=0000000012345006' in trace
            and trace.index('native-cache-fixture-late-ebs') < trace.index('cache-survey-sample'))
        no_entry = 'cache-survey-enter' not in trace and 'resident-ack' not in trace
        ordered_admission = (admitted and trace.rindex('cache-survey-sample')
            < trace.index('cache-survey-admitted') < trace.index('cache-survey-arm')) if 'cache-survey-arm' in trace else False
        if cache_survey == 'pass':
            passed = passed and fresh and ordered_admission and trace.count('cache-survey-enter') == cpus
        else:
            passed = (timed_out and fresh and no_entry and 'FAIL ' not in trace
                and (not admitted if cache_survey == 'mismatch' else admitted)
                and ('cache-survey-arm", 1, 12)' in trace and ordered_admission if cache_survey == 'drift' else
                    'cache-survey-arm' not in trace and '("native-boot-activation-failed", 48)' in trace))
        survey_evidence = {'case':cache_survey,'samples':samples,'expected_sample_order':expected_order,
            'fresh_late_ebs_bsp_bank':fresh,'admitted':admitted,'no_entry':no_entry,
            'all_samples_before_admission_before_arm':ordered_admission,
            'scope':'Actual QEMU standard MSRs; modeled AMD-hidden fields/topology and negative injections; no physical cache-coherence proof'}
    result={'passed':passed,'exit_code':code,'timed_out':timed_out,'cpu':cpu,'cpus':cpus,'memory':memory,
            'cache_survey':survey_evidence,
            'expected_terminal_refusal':next(iter(cache_cases)) if cache_cases else next(iter(negative)) if negative else
                'guest-irq-reset-refusal' if 'guest-irq-reset-refusal' in selected else
                'guest-terminal-await' if 'guest-terminal-await' in selected else None,
            'init_preserving_backend':init_preserving_backend,
            'init_sx_backend':str(init_sx_backend) if init_sx_backend else None,
            'features':features,'trace':trace,'qemu_sha256':sha(qemu),'firmware_sha256':sha(firmware),
            'driver_sha256':sha(out/'driver.efi'),'fixture_sha256':sha(boot/'BOOTX64.EFI')}
    (out/'summary.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
    print(json.dumps(result,indent=2));return passed

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True)
    p.add_argument('--driver',type=Path,required=True);p.add_argument('--features',default='')
    p.add_argument('--cpus',type=int,default=1);p.add_argument('--memory',default='256M')
    p.add_argument('--cpu-extra',default='');p.add_argument('--timeout',type=int,default=40)
    p.add_argument('--init-preserving-backend',action='store_true')
    p.add_argument('--init-sx-backend',type=Path)
    p.add_argument('--cache-survey',choices=['pass','mismatch','drift'])
    a=p.parse_args();sys.exit(0 if run(a.output.resolve(),a.driver.resolve(),a.features,a.cpus,a.memory,a.cpu_extra,a.timeout,a.init_preserving_backend,a.init_sx_backend.resolve() if a.init_sx_backend else None,a.cache_survey) else 1)
