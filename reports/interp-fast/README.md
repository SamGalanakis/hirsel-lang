# Interpreter operation dispatch (2026-10-21)

The starting point is post-cutover main `69412fd9`. Native execution, TIR,
stdlib, and persistent formats are unchanged.

## Implementation

A direct call's Target caches its provider's operation id or declined decision.
The built-in Library resolves interned definition identities to a handler table;
the host adapter resolves its routing category once. Ordinary TIR definitions
are no longer offered to those providers on each invocation. Custom call-only
providers retain an explicit legacy dispatch token. Path-intercepting adapters
forward or override the new resolver and resolved-call hooks.

Anchors carry session DefIds. Only diagnostic/observation projection materializes
a definition path. The existing interpreter body cache now includes host-sized
place field offsets and index slots. Each frame reuses its projection workspace;
there is no new vector allocation for each projected access. Whole-local operands
use their cached multiplicity; unchecked reads avoid the Value/Shadow return pair.

After profiling, evaluator arena-write categories were also resolved once, and
unchecked calls stopped allocating checker-argument vectors. Primitive providers
can opt into immediate completion. This path validates transferred owners,
restores mut inputs and publishes a Complete result without allocating and moving
a suspended operation through two scheduler passes. Updates, released values,
untransferred non-inert owners, callbacks, Free execution, checked execution and
memory-metered runs retain scheduled completion. Existing operation kernels and
scheduler ownership rules supply the implementation; no external algorithm or
new data structure was introduced.

## Workloads and measurement

The actual accepted fixtures, including checksum/length checks, are:

```hirsel
pub fn main() -> Result[(), ()] {
  let mut s: u64 = 0
  let mut i: u64 = 0
  while i < 1000000u64 { s += i; i += 1 }
  if s == 499999500000u64 { Ok(()) } else { Error(()) }
}
```

```hirsel
pub fn main() -> Result[(), ()] {
  let mut out: List[u64] = []
  let mut i: u64 = 0
  while i < 8000u64 { list.push(mut out, i); i += 1 }
  if list.len(out) == 8000u64 { Ok(()) } else { Error(()) }
}
```

Both binaries were built with the profiling platform (release optimization,
line tables and frame pointers). Each matching std image and source tree was
copied beside its executable. Every measured process sets `HIRSEL_DAEMON=off`;
otherwise the resident compiler performs work outside the measured process.
Build the interpreter measurement artifacts without native seed objects:

```sh
.buck2/bin/buck2 build --target-platforms //tools/buck2:profiling \
  //:hirsel-cli-bin-hirsel //:hirsel-std-image --show-full-output
```

`bench/micro/run` measured ten seeded (42), interleaved rounds on CPU 31, retaining
cycles, instructions, user cycles/instructions, task-clock, executable hashes,
output hashes and sibling load. Each loop has a zero-iteration control, also
interleaved. The reported step cost is the median of same-round full-minus-zero
instruction counts divided by the additional interpreter steps. This subtracts
fixed compilation, image loading and process startup, rather than treating them
as loop instructions. Bounds/checksum constants differ in the zero controls.
Exact step budgets were found by minimum-successful-budget searches: arithmetic
58,000,071 versus 71 steps; list push 416,075 versus 75 steps.

| Workload | Before user instructions/step | After | Before total instructions/step | After |
| --- | ---: | ---: | ---: | ---: |
| arithmetic | 86.02 | 91.17 | 86.09 | 91.25 |
| list push | 418.79 | 173.54 | 431.84 | 185.39 |

The operation-heavy/plain ratio is **1.9034x user instructions** (previously
4.8686x), satisfying the requested `instructions:u` comparison. Including kernel
instructions it is **2.0317x** (previously 5.0162x): the remaining 11.85 versus
0.077 kernel instructions per step account for that narrow excess. Arithmetic
steps remain far below 400, but regress by 6.0% in this build; this is an open
performance follow-up, not a claimed arithmetic speedup. Operation-heavy user
instructions per step fall by 58.6%.

Full-process median user counts, before subtracting startup, are arithmetic
6,448,709,094 -> 6,755,011,554 and list push 1,649,717,155 -> 1,555,632,224.
The image/compiler fixed cost therefore obscures much of the list-loop saving.
The user-only attribution is the requested metric; total counters are retained
and reported independently.

Initial malformed/private entry fixtures, resident-forwarded runs, compressed
upload races during source edits, an obsolete unsharded target and a test-runner
argument error were setup failures and contribute no accepted evidence. Final
compiler source hashes were frozen before the accepted candidate build. Later
changes add only test controls and this report.

## Profile-guided iterations

Separate `perf record -e instructions:u -g` recordings were made before/after for
the two minimal programs. An 800,000-push version increases the interpreter's
share for hotspot attribution; it is not substituted for the measured 8,000-push
workload. After id/anchor caching alone, its leaf samples still attributed about
13% to definition-path equality walks, 7.7% to execute_frames, 6.1% to memmove,
and 2.8% to execute_terminator. Caching arena categories and omitting unchecked
checker payloads reduced a preliminary list-loop estimate from 304 to 230 user
instructions per step. Immediate completion reduced it to the final 174.

The final longer-loop profile attributes 26.7% to scalar dispatch, 8.2% to
execute_frames, 7.9% to memmove, 3.4% to execute_terminator, 2.5% to allocation,
and 1.7% to immediate completion. The arithmetic profile was dominated by scalar
dispatch (~53% before). The remaining work is scalar value movement/dispatch and
call argument bookkeeping, not the removed path-string construction. These are
leaf sample shares, not independent causal speedup estimates.

## Example test loop

`hirsel test examples/tasks/hirsel.json` ran all 22 tests successfully in both
executables in each of ten seeded, interleaved rounds. A second ten-round
session wrapped each process in `/usr/bin/time` while retaining the perf
counters, to measure elapsed wall time separately from task-clock. Median wall
time was **6.775 s before / 6.890 s after (1.017x)**. This provides no wall-time
speedup claim. In the first session, user instructions were 46.464 billion
before / 44.786 billion after (0.964x), and the task-clock ratio was 0.981 with
95% bootstrap interval [0.955, 1.031]. The repeat's task-clock interval also
crossed one. Timings are compared only within their own interleaved session.

## Validation

Baseline interpreter tests: 26 unit and 95 integration tests. Candidate: 26
unit and 97 integration tests; a later added diverging-operation control and
two variants of the taken-owner fallback test passed in the focused four-test
operation-mutation group. This yields 26 unit / 98 integration tests on the
final tree, with unchanged groups' earlier results retained.

Driver adapter and completion controls passed (38 focused tests, including
48 trace/step/checker/budget combinations in the direct-versus-scheduled
comparison), along with three pipeline operation-adapter tests and the HTTP
clock-progress replay test. The resolver tests verify one static decision
across repeated calls, including declines, and call-only legacy interception.
Callbacks, unreleased taken handles, released values, mut updates and a return
from a diverging operation exercise scheduled fallback or API validation.

The selected agreement campaign checked 1,735 variants over the 2,000-file
slice, executed 583, and reported no discrepancy clusters. Conformance validated
1,986 files: 1,964 passed and 22 failed before interpretation (checker/TIR
validation), with 14 additional missing-twin-header errors. The full red result
is retained; it is not reported as a green campaign. All 22 failure diagnostics exactly match a focused run of the pristine
starting commit `69412fd9`. All 14 metadata files are byte-identical to that
commit. Exact inherited failures are tracked in `tools/known-failures.tsv`;
no fixture expectation or assertion was weakened. The 1,964 passing cases and
the green agreement campaign were not rerun for this attribution.
