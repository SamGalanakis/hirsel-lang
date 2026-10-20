# Compiler benchmarks

This suite measures how fast Hirsel compiles: the whole compiler, each of its
phases, and Hirsel against Rust on equivalent programs. (`bench/native`
measures the speed of the generated code instead.) It records a result file,
compares user-mode instruction counts with a stored baseline, and runs only when
invoked by hand.

```sh
# The release CLI bundle: hirsel plus its sibling hirsel-std.precheck.
.buck2/bin/buck2 build --target-platforms //tools/buck2:release //:hirsel-toolchain --show-output
tools/compiler-bench/run --toolchain <bundle> --output results.json            # whole suite
tools/compiler-bench/run --toolchain <bundle> --groups examples --modes check --output r.json
tools/compiler-bench/compare baseline.json results.json   # exit 1 on a regression
```

Each measured compile is its own `tools/capped` call, and so is each untimed
Cargo build of a Rust pair's dependencies. The host's capped lock is held for
that one command and released between runs. Never wrap the suite, or any part
of it, in one capped call: that holds the lock for the whole campaign and blocks
every other lane's local runs. Each command runs under the memory cap
(`HIRSEL_MEMORY_LIMIT`, default 4G, no swap) and `--timeout`. The suite runs one
command at a time; never run two suites at once.

A command the kernel kills at the cap is recorded as a `cap` cell, which is a
finding and not something to retry. A scale series stops at the first size that
reaches the cap or the timeout. Native build modes first build the pinned native
toolchains on the Buck2 pool, as `bench/native/suite.py` does.
The runtime archives come from the tree `run` is in, and the toolchain bundle
has none. So build `--toolchain` from that same commit, or a native link can
fail on runtime symbols the two disagree on.

## Per-phase timings

The suite times nothing inside the compiler itself. Each `check` and `build`
run passes `--timings=json`, and the suite reads the compiler's own report:
the last line on stderr, schema 1, documented in `docs/building.md`
("Compile timings") and produced by `hirsel_intern::timings`. A cell keeps two
parts of that report:

- the wall time of each driver **interval**, together with `(untimed)`. These add
  up to the command's wall time.
- the work time of each **phase**, which excludes the phases nested in it.
  Phases overlap the intervals, so they do not add up.

It also keeps the report's counts, such as definitions, units and V runs.
`hirsel precheck` takes no `--timings`, so the std cell has wall time, CPU time
and RSS plus perf counters. When a toolchain's usage shows no `--timings`, as before that
report existed, every cell records wall time, CPU time and RSS plus perf counters.
Peak RSS always comes from `/usr/bin/time`, never from `--mem-usage`, because
measuring held memory walks the allocator and slows the run.

## Corpus

| group | programs | probes |
|---|---|---|
| `examples` (`pairs`) | hello, todo-api, kvstore, logq, csvjson, mdtoc, bench-lite, each with its Rust port | real programs with std: JSON with derive, HTTP, tasks, channels, mutexes, files, CSV, base64, SHA-256, NFC |
| `conformance` | 14 accepted programs, one per rule family (`corpus.CONFORMANCE`) | small-program fixed cost across features |
| `tour` | every runnable program `docs/tour.md` links | the documented feature tour |
| `std` | `hirsel precheck stdlib/std@1/hirsel.json` | std checked whole, from source |
| `stress` (`pairs`) | nest 8/16/32, instances 32/128, wide 2000, long 2000, arms 200, nested-match 16/32, table 2000, derive 64, closures 200, each with a Rust twin | known cliffs: nested generic depth, many generic instances, many definitions, long bodies, wide and deep matches, big literals, comptime derive, closures |
| `scale` (`pairs`) | synthetic 3k, 10k, 30k, 50k, 100k lines | size scaling: copies of the six examples, one module each, all reachable from `main` |
| `edit` | the 10k synthetic program after one edit | the edit loop: a body edit, a private signature edit and a public signature edit |

The Rust ports (`rust/units/`) keep each example's types, functions and error
cases (not its tests, which no bench mode compiles), and use std plus a pinned dependency set that stands in for
Hirsel's std (`serde`/`serde_json` with derive, `sha2`, `base64`, `csv`,
`unicode-normalization`; `rust/Cargo.toml.in`, `rust/Cargo.lock.in`, built
`--offline`). Each port has about the same size as its example (1,765 lines
against 1,817 in all; csvjson is the largest difference, since Rust has
no flag parser in std). HTTP request parsing, a
seeded generator and a bounded arena are written out in the ports, where Hirsel
gets them from std. Their output is not compared byte for byte with Hirsel's:
the pair measures compile time for the same program shape.

## Modes, pairs and caches

| Hirsel mode | command | Rust pair |
|---|---|---|
| `check` | `hirsel check` | `cargo check` |
| `clif` | `hirsel build --debug --backend cranelift` | `cargo build` (debug) |
| `llvm-o0` | `hirsel build --debug` | `cargo build` (debug) |
| `llvm-o2` | `hirsel build` | — |
| `precheck` | `hirsel precheck` (std only) | — |
| `edit-*` | `hirsel check` after the edit | `cargo check` after the edit |

The examples run all four Hirsel modes; stress and scale programs run `check`
and `clif`; conformance and tour programs run `check`. Rust cells run at `-j1`
and `-j8` (`--jobs`). `HIRSEL_JOBS` gives the capped
scope the same number of CPUs. Hirsel compiles on one thread, so its cells are
`jobs: 1`. The scoreboard (`pairs` in the result, printed by `compare`) is each
Hirsel cell's median `instructions:u` divided by its Rust pair's. Above 1x,
Hirsel executes more user instructions. Wall and CPU times are descriptive only.

