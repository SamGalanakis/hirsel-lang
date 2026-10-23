# Prior art

- tinyexpr, 7ab30db449a359e8af77fbb062ab2ec7ee686d62, tinyexpr.c:730–770
  (term/sum loops) and smoke.c:47–87. zlib license (LICENSES/tinyexpr.txt).
  Port preserves left associativity and precedence. Deviations: checked i64,
  truncating division, typed zero/overflow errors, bounded AST, cell references
  and SUM rectangles; no float, exponent, modulo or external functions.
- CPython, 010bebbf9fd12bba1e3f40bdbddacd51a9c2bf20,
  Lib/graphlib.py:86–136 and Lib/test/test_graphlib.py:41–130. PSF license
  (LICENSES/cpython.txt). Ready-layer/release structure and DAG/cycle vectors.
  Deviations: fixed dense grid, row-major ready ties, bounded batches, edits
  invalidate transitive dependents; cycles become typed cached errors rather
  than exceptions. The shipped std toposort implements cycle detection.
- std file.read_all_bounded, create_atomic/commit, JSON and Tasks are reused.
  The Python oracle recursively recomputes from source after every edit and
  uses Python ast for precedence, independently of the app's parser/cache.
