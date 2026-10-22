# LangArena regex ports — 2026-10-22

`Etc::LogParser` and `Template::Regex` were already implemented by `07413cab`.
This session audits those ports against LangArena
`3c9f84ddaeec834c0631747cb789e8cfd602f993`, validates quick checksums on both native backends,
and supplies the missing measurements. Measured Hirsel snapshot:
`d7909027371dde1064f31a3c5652226094937ffe`; Rust uses toolchain `1.99.0`. No language, IR, runtime or regex engine
change is included. This per-session report follows `docs/hot-files.md`;
`RESULTS.md` retains its historical campaign.

## Fidelity and API gaps

- LogParser follows upstream `rust/src/benchmarks/log_parser.rs:13-177`:
  identical thirteen patterns, case-insensitive flags, log bytes (including
  lowercase hexadecimal session IDs), non-overlapping matches, per-pattern map,
  wrapping total and retained warmup state. Preparation compiles the patterns
  before measurement. The port is registered by its `// langarena:` declaration.
- Template follows upstream `rust/src/benchmarks/template.rs:39-183`: identical
  names, cities, HTML, single-brace literals, lazy `\{\{(.*?)\}\}` pattern,
  trimmed capture-one map lookup, empty replacement for a missing key, rendered
  bytes and DJB2 checksum. Its callback receives the borrowed variable map
  explicitly and returns an owned `String` (`unique value`) as required by
  Hirsel's ownership rules; Rust returns a borrowed `&str`.
- No regex syntax, flags, captures or replacement behavior required by these
  inputs is missing. There is one allocation/interface deviation: Rust
  `regex.find_iter(&self.log).count()` is lazy; std `regex.find_all(re, text,
  max_matches, work_limit)` returns `List[Span]`. The port counts that list.
  There is no public lazy match cursor/count API, so all spans are materialized.
  Minimal example: counting matches of `regex.compile("x", 100)` in `"xx"`
  requires `regex.find_all(re, "xx", 18446744073709551615,
  18446744073709551615)` and `list.len`, rather than consuming a lazy iterator.
  Matching itself is entirely std regex; no regex is hand-rolled.

## Measurement protocol

LLVM release and unchanged upstream Rust release use `compare.py`'s
`Session.sample`, capped at 120 seconds per process on CPU31. One seeded
interleaved session runs ten rounds, seed `20261021`, under the shared CPU31
lock. The lock was available immediately. Counters are `perf stat`
`cycles,instructions,cycles:u,instructions:u,task-clock`; counters and SMT
sibling activity are retained for every sample. Instructions are the verdict.
Times are ratios within this session only.

For the requested short run, input sizes remain exactly upstream's 52,000 log
lines and 50,000 template rows; only the repetition counts become one warmup
plus one timed iteration. The unchanged Rust anchor establishes the short-run
checksums: **406572** and **3310942785**. These are distinct from the original
`run.js` checksums. Both participants consume the identical short configuration.
Instruction counts, cycles and RSS include process startup, preparation and
warmup; seconds cover only the harness's timed iteration. Thus instruction and
timing ratios have different scopes, and short-run ratios are not projections
for the original repetition counts or published LangArena rankings. In
particular, template preparation has substantial map and string construction.
The Rust runner prints milliseconds, which bounds precision of its short times.

`compare.py --quick --debug --rounds 1` is a separate setup/correctness artifact;
none of its measurements enter the ten-round medians. The original `run.js`
control attempt and perf profiles are separate diagnostics on CPU7, with no
accepted timing ratios. An initial Cranelift validation launched under the
short-job cap was stopped early and restarted in the separate long-job slot.
The 1,800-second LogParser attempt timed out at 123 MiB, exit 124, without a
checksum result. That is one incomplete original-control attempt, with zero
accepted full-configuration Cranelift cells. A longer retry was canceled before any cell completed; it contributes no
accepted evidence. No further full control or timing campaign was run.

The wrapper checks Hirsel/std source hashes, the fixture, compiler and runtime
identities, shared manifest/grants, upstream Rust sources and Cargo.lock, and
both binary hashes before execution, after every round, and after diagnostics.
The reused executables come from the normal compare.py builds; they are not
rebuilt or modified between correctness, counted samples and profiles.

## Correctness coverage and original-control limitation

| Task | Upstream test.js checksum | LLVM quick | Cranelift quick | Rust quick | Upstream run.js checksum | This session's run.js control |
| --- | ---: | --- | --- | --- | ---: | --- |
| Etc::LogParser | 117 | pass | pass | pass | 2439432 | Cranelift timed out; no accepted full cell |
| Template::Regex | 1554169905 | pass | pass | pass | 3554506535 | not reached; no accepted full cell |

