# Calculator interpreter: a measured no-win diagnosis

The evaluator is unchanged. Current-main Calculator::Interpreter executes
**1.200× Rust's instructions**, reproducing the historical instruction ratio,
not a 2.2× instruction problem. A local scalar-read trial produces the same
normalized evaluator assembly and **identical allocation/copy counters**.
Its instruction ratio to baseline is **0.999898** (−0.010%); this is measurement
variation, not an optimization. No runtime reduction or implementation credit
is claimed. The landed work supplies semantic controls, a reproducible counting
runner, and a concrete explanation of the remaining costs.

## Frozen experiment

Baseline `d5b594d8250ad3c306e3caca6b68aa7cb5b754ba`, prepared in an owned
checkout from origin/main. Candidate copies exactly the same scoped package,
adding only `let operation = op` before the recursive calls and matching that
scalar instead of `op`. This rejected no-op is retained externally as
`scalar-source/bench/langarena/calculator.hir`; it is not the landed evaluator.
Rust is an unchanged owned clone pinned to
`3c9f84ddaeec834c0631747cb789e8cfd602f993`, release Rust 1.99.0.
Both Hirsel participants are LLVM 21.1.8 release/O3, same compiler and runtime.
All use the original run.js Interpreter configuration: **35,000 operations,
100 timed iterations, 20 warmup iterations, checksum 38,749,936**.

The package contains only calculator, harness, original main and a scoped
registry. AST construction remains before harness.measure; a new String-keyed
map is allocated for every step, including warmup. Recursive left-before-right
execution, insert/overwrite behavior and checksum accumulation are unchanged.
No bytecode, variable numbering, map API, parser, AST, IR or runtime ABI change
is included. The original README and other owners' implementations are untouched.

Ten seeded interleaved rounds, seed **20261022**, CPU **18** affinity, nice 10,
ordinary capped host execution. All **30/30** full-output checksum oracles pass;
each process also reports exactly one selected and passing benchmark.
Counter scope includes the isolated process tree: startup, preparation, warmup,
timed iterations and destruction. This is broader than the harness's printed
timed interval. Five perf counters (task-clock, cycles, instructions, cycles:u,
instructions:u) each have **100% running coverage in all 30 samples**.

| Participant | Median instructions | Minimum | Maximum | Median cycles | Median user instructions |
| --- | ---: | ---: | ---: | ---: | ---: |
| before | 12,033,108,129.0 | 12,019,920,406.0 | 12,118,222,392.0 | 9,984,692,079.5 | 11,221,426,678.0 |
| candidate | 12,031,877,048.5 | 12,018,583,892.0 | 12,082,881,061.0 | 10,009,696,964.5 | 11,221,433,954.0 |
| rust | 10,027,867,042.0 | 10,026,141,207.0 | 10,030,407,060.0 | 5,234,384,833.0 | 9,952,741,893.5 |

User instructions are also unchanged to within 0.000065% between the Hirsel
participants. Cycles are retained above as diagnostic counts; no cycle or wall
speedup ratio is asserted. The shared CPU31 timing lock was available at a
nonblocking check and immediately released. CPU31 was never selected, and no
CPU lock was held during this three-participant session. SMT sibling activity
was recorded (10.3–53.0% across participants), so no wall/cycle speedup is
inferred from its unlocked observations.

