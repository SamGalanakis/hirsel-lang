#!/usr/bin/env python3
"""CRC32 recurrence checks on the interpreter, LLVM and Cranelift.

Each engine runs in its own Buck2 target. For an additional live
Rust oracle, set CRC32_9_RUST_REFERENCE to the la_crc32_9_reference executable.
The scored function is extracted verbatim from hash.hir; std.crc32 is only an
independent oracle. Generated test programs live in a temporary directory.
"""
import argparse
import json
import os
from pathlib import Path
import random
import subprocess
import tempfile
import unittest
import zlib


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]

# Go src/hash/crc32/crc32_test.go:40-74, ebe2feefc400 (BSD-3-Clause).
# Copyright 2009 The Go Authors. License: third-party/NOTICES.md, Go section.
GO_GOLDEN = [
    (b'', 0x0), (b'a', 0xe8b7be43), (b'ab', 0x9e83486d),
    (b'abc', 0x352441c2), (b'abcd', 0xed82cd11), (b'abcde', 0x8587d865),
    (b'abcdef', 0x4b8e39ef), (b'abcdefg', 0x312a6aa6),
    (b'abcdefgh', 0xaeef2a50), (b'abcdefghi', 0x8da988af),
    (b'abcdefghij', 0x3981703a),
    (b'Discard medicine more than two years old.', 0x6b9cdfe7),
    (b'He who has a shady past knows that nice guys finish last.', 0xc90ef73f),
    (b"I wouldn't marry him with a ten foot pole.", 0xb902341f),
    (b'Free! Free!/A trip/to Mars/for 900/empty jars/Burma Shave', 0x042080e8),
    (b'The days of the digital watch are numbered.  -Tom Stoppard', 0x154c6d11),
    (b"Nepal premier won't resign.", 0x4c418325),
    (b'For every action there is an equal and opposite government program.', 0x33955150),
    (b"His money is twice tainted: 'taint yours and 'taint mine.", 0x26216a4b),
    (b'There is no reason for any individual to have a computer in their home. -Ken Olsen, 1977', 0x1abbe45e),
    (b"It's a tiny change to the code and not completely disgusting. - Bob Manchek", 0xc89a94f7),
    (b'size:  a.out:  bad magic', 0xab3abe14),
    (b'The major problem is with sendmail.  -Mark Horton', 0xbab102b6),
    (b'Give me a rock, paper and scissors and I will move the world.  CCFestoon', 0x999149d7),
    (b'If the enemy is within range, then so are you.', 0x6d52a33c),
    (b"It's well we cannot hear the screams/That we create in others' dreams.", 0x90631e8d),
    (b"You remind me of a TV show, but that's all right: I watch it anyway.", 0x78309130),
    (b'C is as portable as Stonehedge!!', 0x7d0a377f),
    (b'Even if I could be Shakespeare, I think I should still choose to be Faraday. - A. Huxley', 0x8c79fd79),
    (b'The fugacity of a constituent in a mixture of gases at a given temperature is proportional to its mole fraction.  Lewis-Randall Rule', 0xa20b7167),
    (b'How can you write a big system without C++?  -Paul Glick', 0x8e0bb443),
    (b'01234567' * 1024, 0x1010dab0), (b'a' * 1089, 0x772d04d7),
]


def bitwise(data):
    """LangArena buffer_hash_crc32.rs:4-18, 3c9f84d (MIT, ./LICENSE)."""
    crc = 0xffffffff
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ (0xedb88320 if crc & 1 else 0)
    return crc ^ 0xffffffff


def payloads():
    rng = random.Random(0xc32_9)
    rows = [data for data, _ in GO_GOLDEN]
    rows += [b'123456789', bytes(range(256))]
    rows += [bytes(rng.randrange(256) for _ in range(n))
             for n in [*range(34), 63, 64, 65, 127, 128, 129, 255, 256, 257]]
    return rows


def literal(data):
    return 'b"' + ''.join(f'\\x{byte:02x}' for byte in data) + '"'


