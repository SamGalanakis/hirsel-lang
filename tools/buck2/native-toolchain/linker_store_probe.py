#!/usr/bin/env python3
"""Exercise one stored replay with the actual packaged linker on the pool."""
from pathlib import Path
import os
import re
import subprocess
import sys

CASE = "native::object_queries::capture::tests::packaged_capture_replays_changed_inputs_without_originals"


def command(arguments, environment):
    linker, llvm, harness = (Path(arg).resolve() for arg in arguments)
    env = {**environment, "HIRSEL_CAPTURE_LINKER": str(linker),
           "LLVM_SYS_231_PREFIX": str(llvm), "LD_LIBRARY_PATH": str(llvm / "lib")}
    return [str(harness), "--ignored", "--exact", CASE, "--test-threads=1", "--nocapture"], env


def main(arguments=None):
    selected, env = command(sys.argv[1:] if arguments is None else arguments, os.environ)
    result = subprocess.run(selected, env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True)
    print(result.stdout, end='')
    if result.returncode:
        return result.returncode
    counts = re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;', result.stdout)
    identities = re.findall(r'^test (\S+) \.\.\. ok$', result.stdout, re.M)
    if counts != [('1', '0', '0')] or identities != [CASE]:
        print('linker-store-proof: expected exactly one executed passing capture identity', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