The brief's stock compare.py control also ran on **CPU18**, with its own
`/workspace/tmp/hirsel-micro-cpu18.lock` (this invocation's TMPDIR), ten interleaved rounds, seed **20261021**,
using the restored unchanged evaluator. All **20/20 full checksums passed**;
all five counters had **100% running coverage**. Source/runtime/binary identities
were verified by the runner, whose final status is complete.

| Stock current control | Hirsel | Unchanged Rust | H/R |
| --- | ---: | ---: | ---: |
| Median instructions | 12,031,039,649 | 10,027,513,863.5 | 1.1998 |
| Median cycles | 9,198,517,315 | 4,718,365,972.5 | 1.9495 |
| Median harness timed seconds | 1.7574879025 | 0.8915 | 1.9714 |

These ratios apply only inside that locked session. They are a current
Hirsel/Rust anchor, **not a before/after optimization or a change relative to
the historical wall2.223**. The current wall gap remains larger than the
instruction gap; instruction count alone does not explain that throughput gap.
No CPU-wide cache-stall attribution or causal cycle improvement is claimed.
The stock report/raw JSON label timing_lock with the CPU31 path unconditionally;
`measure.timing_lock(18)` actually uses the CPU18 path above. The raw report is
retained unchanged as stock-report.md with this explicit correction here.
The stock session's whole-suite executable build is setup only; the filter
selected exactly Calculator::Interpreter for all measured processes.
Its generated registries/manifests are retained but are not committed.

All source snapshots (including std and untracked manifests/registries),
configuration, grants, reference source/lockfile/license, Rust compiler/Cargo,
Hirsel compiler/helper, runtime/link kit/LLVM/glibc inputs, runner scripts and
executables were hashed before their consumers and checked afterward. Baseline
and scalar builds, baseline/final correctness tests and the 30-sample session
retain separate freeze manifests. The accepted session reports
`frozen_identity_verified=true`. Incoming main changes did not trigger reruns
of unchanged accepted work.

## Costs and rejected trials

A separate full-workload baseline perf profile has **555 samples, zero lost**.
Self samples: SipHash **26.68%**, evaluator **17.32%**, map.get **12.23%**,
map.insert **5.82%**, map index **2.70%**, memmove **3.35%**,
copy::view_key **1.85%**, copy::clone_block **1.76%**. These small singleton
profiles attribute work; the ten rounds above determine the verdict.
Rust's separate 279-sample profile also spends substantial time in recursive
evaluation, hashing, allocation and map growth.

The separate branch probe records **2,197,331,657 branches**, **55,096,893
misses (2.51%)**, with 100% counter running coverage. It includes setup and is
not an interleaved branch improvement claim. Baseline disassembly loads the
binary operator before recursion already; explicitly naming that scalar changes
no normalized evaluator instructions. Recursive calls borrow boxed children;
there is no per-evaluation deep clone of the whole AST. Box/tree layout and map
internals remain out of scope.

| Hirsel participant | Runtime allocation requests | Frees | Deep copies | Copied bytes | Count increments | Count decrements |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| before | 4,878,753 | 4,878,753 | 4,344,308 | 137,542,977 | 146,424 | 5,362,016 |
| scalar trial | 4,878,753 | 4,878,753 | 4,344,308 | 137,542,977 | 146,424 | 5,362,016 |

These are separately linked **standalone measurement-runtime** counts,
including setup and warmup. The ordinary launcher is not the allocation oracle.
They count physical Hirsel runtime requests, not all host malloc calls; both
report live_bytes=0 at exit. Rust heaptrack separately observes **5,357,748
process-wide allocation calls**; its coverage differs, so no cross-engine
allocation ratio is inferred. Rust string-clone attribution includes
**3,501,100 timed + 700,220 warmup = 4,201,320 assignment-key clones**, exactly
35,011 assignment nodes × 120 steps. Hirsel's deep-copy total includes setup,
but the same per-assignment unique-key copy dominates its interpreter work.
These results do not support blaming a full AST ownership clone per step.

The first trial replaced only `unique name` with `copy name` in the existing
map.insert call. It was rejected at source checking on all three engines:
**MODE-1**, the name borrowed from `node` cannot escape into a map requiring a
global key. No binary or performance result is credited to that invalid trial.
No ownership rule was relaxed. The la-words-9 rejected upsert path was not used.
The scalar trial compiled successfully but produced equivalent code and no win;
it was restored before landing. No third speculative rewrite was attempted.

## Correctness and reproducibility

The lane-private Rust harness is registered through its modules.rs and contains
four tests: one independent Rust evaluator with literal expected results and
final BTreeMap environments, plus H, LLVM and Cranelift fixture executions.
The Hirsel fixture appends assertions to the **actual calculator source**,
not a second Hirsel walker. Twenty-one hand-authored AST programs cover empty
programs, missing variables without insertion, precedence trees, overwrite,
nested assignment, left-before-right side effects (also for an unknown operator),
wrapping +/−/*, all quotient/remainder sign combinations, zero divisors and
MIN/−1. Four additional parser programs check precedence, parentheses and
assignment structure. Each runs twice against fresh environments, observing
both return value and every final map entry: **150 tiny-program executions**
across three engines. The final fixture also calls actual production step twice
on `counter = counter + 1`, requiring accumulated result 2, not 3: **6 extra
step executions** prove per-iteration environment freshness.

Exact upstream test.js controls pass on every engine:
**Interpreter operations10/iterations100/checksum3208680** and
**Ast operations3/iterations3/checksum774028720** (**6 checksum checks**).
Baseline: **4/4 Rust tests pass**, 150 tiny executions and 6 checksums.
Final restored evaluator: **4/4 pass**, the same controls plus 6 freshness steps.
Baseline scoped native before/Rust preflights also passed both test.js and
run.js Ast/Interpreter controls (8 independent checksum processes).

The target was discovered using `.buck2/bin/buck2 targets 'root//:'`:

```sh
.buck2/bin/buck2 test root//:hirsel-cli-test-la_calculator_interpreter_10 -- --test-arg=evaluator::
python3 bench/langarena/la_calculator_interpreter_10.py \
  --spec /workspace/code/hirsel-briefs/la-calculator-interpreter-10/session-spec.json \
  --output /tmp/calculator10-new-session.json --cpu 18 --rounds 10
```

The runner consumes sealed executables; it never rebuilds during measurements.
Its spec records exact argv, cwd and every input path. Use a new output for each
attempt. The retained build-phase.py snapshots a scoped package and std, then
runs compiler build --release, --debug, and the existing native build helper
with HIRSEL_BENCH_MEASUREMENT=1/HIRSEL_BENCH_STANDALONE=1 for allocation coverage.
Runtime/toolchain arguments are recorded in toolchain.json and build logs.
No broad corpus/workspace test campaign or manually raised cap was used.

Retained setup/failure attempts are distinct: initial reference Cargo build
selected stable via the ambient rustc and lacked its self-contained linker
(fixed by explicit RUSTUP_TOOLCHAIN/RUSTC=1.99.0); initial registration created
duplicate Cargo target names (moved the private registry deeper under support);
initial fixture had a relative std manifest (1 oracle passed, 3 setup failures),
then reserved Hirsel identifier `result` (1 passed, 3 source-check failures);
shared-name trial had 1 oracle pass and 3 expected MODE-1 source-check failures;
first branch probe had a wrong relative tools/capped path (zero executions);
first scalar measurement started before its allocation binary existed (zero
executions). No failure was erased or counted as a successful performance sample.
The corrected baseline and final suites and complete 30-sample session above
are the accepted evidence.

## References, adaptations and next route

Reference identity verified at launch and frozen throughout:

- `rust/src/benchmarks/calculator_interpreter.rs:23–40,48–109`, pinned commit
  `3c9f84ddaeec834c0631747cb789e8cfd602f993`, SHA-256
  `848aa45e5d9072897fc2e3b3c557ef32389fda0425fb8316ff5390b9134c431b`.
- `test.js:201–212`, same commit, SHA-256
  `424371dd25f3fa3f779789c823a05dbe90d2eddbc2a3ca3c5e4319252c572797`.
- LangArena MIT copyright/notice remains in `bench/langarena/LICENSE`;
  the owned reference clone retains its LICENSE.

Existing Hirsel adaptations are preserved: boxed recursive enum and byte
operators, owned String map insertion through unique copying, explicit wrapping
arithmetic, quotient truncation toward zero, zero divisor→0, explicit
**MIN/−1→MIN** and remainder **MIN%−1→0**. Rust's reference abs-based division
is not used as an oracle on unsupported overflow inputs; the independent Rust
test guards those two adaptation cases before / or %. The semantic oracle uses
BTreeMap solely for deterministic full-environment equality; production still
uses its original String-keyed Map. No new language behavior is introduced.

A concrete next route is a **separately scoped lifetime-safe borrowed-key insert
or region relation**, investigated with the compiler/map owner before changing
any shared API. Minimal rejected program shape:

```hirsel
fn insert_name(node: box Node, mut variables: Map[String, i64]) {
  if let Assignment(name: name, value: _) = node {
    map.insert(mut variables, copy name, 1i64) // MODE-1
  }
}
```

Current semantics require `unique name`; an implementation must prove the AST
key's lifetime through the environment without extending a local borrow or
removing checks. That route is out of this lane, as are map internals and
bytecode. A control should preserve nested assignment and final environments,
then check allocation counts before any instruction claim. Throughput/cache-stall attribution beyond these instruction counts remains a
possible follow-up. The no-win diagnosis fulfills this lane;
no mandatory interpreter functionality is left unfinished.

## Identities and retained evidence

Original/landed calculator source SHA-256: `e27831130631e153245e8872240e30ee96c1567903b45654ac1e025adb719093`.

Hirsel compiler SHA-256: `90dfbd42bf3380d27041c1634e769a128e4bf0f579a9b98adc937071f08715e9`.

- before executable SHA-256: `53b2d2c717d11a0e76e0a6be7fe36b9eb1fb9633acf58889250c8b617b196922`.
- candidate executable SHA-256: `08219e9d4bbee20bdd29f18e6c1250f8f967dc8782fb8fed8bfe0b7c91f2014c`.
- rust executable SHA-256: `468817173ea176c7c80ee1ddd3e4b0233eecf53a209419de392ec080e0f39f4f`.

Raw full output, perf counters, SMT snapshots, source/runtime/binary freeze
manifests, rejected sources, assembly, heaptrack and build logs are retained in
`/workspace/code/hirsel-briefs/la-calculator-interpreter-10/`; top-level command
logs are `/tmp/la-calculator-interpreter-10-*.log`. Principal artifacts:
`session.json`, `session-spec.json`, `summary.json`, `probe-measurements.json`,
`before-build-freeze.json`, `scalar-build-freeze.json`, `final-test-freeze.json`,
`before-profile-flat.txt`, `before-branches.perf`, `rust-heaptrack.txt`, and
`before-evaluator-normalized.txt`/`scalar-evaluator-normalized.txt`,
`stock-run/measurements.json` and `stock-report.md`.

Stock consumed-source identity: `82146182941917427b6f63a035ff31002f9f2d0885d7f936b3456ec399309758`.
Stock hirsel executable SHA-256: `86fadee39b7814d719139516a560ff6845cf633fa84982432c23fb21af5b3507`.
Stock rust executable SHA-256: `34f46c760fcc739816add285f1328cd3fb8eef465143303f6c96ef7349dc8cb3`.
