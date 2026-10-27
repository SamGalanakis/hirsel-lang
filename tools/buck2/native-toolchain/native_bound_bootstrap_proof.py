#!/usr/bin/env python3
"""Run the exact selected-platform control with its original tool inputs."""
import os
from pathlib import Path
import re
import subprocess
import sys

CASE = 'requests::native_bootstrap::tests::actual_native_bootstrap_binds_run_identity_and_delivers_live_output'


def command(arguments, environment):
    llvm, linker, kit, runtime, auditors, floor, qemu, harness = [str(Path(arg).resolve()) for arg in arguments]
    env = {**environment, 'LLVM_SYS_231_PREFIX': llvm, 'LD_LIBRARY_PATH': llvm + '/lib',
           'HIRSEL_CAPTURE_LINKER': linker, 'HIRSEL_LINK_KIT': kit, 'HIRSEL_RUNTIME_ARCHIVES': runtime,
           'HIRSEL_AUDITOR_PACKAGE': auditors, 'HIRSEL_GLIBC_FLOOR': floor,
           'HIRSEL_QEMU_AARCH64': qemu + '/qemu-aarch64', 'HIRSEL_JOBS': '1'}
    return [harness, '--ignored', '--exact', CASE, '--test-threads=1', '--nocapture'], env


def main(arguments=None):
    selected, environment = command(sys.argv[1:] if arguments is None else arguments, os.environ)
    result = subprocess.run(selected, env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    print(result.stdout, end='')
    if result.returncode:
        return result.returncode
    identities = re.findall(r'^test (\S+) \.\.\. ok$', result.stdout, re.M)
    counts = re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;', result.stdout)
    if identities != [CASE] or counts != [('1', '0', '0')]:
        print('native-bound-bootstrap-proof: expected exactly one passing selected-platform identity', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
