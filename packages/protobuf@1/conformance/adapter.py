#!/usr/bin/env python3
"""Runs the Hirsel conformance testee for conformance_test_runner.

    conformance_test_runner [flags] packages/protobuf@1/conformance/adapter.py

The runner speaks length-prefixed binary on the testee's stdin and stdout. Hirsel's
Stdout writes only text, so the testee answers each request with one base64 line;
this adapter forwards requests unchanged and turns each line back into a
length-prefixed response. TESTEE names the testee command (default: the native
build `testee` beside this script, run with stdin and stdout granted).
"""
import base64
import os
import shlex
import struct
import subprocess
import sys

here = os.path.dirname(os.path.abspath(__file__))
grants = '{"io":{"stdin":"stdin.bytes@1","stdout":"stdout.bytes@1","dirs":[]}}'
command = shlex.split(os.environ.get('TESTEE', '')) or [
    os.path.join(here, 'testee'), '--hirsel-grants', grants]
child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
stdin = sys.stdin.buffer
stdout = sys.stdout.buffer
while True:
    header = stdin.read(4)
    if len(header) < 4:
        break
    (size,) = struct.unpack('<I', header)
    body = stdin.read(size)
    child.stdin.write(header + body)
    child.stdin.flush()
    line = child.stdout.readline()
    if not line:
        sys.exit('testee exited')
    response = base64.b64decode(line.strip())
    stdout.write(struct.pack('<I', len(response)) + response)
    stdout.flush()
child.stdin.close()
sys.exit(child.wait())
