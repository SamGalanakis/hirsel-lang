#!/usr/bin/python3
"""Only direct children; FIFO and inotify handshakes, no startup sleeps."""
import json
import os
from pathlib import Path
import sys

mode, directory, identity = sys.argv[1:4]
root = Path(directory)
(root/(identity+'.started')).write_text(str(os.getpid()))
if mode in ['hold', 'cancel', 'hold-fail']:
    with (root/(identity+'.release')).open('rb') as f:
        f.read(1)
if mode == 'pipes':
    os.write(1, b'O'*98304)
    os.write(2, b'E'*98304)
if mode == 'retry':
    marker=root/(identity+'.first')
    if not marker.exists():
        marker.write_text('failed')
        sys.exit(7)
(root/(identity+'.done')).write_text(json.dumps(dict(stdout=98304 if mode=='pipes' else 0,stderr=98304 if mode=='pipes' else 0)))
sys.exit(7 if mode in ['fail', 'hold-fail'] else 0)
