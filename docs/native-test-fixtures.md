# Scripted native app tests

`hirsel_driver::native::Fixture` is the public Rust test recipe for a full
`main(rt: Io, args: Args)` entry. Use it when regular-file reads must park or
cancel deterministically. The production CLI's `--simulation` and `--native`
options remain separate. Production host-file reads ignore readiness scripts.

The recipe uses the same field shapes and virtual paths as `SimulationConfig`:

```rust
use hirsel_driver::native::*;
use hirsel_run_schema::{Access, FileReadScript};
use std::collections::BTreeMap;

let fixture = Fixture {
    args: vec!["input".into(), "output".into(), "--interruptible".into()],
    files: BTreeMap::from([("input/config".into(), b"payload".to_vec())]),
    mounts: BTreeMap::from([
        ("input".into(), Access::Read),
        ("output".into(), Access::ReadWrite),
    ]),
    file_reads: BTreeMap::from([("input/config".into(), FileReadScript::Pending)]),
    ..Default::default()
};
let simulation: hirsel_engine::SimulationConfig = (&fixture).into();
```

Args contain application arguments only, including literal launch-option-looking
strings. `files` holds initial bytes by resolved virtual path. `mounts` names
virtual directory selectors and their operation rights; no host directory is
read. Mount directories and file parents are created in the snapshot. The
default recipe grants one empty read-write root. An empty `mounts` map grants
no directory. Files outside the mounts do not grant access.

`Fixture.stdin: Vec<u8>` supplies plain bytes followed by EOF in reference,
checked H and native fixture launches. Its default is empty. This adds no
stdin readiness or fault script; use the existing production cooperative stdin
provider for Pending/cancellation tests. Offline replay consumes the recorded
read completions and needs no original stdin bytes. The bounded-line regression
is `stdin_line_bound_35` in the driver stdlib harness.

`Pending` parks an opened file's reads until cancellation. To release a read
when a sibling successfully writes another snapshot file, use
`FileReadScript::AfterWrite { path: "output/release".into() }`. Both paths are
resolved from the snapshot root, including their mount selectors. A write in
another `test.dir` tree cannot release this read.

Run the reference and checked interpreter with `simulation.clone()`. Freeze and
validate the loaded program's native input, select `Options` and construct a
`Toolchain` as in the existing native integration tests. `differential_in` takes
the same `fixture` for checked H, LLVM and Cranelift. For explicit recording and
cross-engine replay, use the public link/launch pair after `build`:

```rust,ignore
let objects = emit_backend_objects(
    &ready, Backend::Cranelift, OptLevel::None, Partition::Whole, false, None,
)?;
let runnable = link_fixture_runnable(
    &ready, &entry, &objects, &toolchain, &dir, &fixture,
)?;
let tape = dir.join("recording");
let recorded = launch_fixture(
    &runnable, &ready.target, &toolchain, &dir, true,
    Tape::Live { record: Some(&tape), revocations: None }, &fixture,
)?;
let observed = observe(&recorded, &ready, &sites)?;
let replayed = launch_fixture(
    &runnable, &ready.target, &toolchain, &dir, true,
    Tape::Replay(&tape), &fixture,
)?;
assert_eq!(observe(&replayed, &ready, &sites)?, observed);
```

Use a separate launch directory for each linked runnable and the same recipe at
link and launch. `link_runnable` alone binds its default empty-Args harness;
`link_fixture_runnable` binds the fixture's Args and mounts. The launch creates
fresh in-memory files each time. Mutations never reach a host mount. Select
`Backend::Llvm` to emit LLVM objects; the remaining recipe is identical.
`real_io_replay_in` and `runner_oracle_in` also accept this extended fixture.

Snapshot run records use a stable synthetic host designator, which the test
launcher never admits as host authority. This keeps run bindings identical
across separate backend launch directories.

A Main fixture without runner calls records its Args, initial snapshot, run binding and
exact Blocked/Ready/cancellation events. It can be replayed by either native
backend and by `hirsel_driver::replay_observations` and
`replay_observations_checked`, without the original files or readiness scripts.
Compare semantic observations and inspect the recorded Blocked read so a fixture
that never actually parked cannot count as a successful cancellation test.

