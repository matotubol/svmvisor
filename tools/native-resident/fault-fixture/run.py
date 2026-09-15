"""Execute real host exceptions through production fault.S in disposable OVMF."""
from pathlib import Path
import argparse,hashlib,json,shutil,subprocess
ROOT=Path(__file__).resolve().parents[3]
def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest()
def run(out):
    out.mkdir(parents=True,exist_ok=False)
    q=ROOT/'work/qemu-smp-ignne/build-final/runtime/bin/qemu-system-x86_64.exe'
    fw=q.parent/'share/edk2-x86_64-code.fd'; template=q.parent/'share/edk2-i386-vars.fd'
    for p,h in [(q,'c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047'),(fw,'33090cc07675baa5190d9f1e84bf5176b33bcbfa9bacac522961150cdb6dbb2a'),(template,'5d2ac383371b408398accee7ec27c8c09ea5b74a0de0ceea6513388b15be5d1e')]:
        assert sha(p)==h,p
    def cmd(args,cwd=ROOT): subprocess.run(list(map(str,args)),cwd=cwd,check=True,capture_output=True)
    results=[]
    for kind,name in [(1,'ud'),(2,'gp'),(3,'pf'),(4,'broken-rsp')]:
        d=out/name; boot=d/'esp/EFI/BOOT';boot.mkdir(parents=True)
        cmd(['clang','--target=x86_64-pc-windows-msvc',f'-DFAULT_KIND={kind}','-c',ROOT/'tools/native-resident/fault-fixture/entry.S','-o',d/'entry.obj'])
        cmd(['clang','--target=x86_64-pc-windows-msvc','-c',ROOT/'tools/native-resident/fault.S','-o',d/'fault.obj'])
        cmd(['lld-link','/subsystem:efi_application','/entry:efi_main','/nodefaultlib','/machine:x64',f'/out:{boot / "BOOTX64.EFI"}',d/'entry.obj',d/'fault.obj'])
        cmd([ROOT/'target/synthetic-tools/qemu-10.1.0/qemu-img.exe','convert','-f','vvfat','-O','raw','fat:'+str(d/'esp'),d/'esp.img'])
        shutil.copyfile(template,d/'vars.fd')
        args=[str(q),'-machine','pc-q35-10.1,accel=tcg','-cpu','max,svm=on,hypervisor=off','-m','256M','-smp','1','-drive','if=pflash,format=raw,readonly=on,file='+str(fw),'-drive','if=pflash,format=raw,file='+str(d/'vars.fd'),'-drive','format=raw,snapshot=on,file='+str(d/'esp.img'),'-no-reboot','-display','none','-monitor','none','-serial','none','-nic','none','-debugcon','file:'+str(d/'debug.log'),'-device','isa-debug-exit,iobase=0xf4,iosize=0x04']
        (d/'command.json').write_text(json.dumps(args,indent=2))
        with (d/'stdout.log').open('w') as stdout,(d/'stderr.log').open('w') as stderr:
            p=subprocess.Popen(args,stdout=stdout,stderr=stderr,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
            try: code=p.wait(timeout=30)
            except subprocess.TimeoutExpired:p.kill();p.wait();code=-1
        trace=(d/'debug.log').read_text(errors='replace')
        passed=code==33 and 'PASS actual-host-fault-frame' in trace
        r={'case':name,'exit':code,'passed':passed,'image_sha256':sha(boot/'BOOTX64.EFI')};results.append(r);print(r,flush=True)
    report={'results':results,'passed':all(r['passed'] for r in results),'production_fault_sha256':sha(ROOT/'tools/native-resident/fault.S'),'fixture_sha256':sha(ROOT/'tools/native-resident/fault-fixture/entry.S')}
    (out/'summary.json').write_text(json.dumps(report,indent=2));assert report['passed']
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);run(p.parse_args().output.resolve())
