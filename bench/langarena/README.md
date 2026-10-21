# LangArena in Hirsel

Upstream: https://github.com/kostya/LangArena at `3c9f84ddaeec834c0631747cb789e8cfd602f993`.
The copied configs contain all 50 tasks. Missing implementations print a warning;
`compare.py` retains a row for every selected task.

Build and run from the repository root:

```sh
python3 bench/langarena/build.py
bench/langarena/build/langarena --hirsel-grants bench/langarena/build/grants.json bench/langarena/test.js CLBG
python3 bench/langarena/compare.py --quick --filter CLBG
```

The build uses the Buck2 release CLI and `hirsel build --release` (LLVM).
Set `HIRSEL=/absolute/path/to/hirsel` to reuse a compiler you already built.
Generated manifests, the registry, binaries and raw measurements live in ignored
`build/`. Do not commit them. Native executables need explicit I/O grants;
`build.py` creates read-only repository access, stdout and clocks in
`build/grants.json`. After the launch options, pass the config and an optional
case-insensitive substring filter. Checksums include warmup and timed iterations;
only timed iterations contribute to the printed duration. A mismatch exits 1.

## Add a group

Create one file per group, such as `compress.hir`. Keep its state and helpers
private. Put these declarations in that file:

```hirsel
// langarena: Compress::LZWEncode
// langarena: Compress::LZWDecode
pub fn execute(rt: Io, name: String, config: harness.Config) -> Result[harness.Report, String] {
  // Construct and prepare state outside timing, then call harness.measure.
}
```

`build.py` discovers these comments in `*.hir`, rejects duplicate or unknown
names, generates `build/registry.hir`, and derives the single package manifest.
There is no central task list to edit. `harness.number(config, "iterations")`
reads integer fields. `harness.measure(rt, config, initial, step, checksum)`
accepts a state, a `fn(mut State, u64) -> ()` and a `fn(State) -> u32`.
It performs the configured warmup (default max(iterations / 5, 1)), keeps that
state, and measures subsequent iterations with IDs restarted at zero. For random
workloads, use `harness.measure_reset` with a reset callback to reset only the
PRNG after warmup. Preparation belongs before measure, checksum after timing.
Keep the upstream RNG constants (IM=139968, IA=3877, IC=29573, seed=42);
reset its state through that callback before timed iterations. Shared checksum
helpers are `harness.hash_bytes` (DJB2, wrapping u32) and `harness.hash_float` (fixed seven decimal places). Keep upstream algorithms,
state retention and helper RNG semantics; cite upstream file:line in comments.

Porter ownership:

| Lane | Group files |
| --- | --- |
| harness | binarytrees, sieve, clbg; harness, main, build.py, compare.py |
| port-1 | brainfuck, matmul, base64, hash, calculator |
| port-2 | json, csv, template, words, log_parser, distance |
| port-3 | compress, sort |
| port-4 | graph, maze, neural_net, text_raytracer, cache_simulation, game_of_life |

Check your task checksums with both `test.js` and `run.js`, natively in release
mode. Then run `compare.py --quick --filter NAME`. For a blocked task, add
`// langarena-blocked: Task::Name | minimal missing feature` to your group file
without registering that task, and notify the orchestrator. The report reads
these declarations. Do not fake results or discard missing tasks.

## Comparisons

Only Hirsel and LangArena's unchanged Rust implementation run locally. Every
other language uses the latest published results file from upstream `results/`.
`compare.py --help` describes filters, checkout locations, rounds and report paths.
By default ten seeded interleaved rounds run on CPU31 under the shared
`bench/micro/measure.py` timing lock; medians and geometric means use
only paired tasks passing every round. Raw measurements, exact commands, logs,
RSS and perf counters are retained in ignored `build/SESSION/measurements.json`.
Use `--output /tmp/my-groups.md` for a porter report to avoid editing RESULTS.md.

Full mode runs `run.js`: estimated Hirsel on the published host = local Hirsel /
local Rust × published Rust. Runtime and RSS estimates are separate. Compile seconds are diagnostic
singleton observations; they receive no ratios, projections or ranks because
compilation was not measured in ten seeded interleaved rounds.
Other languages' raw times are never compared directly with this host. Quick
mode (`--quick`, `test.js`) validates ports and gives local measurements; it
makes no published runtime/memory projections because the workloads differ.
Missing, blocked and filtered tasks remain explicit. A denied perf counter is
reported; timing ratios are informational if instructions cannot be collected.

Runtime uses the runner's timed interval. RSS is the isolated whole process
peak, including preparation and warmup. Cold/incremental compile times apply to
the whole suite; Hirsel's coverage is currently smaller. Fetching dependencies
is excluded; Rust cold includes compiling fetched dependencies. Incremental
compilation changes `RECOMPILE_MARKER_0` to `RECOMPILE_MARKER_1` in one source
file and restores it, as upstream does. Native toolchains come from Buck2;
nothing is installed system-wide. Rust sources are cloned into a fresh,
session-owned directory outside this repository's Cargo workspace.

Source metrics follow `benchmarks.rb:54-159` and `docs/gen.rb:280-321`: source
bytes (including comments), gzip of concatenated source files, lines and
source/gzip redundancy. Per-task source metrics count the owning group files;
whole-suite totals include the harness. Shared groups count once in suite totals.
Upstream's expressiveness score combines redundancy relative to the mean with
relative line count to the power 1.5. Subset source totals receive no full-suite
expressiveness ranking.

The scoped protocol tests run with `.buck2/bin/buck2 test //:gate-langarena`.
They build once, exercise quick checksums for every registered group, and check
warmup, filtering, mismatch exits, discovery and estimate handling. They do not
run reference languages or the full-size benchmark campaign.

Port adaptations: Hirsel uses lists for Rust's fixed numeric arrays and copies
scalar planet records to update pairs without overlapping mutable loans.
Tree arithmetic uses signed 64-bit intermediates to keep checked additions
defined before the final wrapping 32-bit checksum. Arena indices use the list
API's u64 index type; this makes Hirsel's arena nodes wider than Rust's.
The sieve tests `p * p <= limit` instead of converting a floating square root;
this gives the same integer iteration boundary. No algorithms are replaced.

Accepted comparisons freeze and SHA-256 hash consumed Hirsel/std sources,
fixtures, manifests, compiler executables, native toolchain/runtime archives and
both benchmark executables. The driver checks these identities across the
session, fails if they change, and saves each sample immediately. It reuses the
micro harness's pinned perf command and records SMT sibling activity and counter
running percentages. Earlier unpinned sessions are preliminary evidence only.
