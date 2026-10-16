# Native compilation cost

This standalone measurement tool compiles exactly `hirsel_conformance::native::baseline`
with the ordinary driver, default production options, x86-64 and whole partitioning.
It never executes a source program. Runtime archive construction precedes every
case timer and is reported separately. Every error remains a row.

`phases.c` interposes the same LLVM C boundaries as the attribution IR dumper.
One identical probe executable/tree selects either O2/Default or O3/Aggressive;
O0 layout machines and the O0 launch shim are preserved. Monotonic nanoseconds
inside LLVMRunPasses and LLVMTargetMachineEmitToMemoryBuffer are counted separately.
End-to-end time includes loading/checking, native passes, LLVM, verification and
linking. Source collection and the runtime build are outside the per-case timers.
The library is never loaded by a timed workload. The primary probe removes the
preload from runtime-build/link children. The existing secondary helper inherits
it in those children; its parent writes the final LLVM totals after they exit.

From the repository root, with the required four-job environment:

```sh
cargo build --release --manifest-path bench/native/tools/compile-cost/Cargo.toml
clang -std=c11 -O2 -shared -fPIC bench/native/tools/compile-cost/phases.c -ldl -o /tmp/hirsel-compile-phases.so
HIRSEL_PERF_LEVEL=O2 LD_PRELOAD=/tmp/hirsel-compile-phases.so /path/to/hirsel-compile-cost "$PWD" /tmp/compile-O2 > /tmp/compile-O2.jsonl
HIRSEL_PERF_LEVEL=O3 LD_PRELOAD=/tmp/hirsel-compile-phases.so /path/to/hirsel-compile-cost "$PWD" /tmp/compile-O3 > /tmp/compile-O3.jsonl
```

Run each complete measurement once, under the task's resource scope. An optional
third argument filters case IDs for a tool smoke check; a filtered run is labelled
with both the full baseline and selected counts and is not corpus evidence.

For the secondary table, the existing `hirsel-native-build` helper may instead
be launched with the same preload and level selection. Set
`HIRSEL_PERF_PHASES_OUTPUT` to a per-process JSON path to record its LLVM totals;
the helper's ordinary `compile_seconds` continues to exclude runtime construction.
Its printed `hirsel_opt_level` describes the production default; when overriding
it for this comparison, record the actual interposed level separately. The
parent writes the final phase summary after all runtime/link children exit.

`python3 bench/native/tools/compile-cost/benchmark-cost.py /path/to/hirsel-native-build`
runs the secondary 14 × 3 × 2 build comparison, retaining all results and errors
in the gitignored `bench/native/out/compile-cost/benchmark-compile-cost.jsonl`.
Run it from the repository root after building the ordinary benchmark helper and
`/tmp/hirsel-compile-phases.so`; it uses that fixed measurement-only preload path.

The secondary comparison caps each helper at 300 seconds.
Timeouts remain rows with partial output and elapsed wall time. On Linux, the
parent reads the interposer's four completed-call counters from its child before
termination; a failed read is explicitly unavailable. An in-progress LLVM call
has not yet added its duration. Successful calls use the ordinary final summary
and their `compile_seconds` excludes runtime construction.
