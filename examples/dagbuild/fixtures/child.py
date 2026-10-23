#!/usr/bin/env python3
"""Bounded direct child fixture; argv never passes through a shell."""
import os
from pathlib import Path
import sys

mode=sys.argv[1]
marker=None
if mode=='copy':
    source,target=map(Path,sys.argv[2:4])
    target.write_bytes(source.read_bytes())
else:
    root=Path(sys.argv[2]);name=sys.argv[3];marker=root/(name+'.done')
    (root/(name+'.started')).write_text(str(os.getpid()))
    if mode=='hold':
        with (root/(name+'.release')).open('rb') as gate:gate.read(1)
    if mode=='fail':
        marker.write_text('7')
        sys.exit(7)
# Both drain loops and EOF, adapted from Go TestPipes.
sys.stdin.buffer.read()
os.write(1,b'O'*4096)
os.write(2,b'E'*4096)
if marker:marker.write_text('0')
