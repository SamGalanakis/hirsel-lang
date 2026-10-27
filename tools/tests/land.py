#!/usr/bin/env python3
"""tools/land against a scratch origin with fake Buck2 checks: a push rejected
because main moved goes back to the rebase, and a red check refuses the push."""
import fcntl
import json
import os
import runpy
import signal
from types import SimpleNamespace
from unittest.mock import patch
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]
# The harness removes the randomized delay only; production always uses jitter.
DRIVER = '''import os, runpy, sys
from unittest.mock import patch
module = runpy.run_path('tools/land', run_name='land_test')
module['LandQueue'].HEARTBEAT = .05
module['main'].__globals__['BATCH'] = int(os.environ.get('FAKE_BATCH', module['BATCH']))
with patch('random.uniform', return_value=0):
    sys.exit(module['main']())
'''
RECORD = '''import os, pathlib, subprocess, sys
root = pathlib.Path(os.environ['FAKE_CHECKS'])
head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
base = subprocess.check_output(['git', 'rev-parse', 'origin/main'], text=True).strip()
folder = root / head
folder.mkdir(parents=True, exist_ok=True)
(folder / STAGE).write_text(base)
if os.environ.get('FAKE_RED_AFTER_RACE') and STAGE == os.environ['FAKE_RED_AFTER_RACE'] and (root / 'race-0').exists():
    sys.exit(1)
'''

FAKES = {
    '.buck2/bin/buck2': '#!/bin/sh\necho root//:a\n',
    # Land slots are fresh worktrees: prepare installs the fake Buck2 there.
    'tools/buck2/prepare.py': '''import os, pathlib
buck2 = pathlib.Path('.buck2/bin/buck2')
buck2.parent.mkdir(parents=True, exist_ok=True)
buck2.write_text('#!/bin/sh\\necho root//:a\\n')
buck2.chmod(0o755)
''',
    'tools/buck2/sync.py': '',
    'tools/check-hot-files': '',
    'tools/buck2/event_logs.py': '',
    # The first Clippy run lands another lane's commit on origin, as a lane landing meanwhile would.
    'tools/buck2/clippy.py': '''import os, subprocess
other = os.environ.get('FAKE_OTHER_LANE')
if other and not os.path.exists(other):
    subprocess.run(['git', 'clone', '-q', os.environ['FAKE_ORIGIN'], other], check=True)
    open(os.path.join(other, 'other.txt'), 'w').write('other\\n')
    subprocess.run(['git', 'add', 'other.txt'], cwd=other, check=True)
    subprocess.run(['git', 'commit', '-qm', 'other'], cwd=other, check=True)
    subprocess.run(['git', 'push', '-q', 'origin', 'HEAD:main'], cwd=other, check=True)
''',
    'tools/buck2/fmt.py': '''import os, sys
if os.environ.get('FAKE_FMT_RED'):
    print('checking 3 files')
    print('Diff in crates/a/src/lib.rs at line 1:')
    sys.exit(1)
''',
}


