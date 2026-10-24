#!/usr/bin/env python3
# LLVM 23.1.4 prior art: lld/test/ELF/reproduce-linkerscript.s,
# reproduce-thin-archive.s and reproduce-deplibs.s. All link inputs disappear
# before replay; virtual names and filesystem admission remain observable.
from pathlib import Path
import json, os, shutil, subprocess, tarfile, tempfile, sys
linker=Path(sys.argv[1]).resolve()/'bin/ld.lld'
llvm=Path(sys.argv[2]).resolve()
env={'LD_LIBRARY_PATH':str(linker.parent.parent/'lib')}
def call(command,cwd=None):
    done=subprocess.run([str(arg) for arg in command],cwd=cwd,env=env,text=True,capture_output=True)
    assert done.returncode==0,(command,done.stderr)
    return done
with tempfile.TemporaryDirectory(prefix='hirsel-link-inputs-') as raw:
    root=Path(raw)
    source=root/'source with spaces'
    source.mkdir()
    def assemble(name,text):
        asm=source/(name+'.s'); obj=source/(name+'.o')
        asm.parent.mkdir(parents=True,exist_ok=True)
        asm.write_text(text)
        call([llvm/'bin/llvm-mc','-triple=x86_64-unknown-linux','-filetype=obj',asm,'-o',obj])
        return obj
    start=assemble('start','.globl _start\n_start:\n mov $60, %rax\n mov $42, %rdi\n syscall\n')
    dep=assemble('dep','.globl dependency\ndependency:\n ret\n')
    dep_archive=source/'libdep.a'
    call([llvm/'bin/llvm-ar','qcD',dep_archive,dep])
    metadata=assemble('unused/member',f'.globl unused_member\nunused_member:\n ret\n.section ".deplibs","MS",@llvm_dependent_libraries,1\n.asciz "{dep_archive}"\n')
    thin=source/'thin.a'
    call([llvm/'bin/llvm-ar','qcTD',thin,start,metadata])
    scripts={
        'absolute-input':f'INPUT("{start}")\n',
        'relative-input':'INPUT("start.o")\n',
        'absolute-include':f'INCLUDE "{source/"child.ld"}"\n',
        'relative-include':'INCLUDE "child.ld"\n',
        'file-pattern':f'INPUT("{start}")\nSECTIONS {{ . = 0x200000; .probe : {{ "{start}"(.text) }} .text : {{ *(.text*) }} }}\n',
        'thin-and-deplibs':f'GROUP("{thin}")\n',
        'late-assertion':f'INPUT("{start}")\nASSERT(0,"actual final link must refuse")\n',
    }
    (source/'child.ld').write_text(f'INPUT("{start}")\n')
    for name,text in scripts.items(): (source/(name+'.ld')).write_text(text)
    inputs={}
    selected=set(scripts)
    for name in scripts:
        if name not in selected: continue
        archive=root/(name+'.tar')
        output=root/(name+'-must-not-exist')
        call([linker,'-flavor','gnu','-nostdlib','--threads=1','--whole-archive','--capture-inputs-only','--reproduce='+str(archive),source/(name+'.ld'),'-o',output],cwd=source)
        assert not output.exists(),'capture produced a linked output'
        files={}
        with tarfile.open(archive) as tar:
            for entry in tar:
                if not entry.isfile(): continue
                _,relative=entry.name.split('/',1)
                if relative in ('response.txt','version.txt'): continue
                virtual=Path('/'+relative)
                target=root/'captured'/name/relative
                target.parent.mkdir(parents=True,exist_ok=True)
                target.write_bytes(tar.extractfile(entry).read())
                files[str(virtual)]=str(target)
        if name=='thin-and-deplibs':
            assert str(thin) in files and str(metadata) in files and str(start) in files
            assert str(dep_archive) in files,'capture missed a dependent library read during object parsing'
        overlay=root/(name+'.json')
        overlay.write_text(json.dumps({'version':0,'use-external-names':False,'fallthrough':False,
            'roots':[{'type':'directory','name':str(source),'contents':[]}]+[
                {'type':'file','name':virtual,'external-contents':target} for virtual,target in files.items()]}))
        inputs[name]=(overlay,files)
    source.rename(root/'hidden-originals')
    for name,(overlay,files) in inputs.items():
        output=root/(name+'-frozen')
        done=subprocess.run([str(linker),'-flavor','gnu','-nostdlib','--threads=1','--whole-archive','--input-overlay='+str(overlay),'--input-working-directory='+str(source),str(source/(name+'.ld')),'-o',str(output)],env=env,text=True,capture_output=True)
        if name=='late-assertion':
            assert done.returncode!=0 and 'actual final link must refuse' in done.stderr,done.stderr
        else:
            assert done.returncode==0,(name,done.stderr)
            assert subprocess.run([str(output)]).returncode==42,name
            if name=='file-pattern':
                sections=call([llvm/'bin/llvm-readelf','-S',output]).stdout
                assert '.probe' in sections,'virtual filename matching changed'
        print(name,'PASS',len(files),'captured files',flush=True)
    outside=root/'uncaptured.o'
    shutil.copyfile(root/'hidden-originals/start.o',outside)
    overlay=next(iter(inputs.values()))[0]
    denied=subprocess.run([str(linker),'-flavor','gnu','-nostdlib','--input-overlay='+str(overlay),str(outside),'-o',str(root/'denied')],env=env,text=True,capture_output=True)
    assert denied.returncode!=0,'uncaptured real input opened'
    print('uncaptured real input PASS',flush=True)
    if 'thin-and-deplibs' in inputs:
        shutil.copytree(root/'hidden-originals',source)
        overlay=inputs['thin-and-deplibs'][0]
        data=json.loads(overlay.read_text())
        data['roots']=[entry for entry in data['roots'] if entry['name']!=str(start)]
        omitted=root/'omitted-member.json'
        omitted.write_text(json.dumps(data))
        denied=subprocess.run([str(linker),'-flavor','gnu','-nostdlib','--whole-archive','--input-overlay='+str(omitted),'--input-working-directory='+str(source),str(source/'thin-and-deplibs.ld'),'-o',str(root/'denied-member')],env=env,text=True,capture_output=True)
        assert denied.returncode!=0 and 'cannot read thin member' in denied.stderr,denied.stderr
        print('uncaptured thin member exists on real filesystem: PASS',flush=True)
