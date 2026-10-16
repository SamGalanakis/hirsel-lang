# Layout golden vectors and C probes

Fixtures for `crates/hirsel-lir/tests/it/layout.rs`.

- `table-x86_64.txt`, `table-aarch64.txt`: the canonical snapshot of a layout
  table covering every representation family, per executed target. A
  derivation change must be reviewed, then re-blessed with
  `tools/bless.sh layout`.
- `c_records.h`: hand-written C mirrors of the `@layout(c)` fixtures and of
  the dataful variant of two null-pointer niches (the binarytrees node and one
  whose niche word follows other fields). The ignored test
  `c_probes_agree_on_both_targets` (run by `tools/check-native.sh`) asserts
  clang's `sizeof`, `_Alignof` and `offsetof` (and the niche word's offset)
  against the derived layouts for `x86_64-linux-gnu` and `aarch64-linux-gnu`.

C interop beyond plain `@layout(c)` records (variants, pointers, callbacks,
exported wrappers) is native-unavailable pending #6 and has no probe.
