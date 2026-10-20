#!/usr/bin/env python3
"""tools/main-runs names each red's likely culprit and lane from the commits
since the last run, tells each lane its new reds once, and sends a red with no
known lane to the orchestrator."""
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
            'workspace tests: hirsel-foo-test-it-shard-2': dict(target='hirsel-foo-test-it-shard-2',
                                                                ids=['hirsel-foo-test-it::walk']),
            'conformance validation and ratchet: gate-conformance-shard-3': dict(
                target='gate-conformance-shard-3', ids=['conformance/SEM/SEM-1-x-run.hir']),
            'stage: native and Lean checks': dict(target=None, ids=[]),
            'workspace tests: hirsel-bar-unittest': dict(target='hirsel-bar-unittest', ids=[]),
        }
        previous = dict(sha=self.commits['base'], reds={
            'workspace tests: hirsel-bar-unittest': dict(culprit='c0ffee', lane='lane-z', since='5eed')})
        self.route(self.commits['bar'], reds, previous, '/report.md')
        foo, corpus, stage, old = reds.values()
        # lane-a's two commits match the crate equally: the newer one is the culprit.
        self.assertEqual((foo['culprit'], foo['lane']), (self.commits['foo-2'], 'lane-a'))
        self.assertEqual((corpus['culprit'], corpus['lane']), (self.commits['bar'], 'lane-b'))
        self.assertEqual((stage['culprit'], stage['lane']), (None, None))
        self.assertEqual((old['culprit'], old['lane'], old['since']), ('c0ffee', 'lane-z', '5eed'))
        said = self.said.read_text()
        self.assertEqual(said.count('TO '), 3)
        for lane, key in (('lane-a', 'hirsel-foo-test-it-shard-2'), ('lane-b', 'gate-conformance-shard-3'),
                          ('@agentlang-24', 'native and Lean checks')):
            message = said.split(f'TO {lane}\n')[1].split('TO ')[0]
            self.assertIn(key, message)
        self.assertNotIn('hirsel-bar-unittest', said)

    def test_a_lane_that_cannot_be_told_falls_back_to_the_orchestrator(self):
        landings = self.repo / '.git/hirsel-landings.jsonl'
        landings.write_text(landings.read_text().replace('lane-b', 'lane-gone'))
        reds = {'x: hirsel-bar-unittest': dict(target='hirsel-bar-unittest', ids=[])}
        self.route(self.commits['bar'], reds, dict(sha=self.commits['foo-2'], reds={}), '/report.md')
        self.assertEqual(reds['x: hirsel-bar-unittest']['lane'], 'lane-gone')
        self.assertIn('TO @agentlang-24\n', self.said.read_text())


if __name__ == '__main__':
    unittest.main()
