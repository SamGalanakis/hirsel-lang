# First-contact friction

Approximate minutes include investigation, not benchmark timing. Failed drafts
and original logs were retained outside the repository. No runtime, std or
language rule was changed for this application.

| Attempt | Exact diagnostic / behavior | Resolution, cost, disposition and owner | Minutes |
| --- | --- | --- | ---: |
| Used `Pull(_, _) => state.pulls += 1` as a match arm, `Error(bufio.Limit())`, and a local named `result`. | LANG-1 `expected comma or newline between entries`; LANG-13 `expected ','`; LANG-14 `` `result` is reserved for the returned value in a postcondition; choose another name ``. | Statement arms use braces; constructors use expected-type inference (`Error(Limit())`); rename the local. No extra copies. Intended syntax, application owner. | 1 |
| Wrapped fallible stage work in a parameterless immediately called closure that mutated enclosing endpoints. | CLO-1 `this binding grants read-only access: 'admission' is captured by borrowing, but this writes 'admission'`; OWN-1 on taking a borrowed path; CLO-3 on the borrowed query. | Explicit mutable endpoint parameters at the helper boundary; take the path and unique query into their child. No receiver in a payload or erased Flow. One unique query/path snapshot, no row copies at channel receive. Intended ownership, application owner. | 3 |
| Reused enclosing names for helper parameters. | LANG-4 `a local cannot shadow another local`. | Use distinct source/filter/sink parameter names. No runtime cost; intended naming policy, application owner. | 1 |
| Fixture used `json.Text(text)` and String `+`. | LANG-4 `unresolved module member 'json.Text'; looked for std@1/json/Text`; LANG-7 `operator '+' is not defined for operands 'String' and 'String'`. | Use `json.stringify(Text(text))` with its declared error and `f"{rendered}\n"`. Fixture formatting only, application owner. | 1 |
| Fixture generated `b"αhit🐑\n"`. | SEM-3 `non-ASCII character in bytes literal`, with explicit UTF-8 byte-escape guidance. | Generate byte escapes. No language defect or runtime copy; intended bytes-literal rule. | 1 |
| Simulation supplied a clock projection without granting `MonotonicClock`. | `error: Grant(NotGranted)`. | Configure both the actual clock grant and fixture projection. Fixture setup, no API change. | 1 |
| Native differential output directory was absent. | `Unavailable(NativeError { stage: Link, definition: None, validator: false, message: "No such file or directory (os error 2)" })`. | Create the private output directory before invoking the existing public adapter. Harness plumbing, application owner. | 1 |
| Output-cap fixture JSON-encoded each large rendered line. | `ToolingLimit { kind: "Steps", limit: 40000000, site: ... std@1/json.hir ... }`. | Preserve the same input/output cap and step budget; print raw output for this partition, removing test-only JSON escaping. Actual cap and partial output then pass all four engines. No production change. | 2 |
| Standalone production probe relied on template `args`. | `error: Invalid` (template admitted with `args: []` after argv replacement). | Supply actual trailing standalone arguments. CLI `run --grants` and standalone argument rules differ intentionally; documented in README. | 1 |
| Recorded production launcher with clock/deadline and file/channel stages. | Native LLVM and Cranelift aborted at `crates/hirsel-rt/src/io_home.rs:48`: `.insert(task, snapshot).is_none()`. Earlier invalid-argv draft also retained `hirsel-rt: launch refused: incomplete canonical event evidence`. | Open runtime recording seam, routed to its existing owner `e1-trace-record-27`; no second repair here. Unrecorded native launches work. Recording-only production and cross-engine replay pass. Traced production and deterministic traced tape comparisons remain INCOMPLETE pending that owner. | 4 |
| Wrapper quota inherited `HIRSEL_JOBS=1` while placements requested 2/4. | Control reported effective1; forced replay2/4 hit the unchanged60s cap. | `tools/capped` also derives its CPU quota from that setting. A fixed four-CPU wrapper quota across every placement, with sufficiently broad affinity, permits actual1/2/4 and resolves those replay limits. All original findings retained. Compiler jobs alone are not runtime worker admission. Harness setup, application owner. | 3 |

Recording-only production readiness/control subsequently observed effective
widths1/2/4 for success, parser failure and useful I/O on both backends. All
recordings were replayed with original files unavailable; replay topology
hooks are explicitly separate from production proof. The bounded ten-round
LLVM timing campaign used the orchestrator's reserved window and retained
recording, metrics, counters and tapes externally. Wider placements were
slower for this small log workload; no optimization or speedup claim is made.
Checked-H live-tape replay remains unprovided by the existing public checked
simulation adapter; checked-H execution of every full workflow is covered.

## Ownership recipes

A received `Result[Option[unique Row], AppError]` is bound before extraction.
Move its payload with `take`; never contextually ask `receive` for a weaker
receiver element type. The direct-expression MODE-2 and counted-row MODE-4
planner witnesses still reject intentionally on current main; their repaired
controls accept. They are prior fixed diagnostics, not new defects.

Scope authority stays conservative. This application does not try to export a
narrow stored-callable authority promise or public prefetch combinator. The
producer and transform capture Receivers directly from immutable outer
bindings, then rebind them mutable inside the child. The source and sink
helpers accept explicit mutable loans. No mutex callback performs I/O, waits,
or receives; ledger callbacks only update bounded scalar state and tiny audit
fixtures. The production audit history is disabled.

Channel closure and explicit cancel/await are ordinary application plumbing.
A `Closed` send owns its returned payload; discarding that result safely drops
the row. Cancel is not treated as a send outcome. All terminal paths join both
children; a later fixture timer confirms the source-pull counter stays fixed.

The source's bounded bytes-to-UTF-8 conversion makes an independent line copy;
rendering allocates one numbered line. The query and path are copied once for
the task boundary. These costs are explicit and bounded, not ownership-rule
workarounds. Recording retention is separate from retained application rows.
