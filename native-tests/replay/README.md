# native-tests/replay

Stored mismatch artifacts and the rules they must keep.

An artifact is a directory with `source.hir` and `case.txt`: the case, the
configuration name, the matrix and observation schema versions, the pinned
LLVM, the source digest and both observations as text. It holds no path, arena
index or address, so it replays from any directory and in an engine whose
arenas are numbered differently.

- `column-skew/` is recorded against a synthetic engine (every SEM-1 failure
  one column to the right), so the replay and reduction rules are tested when
  no real engine disagrees. `crates/hirsel-conformance/tests/it/native_gate.rs`
  copies it elsewhere, replays it in a fresh and in a warmed engine, refuses it
  under another schema version, and reduces it while keeping the same
  discrepancy. Regenerate `case.txt` with `tools/bless.sh replay`.
- Real mismatches are written by `tools/gate --full` (the `native --full`
  binary) under its report directory's `mismatch/`, each moved after it is
  written and replayed at once from a warmed engine; the first artifact of each
  mismatching case is also reduced to `reduced.hir`.

Replay or reduce one by hand:

```bash
archives="$(.buck2/bin/buck2 build //:hirsel-rt-archives --show-full-simple-output)"
native="$(.buck2/bin/buck2 build --target-platforms //tools/buck2:release //:hirsel-conformance-bin-native --show-full-simple-output)"
HIRSEL_RUNTIME_ARCHIVES="$archives" "$native" --replay <artifact directory>
HIRSEL_RUNTIME_ARCHIVES="$archives" "$native" --reduce <artifact directory>
```
