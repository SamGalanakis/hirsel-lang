#!/usr/bin/env python3
"""Full-value BWT oracles. Each engine runs in its own Buck2 target."""
import argparse
import itertools
import json
import os
from pathlib import Path
import random
import subprocess
import tempfile
import unittest

import build


def rotation_oracle(data):
    """Sort complete cyclic rotations, independently of rank doubling.

    Python's stable sort preserves ascending source indices for equal rotations.
    """
    if not data:
        return [], 0
    suffixes = sorted(range(len(data)), key=lambda i: data[i:] + data[:i])
    return [data[(i - 1) % len(data)] for i in suffixes], suffixes.index(0)


def vectors():
    cases = [b'', b'A', b'\0', b'\xff', b'AAAAAA', b'ABRACADABRA',
             b'banana', b'BANANA', b'ABABAB', b'BABA', b'ABCABCABC',
             b'CABACABA', b'\0\xff\0\xff', bytes([0, 255, 0, 1, 255, 128, 0, 255]),
             bytes(range(16)), bytes(reversed(range(16)))]
    cases += [bytes(xs) for n in range(1, 4)
              for xs in itertools.product((0, 1, 255), repeat=n)]
    rng = random.Random(20261022)
    cases += [bytes(rng.randrange(256) for _ in range(n)) for n in (2, 5, 7, 11, 17, 31)]
    return list(dict.fromkeys(cases))


def literal(data):
    return '[' + ', '.join(f'{x}u8' for x in data) + ']'


def oracle_source():
    tests = []
    for index, data in enumerate(vectors()):
        transformed, primary = rotation_oracle(data)
        tests.append(f'''@test
fn la_bwt_9_vector_{index}(t: Test) {{
  let input: List[u8] = {literal(data)}
  let transformed: List[u8] = {literal(transformed)}
  let encoded = bwt_encode(input)
  test.check(t, encoded.data == transformed)
  test.check(t, encoded.original == {primary})
  test.check(t, bwt_decode(encoded) == input)
  // Also exercise inverse independently of this encoder's output.
  test.check(t, bwt_decode(Bwt(data: transformed, original: {primary})) == input)
}}
''')
    tests.append("""@test
fn la_bwt_9_quick_checksums(t: Test) {
  // Upstream test.js:258-270: one warmup plus three timed iterations.
  let encode_input = test_data(50)
  let decode_input = test_data(51)
  let encoded = bwt_encode(decode_input)
  let mut encode_sum: u64 = 0
  let mut decode_sum: u64 = 0
  let mut decoded: List[u8] = []
  for _ in 0u64..4u64 {
    encode_sum += list.len(bwt_encode(encode_input).data)
    decoded = bwt_decode(encoded)
    decode_sum += list.len(decoded)
  }
  test.check(t, encode_sum == 200)
  test.check(t, decoded == decode_input)
  test.check(t, decode_sum + if decoded == decode_input { 100000 } else { 0 } == 100204)
}
""")
    return '\n'.join(tests)


class BwtOracles(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scratch = tempfile.TemporaryDirectory(prefix='la-bwt-9-')
        cls.directory = Path(cls.scratch.name)
        cls.cli = Path(os.environ.get('HIRSEL') or build.ROOT / 'buck-out/tmp/cargo-bin-exe/gate-langarena/hirsel').resolve()
        source = (build.HERE / 'compress.hir').read_text()
        (cls.directory / 'compress.hir').write_text(source + '\n' + oracle_source())
        (cls.directory / 'harness.hir').write_bytes((build.HERE / 'harness.hir').read_bytes())
        cls.manifest = cls.directory / 'hirsel.json'
        cls.manifest.write_text(json.dumps(dict(schema=1, package=dict(name='la_bwt_9', version='1'),
            sources=[dict(path='compress.hir', module=['compress']),
                     dict(path='harness.hir', module=['harness'])],
            dependencies=[dict(manifest=str(build.ROOT / 'stdlib/std@1/hirsel.json'),
                               package=dict(name='std', version='1'), expose=[])])))

    @classmethod
    def tearDownClass(cls):
        cls.scratch.cleanup()

    def check_engine(self, flags):
        command = [str(self.cli), 'test', '--json', *flags,
                   str(self.manifest), 'la_bwt_9_']
        result = subprocess.run(command, cwd=build.ROOT, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report['summary'], dict(passed=len(vectors()) + 1, failed=0, total=len(vectors()) + 1))

    def test_interpreter(self):
        self.check_engine([])

    def test_llvm(self):
        self.check_engine(['--native', '--backend', 'llvm'])

    def test_cranelift(self):
        self.check_engine(['--native', '--backend', 'cranelift'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument('--hirsel')
    options, tests = parser.parse_known_args()
    if options.hirsel:
        os.environ['HIRSEL'] = options.hirsel
    unittest.main(argv=[__file__, *tests], verbosity=2)