The replay helpers keep their `Result<Observable, ToolError>` API. A tooling
stop has no comparable semantic observation; its error now reports the actual
`RunOutcome`, for example `ToolingLimit(ToolingLimit { kind: "Steps", limit: 1,
... })`. Admission refusals and replay mismatches retain their own diagnostics.
Do not classify an arbitrary replay error as a Steps stop.

The executable regression is
[`native_harness_gaps_30.rs`](../crates/hirsel-pipeline-tests/tests/front/native_harness_gaps_30.rs).
It checks a Pending sibling cancellation and an AfterWrite release with real
Args, initial files, a read-only input mount and a writable output mount. Run
only these cases with:

```sh
.buck2/bin/buck2 test //:hirsel-pipeline-tests-test-front-shard-1 -- \
  --test-arg=native_harness_gaps_30 --test-arg=--nocapture
```

`Fixture.file_faults` adds ordered queues keyed by `FileOperation`. Supported
operations are `file.open`, `file.create`, `file.open_append`, `file.read`,
`file.read_bytes`, `file.write`, `file.append`, `file.write_bytes`,
`file.create_atomic`, `file.write_all`, `file.commit`, `file.sync`, and
`file.put_new`. These are snapshot provider inputs, never host errno. Close/drop
is ordinary cleanup and cannot be faulted. No readiness behavior changes.

Every byte operation (`read`, the four writes and `put_new`) requires `requested`
with the exact read maximum or supplied byte length. Other operations require
`None`. Request mismatches refuse before consuming an entry or applying effects.
A `FileFault` with `error: None` passes one ordinary operation; otherwise it
returns the selected `FileFaultError`, preserving its code or CancelCause ordinal
(0 Requested, 1 Parent, 2 ChildFailed, 3 DeadlineExceeded). Only `write`, `append`,
`write_bytes` and `write_all` accept a nonzero `partial`: exactly that prefix
is applied before the error. Reads leave the buffer and position unchanged on
error; use a pass entry before a later read error. Cancellation has zero effects.

Empty queues, missing/unexpected request counts, oversized/unsupported partials,
invalid causes and overlapping typed/legacy simulation queues are admission
errors. Queue exhaustion restores normal provider behavior. Pending reads,
real task cancellation, and calls rejected by authority/mode/path preflight do
not consume entries. Unused entries (for paths the app never visits) are allowed.
The lowered Heap snapshot provider also accepts these queues through
`HeapWorld.file_faults`. Complete scripts are validated before entry, and every
independent run and allocation-failure campaign variant starts with fresh queues.
Authority, lexical paths, file modes and read readiness precede admission;
cancellation does not consume a head. Exact count mismatches refuse the model
without popping or applying effects. Failed reads preserve the caller buffer,
partial writes use the ordinary model filesystem, failed atomic commit drops its
temporary and preserves the destination, and sync failure does not synchronize.
`HeapRun.files` and `file_handles` retain the final fixture-tree bytes and live
file grants for single runs; campaigns return `None` for those fields. These are
model observations, not physical allocation reclamation evidence.
`heap_file_faults_38` qualifies finite reference/Heap vectors; historical
Unsupported receipts from other app campaigns are not retroactive passes.

Scripts are bound in `inputs.fixture_file_faults` of the test run record. A live
launch must match the link recipe; replay needs no live files or script.

This executable full-main example is covered by `native_file_faults_33`:

```text
pub fn main(rt: Io, args: Args) -> Result[(), FileError] {
  let d = io.dir(rt, arg.path(args, 0u64)? argument_error)?
  let mut f = file.create_atomic(d, path.of("old"))?
  file.write_all(d, mut f, b"new")?
  failure.check(file.commit(d, take f) == Error(PermissionDenied()), Arithmetic())
  failure.check(file.read_bytes(d, path.of("old"))? == b"old", Arithmetic())
  Ok(())
}
@noalloc
fn argument_error(e: ArgError) -> FileError uses {} { FileFault(9u32) }
```

