#!/usr/bin/env python3
"""Focused Words vectors against its actual step, on interpreter and native backends.

HIRSEL selects an already built compiler. WORDS_9_SOURCE optionally selects a
frozen baseline words.hir. The injected assertions observe the actual frequency
map and winner before the checksum; a Python oracle supplies independent counts.
The quick test.js Words checksum runs on each engine too.
"""
import collections
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

import build

VECTORS = [
    '', ' ', ' \t\r\n\f ', 'one', 'one one one', 'a b c',
    'a b b a', 'b a a b', 'a b b a c c', 'a b b a b',
    ' \ta\nb\rb\fa\t ', 'a\vb a\vb',
]


def oracle(source):
    words = re.findall(r'[^ \t\n\r\f]+', source)
    counts = collections.Counter(words)
    winner = max(counts, key=counts.get) if counts else ''
    return counts, winner, counts.get(winner, 0)


def hash_word(word):
    value = 5381
    for byte in word.encode():
        value = (value * 33 + byte) & 0xffffffff
    return value


def literal(text):
    return json.dumps(text).replace(r"\f", r"\u{c}").replace(r"\u000b", r"\u{b}")


def fixture(source):
    # Preserve the production step body, adding assertions at the checksum seam.
    original = source
    source = source[source.index('fn step('):source.index('fn checksum(')]
    source = source.replace(
        'fn step(mut s: State, iteration: u64)',
        'fn checked_step(mut s: State, iteration: u64, expected: Map[String, u32], '
        'winner: String, max_count: u32)')
    seam = '  s.total = u32.wrapping_add(s.total, maximum)'
    assert source.count(seam) == 1
    source = source.replace(seam, '''  failure.check(map.same_entries(frequencies, expected), Bounds())
  failure.check(best == winner && maximum == max_count, Bounds())
''' + seam)
    checks = []
    for text in VECTORS:
        counts, winner, maximum = oracle(text)
        inserts = '\n'.join(f'  map.insert(mut expected, {literal(k)}, {v}u32)'
                            for k, v in counts.items())
        # Hirsel spells form feed and vertical tab as scalar escapes.
        check = f'''{{
  let mut expected: Map[String, u32] = map.empty()
{inserts}
  let mut state = State(source: {literal(text)}, total: 0)
  checked_step(mut state, 0, expected, {literal(winner)}, {maximum}u32)
  checked_step(mut state, 1, expected, {literal(winner)}, {maximum}u32)
  failure.check(state.total == {2 * (maximum + hash_word(winner) + len(counts)) & 0xffffffff}u32, Bounds())
}}'''
        checks.append(check)
    task = next(task for task in json.loads((build.HERE / 'test.js').read_text())
                if task['name'] == 'Etc::Words')
    config = '\n'.join(f'  map.insert(mut config, "{key}", Number("{task[key]}"))'
                       for key in ['words', 'word_len', 'iterations', 'checksum'])
    return original + '\n' + source + '\npub fn main(rt: Io) -> Result[(), String] {\n' + '\n'.join(checks) + f'''\n
  let mut config: harness.Config = map.empty()
{config}
  let report = execute(rt, "Etc::Words", config)?
  failure.check(report.actual == {task['checksum']}u32, Bounds())
  Ok(())
}}\n'''


class WordsVectors(unittest.TestCase):
    def test_vectors_all_engines(self):
        cli = os.environ.get('HIRSEL') or str(
            build.ROOT / 'buck-out/tmp/cargo-bin-exe/gate-langarena-words-9/hirsel')
        words = Path(os.environ.get('WORDS_9_SOURCE', build.HERE / 'words.hir'))
        with tempfile.TemporaryDirectory(prefix='la-words-9-') as temporary:
            directory = Path(temporary)
            (directory / 'main.hir').write_text(fixture(words.read_text()))
            (directory / 'harness.hir').write_bytes((build.HERE / 'harness.hir').read_bytes())
            manifest = directory / 'hirsel.json'
            manifest.write_text(json.dumps(dict(
                schema=1, package=dict(name='words9', version='1'),
                entry=dict(module=[], name='main'),
                sources=[dict(path='main.hir', module=[]),
                         dict(path='harness.hir', module=['harness'])],
                dependencies=[dict(manifest=str(build.ROOT / 'stdlib/std@1/hirsel.json'),
                                   package=dict(name='std', version='1'), expose=[])])))
            grants = directory / 'grants.json'
            grants.write_text(json.dumps(dict(io=dict(
                monotonic_clock='clock.monotonic.ns@1', dirs=[]), args=[])))
            for engine in ['interpreter', 'cranelift', 'llvm']:
                with self.subTest(engine=engine):
                    command = [cli, 'run', str(manifest), '--grants', str(grants)]
                    if engine != 'interpreter':
                        command += ['--native', '--backend', engine]
                    result = subprocess.run(command, cwd=build.ROOT, capture_output=True, text=True)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main(verbosity=2)
