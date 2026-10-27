# Host replay fixtures

`current-tapes` in ToyAnalyzer, SheetEval and DurableQueue contains fresh native
recordings of the original host cells, their original app sources, and output
oracles. ToyAnalyzer and DurableQueue have 18 cells each. SheetEval has 18
production cells and two recordings of real `/dev/full` output failures.
Runtime changes may intentionally invalidate these pre-release tapes.

Prepare a scratch copy with the current native replay consumers:

```
python3 crates/hirsel-cli/tests/support/prepare_host_tapes.py toy_analyzer_31 \
  --hirsel /path/to/hirsel --output /tmp/toy-tapes
```

Use the native toolchain/product environment normally supplied to host tests.
The script preserves all tape bytes and builds LLVM and Cranelift consumers
under the existing 300-second build cap. Select `sheet_eval_31` or
`durable_queue_31` for the other archives. Pass the resulting directory through
`TOY_ANALYZER_31_ARCHIVES`, `SHEET_EVAL_31_PRODUCTION`, or
`DURABLE_QUEUE_31_TAPES` to the corresponding explicit ignored host audit.
The refusal tests alter scratch copies and retain exact consumption checks.
The ToyAnalyzer cancellation audit still exposes a current read-suppression
failure after successful binding and replay. Its known-failure row belongs to
runtime-reds-82; the fresh tapes preserve that failure.

PubSub also retains the ten recovered historical prefixes in its `current-tapes`
directory. Its explicit historical complement test generates fresh native
recordings and checks both native and both interpreter replay adapters.