Supply Args `["output"]`, an RW `output` mount, initial `output/old = b"old"`,
and `file_faults = BTreeMap::from([(FileOperation::Commit,
VecDeque::from([FileFault { requested: None,
error: Some(FileFaultError::PermissionDenied), partial: 0 }]))])`.
Commit refusal consumes the AtomicFile and removes its temporary, preserving the
old target. A failed `put_new` script runs before publication; this fixture does
not model errors after a completed but non-durable publication. Such an outcome
is never evidence of rollback.

Live full-main fixtures also capture stderr. A replay that emits a valid stdout
or stderr prefix and then finds unused events exits3 and returns its exact
refusal through `Launched.record`; `observe_run` classifies it as
`Observed::Refused`, including when the observation file was created empty.
The file-fault regression checks the retained prefix and the exact event count.

## CLI simulation JSON

`hirsel run --simulation world.json program.hir` transports the same typed
readiness, faults, signal grant and mount rights into the interpreter. It remains
incompatible with `--native`. No host file or host signal supplies these worlds.
The JSON adapter uses the existing MIT `hirsel-run-schema` types and validators
(`file_reads.rs:7`, `file_faults.rs:78`, `signals.rs:62`) and the standalone
`hirsel-run-config` signal validator. Its adaptations are text/byte JSON inputs,
a closed `AfterWrite` object, and CLI diagnostics; provider effects are unchanged.

The optional top-level fields are `args`, `grants`, `directories`, `projections`,
`race`, `sim_points`, `scheduler`, `stdin`, `files`, `env`, `net`, `process`,
`signals`, `file_reads`, `file_faults`, `mounts`, `monotonic`, and `wall`.
Unknown fields are errors. Omission preserves `SimulationConfig::default()`:
roots Io/Args/Env, one empty read-write mount, no signal grant, empty readiness
and fault maps, and zero clocks. An explicit empty `mounts: {}` grants no
mount. An empty script `[]` supplies no arrivals; it differs from omitting the
signal grant. `signals: null`, `signals.script: null`, and null maps are errors.
`env` alone accepts null values for unset variables. `FileFault.requested` and
`error` retain their optional schema values (null or omission means None);
`partial` is required. Bytes are exact integers 0–255, including binary 0/255;
`files` values and `stdin` also accept UTF-8 text. No lossy byte coercion occurs.

Recognized projections are `stdin`, `stderr`, `stdout`, `tasks`, `net`,
`process`, `random`, `dir`, `signals`, and `telemetry`. Names do not create
root grants, script arrivals or mount rights. Signals projection follows the
provider's explicit SignalsGrant. Clocks are projected from explicit root
grants, independently of the projection-name list. For example, this complete
clock world grants both clocks and configures their initial values:

```json
{
  "grants": ["Io", "MonotonicClock", "WallClock"],
  "projections": ["stdout"],
  "monotonic": 7,
  "wall": -9
}
```

`projections: ["monotonic_clock"]` remains invalid and points to this grant
recipe. Merely setting `monotonic` or `wall` grants no clock access.

For delayed child fixtures, select `process.script.timed@1`, grant
`MonotonicClock`, and set the fixture's `monotonic` initial tick. Its process
object uses the ordinary rules and one-shot `runs`, with a separate child schema:

```json
{
  "provider": "process.script.timed@1",
  "clock": "run.monotonic",
  "rules": [{"program": "/fixture/child", "env": []}],
  "runs": [],
  "children": [{"program": "/fixture/child", "args": [], "steps": [
    {"at": 5000000}, {"print": [120]}, {"exit": {"exited": 0}}
  ]}]
}
```

`at` is an absolute u64 nanosecond tick in the shared granted virtual domain.
Gates must be nondecreasing; equal ticks and u64::MAX are valid. A gate behind
an unmet `expect` is ineligible. Polling leaves time unchanged; quiescence uses
the existing timer seam. Writes remain immediate. At equality, scope
cancellation precedes a process-wait timeout, which precedes child readiness.
Closing stdin before a gate defers an unmet-expect failure until that gate;
dropping the child removes its future gates without advancing time.

