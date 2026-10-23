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