class Land(unittest.TestCase):
    def setUp(self):
        self.top = Path(tempfile.mkdtemp(prefix='land-test-'))
        self.addCleanup(shutil.rmtree, self.top)
        self.origin, self.lane = self.top / 'origin.git', self.top / 'lane'
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(('GIT_', 'HIRSEL_'))} | dict(
            GIT_AUTHOR_NAME='t', GIT_AUTHOR_EMAIL='t@t', GIT_COMMITTER_NAME='t', GIT_COMMITTER_EMAIL='t@t',
            FAKE_ORIGIN=str(self.origin))
        self.git(self.top, 'init', '-q', '--bare', '-b', 'main', str(self.origin))
        self.git(self.top, 'clone', '-q', str(self.origin), str(self.lane))
        files = dict(FAKES, **{'tools/land': (ROOT / 'tools/land').read_text(),
                               'tools/buck2/generated.py': (ROOT / 'tools/buck2/generated.py').read_text(),
                               'tools/buck2/merging.py': (ROOT / 'tools/buck2/merging.py').read_text(),
                               'tools/buck2/rust_environment.py': (ROOT / 'tools/buck2/rust_environment.py').read_text(),
                               '.gitignore': '.buck2/\n'})
        for name, text in files.items():
            (self.lane / name).parent.mkdir(parents=True, exist_ok=True)
            (self.lane / name).write_text(text)
        (self.lane / '.buck2/bin/buck2').chmod(0o755)
        self.git(self.lane, 'checkout', '-q', '-b', 'main')
        self.git(self.lane, 'add', '-A')
        self.git(self.lane, 'commit', '-qm', 'base')
        self.git(self.lane, 'push', '-q', 'origin', 'main')
        self.git(self.lane, 'checkout', '-q', '-b', 'my-lane')
        (self.lane / 'change.txt').write_text('change\n')
        self.git(self.lane, 'add', 'change.txt')
        self.git(self.lane, 'commit', '-qm', 'change')

    def git(self, cwd, *arguments):
        return subprocess.run(['git', *arguments], cwd=cwd, env=self.env, check=True, capture_output=True,
                              text=True).stdout

    def land(self, **env):
        return subprocess.run([sys.executable, '-c', DRIVER], cwd=self.lane, env=self.env | env,
                              capture_output=True, text=True)

    def main_log(self):
        return self.git(self.origin, 'log', '--format=%s', 'main').split('\n')[:-1]

    def test_slot_preparation_finds_cargo_without_it_on_the_leaders_path(self):
        cargo_bin = self.top / 'cargo-home/bin'
        cargo_bin.mkdir(parents=True)
        cargo = cargo_bin / 'cargo'
        cargo.write_text('#!/bin/sh\necho cargo-found\n')
        cargo.chmod(0o755)
        (cargo_bin / 'rustc').symlink_to(cargo)
        # The same nested process boundary as prepare -> bootstrap -> sync.
        (self.lane / 'tools/buck2/prepare.py').write_text(FAKES['tools/buck2/prepare.py'] + '''import subprocess, sys
subprocess.run([sys.executable, 'tools/buck2/sync.py'], check=True)
''')
        (self.lane / 'tools/buck2/sync.py').write_text('''import subprocess
from pathlib import Path
found = subprocess.check_output(['cargo', '--version'], text=True).strip()
assert found == 'cargo-found', found
Path('.buck2/prepared').write_text(found)
''')
        self.git(self.lane, 'commit', '-qam', 'prepare needs cargo')
        stripped = dict(PATH=os.defpath, CARGO='cargo', CARGO_HOME=str(cargo_bin.parent),
                        RUSTUP_HOME=str(self.top / 'rustup-home'))
        self.assertIsNone(shutil.which('cargo', path=stripped['PATH']))
        landed = self.land(**stripped)
        self.assertEqual(landed.returncode, 0, landed.stdout + landed.stderr)
        self.assertIn('pass  prepare.py', landed.stdout)
        slots = self.top / 'lane-land-slots'
        self.assertTrue(any(path.read_text() == 'cargo-found' for path in slots.glob('*/.buck2/prepared')))

    def test_missing_toolchain_is_refused_before_a_ticket_is_published(self):
        for installed in (False, True):
            with self.subTest(installed=installed):
                if installed:
                    cargo_bin = self.top / 'cargo/bin'
                    cargo_bin.mkdir(parents=True)
                    for name in ('cargo', 'rustc'):
                        tool = cargo_bin / name
                        tool.write_text('#!/bin/sh\nexit 1\n')
                        tool.chmod(0o755)
                landed = self.land(PATH=os.defpath, HOME=str(self.top / 'home'), CARGO='cargo',
                                   CARGO_HOME=str(self.top / 'cargo'), RUSTUP_HOME=str(self.top / 'rustup'))
                self.assertEqual(landed.returncode, 1, landed.stdout + landed.stderr)
                self.assertEqual(len(landed.stdout.splitlines()), 1)
                self.assertIn('Rust toolchain unavailable' if installed else 'no Rust toolchain found', landed.stdout)
                self.assertFalse((self.lane / '.git/land-queue').exists())

    def test_a_push_rejected_because_main_moved_rebases_rebuilds_and_lands(self):
        landed = self.land(FAKE_OTHER_LANE=str(self.top / 'other'))
        self.assertEqual(landed.returncode, 0, landed.stdout + landed.stderr)
        self.assertIn('main moved during the checks; rebasing again', landed.stdout)
        # Every attempted tree passes all three checks, including a retry.
        self.assertEqual(landed.stdout.count('pass  workspace build'), 2)
        self.assertEqual(landed.stdout.count('pass  Clippy'), 2)
        self.assertEqual(self.main_log(), ['change', 'other', 'base'])
        [row] = [json.loads(line) for line in (self.lane / '.git/hirsel-landings.jsonl').read_text().splitlines()]
        self.assertEqual((row['branch'], row['landed']), ('my-lane', self.git(self.origin, 'rev-parse', 'main').strip()))

    def recording_remote(self):
        checks = self.top / 'checks'
        checks.mkdir()
        self.env['FAKE_CHECKS'] = str(checks)
        # A real receive hook verifies every proposed tree has all checks,
        # and every accepted tree was checked on the remote's exact old tip.
        hook = self.origin / 'hooks/pre-receive'
        hook.write_text("""#!/usr/bin/env python3
import os, pathlib, subprocess, sys
root = pathlib.Path(os.environ['FAKE_CHECKS'])
for line in sys.stdin:
    old, new, ref = line.split()
    stages = [root / new / stage for stage in ('build', 'clippy', 'fmt')]
    if not all(path.is_file() for path in stages):
        print('UNCHECKED TREE', new, file=sys.stderr)
        sys.exit(1)
    if any(path.read_text() != old for path in stages):
        print('WRONG CHECKED BASE', new, file=sys.stderr)
        sys.exit(1)
    with (root / 'received').open('a') as out:
        out.write(new + '\\n')
""")
        hook.chmod(0o755)
        for name, stage in [('event_logs', 'build'), ('clippy', 'clippy'), ('fmt', 'fmt')]:
            (self.lane / f'tools/buck2/{name}.py').write_text('STAGE = ' + repr(stage) + '\n' + RECORD)
        self.git(self.lane, 'add', 'tools/buck2')
        self.git(self.lane, 'commit', '-qm', 'record checks')
        return checks

    def wait_for(self, predicate):
        deadline = time.monotonic() + 15
        while not predicate():
            if time.monotonic() > deadline:
                self.fail('queue rendezvous timed out')
            time.sleep(.02)

    def queue_lanes(self):
        checks = self.recording_remote()
        # Hold the first check so later, faster lands cannot overtake it.
        with (self.lane / 'tools/buck2/fmt.py').open('a') as out:
            out.write("""
# Only the first lane's own tree: it lands alone, and later batches do not hold.
if pathlib.Path('lane-0.txt').exists() and not pathlib.Path('lane-1.txt').exists():
    import time
    (root / 'holding').touch()
    deadline = time.monotonic() + 20
    while not (root / 'release').exists():
        if time.monotonic() > deadline:
            raise SystemExit('holder timed out')
        time.sleep(.02)
""")
        self.git(self.lane, 'commit', '-qam', 'slow first check')
        lanes = [self.lane]
        # Worktrees share the queue and remote refs, just like host lanes.
        for name in ('second', 'third'):
            lane = self.top / name
            self.git(self.lane, 'worktree', 'add', '-qb', name, str(lane), 'HEAD')
            (lane / '.buck2/bin').mkdir(parents=True)
            shutil.copy(self.lane / '.buck2/bin/buck2', lane / '.buck2/bin/buck2')
            lanes.append(lane)
        for i, lane in enumerate(lanes):
            (lane / f'lane-{i}.txt').write_text(f'{i}\n')
            self.git(lane, 'add', f'lane-{i}.txt')
            self.git(lane, 'commit', '-qm', f'lane {i}')
        return checks, lanes

    def start_land(self, lane):
        log = (self.top / (lane.name + '.log')).open('w+')
        self.addCleanup(log.close)
        proc = subprocess.Popen([sys.executable, '-c', DRIVER], cwd=lane,
                                env=self.env, stdout=log, stderr=log, text=True)
        def cleanup():
            if proc.poll() is None:
                proc.kill()
            proc.wait(timeout=10)
        self.addCleanup(cleanup)
        return proc, log

    def finish_land(self, process, code=0):
        proc, log = process
        proc.wait(timeout=30)
        log.seek(0)
        output = log.read()
        self.assertEqual(proc.returncode, code, output)
        return output

    @staticmethod
    def landed(output):
        """The commits a waiting lane says a batch landed for it."""
        [line] = [line for line in output.splitlines() if line.startswith('land: landed ') and ' led by ' in line]
        return line.rsplit(': ', 1)[1].split()

    def batch_lanes(self, count, edits=None, old=(), red=None, hold_build=False):
        """`count` worktrees with one commit each, queued in lane order behind a live ticket this test holds.

        Lanes in `old` are queued as an older tools/land queues them: a ticket
        without its commit, held by a live process that never reads a result.
        """
        checks = self.recording_remote()
        # `red` names the fake check (fmt or build) that fails on a red.txt that says red.
        if red:
            error = {'fmt': 'Diff in red.txt at line 1:', 'build': 'error[E0425]: red.txt is red'}[red]
            fake = {'fmt': 'fmt', 'build': 'event_logs'}[red]
            with (self.lane / f'tools/buck2/{fake}.py').open('a') as out:
                out.write("if pathlib.Path('red.txt').is_file() and pathlib.Path('red.txt').read_text() == 'red\\n':\n"
                          f"    print({error!r})\n    sys.exit(1)\n")
            if red == 'build':
                # Each build records when it ran, to show that slots build at once.
                with (self.lane / 'tools/buck2/event_logs.py').open('a') as out:
                    out.write("import time\nstarted = time.time()\ntime.sleep(.4)\n"
                              "with (root / 'builds').open('a') as log:\n"
                              "    log.write(f'{started} {time.time()}\\n')\n")
            self.git(self.lane, 'commit', '-qam', f'red {red}')
        if hold_build:
            with (self.lane / 'tools/buck2/event_logs.py').open('a') as out:
                out.write("""
if not (root / 'release-build').exists():
    import time
    (root / 'holding-build').touch()
    deadline = time.monotonic() + 20
    while not (root / 'release-build').exists():
        if time.monotonic() > deadline:
            raise SystemExit('build holder timed out')
        time.sleep(.02)
""")
            self.git(self.lane, 'commit', '-qam', 'hold first build')
        # The fake checks are main's; only each lane's own commit is queued.
        hook = self.origin / 'hooks/pre-receive'
        hook.rename(hook.with_name('held'))
        self.git(self.lane, 'push', '-q', 'origin', 'HEAD:main')
        hook.with_name('held').rename(hook)
        lanes = [self.lane]
        for i in range(1, count):
            lane = self.top / f'lane{i}'
            self.git(self.lane, 'worktree', 'add', '-qb', f'lane{i}', str(lane), 'HEAD')
            (lane / '.buck2/bin').mkdir(parents=True)
            shutil.copy(self.lane / '.buck2/bin/buck2', lane / '.buck2/bin/buck2')
            lanes.append(lane)
        for i, lane in enumerate(lanes):
            path, text = (edits or {}).get(i, (f'lane-{i}.txt', f'{i}\n'))
            (lane / path).write_text(text)
            self.git(lane, 'add', path)
            self.git(lane, 'commit', '-qm', f'lane {i}')
        queue = runpy.run_path(str(ROOT / 'tools/land'))['LandQueue']
        directory = self.lane / '.git/land-queue'
        directory.mkdir()
        def ticket(number, pid, lane, worktree):
            now = time.time()
            path = directory / f'{number:020d}'
            path.write_text(json.dumps(dict(pid=pid, process_start=queue.process(pid), started=now,
                                            heartbeat=now, lane=lane, worktree=str(worktree))))
            (directory / 'sequence').write_text(f'{number}\n')
            return path
        # As a head running its checks would, this ticket holds every lane behind it.
        holder = ticket(1, os.getpid(), 'holder', self.top / 'holder')
        lands = []
        for i, lane in enumerate(lanes):
            if i in old:
                sleeper = subprocess.Popen(['sleep', '60'])
                self.addCleanup(sleeper.wait)
                self.addCleanup(sleeper.kill)
                ticket(i + 2, sleeper.pid, lane.name if i else 'my-lane', lane)
                lands.append(sleeper)
            else:
                lands.append(self.start_land(lane))
            self.wait_for(lambda i=i: len(self.queued()) == i + 2)
        for i in range(count):
            if i not in old:
                log = self.top / (lanes[i].name + '.log')
                self.wait_for(lambda log=log: 'queue position' in log.read_text())
        return checks, lanes, lands, holder

    def subjects(self, shas):
        return [self.git(self.origin, 'show', '-s', '--format=%s', sha).strip() for sha in shas]

    def test_a_batch_of_three_clean_changes_lands_with_one_check_run(self):
        checks, lanes, lands, holder = self.batch_lanes(3)
        holder.unlink()
        outputs = [self.finish_land(land) for land in lands]
        self.assertIn('batch of 3 tickets', outputs[0])
        self.assertEqual(sum(out.count('pass  workspace build') for out in outputs), 1, outputs)
        self.assertEqual(sum(out.count('pass  formatting') for out in outputs), 1, outputs)
        # One push of all three commits, each lane's own and in ticket order.
        self.assertEqual(len((checks / 'received').read_text().splitlines()), 1)
        self.assertEqual(self.main_log()[:4], ['lane 2', 'lane 1', 'lane 0', 'record checks'])
        for i in (1, 2):
            self.assertEqual(self.subjects(self.landed(outputs[i])), [f'lane {i}'])
            self.assertEqual(self.git(lanes[i], 'rev-parse', 'HEAD'), self.git(self.origin, 'rev-parse', f'main~{2 - i}'))
        self.assertEqual(self.queued(), [])

    def test_a_formatting_red_change_in_a_batch_of_four_leaves_before_any_build(self):
        checks, lanes, lands, holder = self.batch_lanes(4, edits={1: ('red.txt', 'red\n')}, red='fmt')
        holder.unlink()
        outputs = [self.finish_land(land, 1 if i == 1 else 0) for i, land in enumerate(lands)]
        self.assertIn('Diff in red.txt at line 1:', outputs[1])
        self.assertIn('refusing to push a red tree', outputs[1])
        self.assertNotIn('Diff in', outputs[2] + outputs[3])
        # The screen: all four red, main green, lanes 0-1 red, lane 0 green, then the other three green.
        self.assertEqual(outputs[0].count('screen: formatting'), 5, outputs[0])
        self.assertEqual(outputs[0].count('workspace build'), 1, outputs[0])
        self.assertEqual(outputs[0].count('FAIL  formatting'), 0, outputs[0])
        self.assertEqual(len((checks / 'received').read_text().splitlines()), 1)
        self.assertEqual(self.main_log()[:4], ['lane 3', 'lane 2', 'lane 0', 'red fmt'])
        self.assertEqual(self.queued(), [])

    def test_a_build_red_change_in_a_batch_of_four_bisects_and_the_green_half_lands_first(self):
        checks, lanes, lands, holder = self.batch_lanes(4, edits={2: ('red.txt', 'red\n')}, red='build')
        holder.unlink()
        outputs = [self.finish_land(land, 1 if i == 2 else 0) for i, land in enumerate(lands)]
        self.assertIn('error[E0425]: red.txt is red', outputs[2])
        self.assertIn('refusing to push a red tree', outputs[2])
        # All four red; lanes 0-1 and 2-3 at once; lanes 0-1 green and pushed; lanes 2 and 3 at once,
        # lane 2 red, lane 3 green (checked again if it finished before lanes 0-1 were pushed).
        self.assertIn(outputs[0].count('workspace build'), (5, 6), outputs[0])
        received = (checks / 'received').read_text().splitlines()
        self.assertEqual(self.subjects(received), ['lane 1', 'lane 3'])
        builds = sorted(tuple(map(float, line.split())) for line in (checks / 'builds').read_text().splitlines())
        self.assertTrue(any(later[0] < earlier[1] for earlier, later in zip(builds, builds[1:])), builds)
        self.assertEqual(self.subjects(self.landed(outputs[1])), ['lane 1'])
        self.assertEqual(self.main_log()[:4], ['lane 3', 'lane 1', 'lane 0', 'red build'])
        self.assertEqual(self.queued(), [])

    def test_while_a_red_batch_bisects_the_next_batch_is_checked_speculatively(self):
        self.env['FAKE_BATCH'] = '2'
        checks, lanes, lands, holder = self.batch_lanes(4, edits={1: ('red.txt', 'red\n')}, red='build')
        holder.unlink()
        outputs = [self.finish_land(land, 1 if i == 1 else 0) for i, land in enumerate(lands)]
        head = outputs[0]
        self.assertIn('batch of 2 tickets', head)
        # Lanes 0 and 1 bisect in two slots while the third checks lanes 2 and 3.
        self.assertLess(head.index('speculative batch of 2 tickets'), head.index('ticket 3 for lane1: refusing'))
        self.assertIn('error[E0425]: red.txt is red', outputs[1])
        for i in (2, 3):
            self.assertEqual(self.subjects(self.landed(outputs[i])), [f'lane {i}'])
        self.assertEqual(self.main_log()[:4], ['lane 3', 'lane 2', 'lane 0', 'red build'])
        builds = sorted(tuple(map(float, line.split())) for line in (checks / 'builds').read_text().splitlines())
        self.assertLessEqual(max(sum(start < end and stop > begin for start, stop in builds)
                                 for begin, end in builds), 3)
        self.assertEqual(self.queued(), [])

    def test_a_conflict_in_the_middle_ticket_leaves_only_that_ticket(self):
        edits = {0: ('shared.txt', 'zero\n'), 1: ('shared.txt', 'one\n')}
        checks, lanes, lands, holder = self.batch_lanes(3, edits=edits)
        holder.unlink()
        outputs = [self.finish_land(land, 1 if i == 1 else 0) for i, land in enumerate(lands)]
        self.assertIn('conflicts in: shared.txt', outputs[1])
        self.assertEqual(outputs[0].count('pass  workspace build'), 1, outputs[0])
        self.assertEqual(self.main_log()[:3], ['lane 2', 'lane 0', 'record checks'])
        self.assertEqual(self.git(lanes[1], 'status', '--porcelain'), '')
        self.assertEqual(self.queued(), [])

    def test_a_conflict_with_a_red_ticket_ahead_stays_queued_and_lands_next(self):
        edits = {0: ('red.txt', 'red\n'), 1: ('red.txt', 'other\n')}
        checks, lanes, lands, holder = self.batch_lanes(3, edits=edits, red='fmt')
        holder.unlink()
        head = self.finish_land(lands[0], 1)
        self.assertIn('it stands if the batch ahead of it lands', head)
        self.assertIn('ticket 3 for lane1 stays queued', head)
        self.assertIn('refusing to push a red tree', head)
        # Lane 1 never conflicted with main: once the red head leaves, it lands by itself.
        self.assertNotIn('conflicts', self.finish_land(lands[1]))
        self.finish_land(lands[2])
        self.assertEqual(self.main_log()[:3], ['lane 1', 'lane 2', 'red fmt'])
        self.assertEqual(self.queued(), [])

    def test_a_dead_lanes_ticket_is_dropped_from_the_batch(self):
        checks, lanes, lands, holder = self.batch_lanes(3)
        lands[1][0].kill()
        lands[1][0].wait(timeout=10)
        holder.unlink()
        outputs = [self.finish_land(lands[i]) for i in (0, 2)]
        self.assertIn('dropped ticket 00000000000000000003 for lane1 from the batch: dead process', outputs[0])
        self.assertIn('batch of 2 tickets', outputs[0])
        self.assertEqual(self.main_log()[:3], ['lane 2', 'lane 0', 'record checks'])
        self.assertEqual(self.queued(), [])

    def test_a_batch_that_loses_a_push_race_is_checked_again_and_lands_whole(self):
        checks, lanes, lands, holder = self.batch_lanes(3)
        with (self.origin / 'hooks/pre-receive').open('a') as out:
            out.write("""
marker = root / 'race-0'
if not marker.exists():
    marker.touch()
    print('[rejected] simulated sibling won', file=sys.stderr)
    sys.exit(1)
""")
        holder.unlink()
        outputs = [self.finish_land(land) for land in lands]
        self.assertIn('main moved during the checks', outputs[0])
        self.assertEqual(outputs[0].count('pass  workspace build'), 2, outputs[0])
        self.assertEqual(outputs[0].count('batch of 3 tickets'), 2, outputs[0])
        self.assertEqual(self.main_log()[:3], ['lane 2', 'lane 1', 'lane 0'])
        for i in (1, 2):
            self.assertEqual(self.subjects(self.landed(outputs[i])), [f'lane {i}'])

    def batch_with_main_moves(self, moves, newcomer=False, retire=None, ledger_member=1):
        """Move the real remote after each green batch, immediately before its push."""
        (self.lane / 'tools/known-failures.tsv').write_text('base-row\n')
        (self.lane / '.gitattributes').write_text('tools/known-failures.tsv merge=hirsel\n')
        (self.lane / 'source.rs').write_text('fn original() {}\n')
        self.git(self.lane, 'add', '-A')
        self.git(self.lane, 'commit', '-qm', 'ledger and source')
        checks, lanes, lands, holder = self.batch_lanes(
            3, edits={ledger_member: ('tools/known-failures.tsv', 'base-row\nlane-row\n')},
            hold_build=newcomer)
        # This test permits inherited passes; the stage records below prove
        # exactly which trees ran checks. Other tests retain the strict hook.
        (self.origin / 'hooks/pre-receive').write_text('#!/bin/sh\nexit 0\n')
        received = self.origin / 'hooks/post-receive'
        received.write_text('''#!/usr/bin/env python3
import os, pathlib, sys
with (pathlib.Path(os.environ['FAKE_CHECKS']) / 'received').open('a') as out:
    for line in sys.stdin:
        out.write(line.split()[1] + '\\n')
''')
        received.chmod(0o755)
        hook = self.lane / '.git/hooks/pre-push'
        hook.write_text('''#!/usr/bin/env python3
import os, pathlib, subprocess
''' + 'moves = ' + repr(moves) + '''
root = pathlib.Path(os.environ['FAKE_CHECKS'])
env = {k: v for k, v in os.environ.items() if k not in ('GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE')}
for i, commits in enumerate(moves):
    marker = root / ('race-' + str(i))
    if marker.exists():
        continue
    marker.touch()
    other = root / ('other-' + str(i))
    def git(*args):
        subprocess.run(['git', *args], cwd=other, env=env, check=True, capture_output=True)
    subprocess.run(['git', 'clone', '-q', os.environ['FAKE_ORIGIN'], str(other)], env=env, check=True)
    for path, text in commits:
        (other / path).write_text(text)
        git('add', path)
        git('commit', '-qm', 'main move')
    git('push', '-q', 'origin', 'HEAD:main')
    break
''')
        if retire is not None:
            with hook.open('a') as out:
                out.write('''import time
if not (root / 'release-retry').exists():
    (root / 'holding-retry').touch()
    deadline = time.monotonic() + 20
    while not (root / 'release-retry').exists():
        if time.monotonic() > deadline:
            raise SystemExit('retry holder timed out')
        time.sleep(.02)
''')
        hook.chmod(0o755)
        holder.unlink()
        if newcomer:
            self.wait_for(lambda: (checks / 'holding-build').exists())
            lane = self.top / 'newcomer'
            self.git(self.lane, 'worktree', 'add', '-qb', 'newcomer', str(lane),
                     self.git(lanes[1], 'rev-parse', 'HEAD^').strip())
            (lane / 'newcomer.txt').write_text('newcomer\n')
            self.git(lane, 'add', 'newcomer.txt')
            self.git(lane, 'commit', '-qm', 'newcomer')
            lanes.append(lane)
            lands.append(self.start_land(lane))
            self.wait_for(lambda: len(self.queued()) == 4)
            (checks / 'release-build').touch()
        if retire is not None:
            self.wait_for(lambda: (checks / 'holding-retry').exists())
            lands[retire][0].kill()
            lands[retire][0].wait(timeout=10)
            (checks / 'release-retry').touch()
        outputs = {i: self.finish_land(land) for i, land in enumerate(lands) if i != retire}
        expected = (['newcomer'] if newcomer else []) + [f'lane {i}' for i in (2, 1, 0) if i != retire]
        self.assertEqual(self.main_log()[:len(expected)], expected)
        for i in (1, 2):
            if i != retire:
                self.assertEqual(self.subjects(self.landed(outputs[i])), [f'lane {i}'])
        self.assertEqual(self.queued(), [])
        return checks, outputs[0]

    def test_a_checked_prefix_can_reuse_checks_when_only_a_ledger_tail_left(self):
        checks, output = self.batch_with_main_moves([
            [('tools/known-failures.tsv', 'base-row\nmain-row\n')],
        ], retire=2, ledger_member=2)
        self.assertIn('reuses all passed checks', output)
        self.assertEqual(len(list(checks.glob('*/build'))), 1)

    def test_a_checked_prefix_rechecks_when_a_source_tail_left(self):
        checks, output = self.batch_with_main_moves([
            [('tools/known-failures.tsv', 'base-row\nmain-row\n')],
        ], retire=2)
        self.assertNotIn('reuses all passed checks', output)
        self.assertEqual(len(list(checks.glob('*/build'))), 2)

    def test_a_hole_in_the_checked_group_rechecks_even_when_only_ledger_rows_differ(self):
        checks, output = self.batch_with_main_moves([
            [('tools/known-failures.tsv', 'base-row\nmain-row\n')],
        ], retire=1)
        self.assertNotIn('reuses all passed checks', output)
        self.assertEqual(len(list(checks.glob('*/build'))), 2)

    def test_row_only_move_lands_the_checked_batch_while_a_newcomer_waits_for_its_own_checks(self):
        checks, output = self.batch_with_main_moves([
            [('tools/known-failures.tsv', 'base-row\nmain-row\n')],
        ], newcomer=True)
        self.assertEqual(output.count('pass  workspace build'), 1, output)
        self.assertIn('main moved only by known-failures rows', output)
        self.assertNotIn('pass  ', output[output.index('main moved during the checks'):], output)
        received = (checks / 'received').read_text().splitlines()
        # The ledger writer, the checked cohort, then the independently checked newcomer.
        self.assertEqual(len(received), 3, received)
        self.assertEqual(self.subjects(received), ['main move', 'lane 2', 'newcomer'])
        self.assertNotIn('newcomer', self.git(self.origin, 'log', '--format=%s', received[1]).splitlines())
        for stage in ('build', 'clippy', 'fmt'):
            self.assertEqual((checks / received[2] / stage).read_text(), received[1])
        self.assertEqual(len(list(checks.glob('*/build'))), 2)

    def test_row_only_main_moves_rebase_a_passed_batch_without_any_checks(self):
        checks, output = self.batch_with_main_moves([
            [('tools/known-failures.tsv', 'base-row\nmain-row-1\n')],
            [('tools/known-failures.tsv', 'base-row\nmain-row-1\nmain-row-2\n')],
        ])
        self.assertEqual(output.count('main moved only by known-failures rows'), 2, output)
        self.assertEqual(output.count('pass  workspace build'), 1, output)
        retry = output[output.index('main moved during the checks'):]
        self.assertNotIn('pass  ', retry, output)  # Includes prepare, sync, hot files and the fast screen.
        self.assertEqual(len(list(checks.glob('*/build'))), 1)
        self.assertEqual(set(self.git(self.origin, 'show', 'main:tools/known-failures.tsv').splitlines()),
                         {'base-row', 'lane-row', 'main-row-1', 'main-row-2'})
        tip = self.git(self.origin, 'rev-parse', 'main').strip()
        self.assertFalse((checks / tip).exists())

    def test_a_source_main_move_still_rechecks_the_batch(self):
        checks, output = self.batch_with_main_moves([[('source.rs', 'fn changed() {}\n')]])
        self.assertNotIn('reuses all passed checks', output)
        for stage in ('workspace build', 'Clippy', 'formatting', 'screen: formatting'):
            self.assertEqual(output.count('pass  ' + stage), 2, output)
        self.assertEqual(len(list(checks.glob('*/build'))), 2)

    def test_a_source_edit_and_revert_is_not_a_row_only_main_move(self):
        checks, output = self.batch_with_main_moves([[
            ('source.rs', 'fn changed() {}\n'),
            ('source.rs', 'fn original() {}\n'),
            ('tools/known-failures.tsv', 'base-row\nmain-row\n'),
        ]])
        self.assertNotIn('reuses all passed checks', output)
        self.assertEqual(output.count('pass  workspace build'), 2, output)
        self.assertEqual(len(list(checks.glob('*/build'))), 2)

    def test_an_older_lands_waiting_ticket_is_batched_and_later_finds_nothing_to_land(self):
        checks, lanes, lands, holder = self.batch_lanes(2, old=(1,))
        holder.unlink()
        output = self.finish_land(lands[0])
        self.assertIn('batch of 2 tickets', output)
        self.assertEqual(self.main_log()[:2], ['lane 1', 'lane 0'])
        # The older land never reads its result; when it runs again its rebase drops the landed commit.
        [old] = self.queued()
        self.assertEqual(json.loads(old.read_text())['result']['code'], 0)
        lands[1].kill()
        lands[1].wait(timeout=10)
        again = subprocess.run([sys.executable, '-c', DRIVER], cwd=lanes[1], env=self.env,
                               capture_output=True, text=True)
        self.assertEqual(again.returncode, 0, again.stdout + again.stderr)
        self.assertIn('nothing to land; HEAD is on origin/main', again.stdout)
        self.assertEqual(self.main_log()[:2], ['lane 1', 'lane 0'])
        self.assertEqual(self.queued(), [])

    def queued(self):
        return sorted((self.lane / '.git/land-queue').glob('[0-9]*'))

    def test_three_concurrent_lands_are_fifo_batch_the_waiters_and_check_exactly_their_push_base(self):
        checks, lanes = self.queue_lanes()
        first = self.start_land(lanes[0])
        self.wait_for(lambda: (checks / 'holding').exists())
        second = self.start_land(lanes[1])
        self.wait_for(lambda: len(self.queued()) == 2)
        third = self.start_land(lanes[2])
        self.wait_for(lambda: len(self.queued()) == 3)
        self.wait_for(lambda: 'queue position 3; head my-lane' in Path(third[1].name).read_text())
        waiting = self.queued()[1]
        before = json.loads(waiting.read_text())['heartbeat']
        self.wait_for(lambda: json.loads(waiting.read_text())['heartbeat'] > before)
        (checks / 'release').touch()
        outputs = [self.finish_land(proc) for proc in (first, second, third)]
        # The holder had no waiters when it started; the second lands the third with its own.
        for out, runs in zip(outputs, (1, 1, 0)):
            for stage in ('workspace build', 'Clippy', 'formatting'):
                self.assertEqual(out.count('pass  ' + stage), runs, out)
            self.assertNotIn('main moved during the checks', out)
        self.assertIn('queue position 2; head my-lane', outputs[1])
        self.assertIn('queue position 3; head my-lane', outputs[2])
        self.assertIn('batch of 2 tickets', outputs[1])
        received = (checks / 'received').read_text().splitlines()
        self.assertEqual(len(received), 2)
        self.assertEqual(self.main_log()[:3], ['lane 2', 'lane 1', 'lane 0'])
        self.assertEqual(self.landed(outputs[2]), [self.git(self.origin, 'rev-parse', 'main').strip()])
        self.assertEqual(self.queued(), [])

    def test_withdraw_checks_two_fake_tickets_and_refuses_a_reused_pid(self):
        queue = runpy.run_path(str(ROOT / 'tools/land'))['LandQueue']
        directory = self.lane / '.git/land-queue'
        directory.mkdir()
        holders = []
        for _ in range(2):
            proc = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])
            holders.append(proc)
            def cleanup(proc=proc):
                if proc.poll() is None:
                    proc.terminate()
                proc.wait(timeout=10)
            self.addCleanup(cleanup)
        own, other = holders
        paths = [directory / f'{i:020d}' for i in (1, 2)]
        for path, proc, worktree in zip(paths, holders, (self.lane, self.top / 'another-lane')):
            # Even a reused lane name cannot override another checkout's path.
            path.write_text(json.dumps(dict(pid=proc.pid, process_start=queue.process(proc.pid),
                                            lane='my-lane', worktree=str(worktree))))
        untouched = paths[1].read_bytes()
        result = subprocess.run([sys.executable, 'tools/land', '--withdraw'], cwd=self.lane,
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f'withdrew ticket {paths[0].name} pid {own.pid}', result.stdout)
        self.assertEqual(own.wait(timeout=10), -signal.SIGTERM)
        self.assertIsNone(other.poll())
        self.assertEqual(paths[1].read_bytes(), untouched)
        # Reuse the surviving process's PID with an old identity in our ticket.
        paths[0].write_text(json.dumps(dict(pid=other.pid, process_start=['old boot', '0'],
                                           lane='my-lane', worktree=str(self.lane))))
        result = subprocess.run([sys.executable, 'tools/land', '--withdraw'], cwd=self.lane,
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('process start does not match', result.stdout)
        self.assertIn('nothing is queued', result.stdout)
        self.assertIsNone(other.poll())
        self.assertEqual(paths[1].read_bytes(), untouched)

    def test_withdraw_waiting_lane_releases_only_its_ticket(self):
        checks, lanes = self.queue_lanes()
        first = self.start_land(lanes[0])
        self.wait_for(lambda: (checks / 'holding').exists())
        second = self.start_land(lanes[1])
        self.wait_for(lambda: len(self.queued()) == 2)
        # Withdrawal must work even with local edits or a detached branch.
        self.git(lanes[1], 'checkout', '-q', '--detach')
        (lanes[1] / 'uncommitted.txt').touch()
        result = subprocess.run([sys.executable, 'tools/land', '--withdraw'], cwd=lanes[1],
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f'pid {second[0].pid}', result.stdout)
        self.assertIn('queue ticket released', self.finish_land(second, 143))
        self.assertEqual(len(self.queued()), 1)
        self.assertIsNone(first[0].poll())
        (checks / 'release').touch()
        self.finish_land(first)
        self.assertEqual(self.main_log()[0], 'lane 0')
        self.assertEqual(self.queued(), [])

    def test_withdraw_running_lane_cancels_checks_and_restores_its_branch(self):
        checks, lanes = self.queue_lanes()
        first = self.start_land(lanes[0])
        self.wait_for(lambda: (checks / 'holding').exists())
        result = subprocess.run([sys.executable, 'tools/land', '--withdraw'], cwd=lanes[0],
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('queue ticket released', self.finish_land(first, 143))
        self.assertEqual(self.queued(), [])
        self.assertEqual(self.git(lanes[0], 'branch', '--show-current').strip(), 'my-lane')
        self.assertEqual(self.main_log(), ['base'])
        self.assertEqual(self.git(lanes[0], 'status', '--porcelain'), '')
        result = subprocess.run([sys.executable, 'tools/land', '--withdraw'], cwd=lanes[0],
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('nothing is queued', result.stdout)

    def test_killed_holder_is_reaped_and_the_next_lane_lands(self):
        checks, lanes = self.queue_lanes()
        first = self.start_land(lanes[0])
        self.wait_for(lambda: (checks / 'holding').exists())
        second = self.start_land(lanes[1])
        self.wait_for(lambda: len(self.queued()) == 2)
        first[0].kill()
        first[0].wait(timeout=10)
        # Let the fake checker child finish too; only the holder is killed.
        (checks / 'release').touch()
        output = self.finish_land(second)
        self.assertIn('reaped ticket', output)
        self.assertIn('my-lane', output)
        self.assertIn('dead process', output)
        self.assertEqual(self.main_log()[0], 'lane 1')
        self.assertEqual(self.queued(), [])

    def test_expired_live_holder_resumes_without_pushing(self):
        checks, lanes = self.queue_lanes()
        first = self.start_land(lanes[0])
        self.wait_for(lambda: (checks / 'holding').exists())
        ticket = self.queued()[0]
        before = json.loads(ticket.read_text())['heartbeat']
        self.wait_for(lambda: json.loads(ticket.read_text())['heartbeat'] > before)
        # Stop the process, including its heartbeat thread, while its checker
        # blocks. It is alive but has lost its 15-minute lease when it resumes.
        with (ticket.parent / 'lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            os.kill(first[0].pid, signal.SIGSTOP)
            # Hold the metadata lock until it is stopped, so its heartbeat
            # cannot race this edit or leave a stopped thread holding the lock.
            self.wait_for(lambda: Path(f'/proc/{first[0].pid}/stat').read_text().rsplit(')', 1)[1].split()[0] == 'T')
            record = json.loads(ticket.read_text())
            record['heartbeat'] = time.time() - 901
            ticket.write_text(json.dumps(record))
        second = self.start_land(lanes[1])
        output = self.finish_land(second)
        self.assertIn('heartbeat older than 15 minutes', output)
        (checks / 'release').touch()
        os.kill(first[0].pid, signal.SIGCONT)
        first[0].wait(timeout=15)
        first[1].seek(0)
        refused = first[1].read()
        self.assertEqual(first[0].returncode, 1, refused)
        self.assertIn('queue ownership lost; refusing to push', refused)
        self.assertEqual(len((checks / 'received').read_text().splitlines()), 1)
        self.assertEqual(self.main_log()[0], 'lane 1')
        self.assertEqual(self.queued(), [])

    def test_reexec_preserves_ticket_and_process_identity(self):
        other = self.top / 'other'
        self.git(self.top, 'clone', '-q', str(self.origin), str(other))
        with (other / 'tools/land').open('a') as out:
            out.write('\n# Updated land on main.\n')
        self.git(other, 'commit', '-qam', 'update land')
        self.git(other, 'push', '-q', 'origin', 'main')
        landed = self.land()
        self.assertEqual(landed.returncode, 0, landed.stdout + landed.stderr)
        self.assertIn('rebased tree has another tools/land; running it', landed.stdout)
        self.assertEqual(landed.stdout.count('queued ticket 00000000000000000001'), 2)
        self.assertEqual((self.lane / '.git/land-queue/sequence').read_text().strip(), '1')
        self.assertEqual(self.queued(), [])

    def test_ticket_sequence_never_resets_when_the_queue_drains(self):
        for sequence in (1, 2):
            landed = self.land()
            self.assertEqual(landed.returncode, 0, landed.stdout + landed.stderr)
            self.assertIn(f'queued ticket {sequence:020d}', landed.stdout)
            self.assertEqual(self.queued(), [])

    def test_more_than_twenty_lost_races_recheck_all_stages_and_land(self):
        checks = self.recording_remote()
        # The fake remote declines twenty-two fully checked pushes as races.
        hook = self.origin / 'hooks/pre-receive'
        with hook.open('a') as out:
            out.write("""
for i in range(22):
    marker = root / ('race-' + str(i))
    if not marker.exists():
        marker.touch()
        print('[rejected] simulated sibling won', file=sys.stderr)
        sys.exit(1)
""")
        landed = self.land()
        self.assertEqual(landed.returncode, 0, landed.stdout + landed.stderr)
        for stage in ('workspace build', 'Clippy', 'formatting'):
            self.assertEqual(landed.stdout.count('pass  ' + stage), 23)
        self.assertEqual(landed.stdout.count('main moved during the checks'), 22)
        self.assertNotIn('unchecked', landed.stdout)

    def test_a_red_retry_never_pushes_for_each_aggregate(self):
        for stage in ('build', 'clippy', 'fmt'):
            with self.subTest(stage=stage):
                checks = self.recording_remote() if stage == 'build' else self.top / 'checks'
                hook = self.origin / 'hooks/pre-receive'
                with hook.open('a') as out:
                    out.write("""
marker = root / 'race-0'
if not marker.exists():
    marker.touch()
    print('[rejected] simulated sibling won', file=sys.stderr)
    sys.exit(1)
""")
                (checks / 'race-0').unlink(missing_ok=True)
                before = self.git(self.origin, 'rev-parse', 'main')
                refused = self.land(FAKE_RED_AFTER_RACE=stage)
                self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
                self.assertIn('refusing to push a red tree', refused.stdout)
                self.assertEqual(self.git(self.origin, 'rev-parse', 'main'), before)

    def test_head_or_source_change_during_checks_refuses_the_push(self):
        for command in ("open('change.txt', 'a').write('edit')",
                        "subprocess.run(['git', 'commit', '--allow-empty', '-qm', 'moved'], check=True)"):
            with self.subTest(command=command):
                (self.lane / 'tools/buck2/fmt.py').write_text('import subprocess\n' + command + '\n')
                self.git(self.lane, 'add', '-A')
                self.git(self.lane, 'commit', '-qm', 'mutate during check')
                refused = self.land()
                self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
                self.assertIn('worktree changed during checks', refused.stdout)
                self.assertEqual(self.main_log(), ['base'])

    def test_retry_scope_requires_exact_disjoint_existing_rust_changes(self):
        files = {'crates/a/Cargo.toml': '[package]\nname = "a"\n',
                 'crates/a/src/lib.rs': 'pub fn a() {}\n',
                 'crates/b/Cargo.toml': '[package]\nname = "b"\n',
                 'crates/b/src/lib.rs': 'pub fn b() {}\n'}
        for name, text in files.items():
            path = self.lane / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        self.git(self.lane, 'add', '-A')
        self.git(self.lane, 'commit', '-qm', 'proof base')
        old_base = self.git(self.lane, 'rev-parse', 'HEAD').strip()
        (self.lane / 'crates/b/src/lib.rs').write_text('pub fn b() { /* lane */ }\n')
        self.git(self.lane, 'commit', '-qam', 'own Rust change')
        old_head = self.git(self.lane, 'rev-parse', 'HEAD').strip()
        cases = [
            ('crates/a/src/lib.rs', 'pub fn a() { /* incoming */ }\n', True),
            ('crates/a/Cargo.toml', '[package]\nname = "a"\nversion = "1"\n', False),
            ('crates/a/src/new.rs', 'pub fn new() {}\n', False),
            ('crates/a/src/lib.rs', None, False),
            ('crates/a/build.rs', 'fn main() {}\n', False),
            ('docs/notes.md', 'docs\n', False),
            ('crates/b/src/lib.rs', 'pub fn b() { /* lane */ }\n', False),
        ]
        for name, text, narrow in cases:
            with self.subTest(path=name, text=text):
                self.git(self.lane, 'checkout', '-q', '--detach', old_base)
                path = self.lane / name
                if text is None:
                    path.unlink()
                else:
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(text)
                self.git(self.lane, 'add', '-A')
                self.git(self.lane, 'commit', '-qm', 'incoming')
                base = self.git(self.lane, 'rev-parse', 'HEAD').strip()
                self.git(self.lane, 'checkout', '-q', '--detach', old_head)
                self.git(self.lane, 'rebase', base)
                head = self.git(self.lane, 'rev-parse', 'HEAD').strip()
                code = ("import runpy, json; m=runpy.run_path('tools/land'); "
                        f"print(json.dumps(m['retry_scope'](({old_base!r}, {old_head!r}), {base!r}, {head!r})))")
                result = subprocess.run([sys.executable, '-c', code], cwd=self.lane,
                                        env=self.env, check=True, capture_output=True, text=True)
                scope = json.loads(result.stdout)
                self.assertEqual(scope is not None, narrow, scope)
                if narrow:
                    self.assertIn('rdeps(', scope)
                    self.assertIn('owner("crates/a/src/lib.rs")', scope)

    def test_legacy_linted_environment_cannot_bypass_checks(self):
        refused = self.land(HIRSEL_LAND_LINTED='1', HIRSEL_LAND_ATTEMPT='30', FAKE_FMT_RED='1')
        self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
        self.assertIn('pass  workspace build', refused.stdout)
        self.assertIn('pass  Clippy', refused.stdout)
        self.assertIn('FAIL  formatting', refused.stdout)
        self.assertEqual(self.main_log(), ['base'])

    def conflicting_lanes(self, path):
        # Land has two source commits to replay, both carry a generated edit.
        self.git(self.lane, 'checkout', '-q', 'main')
        target = self.lane / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text('base\n')
        self.git(self.lane, 'add', path)
        self.git(self.lane, 'commit', '-qm', 'common generated base')
        self.git(self.lane, 'checkout', '-q', 'my-lane')
        self.git(self.lane, 'rebase', 'main')
        for i in range(2):
            target.write_text(f'lane {i}\n')
            (self.lane / f'source-{i}').write_text('source change\n')
            self.git(self.lane, 'add', '-A')
            self.git(self.lane, 'commit', '-qm', f'change {i}')
        self.git(self.lane, 'checkout', '-q', 'main')
        target.write_text('main generator output\n')
        self.git(self.lane, 'add', path)
        self.git(self.lane, 'commit', '-qm', 'main generator')
        self.git(self.lane, 'push', '-q', 'origin', 'main')
        self.git(self.lane, 'checkout', '-q', 'my-lane')

    def test_generated_pages_keep_mains_writer_and_preserve_source_commits(self):
        self.conflicting_lanes('docs/api.md')
        landed = self.land()
        self.assertEqual(landed.returncode, 0, landed.stdout + landed.stderr)
        self.assertEqual(landed.stdout.count('resolved generated conflict in docs/api.md'), 2)
        self.assertEqual((self.lane / 'docs/api.md').read_text(), 'main generator output\n')
        self.assertEqual(self.main_log()[:2], ['change 1', 'change 0'])
        for i in range(2):
            self.assertEqual((self.lane / f'source-{i}').read_text(), 'source change\n')

    def test_a_generated_only_conflict_drops_the_obsolete_generated_commit(self):
        self.conflicting_lanes('docs/api.md')
        (self.lane / 'docs/api.md').write_text('obsolete generated-only page\n')
        self.git(self.lane, 'add', 'docs/api.md')
        self.git(self.lane, 'commit', '-qm', 'obsolete generated page')
        landed = self.land()
        self.assertEqual(landed.returncode, 0, landed.stdout + landed.stderr)
        self.assertNotIn('obsolete generated page', self.main_log())
        self.assertEqual((self.lane / 'docs/api.md').read_text(), 'main generator output\n')

    def test_generated_build_inventory_is_untracked_and_regenerated(self):
        self.conflicting_lanes('BUCK')
        (self.lane / 'tools/buck2/sync.py').write_text(
            "from pathlib import Path\nPath('BUCK').write_text('fresh inventory\\n')\n")
        with open(self.lane / '.gitignore', 'a') as ignore:
            ignore.write('BUCK\n')
        self.git(self.lane, 'add', 'tools/buck2/sync.py', '.gitignore')
        self.git(self.lane, 'commit', '-qm', 'generator')
        landed = self.land()
        self.assertEqual(landed.returncode, 0, landed.stdout + landed.stderr)
        # The checked tree lives in a land slot, which regenerated its inventory.
        self.assertEqual((self.top / 'lane-land-slots/0/BUCK').read_text(), 'fresh inventory\n')
        self.assertEqual(self.git(self.lane, 'ls-files', 'BUCK'), '')

    def test_source_conflicts_still_refuse_the_push_and_abort_the_rebase(self):
        self.conflicting_lanes('source.rs')
        refused = self.land()
        self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
        self.assertIn('conflicts in: source.rs', refused.stdout)
        self.assertEqual(self.git(self.lane, 'branch', '--show-current').strip(), 'my-lane')

    def test_a_red_check_refuses_the_push_and_prints_its_first_error(self):
        refused = self.land(FAKE_FMT_RED='1')
        self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
        self.assertIn('FAIL  formatting', refused.stdout)
        self.assertIn('Diff in crates/a/src/lib.rs', refused.stdout)
        self.assertNotIn('checking 3 files', refused.stdout)
        self.assertIn('refusing to push a red tree', refused.stdout)
        self.assertEqual(self.main_log(), ['base'])


class RetryChecks(unittest.TestCase):
    def test_a_live_ticket_must_still_name_the_checked_head(self):
        module = runpy.run_path(str(ROOT / 'tools/land'))
        queue_type = module['LandQueue']
        with tempfile.TemporaryDirectory() as directory:
            queue = queue_type.__new__(queue_type)
            queue.directory = Path(directory)
            record = dict(pid=os.getpid(), process_start=queue.process(os.getpid()),
                          heartbeat=time.time(), head='checked')
            member = module['Member']('1', 'lane', 'checked', record.copy())
            ticket = queue.directory / member.ticket
            for head, expected in (('checked', [member]), ('changed', []), (None, [member])):
                with self.subTest(head=head):
                    if head is None:
                        record.pop('head')  # Older waiting tools/land records have no head.
                    else:
                        record['head'] = head
                    ticket.write_text(json.dumps(record))
                    self.assertEqual(queue.live([member]), expected)

    def test_pid_reuse_is_dead_even_with_a_recent_heartbeat(self):
        queue = runpy.run_path(str(ROOT / 'tools/land'))['LandQueue']
        record = dict(pid=os.getpid(), process_start=('previous boot', '0'), heartbeat=time.time())
        self.assertEqual(queue.stale(queue, record), 'dead process')
        record['process_start'] = queue.process(os.getpid())
        self.assertIsNone(queue.stale(queue, record))

    def slot(self):
        module = runpy.run_path(str(ROOT / 'tools/land'))
        return module['Slot'](0, ROOT, -1)

    def test_uncertain_graph_or_inventory_runs_full_aggregates(self):
        for before, after, graph in ((None, ('a',), 'root//:a'),
                                     (('a',), ('b',), 'root//:a'),
                                     (('a',), ('a',), ''),
                                     (('a',), ('a',), 'root//:a')):
            with self.subTest(before=before, after=after, graph=graph):
                slot, commands = self.slot(), []
                def check(name, command):
                    commands.append(command)
                    return True
                def run(command):
                    return SimpleNamespace(returncode=0, stdout=graph if command[-1] == 'scope' else 'root//:a',
                                           stderr='')
                slot.check, slot.run, slot.inventory = check, run, lambda: after
                self.assertTrue(slot.checks('scope', before))
                scoped = [command for command in commands if '--scope' in command]
                self.assertEqual(len(scoped), 2 if before == after and graph else 0)
                self.assertEqual(len(commands), 5)  # prepare, sync, build, Clippy, fmt

    def test_missing_or_malformed_commit_cannot_become_a_push_refspec(self):
        module = runpy.run_path(str(ROOT / 'tools/land'))
        resolve = module['revision']
        for status, value in ((1, ''), (0, ''), (0, 'HEAD')):
            with self.subTest(status=status, value=value):
                with patch.dict(resolve.__globals__, git=lambda *args: SimpleNamespace(
                        returncode=status, stdout=value, stderr='missing')):
                    with self.assertRaises(ValueError):
                        resolve('HEAD')

    def test_empty_build_query_refuses_the_aggregate(self):
        slot = self.slot()
        slot.check = lambda *args: True
        slot.run = lambda command: SimpleNamespace(returncode=0, stdout='', stderr='')
        self.assertFalse(slot.checks())

    def test_scoped_clippy_still_rejects_error_diagnostics(self):
        module = runpy.run_path(str(ROOT / 'tools/buck2/clippy.py'))
        main = module['main']
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / 'clippy.txt'
            report.write_text('error[E0063]: missing field\n')
            built = SimpleNamespace(returncode=0, stdout=json.dumps({'root//:a-clippy[clippy.txt]': str(report)}), stderr='')
            with patch.object(sys, 'argv', ['clippy.py', '--scope', 'scope']), patch('subprocess.run') as query, \
                    patch.object(main.__globals__['event_logs'], 'run', return_value=built):
                query.return_value = SimpleNamespace(returncode=0, stdout='root//:a-clippy')
                self.assertEqual(main(), 1)
                self.assertIn('intersect (scope)', query.call_args.args[0][-1])

    def test_empty_clippy_or_formatting_scope_is_never_green(self):
        for helper, output in (('clippy', ''), ('fmt', '{}')):
            with self.subTest(helper=helper):
                module = runpy.run_path(str(ROOT / f'tools/buck2/{helper}.py'))
                with patch.object(sys, 'argv', [helper, '--scope', 'scope']), patch('subprocess.run') as query:
                    query.return_value = SimpleNamespace(returncode=0, stdout=output)
                    with self.assertRaises(SystemExit):
                        module['main']()


if __name__ == '__main__':
    unittest.main()