The timed domain refuses live host clocks and effects. Bound run records carry
`inputs.fixture_monotonic`; fixtures derive it from `monotonic`. Recording binds
the entire initial clock, grant and script, and replay requires that binding and
full tape consumption. The separate provider leaves `process.script@1`, public
`ChildStep`, and their byte encodings unchanged. Historical tapes retain the
ordinary runtime-revision compatibility check.

These fresh, small adapter reproductions reduce the existing pnmtiles pending
Interrupt race and treelearn commit-denial fixtures. Their identities are
`simulation-world-cli-39/pending-interrupt` and
`simulation-world-cli-39/commit-denied`; they do not replace original app runs,
capacity claims or RE600 receipts. From the repository root, with `hirsel` on PATH:

```sh
hirsel run --simulation crates/hirsel-cli/tests/support/simulation_world_cli_39/pending-interrupt.json crates/hirsel-cli/tests/support/simulation_world_cli_39/cancel.hir
hirsel run --simulation crates/hirsel-cli/tests/support/simulation_world_cli_39/commit-denied.json crates/hirsel-cli/tests/support/simulation_world_cli_39/commit.hir
```

The complete pending world (unused maps omitted, retaining their defaults):

```json
{
  "grants": ["Io", "Args", "Env"],
  "projections": ["stdout", "stderr", "tasks", "dir", "signals", "telemetry"],
  "mounts": {"input": "read", "output": "read_write"},
  "files": {"input/data": [0, 255]},
  "file_reads": {"input/data": "Pending"},
  "signals": {
    "provider": "signals.script@1",
    "kinds": ["interrupt"],
    "script": ["interrupt"]
  }
}
```

It prints `canceled; joined` on stdout and `pending adapter` on stderr, exits0,
and leaves only the input and `output/joined = "yes"`. `no-signal.json` changes
the script to `[]` and stays Blocked. No actual SIGINT is sent.

The complete commit-denial world:

```json
{
  "grants": ["Io", "Args", "Env"],
  "projections": ["stdout", "dir"],
  "mounts": {"output": "read_write"},
  "files": {"output/old": "old"},
  "file_faults": {
    "file.commit": [{"requested": null, "error": "PermissionDenied", "partial": 0}]
  }
}
```

It prints `commit denied; old preserved`, exits0 and preserves `output/old`.
The second commit publishes `output/published = "new"`, checking queue exhaustion;
no temporary or live file handle survives. Fault names are the exact public
`file.*` operation keys; error tags retain Rust schema case, for example
`"PermissionDenied"`, `{"Fault":77}` or `{"Canceled":0}`. Every byte operation
requires an exact `requested` count. A structurally invalid queue refuses before
entry; a count mismatch refuses at operation admission before popping its head.
A nonzero partial Canceled completion is always invalid.

For an AfterWrite gate use:

```json
{
  "mounts": {"input": "read", "output": "read_write"},
  "files": {"input/data": [0, 255]},
  "file_reads": {"input/data": {"AfterWrite": {"path": "output/release"}}}
}
```

```sh
hirsel run --simulation crates/hirsel-cli/tests/support/simulation_world_cli_39/after-write.json crates/hirsel-cli/tests/support/simulation_world_cli_39/read.hir
hirsel run --simulation crates/hirsel-cli/tests/support/simulation_world_cli_39/failed-write.json crates/hirsel-cli/tests/support/simulation_world_cli_39/read.hir
```

Only a **successful `file.write_bytes`** to `output/release` in the same tree
releases this gate, including a write before the read. Failed or partial-error
writes do not release it. `file.put_new` and other write operations do not
release it: `put-new.json` demonstrates Blocked despite publishing that path.
`failed-write.json` cancels and joins the still-pending child after denial.
`binary-readonly.json` checks that read-only denial leaves the queued fault
available to the following authorized call. The executable adapter tests compare
complete JSON/typed observations in reference and checked H, then invoke the real
CLI; their negative vectors include nested unknown fields and explicit nulls.

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=simulation_world_cli_39 --test-arg=--nocapture
```
