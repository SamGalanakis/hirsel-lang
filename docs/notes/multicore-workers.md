# Worker substrate

`pool.rs` runs compute workers beside the admission thread. At width one the
admission thread still executes the root. At greater widths it services home
calls and foreign intervals; ordinary roots and children execute on compute
workers. `TaskCtx` always names the task, including across a suspended finalizer.

The cooperative scheduler transfers a single run token. Posting happens from
the outgoing worker's idle stack, after the task stack pointer has been saved.
An activity lock covers clearing the sender and publishing the receiver, and
covers stop-time snapshots. Clearing the sender after publication is incorrect:
a fast receiver can bounce the task back before that clear and lose its busy
flag. `workers::a_stop_during_a_pure_loop_returns_at_once_and_the_root_later_finishes_into_its_frame`
repeats this stop/return race. The ordered and free executors own concurrent
turn/object execution; the token preserves current cooperative observations while
exercising their shared migration and lifetime substrate.

`LaunchFrame` owns the entry, its arguments, root handles, result buffer and root
stack. A late root never writes to an expired launcher stack. After a stop, home
closes its inbox and withdraws queued calls using `hirsel_free::claim::Requests`.
Its non-`Send` adapter and sinks are destroyed on admission. A run-owned binding
facade forwards every adapter method to admission, including from a foreign
callback that cannot move its C frames. Call buffers remain with the posting
fiber until home has completed or withdrawn the call. No detached worker owns
the actual adapter.
After a blocked test run quiesces, admission reports parked wait kinds directly
from the run-owned table. Reporting must not enter a stop-aware binding accessor:
that would abandon admission after its home service loop has already returned.
The compiled `CONC-8-watch-no-event-blocked` fixture covers this path.

`RunControl` can request a stop or read status from any thread. A stopped pure
loop can retain its worker, heap, stack and compute permit until it reaches an
operation or fuel refill. A home foreign interval delays return until a supported
boundary; status exposes `home_busy`. An external stop at a callback operation
abandons the callback. A callback failure or a language stop inside C remains
process-fatal. The last holder reclaims the detached run.

There is one compute permit pool per process. The embedding manager can call
`pool::set_compute_grant`; `pool::compute_permits` returns (grant, held, waiting).
The initial grant is admission's available parallelism. Lowering a grant never
replaces outstanding holders. Width-one roots, wider workers, and callbacks use
the same pool; nested local runners share the executing thread's permit. C and
host waits release it. `RunStatus::lost_permits` counts permits retained after a
stopped run returned. Width and forced migration remain test controls until the
mode-specific differential gates enable managed execution.

At greater widths a task returns unspent fuel on switch-out and takes a chunk on
switch-in. Only its executing worker writes its fuel word. The width-one exact
budget path remains unchanged. Fuel builds observe a stop at least once every
64 Ki points. Uninstrumented pure loops retain their workers by design.

Worker metrics are collected only when requested and use the metric names in
`bench/multicore/telemetry.json`. The ordinary path reads no metric clocks.

## Validation

- Runtime integration module `workers` covers migration, divergence, callback
  failure, cleanup exhaustion, allocation versus stop, late roots, home C and
  callback boundaries, affinity, fuel and retained process permits.
- `native --migrate` / `gate-native-migrate` runs the executable corpus through
  LLVM Speed and Cranelift using optimized runtime archives, and compares its
  observations with the interpreter. Build with `-c hirsel.full=true`; explicit
  `--test-arg=--filter` arguments select a focused slice. Failures before launch
  remain failures; they do not prove migration.
- `tools/workers-tsan` compiles these same worker tests with instrumented std and
  fiber annotations, without the regular IT binary's custom test allocator.
- `budgets::a_stop_ends_a_fuel_builds_loop_at_its_next_refill_on_both_backends`
  checks optimized allocation-free generated loops on both native backends.

## Prior art

The fiber save/restore convention follows corosensei `4b5db484`,
`src/arch/x86_64.rs:374-402,407-510` (layout comparison, no assembly copied).
The permit wait predicate uses Rust std `ea137335b788`,
`library/std/src/sync/poison/condvar.rs:182-194`; return-before-wake follows Tokio
`ce685ee69165`, `tokio/src/sync/batch_semaphore.rs:334-378`. OS threads use a
condition variable rather than future wakers. Unlike Tokio's ready-task
scheduler, the cooperative run token preserves ordered observations; the free
executor can replace the scheduling policy without changing task contexts,
foreign ownership, home affinity or worker lifetime. TSan annotations use
compiler-rt's `include/sanitizer/tsan_interface.h:150-172` fiber interface.