Each cell states its cache in its `cache` field.

- **Hirsel**: every run is a fresh process. It reads the std precheck and keeps
  nothing else between runs, because the query cache is process-local. So a
  cold run and a warm run are the same command.
- **Rust, cold**: the dependencies are built beforehand, as Hirsel's std is
  prechecked. Before each timed run, `cargo clean -p` removes the package's own
  artifacts and incremental state.
- **Rust, warm** (`edit-*`): the edit runs after an untimed `cargo check` of the
  unedited copy, keeping Cargo's incremental cache.

## Result file

`run` writes schema 1. The top level has `commit`, `started`, `host`,
`toolchain`, `repeat`, `cells` and `pairs`. Each cell has:

- `id` (`program/language/mode[/jN]`), `lines`, `groups`, `cache`;
- `status`: `ok`, `failed`, `cap` or `timeout`;
- every run, with exit status, wall and CPU seconds, peak RSS in MiB from
  `/usr/bin/time`, and Hirsel's intervals, phases and counts;
- a `summary`: the median, the median absolute deviation (MAD) and `n` for wall,
  CPU, RSS, `instructions_u` and each interval and phase. Every run also records
  all five perf events, multiplex running percentages, and SMT sibling snapshots
  and busy shares from `/proc/stat`.

By default each cell runs 3 times (`--repeat`). Programs of 30k lines or more
run once unless `--repeat-large` is given.

## Budget and regressions

`compare BASELINE RESULTS` compares only `instructions_u`, with a +1% threshold.
A cell that completed in the baseline but now fails, times out, or hits its cap
also regresses. Missing counters fail the measured cell; legacy successful
results without counters are rejected. Wall/CPU time, RSS, intervals and phases
remain descriptive in `report` and never influence this cross-session verdict.
`compare` exits 1 on a regression and 2 on unusable input.

Every compile is pinned to `--cpu` (31 by default), under
`perf stat -e task-clock,cycles,instructions,cycles:u,instructions:u`. The whole
suite holds CPU31's timing lock (other CPUs use a per-CPU lock in `/tmp`).
Cargo job counts select parallel jobs; all their threads share the pinned CPU. Each compile still gets its own capped slot.
Counters include compiler child processes (including rustc and the linker).

The following interval and phase names remain documented for attribution:

| interval | driver step |
|---|---|
| `image open` | reading and decoding the std precheck |
| `S0` | loading the package graph and sources, parsing, publishing std's declarations, the program check's own steps |
| `parallel` | checking each file's definitions and lowering the reachable closure (resolve, check, modes, lowering, V) |
| `S1` | reserved; 0 until the driver step exists |
| `parallel2` | reserved; 0 until the driver step exists |
| `merge` | publishing checked packages and dropping the session |
| `plan` | exporting, importing and adapting the native input |
| `units` | each unit's LIR import, placement, RC, reuse, cleanup lowering, LIR validation and code generation |
| `link` | the toolchain, the link and the output copy |
| `(untimed)` | the rest of the wall time: CLI startup, and a program's own execution under `run` and `test` |

| phase | work |
|---|---|
| `parse` | lexing and parsing |
| `resolve` | name resolution |
| `signatures` | signature normalization |
| `check` | body type checking (H) |
| `modes` | ownership and authority (I, J) |
| `lower` | lowering to TIR (K) |
| `verify` | the TIR checker V |
| `comptime` | compile-time evaluation |
| `plan` | native input planning |
| `import` | TIR to LIR import |
| `place` | placement |
| `rc` | reference-count insertion |
| `reuse` | reuse |
| `ready` | Ready lowering |
| `lir validate` | the LIR checker C |
| `codegen` | LLVM or Cranelift code generation |
| `store lookup` | code-store lookups |

The instruction threshold exceeds the audit's 0.024% within-session range for
`hirsel check todo-api`. Shared-host wall time can drift by 140% between
sessions; use `bench/micro/run` with old/new compiler commands in one interleaved
session when descriptive timing ratios are needed.

The baseline is the result of an explicit manual run on main, on the same
host. `run-main` records the first run as
`$HIRSEL_MAIN_RUNS/compiler-bench/baseline.json`; later manual runs compare
with it. A committed `tools/compiler-bench/baseline.json` takes precedence.
Reproduce the initial selection with:

```sh
tools/compiler-bench/run --toolchain <bundle> --groups examples --modes check --jobs 1 --cpu 30 --output baseline.json
```

The committed first baseline covers the seven example programs in Hirsel and
Rust `check` mode at one job (three runs each), on CPU30; its `selection` records the exact scope. It is
an initial count baseline, not coverage of all backend/stress/scale cells.

After a change that intends to move the numbers, replace the baseline with
the next manual run's result (copy `latest.json` over it).

## Run against main by hand

```sh
tools/compiler-bench/run-main
# Extra arguments select cells, for example:
tools/compiler-bench/run-main --groups examples --modes check
```

`run-main` fetches `origin/main` into a detached worktree and builds the
release bundle on the Buck2 pool. It runs the suite (one capped process at a
time) and compares the result with the baseline. Each result and comparison
is kept under `$HIRSEL_MAIN_RUNS/compiler-bench/`, and a regression appends
one line to `$HIRSEL_MAIN_RUNS/alarms.log`, the file `tools/main-stall-alarm`
writes. `HIRSEL_MAIN_RUNS` defaults to
`${XDG_STATE_HOME:-$HOME/.local/state}/hirsel/main-runs`. Timestamped results
are named `<UTC timestamp>-<commit>.json`; `latest.json` holds the most recent
result. Nothing is installed or enabled by these commands.
