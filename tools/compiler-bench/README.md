# Compiler benchmarks

This extends the compiler corpus and offline Rust ports; `bench/native` measures
execution speed instead. Every measured command and dependency preparation takes
its own `tools/capped` slot, released between commands. Never wrap the runner in
another capped call. Do not run your own compiler builds during a measurement.

```sh
# Distribution measurements: fat LTO, one codegen unit, PGO and std image.
tools/toolchain-dist/build --pgo --output /tmp/hirsel-dist
# Developer release bundle (keeps its existing profile):
.buck2/bin/buck2 build --target-platforms //tools/buck2:release //:hirsel-toolchain --show-full-output
# The bundle contains hirsel and its sibling hirsel-std.image.
tools/compiler-bench/run --quick --toolchain <bundle> --output quick.json
tools/compiler-bench/report quick.json --scoreboard > scoreboard.md
```

`--quick` selects hello and csvjson, all four modes and their Rust pairs, one
cold/warm round, at one job. Each command has a 45-second timeout and the runner
stops issuing measurements after a 540-second work budget. This keeps the
reduced matrix below ten minutes of active work, including dependency setup;
waiting for another lane's capped slot and initial Buck2 tool preparation are
outside the budget. A timeout is a result, never an invitation to raise the cap.
Quick is for iteration and counter comparisons, not stable wall-time ratios.

The archived historical campaign used ten rounds, with seeded language/mode order within each input
(`--seed 20261021`). Cold is immediately followed by unchanged warm. Both sides
are pinned to `--cpu` (31 by default) and counted with the existing microbench
`perf stat` wrapper: task-clock, cycles, instructions, cycles:u, instructions:u.
Each round holds the CPU timing lock and yields it before the next round, so a
large compiler corpus cannot block other lanes for hours. Only individual
commands hold capped. `--lock-scope session` explicitly holds the convention
lock for all ten rounds; only that uninterrupted mode permits wall ratios.
Only user instructions decide cross-session regressions (`compare`, +1%). Wall
ratios require an uninterrupted ten-round session; normal round-scoped runs
retain descriptive wall values and instruction ratios. SMT sibling busy shares and perf
multiplex percentages remain in the raw JSON.

| mode | Hirsel | Rust pair |
|---|---|---|
| check | `check --timings=json` | `cargo check --offline` |
| debug | `build --debug --timings=json` | `cargo build --offline` |
| release | `build --release --timings=json` | `cargo build --release --offline` |
| native | `run --native --timings=json` until first output/listening | `cargo build --release --offline` (compile only) |
| image | `image stdlib/std@1/hirsel.json` | none |

The older explicit modes `clif`, `llvm-o0`, `llvm-o2` and the `edit-*` selectors
remain accepted. `--groups`, `--select`, `--modes`, `--jobs`, `--repeat` and
`--timeout` narrow a run; a native-only selection still includes Rust release.
The edit group must be explicitly selected with its `edit-*` modes. It uses the
existing body/private/public signature probes on syn10k. They do not measure a
resident engine; unchanged warm is not an edit benchmark.

Cold deletes the runner-owned native/cache directory and removes the Rust
package's own artifacts and incremental state. It retains the published std
image and builds Rust dependencies beforehand (the pinned Cargo manifest and
lockfile under `rust/`, offline). Warm runs the exact command again without
cleaning. Neither flushes the host page cache or deletes shared host caches.
The existing `HIRSEL_PROGRAM_CACHE=off` policy is retained so compiler work is
measured even for unchanged commands; this is not an executable-cache-hit
benchmark. Each mode and input has its own native cache; Rust modes share dependency
artifacts. The runner never cleans another lane's cache or example data.

The corpus includes the seven original example pairs (hello, todo-api, kvstore,
logq, csvjson, mdtoc, bench-lite), tasks and hello-web, the 14 conformance probes,
the runnable tour, std alone, all existing stress pairs, and synthetic copies of
the six examples at 3k, 10k and 30k lines. The generators can still accept larger
sizes explicitly. Every scale module is reachable. Tasks reuses the existing `bench/apps/tasks/rust` port and pinned lockfile,
compiling only its app binary (not its load-generator binaries). Hello-web, std,
tour and conformance have no paired Rust port; their ratios are unavailable.
Tour/conformance programs without an executable entry run check only; std has no
native entry, so it runs image only. `image` accepts no timing flag.

The stress group includes `inline-constant-{1024,2048,4096}` (a homogeneous
`Inline[u64, N]` constant indexed once) and `record-flow-{4,8,16}` (that many
match arms per mutable helper over a fixed 165-field message). The record
helpers assign a field through `?`; a compile-time caller forces independent V
validation during `check`, including the failure edges. Record width and helper
count stay fixed across the series. Select both with
`--groups stress --select 'stress/(inline-constant|record-flow)-' --modes check`.
Compare instruction counts across each doubling and retain failed/capped cells;
the older List table and long-statement cases exercise different shapes.
[SCALING.md](SCALING.md) records the post-cutover measurements and their limits.

Native wall time is measured by `startup.py` inside the counted command, from
launch to the first stdout byte. Tasks stops at its listening announcement and
gets `quit` on stdin. Hello-web prints no stdout: the endpoint is its first
successful TCP connection, after refusing a pre-existing listener. The probe
then sends and drains a valid HTTP GET using Python `http.client`; a bare
connect-and-close exposed a pinned-runtime MEM-13 EOF path. Those original
failed samples remain failed and are excluded from accepted startup summaries. Other server
examples also stop at their listening announcement; their native child is
terminated so the CLI can still emit its actual compilation phases. Example filesystem grants
point at private scratch; tasks uses its original static assets and an ephemeral
port. Silent stress programs report wall to exit, explicitly labeled in the
scoreboard. Logq/mdtoc use `--help` so their first output needs no input fixture.
Counters and peak RSS include bounded shutdown (one second maximum grace before
termination); `total_wall_s` retains that duration. A server killed before CLI
exit cannot emit the CLI's phase report: the scoreboard marks it unavailable.
Finite programs and gracefully stopped tasks retain their actual report. No
build-mode phase timings are substituted for native timings.

