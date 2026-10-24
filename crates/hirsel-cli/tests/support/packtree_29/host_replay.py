"""Bounded groups reuse compiler state; each tape has a fresh runtime.

PACKTREE_29_HOST_REPLAYER names the CLI test harness supplied by the Rust parent.
Standalone verification and qualification callers must supply the same harness.
"""
import hashlib
import json
from pathlib import Path
import subprocess
import sys


class Batch:
    def __init__(self, root, manifest, env, label):
        assert env.get('PACKTREE_29_HOST_REPLAYER'), 'PACKTREE_29_HOST_REPLAYER must name the CLI test harness'
        self.root, self.manifest, self.env = root, manifest, env
        self.pending = []
        self.ordinal = 0
        self.label = label

    def add(self, tape, result, complete):
        self.pending.append((Path(tape), Path(result), complete))
        if len(self.pending) == 8:
            self.flush()

    def flush(self):
        if not self.pending:
            return
        jobs, self.pending = self.pending, []
        request = self.root / ('host-' + self.label + '-' + str(self.ordinal) + '.json')
        self.ordinal += 1
        request.write_text(json.dumps([dict(manifest=str(self.manifest), tape=str(t), result=str(r)) for t, r, _ in jobs]))
        hashes = [hashlib.sha256(t.read_bytes()).hexdigest() for t, _, _ in jobs]
        env = dict(self.env, PACKTREE_29_HOST_REPLAY_BATCH=str(request))
        command = [sys.executable, str(self.root / 'capped'), '--timeout', '60', 'prlimit', '--cpu=60:61',
                   env['PACKTREE_29_HOST_REPLAYER'], '--exact', 'packtree_29::packtree_29_native_workflows_replay', '--nocapture']
        done = subprocess.run(command, env=env, capture_output=True, timeout=90)
        request.with_suffix('.resources').write_bytes(done.stdout + done.stderr)
        assert done.returncode == 0 and b'test result: ok. 1 passed;' in done.stdout, (command, done)
        for (tape, result, complete), original in zip(jobs, hashes):
            assert hashlib.sha256(tape.read_bytes()).hexdigest() == original
            complete(json.loads(result.read_text()))
