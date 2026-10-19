#!/usr/bin/env python3
"""tools/capped runs one command in a capped systemd scope, a run at a time."""
import contextlib
import fcntl
import importlib.machinery
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]
CAPPED = ROOT / 'tools/capped'
# Records the scope's properties, then runs the command as systemd-run would.
FAKE_SYSTEMD_RUN = r'''#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
split = args.index('--')
if os.environ.get('CAPPED_NO_BUS'):
    sys.exit('Failed to connect to bus: No medium found')
with open(os.environ['CAPPED_CALLS'], 'a') as calls:
    print(json.dumps(args[:split]), file=calls)
os.execvp(args[split + 1], args[split + 1:])
'''


class Capped(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        root = Path(self.directory.name)
        (root / 'bin').mkdir()
        fake = root / 'bin/systemd-run'
        fake.write_text(FAKE_SYSTEMD_RUN)
        fake.chmod(0o755)
        self.calls = root / 'calls.jsonl'
        self.env = {key: value for key, value in os.environ.items()
                    if key not in ('HIRSEL_MEMORY_LIMIT', 'HIRSEL_JOBS')}
        self.env |= {'PATH': f'{root}/bin:{os.environ["PATH"]}', 'CAPPED_CALLS': str(self.calls),
                     'XDG_RUNTIME_DIR': str(root)}
        self.lock = root / f'hirsel-capped-{os.getuid()}.lock'

    def tearDown(self):
        self.directory.cleanup()

    def capped(self, *command, env=None):
        return subprocess.run([str(CAPPED), *command], env=self.env | (env or {}),
                              capture_output=True, text=True, timeout=60)

    def properties(self):
        # The scope probe (`systemd-run ... -- true`) sets no properties.
        [call] = [c for c in map(json.loads, self.calls.read_text().splitlines()) if '-p' in c]
        return [call[i + 1] for i, a in enumerate(call) if a == '-p']

    def test_defaults_cap_memory_swap_and_cpus_and_pass_them_on(self):
        result = self.capped('sh', '-c', 'echo "$HIRSEL_MEMORY_LIMIT $HIRSEL_JOBS"; exit 3')
        self.assertEqual(result.returncode, 3, result.stderr)
        self.assertEqual(result.stdout, '4G 2\n')
        self.assertEqual(self.properties(),
                         ['MemoryMax=4G', 'MemorySwapMax=0', 'OOMPolicy=continue', 'CPUQuota=200%'])
        self.assertRegex(result.stderr, r'capped: peak .* of 4G \(systemd scope\), [0-9.]+ s, exit 3\n')

    def test_the_environment_sets_the_cap_and_the_quota(self):
        result = self.capped('true', env={'HIRSEL_MEMORY_LIMIT': '512M', 'HIRSEL_JOBS': '3'})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.properties(),
                         ['MemoryMax=512M', 'MemorySwapMax=0', 'OOMPolicy=continue', 'CPUQuota=300%'])

    def test_without_a_user_manager_process_limits_apply(self):
        result = self.capped('sh', '-c', 'ulimit -d; nproc; echo "$HIRSEL_MEMORY_LIMIT $HIRSEL_JOBS"',
                             env={'CAPPED_NO_BUS': '1', 'HIRSEL_MEMORY_LIMIT': '512M'})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, f'{512 * 1024}\n{min(2, os.cpu_count())}\n512M 2\n')
        self.assertIn('using process limits', result.stderr)
        self.assertRegex(result.stderr, r'capped: peak .* of 512M \(process limits\), [0-9.]+ s, exit 0\n')

    def test_a_malformed_limit_or_job_count_refuses_before_running(self):
        for env in ({'HIRSEL_MEMORY_LIMIT': '4GB'}, {'HIRSEL_MEMORY_LIMIT': '0'}, {'HIRSEL_JOBS': '0'}):
            result = self.capped('true', env=env)
            self.assertEqual(result.returncode, 2, env)
            self.assertFalse(self.calls.exists(), env)

    def test_the_timeout_kills_the_command_and_its_children(self):
        pids = Path(self.directory.name) / 'pids'
        result = self.capped('--timeout', '1', 'sh', '-c',
                             # Off the captured pipes, so an orphan cannot delay the result.
                             f'for _ in 1 2; do sleep 30 >/dev/null 2>&1 & echo $! >> {pids}; done; '
                             f'echo $$ >> {pids}; wait')
        self.assertEqual(result.returncode, 124, result.stderr)
        self.assertIn('timed out after 1 s', result.stderr)
        survivors = [int(pid) for pid in pids.read_text().split()]
        self.assertEqual(len(survivors), 3)
        # Killed processes may linger as zombies until their new parent reaps them.
        deadline = time.monotonic() + 5
        while survivors and time.monotonic() < deadline:
            survivors = [pid for pid in survivors if alive(pid)]
            time.sleep(0.05)
        self.assertEqual(survivors, [], 'the timeout left processes of the command running')

    def test_only_an_oom_kill_during_the_run_reports_the_cap(self):
        cgroup = Path(self.directory.name) / 'cgroup'
        cgroup.mkdir()
        events = cgroup / 'memory.events'
        (cgroup / 'memory.peak').write_text(str(5 << 20))
        loader = importlib.machinery.SourceFileLoader('capped', str(CAPPED))
        capped = importlib.util.module_from_spec(importlib.util.spec_from_loader('capped', loader))
        loader.exec_module(capped)
        capped.own_cgroup = lambda: cgroup
        # The session's cgroup already saw two kills; the first run adds none.
        for command, note in ((['true'], False),
                              (['sh', '-c', f'printf "oom 3\\noom_kill 3\\n" > {events}'], True)):
            events.write_text('oom 2\noom_kill 2\n')
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                self.assertEqual(capped.inside(60, '4G', command), 0)
            self.assertRegex(stderr.getvalue(), r'^capped: peak 5 MiB of 4G \(systemd scope\), [0-9.]+ s, exit 0')
            self.assertEqual('memory limit 4G reached' in stderr.getvalue(), note, stderr.getvalue())

    def test_a_second_run_waits_for_the_first(self):
        with open(self.lock, 'a') as held:
            fcntl.flock(held, fcntl.LOCK_EX)
            waiting = subprocess.Popen([str(CAPPED), 'true'], env=self.env, stderr=subprocess.PIPE, text=True)
            line = waiting.stderr.readline()
            self.assertIn('capped: waiting for the run holding', line)
            self.assertFalse(self.calls.exists())
        self.assertEqual(waiting.wait(60), 0)
        waiting.stderr.close()
        self.assertEqual(len(self.properties()), 4)


def alive(pid: int) -> bool:
    """Whether the process exists and is not a zombie."""
    try:
        return Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[0] != 'Z'
    except (OSError, IndexError):
        return False


if __name__ == '__main__':
    unittest.main()