The earlier port's LLVM full-configuration passes are recorded in
[the historical validation](RESULTS-pre-full-20261022.md#regex-captureflags-port-validation-2026-10-22).
They are historical evidence, not newly run full-control cells. This session
proves unchanged full-size inputs and short-run checksums on LLVM/Rust, and
quick upstream checksums on LLVM/Cranelift/Rust. It does **not** prove the
original repetition-count checksums on Cranelift. The original full-configuration
Cranelift checks remain open; the bounded measurement campaign is complete.

## Ten-round short-run medians

| Task | Short checksum | LLVM instructions | Rust instructions | H/R instructions | LLVM s | Rust s | H/R time |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Etc::LogParser | 406572 | 485,103,296,413 | 893,896,646.5 | 542.684 | 17.523790 | 0.0305 | 574.550 |
| Template::Regex | 3310942785 | 37,615,978,950 | 2,269,408,625.5 | 16.575 | 1.361085 | 0.0845 | 16.108 |

All 40 samples pass. Counter running percentages are 100% throughout.
CPU31's SMT sibling is CPU15; its median busy share by task/participant ranges
from 4.0% to 6.6%, with a maximum sample of 27.3%. The timing lock spans
`2026-10-22T03:56:25.155258Z` through `2026-10-22T04:03:12.396765Z`.
CPU7 runs the separate full-configuration validation concurrently; it is not
CPU31's SMT sibling. Unrelated host activity remains possible.

| Task | LLVM cycles | Rust cycles | H/R cycles | LLVM peak MiB | Rust peak MiB | H/R RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Etc::LogParser | 141,434,038,869.5 | 342,438,925.5 | 413.020 | 115.182 | 12.000 | 9.598 |
| Template::Regex | 11,319,890,994.5 | 1,061,374,858.5 | 10.665 | 133.549 | 39.516 | 3.380 |

## Profiles and hypothesis

Profiles use the identical measured release executables and short configuration,
`perf record -e cycles:u -F 199 -g --call-graph dwarf,8192`, on CPU7. Every
profile passes its checksum. Percentages are whole-process flat self samples,
including preparation and warmup. Hirsel has 6,910 LogParser samples and 537
Template samples, with zero lost samples. Raw reports retain unresolved addresses
and kernel/skid buckets; malformed/unresolved caller frames are not reassigned.
These profiles provide attribution, not additional timing observations.

| Task | Leading Hirsel symbols (self %) | Finding |
| --- | --- | --- |
| Etc::LogParser | `std.regex.add` 27.38%; `std.regex.step` 11.35%; `std.regex.accepts` 9.10%; `hirsel_rt_v2_seq_grow` 8.82%; `hirsel_rt.copy.copy_graph` 7.58%; `hirsel_rt.alloc.grow` 4.59%; `hirsel_rt_v2_alloc` 4.07% | Regex symbols alone exceed 53% of self samples; VM storage growth and copying add substantial cost. |
| Template::Regex | `std.regex.add` 17.81%; `hirsel_rt_v2_seq_grow` 12.48%; `std.regex.step` 12.29%; `hirsel_rt.copy.copy_graph` 7.95%; `hirsel_rt.alloc.grow` 4.76%; `hirsel_rt_v2_unique_copy_value` 4.74%; `std.regex.empty_slots` 4.59% | Regex symbols occupy about 44% directly; `add` reaches 42.40% inclusive through capture/thread copying. Including VM storage makes matching the dominant cost; map lookup is only 1.41% self. Inclusive percentages overlap and must not be summed. |

Hypothesis: the ordered Pike VM repeatedly allocates and copies its epsilon
closure stack, ready/next thread lists and capture-slot lists at scalar boundaries
(`stdlib/std@1/regex.hir:915-1088`). `add` starts with `unique seed`; perf's
callers connect it through `unique_copy_value`, `holder_views`, `copy_graph`,
`clone_block` and allocation. `find_from` initializes a new seen list per match
(`regex.hir:1128-1167`). LogParser also decodes the entire input into scalar and
offset lists for each of thirteen scans (`regex.hir:1172-1220`), and materializes
all match spans. Template uses the same VM with capture state for every match
(`regex.hir:1478-1527`). Prefix skipping, cached transitions and scratch reuse
are absent from these paths. This identifies plausible costs; it does not
isolate their individual contributions or prove a particular optimization.

Go comparison, at `ebe2feefc400f813aefdc8a036e66d2619da6649`:

- `src/regexp/exec.go:531-541` dispatches to onepass, bounded backtracking or
  the ordinary NFA. `onepass.go:455-483` requires anchoring and unambiguous
  alternatives. These unanchored benchmark patterns do not qualify for Go's
  onepass compiler; merely adding that path would not speed these exact inputs.
- `backtrack.go:5-13,30-32,62-74` bounds the visited bitmap to 256 Ki bits and
  programs to 500 instructions, avoiding repeat visits to a `(pc, position)`
  pair. Its unanchored search uses literal-prefix skipping
  (`backtrack.go:304-349`). At these multi-megabyte input lengths, Go's dispatcher
  would reject that bounded-backtrack path; its ordinary NFA remains relevant.
  Hirsel currently implements the NFA family without these alternative engines.

Rust comparison uses the anchor's locked `regex-automata 0.4.18`:

- `src/hybrid/mod.rs:1-9` describes a bounded transition cache populated lazily.
  `src/meta/strategy.rs:552-566` selects full/lazy DFA before NFA fallback.
  LogParser's diagnostic Rust profile actually shows
  `hybrid::search::find_fwd` (44.90%) and AVX2 Aho-Corasick/Teddy prefilters
  (19.01% + 6.51%), rather than repeated Hirsel-style closure construction.
  This short Rust profile has only 20 samples; the percentages are qualitative.
- Captures first find match bounds with the fast engine, then resolve slots
  inside the matched span (`strategy.rs:810-864`). Template's Rust profile shows
  `BoundedBacktracker::search_imp` 19.27%, `hybrid::search::find_rev` 14.93%,
  and the template run/callback path 27.80%. This supports a fast scan plus
  bounded capture resolution, whereas Hirsel carries capture state through its
  whole scalar search. These whole-process profiles include preparation.

Both tasks exceed 3x Rust. Investigating reusable VM scratch/thread storage,
cheaper capture propagation, a lazy match/count interface and prefix/DFA paths
is follow-up work. No performance fix is included here.

## Reproduction and evidence

The native/checksum harness remains at 11 tests before and after this work;
`.buck2/bin/buck2 test root//:gate-langarena` executed all 11 successfully,
including all 47 registered quick checksums. No test was removed or weakened.
The one-round setup ran:

```sh
python3 bench/langarena/compare.py --debug --quick --filter LogParser \
  --filter Template::Regex --rounds 1 --session la-regex-ports-8-quick \
  --timeout 900 --output /tmp/la-regex-ports-8-quick.md
.buck2/bin/buck2 test root//:gate-langarena
```

The short counted fixture is exactly:

```json
[
  {"name":"Etc::LogParser","checksum":406572,"iterations":1,"lines_count":52000,"warmup_iterations":1},
  {"name":"Template::Regex","checksum":3310942785,"iterations":1,"count":50000,"warmup_iterations":1}
]
```

Raw evidence is retained outside the disposable worktree at
`/workspace/tmp/langarena-tools/evidence/la-regex-ports-8/`:
`quick/measurements.json` records normal compare.py builds, source and toolchain
identities; `short/measurements.json` records every accepted sample, exact
commands, counters, profiles and successful frozen-input checks. The directory
also preserves all consumed Hirsel/std sources, unchanged Rust sources/Cargo.lock,
compiler, release/debug executables, grants, manifests, the fixture and its Rust
checksum derivation. Measurement wrappers are archived there as
`la-regex-ports-8-measure.py` and `la-regex-ports-8-validate.py`; their path constants
name this session's worktree. The final counted invocation was:

```sh
python3 /tmp/la-regex-ports-8-measure.py --session la-regex-ports-8-short \
  --config bench/langarena/build/regex-short.json --rounds 10 \
  --timeout 120 --release-only
```

The wrapper calls the unmodified `compare.Session.sample` for each participant,
and replaces the unbounded timing-lock wait with a 600-second deadline. Its
instruction-only fallback was not used. No full-size timing campaign was run.

| Participant | Compiler SHA-256 | Executable SHA-256 |
| --- | --- | --- |
| hirsel | `f7ec897cad1d2c251d1964b3cc28b553ce655164de5bce048745fe900a47b4c2` | `f98ce26a9dd3e92f5d85e07aedf5deb03fdffd6b0cc50e3b44ce9d320c4a4cfb` |
| rust | `f3834d26669b03f6855fa54bd2381443123e860167bc0c7a8dd137e5cf6e5e4f` | `34f46c760fcc739816add285f1328cd3fb8eef465143303f6c96ef7349dc8cb3` |
| cranelift | `f7ec897cad1d2c251d1964b3cc28b553ce655164de5bce048745fe900a47b4c2` | `8cef4e14fe3af3e6eb1eb5a7031da0a8c8cefcb5e2e65184eb9677bc41b55d73` |

Consumed Hirsel/std source-set SHA-256: `dc95ead2b657d884e580b8c52fbaf6ac8f25d5d97271fa0cd69f9dfee55127da`.

The 1,800-second diagnostic timeout also exposed a runner robustness issue:
`compare.py:135` indexes `timing.read_text().splitlines()[-1]` when the killed
`/usr/bin/time` leaves an empty `.elapsed` file. A minimal trigger is an empty
timing file after `capped` terminates a timed subprocess. This timeout and the
subsequent reporting error belong to the same incomplete original-control
attempt. This report does not change the shared runner. The accepted ten-round
session had no timeouts.

Accepted raw measurements SHA-256: `ea47ca2fe56247f88e885e7151af54e137582c7585be4c3ba62a670bdd2e91ca`.
Setup/quick measurements SHA-256: `2439465def3e742a437e0690317732d75124410d9938e80b651b06d76677ff4f`.
A final post-diagnostic frozen-input check passed at `2026-10-22T04:31:02.040573+00:00`.
