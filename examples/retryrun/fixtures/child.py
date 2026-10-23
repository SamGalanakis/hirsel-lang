#!/usr/bin/python3
"""Private deterministic integration child. Never executes another program."""
import os
from pathlib import Path
import sys
import time

mode, state = sys.argv[1:]
state = Path(state)
# Each attempt records its pid. The harness verifies every pid is reaped.
n = len(state.read_text().splitlines()) if state.exists() else 0
with state.open('a') as output:
    output.write(str(os.getpid()) + '\n')
if mode == 'hang':
    time.sleep(30)
elif mode == 'slow':
    time.sleep(0.02)
elif mode == 'streams':
    for _ in range(16):
        os.write(1, b'o' * 4096)
        os.write(2, b'e' * 4096)
if mode == 'success' or mode == 'streams' or mode == 'flaky' and n >= 2:
    sys.exit(0)
sys.exit(2 if mode == 'permanent' else 75)
