#!/usr/bin/env python3
"""Finite direct-child fixture. Only the private state directory is written.

Go exec_test.go TestPipes (470-525) is adapted to alternating output followed
by stdin EOF. Output is discarded by batchrun, so the receipt records byte counts.
"""
import json
import os
from pathlib import Path
import signal
import sys
import time

mode, directory, identity = sys.argv[1:4]
root = Path(directory)
pid = os.getpid()
(root / (identity + '.started')).write_text(str(pid))
if mode == 'hold':
    # A FIFO handshake, rather than a startup sleep, holds qualification runs.
    with (root / (identity + '.release')).open('rb') as release:
        release.read(1)
elif mode == 'hang':
    # Parent test only targets this fixture; app deadlines must terminate it.
    signal.pause()
elif mode == 'pipes':
    for _ in range(24):
        os.write(1, b'O' * 4096)
        os.write(2, b'E' * 4096)
elif mode == 'eof':
    assert sys.stdin.buffer.read() == b''
elif mode == 'slow':
    time.sleep(0.08)
elif mode == 'signal':
    os.kill(pid, signal.SIGTERM)
code = 7 if mode == 'fail' else 0
(root / (identity + '.done')).write_text(json.dumps(dict(pid=pid, exit=code,
    stdout=98304 if mode == 'pipes' else 0, stderr=98304 if mode == 'pipes' else 0)))
sys.exit(code)