Each schema-1 cell has a temperature-suffixed ID, source line count, exact
commands, status, every run and median/MAD summaries of wall, CPU, instructions,
peak RSS, driver intervals, phase self time and compiler counts. Native samples
also retain the readiness endpoint and whether termination was needed. Raw
intervals sum to the CLI wall; phases exclude nested phases and overlap driver
intervals. `report --scoreboard` prints one table per mode, Rust wall/count/RSS
and ratios, and the five largest self phases per input and temperature.

The runner writes a `.partial` checkpoint after every round.
`--resume <file.partial>` in the same `--work` directory skips completed inputs
and rounds while preserving the seeded order. It refuses mismatched compiler
hashes, commit, CPU, selection, seed, round count or work directory. Resumed
cells never claim wall ratios across the gap; their instruction counts remain
usable. A continuous completed input from before a resume keeps its original
wall-ratio validity. A failed pair is
not retried in later rounds. A capped/timed-out scale input blocks larger sizes
in that language/mode; all failures remain findings. Partial successes never
turn a failed cell into a claimed speedup. Native tools/runtime archives are
prepared from the same tree as the compiler. `--environment FILE` reuses the
JSON returned by the runner's `environment(True)` helper after preparation has
finished, so no own Buck2 build overlaps measurements.

`run-main` builds the PGO distribution bundle (see `tools/toolchain-dist/README.md`),
runs the quick selection on a detached origin/main and saves results
under `$HIRSEL_MAIN_RUNS/compiler-bench/`. It compares instructions with the
committed `baseline.json` (the original check-only baseline), or records a first
host baseline when none exists. It writes alarms on regressions. Nothing here
installs a timer. See `RESULTS.md` for the superseded, incomplete historical baseline. New
measurements use `--quick` against current main or arch; the old multi-hour
campaign must not be repeated.

## Prior art and deliberate differences

- Cold/unchanged scenarios follow rust-lang/rustc-perf `collector/src/compile/benchmark/mod.rs:496–550`
  (commit `10c88d4`; inspected clone `/tmp/ref-rustc-perf`). It prepares dependencies,
  then measures a fresh incremental state and an unchanged second compilation.
  This suite measures whole Cargo commands, including Cargo's no-op fast path,
  because the requested unit is command-to-output latency; rustc-perf forces the
  leaf rustc invocation. Cold removes only package artifacts rather than copying
  the entire prepared target directory, keeping the existing harness's approach.
- Counts, affinity, sibling snapshots and ten-round seeded interleaving reuse
  `bench/micro/measure.py:79–125`, also used by the app benchmark at
  `bench/apps/tasks/bench:511–539`. Cold/warm stay adjacent because the second
  command must inherit exactly the first command's cache state.
- Readiness observation and directory rebinding follow
  `bench/apps/tasks/support.py:26–49,80–104`: selectors observe readiness, filesystem
  grants target scratch, and stdin allows graceful shutdown. `startup.py` observes
  the first byte instead of requiring a complete line for finite programs. It
  stops only the launcher's native child for other servers so the CLI can emit
  its phase report; whole-group termination is reserved for final cleanup.
  Hello-web needs a TCP readiness probe because it prints no announcement. Its
  valid-request/drained-body behavior follows `bench/micro/http/load.py:14,29–38`;
  Python stdlib `http.client.HTTPResponse.read` handles HTTP framing. This differs
  from a bare connect-and-close because that triggered a real runtime EOF error
  in the pinned baseline, and differs from the old firstout.py GET probe by
  draining the response before closing the connection.
- Cache ownership follows rustc's private-session separation documented in
  `compiler/rustc_incremental/src/persist/fs.rs:4–37` (rust-lang/rust commit
  `15c1044`; `/tmp/ref-rustc`). The harness owns disposable per-input/mode
  directories and never cleans a shared published compiler cache. It measures
  existing compiler caches; it does not implement an artifact cache.

## Resident edit loop

`resident oracle` compares the stdout, stderr and exit code of a resident
check, a fresh process loading disk results and a cache-free process after
each unchanged, body, signature, insertion, movement, deletion, syntax-error
and recovery edit, in both human and JSON modes. It uses the corpus generators
above and edits private copies of the programs.

```sh
tools/compiler-bench/resident oracle \
  --toolchain <bundle> --work /tmp/hirsel-resident-oracle --output oracle.json
tools/compiler-bench/resident measure \
  --toolchain <bundle> --native --work /tmp/hirsel-resident-measure --output edit-loop.json
```

Each program holds one capped slot for its resident session and releases it
before the next program. Scale sizes stop at a resource limit. The std image
command is outside the resident protocol and is reported as excluded.

`measure --native` runs ten seeded interleaved rounds on tasks and syn10k.
Set `LLVM_SYS_211_PREFIX`, `HIRSEL_LINK_KIT`, and `HIRSEL_RUNTIME_ARCHIVES`
to this tree's Buck2 native tool inputs, as the main compiler suite does. The server
uses CPU31 and the client CPU7. `perf stat` counts cycles and instructions in
both processes, and a control/ack pipe enables the server counters only for
the request. The report retains each sample and medians; compare times only
as ratios inside that session. `--select` filters oracle programs, and
`--scale` supplies comma-separated generated scale sizes.
