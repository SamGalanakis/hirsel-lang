#!/usr/bin/env python3
"""Materialize Go ebe2feef's SimpleFold orbits and Expand name properties.

These source constants remain private to regex; std unicode stays pinned to
Unicode 16. Run with a checkout of golang/go at COMMIT. The representation is
six-byte scalar pairs or nine-byte range/stride rows (24-bit big endian).
"""
from pathlib import Path
import re
import subprocess
import sys

COMMIT = 'ebe2feefc400f813aefdc8a036e66d2619da6649'
ROOT = Path(__file__).resolve().parents[1]
RUNTIME = r'''fn fold_scalar(offset: u64) -> u32 uses {} {
  let a: u32 = core@1/int.widen(FOLD[offset])
  let b: u32 = core@1/int.widen(FOLD[offset + 1])
  let c: u32 = core@1/int.widen(FOLD[offset + 2])
  a << 16 | b << 8 | c
}

fn simple_fold(cp: u32, mut work: u64) -> Result[u32, MatchError] uses {} {
  spend(mut work)?
  if cp < 128 {
    if cp >= 65 && cp <= 90 {
      return Ok(cp + 32)
    }
    if cp == 107 {
      return Ok(8490)
    }
    if cp == 115 {
      return Ok(383)
    }
    if cp >= 97 && cp <= 122 {
      return Ok(cp - 32)
    }
    return Ok(cp)
  }
  let mut low = 0u64
  let mut high = core@1/bytes.len(FOLD) / 6
  while low < high {
    spend(mut work)?
    let middle = low + (high - low) / 2
    let value = fold_scalar(middle * 6)
    if cp < value {
      high = middle
    } else if cp > value {
      low = middle + 1
    } else {
      return Ok(fold_scalar(middle * 6 + 3))
    }
  }
  Ok(cp)
}

fn name_scalar(table: Bytes, offset: u64) -> u32 uses {} {
  let a: u32 = core@1/int.widen(table[offset])
  let b: u32 = core@1/int.widen(table[offset + 1])
  let c: u32 = core@1/int.widen(table[offset + 2])
  a << 16 | b << 8 | c
}

fn name_property(table: Bytes, cp: u32, mut work: u64) -> Result[bool, MatchError] uses {} {
  let mut low = 0u64
  let mut high = core@1/bytes.len(table) / 9
  while low < high {
    spend(mut work)?
    let middle = low + (high - low) / 2
    let start = name_scalar(table, middle * 9)
    let end = name_scalar(table, middle * 9 + 3)
    if cp < start {
      high = middle
    } else if cp > end {
      low = middle + 1
    } else {
      return Ok((cp - start) % name_scalar(table, middle * 9 + 6) == 0)
    }
  }
  Ok(false)
}

fn expansion_name(c: char, mut work: u64) -> Result[bool, MatchError] uses {} {
  spend(mut work)?
  let cp = core@1/char.to_u32(c)
  if cp < 128 {
    return Ok(ascii_alnum(c) || c == '_')
  }
  if name_property(NAME_LETTERS, cp, mut work)? {
    return Ok(true)
  }
  name_property(NAME_DIGITS, cp, mut work)
}
'''


def main():
    ref = Path(sys.argv[1])
    assert subprocess.check_output(['git', '-C', ref, 'rev-parse', 'HEAD'], text=True).strip() == COMMIT
    src = (ref / 'src/unicode/tables.go').read_text()
    cases = src.split('var _CaseRanges = []CaseRange{')[1].split('\n}')[0]
    mapping = {}
    for lo, hi, upper, lower, _ in re.findall(r'\{(0x[\dA-Fa-f]+), (0x[\dA-Fa-f]+), d\{([^,]+), ([^,]+), ([^}]+)\}\}', cases):
        lo, hi = int(lo, 16), int(hi, 16)
        for cp in range(lo, hi + 1):
            if upper == 'UpperLower':
                target = lo + ((cp - lo) ^ 1)
            else:
                target = cp + (int(lower) or int(upper))
            mapping[cp] = target
    orbit = src.split('var caseOrbit = []foldPair{')[1].split('\n}')[0]
    for a, b in re.findall(r'\{(0x[\dA-Fa-f]+), (0x[\dA-Fa-f]+)\}', orbit):
        mapping[int(a, 16)] = int(b, 16)
    mapping = {a: b for a, b in mapping.items() if a != b}
    tables = {'FOLD': [n for pair in sorted(mapping.items()) for n in pair]}
    for category, name in [('L', 'NAME_LETTERS'), ('Nd', 'NAME_DIGITS')]:
        table = src.split('var _' + category + ' = &RangeTable{')[1].split('\n}')[0]
        tables[name] = [int(n, 0) for row in re.findall(r'\{(0x[\dA-Fa-f]+), (0x[\dA-Fa-f]+), (\d+)\}', table) for n in row]
    path = ROOT / 'stdlib/std@1/regex_fold.hir'
    header = '''// Go ebe2feef unicode/letter.go:354-389 and unicode/tables.go (Unicode 17).
// Copyright 2009 The Go Authors. BSD-3-Clause; third-party/NOTICES.md.
// Ported by tools/port-regex-unicode.py: sorted SimpleFold pairs and L/Nd ranges.
// Private to regex: std unicode's Unicode 16 normalization is unchanged.
'''
    for name, numbers in tables.items():
        data = b''.join(n.to_bytes(3, 'big') for n in numbers)
        header += f'const {name}: Bytes = b"' + ''.join('\\x%02x' % b for b in data) + '"\n\n'
    path.write_text(header + RUNTIME)
    print('SimpleFold pairs:', len(mapping), 'L/Nd ranges:', len(tables['NAME_LETTERS']) // 3, len(tables['NAME_DIGITS']) // 3)

if __name__ == '__main__': main()
