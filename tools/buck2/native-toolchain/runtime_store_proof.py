#!/usr/bin/env python3
"""Exercise the shared production kit planner with the frozen linker."""
import os
from pathlib import Path
import re
import subprocess
import sys

CASE = "native::object_queries::runtime::tests::actual_runtime_products_reopen_changed_leaf_versions_and_refuse_missing_closure"


def command(arguments, environment):
    llvm, auditor, floor, qemu, harness = map(lambda arg: Path(arg).resolve(), arguments)
    env = {**environment, "LLVM_SYS_231_PREFIX": str(llvm), "LD_LIBRARY_PATH": str(llvm / "lib"),
           "HIRSEL_AUDITOR_PACKAGE": str(auditor), "HIRSEL_GLIBC_FLOOR": str(floor),
           "HIRSEL_QEMU_AARCH64": str(qemu / "qemu-aarch64"), "HIRSEL_JOBS": "1"}
    return [str(harness), "--ignored", "--exact", CASE, "--test-threads=1", "--nocapture"], env


def main(arguments=None):
    selected, env = command(sys.argv[1:] if arguments is None else arguments, os.environ)
    result = subprocess.run(selected, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    print(result.stdout, end="", flush=True)
    if result.returncode:
        return result.returncode
    identities = re.findall(r"^test (\S+) \.\.\. ok$", result.stdout, re.M)
    counts = re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;", result.stdout)
    if identities != [CASE] or counts != [('1', '0', '0')]:
        print("runtime-store-proof requires exactly one passing identity", file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
