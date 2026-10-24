#!/usr/bin/env python3
"""Build glibc auditing DSOs without a libc or ambient linker dependency."""
import argparse
import os
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--llvm', type=Path, required=True)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    llvm = args.llvm.resolve()
    args.output.mkdir(parents=True)
    environment = dict(os.environ, LD_LIBRARY_PATH=str(llvm / 'lib'))
    for target in ['x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu']:
        obj = args.output / (target + '.o')
        subprocess.run([
            str(llvm / 'bin/clang'), '--target=' + target, '-O2', '-fPIC',
            '-ffreestanding', '-fno-builtin', '-fno-stack-protector',
            '-Wall', '-Wextra', '-Werror', '-c', str(args.source), '-o', str(obj),
        ], env=environment, check=True)
        subprocess.run([
            str(llvm / 'bin/ld.lld'), '--shared', '--threads=1', '--no-undefined',
            '--init=hirsel_audit_init', '-z', 'now', str(obj),
            '-o', str(args.output / (target + '.so')),
        ], env=environment, check=True)
        obj.unlink()


if __name__ == '__main__':
    main()