def program():
    # Keep the exact scored body. A test-only copy accepts a finalized initial
    # checksum to exercise continuation; the only edits are its name/parameter
    # and initial complement. No table implementation enters the scored body.
    source = (HERE / 'hash.hir').read_text()
    recurrence = source[source.index('fn crc32('):source.index('fn simple_sha256(')]
    update = recurrence.replace('fn crc32(data: Bytes)',
                                'fn update(initial: u32, data: Bytes)').replace(
                                    'let mut crc = 0xffffffffu32',
                                    'let mut crc = initial ^ 0xffffffffu32')
    checks = []
    rows = payloads()
    reference = os.environ.get('CRC32_9_RUST_REFERENCE')
    if reference:
        inputs = [data[:i] for data in rows for i in range(len(data) + 1)
                  if len(data) <= 257 or i == len(data)]
        output = subprocess.run([reference, 'vectors'],
                                input=''.join(data.hex() + '\n' for data in inputs),
                                text=True, capture_output=True, check=True)
        actual = [int(line) for line in output.stdout.splitlines()]
        expected = [bitwise(data) for data in inputs]
        if actual != expected:
            raise AssertionError('Rust bitwise oracle mismatch or missing rows')
    for data, expected in GO_GOLDEN:
        if bitwise(data) != expected:
            raise AssertionError('Go IEEE vector mismatch')
    for row, data in enumerate(rows):
        # All prefixes and split points of short/binary rows; long Go rows
        # exercise slicing cutoffs without quadratic interpreter work.
        prefixes = list(range(len(data) + 1)) if len(data) <= 257 else [len(data)]
        expected = [bitwise(data[:i]) for i in prefixes]
        if any(zlib.crc32(data[:i]) != value for i, value in zip(prefixes, expected)):
            raise AssertionError('independent zlib oracle mismatch')
        values = ', '.join(f'{value}u32' for value in expected)
        checks.append(f'''@test
fn crc32_row_{row}(t: Test) {{
  let data = {literal(data)}
  let expected: Array[u32] = [{values}]
  let ends: Array[u64] = [{', '.join(f'{i}u64' for i in prefixes)}]
  for j in 0u64..array.len(ends) {{
    let prefix = bytes.slice(data, 0, ends[j])
    test.check(t, crc32(prefix) == expected[j])
    test.check(t, std@1/crc32.digest(prefix) == expected[j])
    let continued = update(crc32(prefix), bytes.slice(data, ends[j], bytes.len(data)))
    test.check(t, continued == {bitwise(data)}u32)
    let mut state = std@1/crc32.new()
    std@1/crc32.update(mut state, prefix)
    std@1/crc32.update(mut state, b"")
    std@1/crc32.update(mut state, bytes.slice(data, ends[j], bytes.len(data)))
    test.check(t, std@1/crc32.finish(take state) == continued)
  }}
}}
''')
    return recurrence + update + '\n'.join(checks)


class CRC32Recurrence(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scratch = tempfile.TemporaryDirectory(prefix='la-crc32-9-')
        cls.directory = Path(cls.scratch.name)
        cls.cli = os.environ.get('HIRSEL') or str(
            ROOT / 'buck-out/tmp/cargo-bin-exe/gate-langarena/hirsel')
        source = cls.directory / 'vectors.hir'
        source.write_text(program())
        cls.manifest = cls.directory / 'hirsel.json'
        cls.manifest.write_text(json.dumps(dict(
            schema=1, package=dict(name='crc32_9', version='1'),
            sources=[dict(path='vectors.hir', module=[])],
            dependencies=[dict(manifest=str(ROOT / 'stdlib/std@1/hirsel.json'),
                               package=dict(name='std', version='1'), expose=[])])))

    @classmethod
    def tearDownClass(cls):
        cls.scratch.cleanup()

    def check_backend(self, flags):
        result = subprocess.run([self.cli, 'test', *flags, str(self.manifest)],
                                text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        count = len(payloads())
        self.assertIn(f'{count} passed; 0 failed; {count} total', result.stdout)

    def test_interpreter(self):
        self.check_backend([])

    def test_llvm(self):
        self.check_backend(['--native', '--backend', 'llvm'])

    def test_cranelift(self):
        self.check_backend(['--native', '--backend', 'cranelift'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument('--hirsel')
    options, tests = parser.parse_known_args()
    if options.hirsel:
        os.environ['HIRSEL'] = options.hirsel
    unittest.main(argv=[__file__, *tests], verbosity=2)
