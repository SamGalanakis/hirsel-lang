#!/usr/bin/env python3
"""Independent bigint_mul reference; run with --derive to print its provenance.

Python integers derive the budget oracle without running Hirsel. Tiny products
are also checked with Go's independent big-endian byte-schoolbook algorithm.
No Karatsuba, machine-word multiplication, or Hirsel runtime code is ported.
"""
import hashlib
import json
from pathlib import Path
import random
import sys
import unittest

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
MASK = (1 << 64) - 1
SOURCE_SHA256 = 'c11bee845c4f95af0537d9994ecc6b16f34f1565897803d6e0ac08e9bedfb567'
REFERENCE_COMMIT = 'ebe2feefc400f813aefdc8a036e66d2619da6649'
REFERENCE_SHA256 = {
    'src/math/big/natmul.go': 'f1767186e544a77fea89923a0051db1f3a36658d34f12ef4b35b78bb42234d40',
    'src/math/big/int_test.go': '9c49e5f12402151e7cc0776ec296ca89012df603bae922571296432c71cc6a08',
}


def operand(n, salt):
    value, state = 0, salt
    for _ in range(n):
        state = (state ^ (state << 13)) & MASK
        state = (state ^ (state >> 7)) & MASK
        state = (state ^ (state << 17)) & MASK
        value = (value << 64) + (state | (1 << 63))
    return value


def checksum(n, rounds, extra=0, multiply=lambda a, b: a * b):
    a, b = operand(n, 123456789), operand(n + extra, 987654321)
    total = 0
    for _ in range(rounds):
        product = multiply(a, b)
        total = (total + ((product & MASK) ^ product.bit_length())) & MASK
        a += 1
    return total


# Adapted from Go src/math/big/int_test.go:130-178 at REFERENCE_COMMIT.
# Copyright 2009 The Go Authors.
# Redistribution and use in source and binary forms, with or without
# modification, are permitted provided that the following conditions are met:
# * Redistributions of source code must retain the above copyright notice,
#   this list of conditions and the following disclaimer.
# * Redistributions in binary form must reproduce the above copyright notice,
#   this list of conditions and the following disclaimer in the documentation
#   and/or other materials provided with the distribution.
# * Neither the name of Google LLC nor the names of its contributors may be
#   used to endorse or promote products derived from this software without
#   specific prior written permission.
# THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
# AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
# IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
# ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE
# LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
# CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
# SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
# INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
# CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
# ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
# POSSIBILITY OF SUCH DAMAGE.
def mul_bytes(x, y):
    """Unsigned big-endian inputs/output; retain Go's carry and normalization."""
    z = bytearray(len(x) + len(y))
    k0 = len(z) - 1
    for d in reversed(y):
        if d:
            k, carry = k0, 0
            for digit in reversed(x):
                t = z[k] + digit * d + carry
                z[k], carry = t & 255, t >> 8
                k -= 1
            z[k] = carry
        k0 -= 1
    return bytes(z).lstrip(b'\0')


def bytes_product(a, b):
    def encoded(n):
        return n.to_bytes((n.bit_length() + 7) // 8, 'big')
    return int.from_bytes(mul_bytes(encoded(a), encoded(b)), 'big')


def derivation():
    a, b = operand(1000, 123456789), operand(1000, 987654321)
    p = a * b
    return dict(source_sha256=SOURCE_SHA256, reference_commit=REFERENCE_COMMIT,
                reference_sha256=REFERENCE_SHA256, arguments=[1000, 1, 0],
                operand_bits=[a.bit_length(), b.bit_length()],
                operand_sha256=[hashlib.sha256(v.to_bytes(8000, 'big')).hexdigest() for v in (a, b)],
                product_bits=p.bit_length(), product_low64=p & MASK,
                checksum=checksum(1000, 1), stdout=f'{checksum(1000, 1)}\n')


class BigintOracle(unittest.TestCase):
    def test_workload_identity(self):
        source = ROOT / 'bench/micro/programs/bigint_mul.hir'
        self.assertEqual(hashlib.sha256(source.read_bytes()).hexdigest(), SOURCE_SHA256,
                         'workload changed: independently rederive the oracle and review its binding')

    def test_budget_derivation(self):
        self.assertEqual(checksum(1000, 1, 0), 4901293426875656068)
        result = derivation()
        self.assertEqual(result['product_bits'], 127999)
        self.assertEqual(result['product_low64'], 4901293426875602043)

    def test_schoolbook_hand_vectors_and_go_unsigned_boundaries(self):
        # Go prodZZ zero/one cases, plus hand-computable byte and word carries.
        vectors = [(0, 0, 0), (1, 0, 0), (1, 1, 1), (991, 991, 982081),
                   (255, 255, 65025), (256, 256, 65536), (65535, 65535, 4294836225),
                   (MASK, 2, 36893488147419103230),
                   (MASK, MASK, 340282366920938463426481119284349108225)]
        for a, b, expected in vectors:
            for x, y in ((a, b), (b, a)):
                with self.subTest(x=x, y=y):
                    self.assertEqual(bytes_product(x, y), expected)
                    self.assertEqual(x * y, expected)
        self.assertEqual(mul_bytes(b'\0\0\xff', b'\0\xff'), b'\xfe\x01')
        self.assertEqual(mul_bytes(b'', b'\x01'), b'')

    def test_schoolbook_independent_products(self):
        rng = random.Random(10)
        for _ in range(100):
            a, b = rng.getrandbits(rng.randrange(513)), rng.getrandbits(rng.randrange(513))
            self.assertEqual(a * b, bytes_product(a, b))

    def test_extra_rounds_zero_and_increment_controls(self):
        for n in range(5):
            for rounds in (0, 1, 2, 7):
                for extra in (0, 1, 3):
                    with self.subTest(n=n, rounds=rounds, extra=extra):
                        self.assertEqual(checksum(n, rounds, extra),
                                         checksum(n, rounds, extra, bytes_product))
        self.assertEqual(checksum(2, 7, 3), 14564464905320657940)
        self.assertEqual(checksum(3, 4, 1), 16353269618223746260)
        # n=0 becomes one after the first epoch, even though its first product is zero.
        self.assertEqual(checksum(0, 2, 1), (operand(1, 987654321) ^ 64) & MASK)
        self.assertNotEqual(checksum(2, 1), checksum(2, 1, 1))
        self.assertNotEqual(checksum(2, 7), (7 * checksum(2, 1)) & MASK)


if __name__ == '__main__':
    if sys.argv[1:] == ['--derive']:
        print(json.dumps(derivation(), indent=2))
    else:
        unittest.main()
