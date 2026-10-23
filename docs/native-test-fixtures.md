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
