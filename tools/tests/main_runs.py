#!/usr/bin/env python3
"""tools/main-runs names each red's likely culprit and lane from the commits
since the last run of the red's stage, tells each lane its new reds once, and
sends a red with no known lane to the orchestrator; a fast tier runs only the
quick stages."""
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class Route(unittest.TestCase):
    def setUp(self):
        self.repo = Path(tempfile.mkdtemp(prefix='main-runs-test-'))
        self.addCleanup(shutil.rmtree, self.repo)
        self.env = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')} | dict(
            GIT_AUTHOR_NAME='t', GIT_AUTHOR_EMAIL='t@t', GIT_COMMITTER_NAME='t', GIT_COMMITTER_EMAIL='t@t')
        self.git('init', '-q', '-b', 'main')
        self.commits = {}
        landings = []
        # lane-a lands two commits under hirsel-foo; lane-b one under hirsel-bar and the corpus.
        for name, lane, paths in (('base', None, ['README.md']),
                                  ('foo-1', None, ['crates/hirsel-foo/src/a.rs']),
                                  ('foo-2', 'lane-a', ['crates/hirsel-foo/src/b.rs', 'docs/x.md']),
                                  ('bar', 'lane-b', ['crates/hirsel-bar/src/c.rs', 'conformance/SEM/SEM-1-x-run.hir'])):
            for path in paths:
                (self.repo / path).parent.mkdir(parents=True, exist_ok=True)
                (self.repo / path).write_text(name + '\n')
            self.git('add', '-A')
            self.git('commit', '-qm', name)
            self.commits[name] = self.git('rev-parse', 'HEAD')
            if lane:
                landings.append(f'{{"landed": "{self.commits[name]}", "branch": "{lane}"}}\n')
        (self.repo / 'crates/hirsel-foo-macros').mkdir()
        (self.repo / '.git/hirsel-landings.jsonl').write_text(''.join(landings))
        self.said = self.repo / 'said.txt'
        sluice = self.repo / 'sluice'
        sluice.write_text(f'#!/bin/sh\n[ "$6" = lane-gone ] && exit 1\n'
                          f'{{ echo "TO $6"; cat "$8"; }} >> {self.said}\n')
        sluice.chmod(0o755)
        self.route = runpy.run_path(str(ROOT / 'tools/main-runs'))['route']
        self.route.__globals__.update(TREE=self.repo, SLUICE=str(sluice))

    def git(self, *arguments):
        return subprocess.run(['git', *arguments], cwd=self.repo, env=self.env, check=True, capture_output=True,
                              text=True).stdout.strip()

    def test_each_red_goes_to_the_lane_whose_commits_match_it(self):
        reds = {
            'workspace tests: hirsel-foo-test-it-shard-2': dict(stage='workspace tests', target='hirsel-foo-test-it-shard-2',
                                                                ids=['hirsel-foo-test-it::walk']),
            'conformance validation and ratchet: gate-conformance-shard-3': dict(
                stage='conformance validation and ratchet', target='gate-conformance-shard-3', ids=['conformance/SEM/SEM-1-x-run.hir']),
            'stage: native and Lean checks': dict(stage='native and Lean checks', target=None, ids=[]),
            'workspace tests: hirsel-bar-unittest': dict(stage='workspace tests', target='hirsel-bar-unittest', ids=[]),
        }
        # The full run before has the old red; the fast run since ran only workspace tests and had it too.
        old_red = dict(culprit='c0ffee', lane='lane-z', since='5eed')
        history = [dict(sha=self.commits['base'], reds={'workspace tests: hirsel-bar-unittest': old_red}),
                   dict(sha=self.commits['foo-2'], tier='fast', stages=['workspace tests'],
                        reds={'workspace tests: hirsel-bar-unittest': old_red})]
        self.route(self.commits['bar'], reds, history, '/report.md')
        foo, corpus, stage, old = reds.values()
        # A red in a stage the fast run ran is new since that run, so only bar can be its culprit;
        # the corpus red's stage last ran in the full run, so its culprit is among all three commits.
        self.assertEqual((foo['culprit'], foo['lane']), (self.commits['bar'], 'lane-b'))
        self.assertEqual((corpus['culprit'], corpus['lane']), (self.commits['bar'], 'lane-b'))
        self.assertEqual((stage['culprit'], stage['lane']), (None, None))
        self.assertEqual((old['culprit'], old['lane'], old['since']), ('c0ffee', 'lane-z', '5eed'))
        said = self.said.read_text()
        self.assertEqual(said.count('TO '), 2)
        for lane, key in (('lane-b', 'hirsel-foo-test-it-shard-2'), ('lane-b', 'gate-conformance-shard-3'),
                          ('@agentlang-24', 'native and Lean checks')):
            message = said.split(f'TO {lane}\n')[1].split('TO ')[0]
            self.assertIn(key, message)
        self.assertNotIn('hirsel-bar-unittest', said)

    def test_a_lane_that_cannot_be_told_falls_back_to_the_orchestrator(self):
        landings = self.repo / '.git/hirsel-landings.jsonl'
        landings.write_text(landings.read_text().replace('lane-b', 'lane-gone'))
        reds = {'x: hirsel-bar-unittest': dict(stage='x', target='hirsel-bar-unittest', ids=[])}
        self.route(self.commits['bar'], reds, [dict(sha=self.commits['foo-2'], reds={})], '/report.md')
        self.assertEqual(reds['x: hirsel-bar-unittest']['lane'], 'lane-gone')
        self.assertIn('TO @agentlang-24\n', self.said.read_text())


