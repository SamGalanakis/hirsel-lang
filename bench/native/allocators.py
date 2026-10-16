"""Pinned allocator libraries: installed packages or verified source tarballs.
No package installation. Source builds remain under build/allocators/.
"""
import hashlib
import json
import subprocess
import tarfile
import urllib.request
from pathlib import Path

ROOT=Path(__file__).resolve().parent

def prepare(manifest):
    result={}
    for name,pin in manifest['allocator_libraries'].items():
        installed=subprocess.run(['dpkg-query','-W','-f=${Version}',pin['package']],capture_output=True,text=True)
        if installed.returncode==0 and installed.stdout==pin['package_version']:
            result[name]=['-l'+name]
            continue
        work=ROOT/'build/allocators'/name;work.mkdir(parents=True,exist_ok=True)
        archive=work/'source.tar.gz'
        if not archive.exists():
            with urllib.request.urlopen(pin['url']) as response:
                archive.write_bytes(response.read())
        assert hashlib.sha256(archive.read_bytes()).hexdigest()==pin['sha256'],'allocator tarball digest mismatch'
        source=work/pin['source_directory']
        if not source.exists():
            with tarfile.open(archive) as tf:tf.extractall(work,filter='data')
        prefix=work/'install'
        def run(cmd,cwd=source):subprocess.run(cmd,cwd=cwd,check=True,capture_output=True)
        if not (prefix/'lib'/pin['static_library']).exists():
            run(['cmake','-S',str(source),'-B',str(work/'obj'),'-DCMAKE_BUILD_TYPE=Release','-DMI_BUILD_TESTS=OFF','-DMI_BUILD_SHARED=OFF',f'-DCMAKE_INSTALL_PREFIX={prefix}'])
            run(['cmake','--build',str(work/'obj'),'-j2'])
            run(['cmake','--install',str(work/'obj')])
        # A static mimalloc override archive needs whole-archive to replace
        # libc symbols; both C variants receive the same link policy.
        result[name]=['-Wl,--whole-archive',str(prefix/'lib'/pin['static_library']),'-Wl,--no-whole-archive','-lpthread','-ldl','-lm']
    return result