class Tiers(unittest.TestCase):
    def test_fast_runs_only_the_quick_stages_and_full_runs_them_too(self):
        tool = runpy.run_path(str(ROOT / 'tools/main-runs'))
        tool['subprocess'].run = lambda *a, **k: subprocess.CompletedProcess(a, 0, '', '')  # no uquery
        named = {}
        for tier in ('fast', 'full'):
            named[tier] = []
            env = dict(os.environ)
            try:
                tool['stages'](lambda name, *_, **__: named[tier].append(name), tier)
            finally:
                os.environ.clear()
                os.environ.update(env)
        fast = [n for n in named['fast'] if not n.startswith('tool tests: ')]
        self.assertEqual(fast, ['Buck2 toolchains and pool configuration', 'Buck2 build files and test-cost coverage',
                                'workspace build', 'workspace clippy', 'formatting', 'workspace tests'])
        self.assertEqual(named['full'][:len(named['fast'])], named['fast'])
        self.assertIn('native passing corpus with independent second hash seed', named['full'])
        # A fast tick is skipped by a full run of main; a full tick is not skipped by a fast run.
        covers = tool['covers']
        self.assertTrue(covers({}, 'fast') and covers({'tier': 'full'}, 'fast') and covers({}, 'full'))
        self.assertFalse(covers({'tier': 'fast'}, 'full'))


class Reds(unittest.TestCase):
    def test_failing_ids_name_libtest_cases_once_and_corpus_cases_by_path(self):
        failing_ids = runpy.run_path(str(ROOT / 'tools/main-runs'))['failing_ids']
        # Parallel libtest output can split a case's `... FAILED` line; its
        # `---- stdout ----` block still names it.
        details = ('test a::ok ... ok\n'
                   'test a::first ... FAILED\n'
                   'test a::split ... test a::other ... ok\nFAILED\n'
                   '---- a::first stdout ----\npanicked\n'
                   '---- a::split stdout ----\npanicked\n'
                   'conformance/MEM/MEM-7-x-run.hir: expected run, got reject\n'
                   '{"path": "conformance/DROP/DROP-2-y-run.hir", "verdict": "mismatch"}\n')
        self.assertEqual(failing_ids('hirsel-foo-test-it-shard-3', details),
                         ['hirsel-foo-test-it::a::first', 'hirsel-foo-test-it::a::split',
                          'conformance/MEM/MEM-7-x-run.hir', 'conformance/DROP/DROP-2-y-run.hir'])


if __name__ == '__main__':
    unittest.main()
