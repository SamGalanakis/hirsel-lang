# Ownership diagnostics

To inspect a resource's lifetime, request an ownership trace:

```sh
hirsel run app.hir --trace-ownership /tmp/ownership.json
hirsel test app.hir --trace-ownership /tmp/test-ownership
```

A run prints the lineage to stderr and writes JSON to the requested new file.
Tests print each selected test's verdict and lineage, and write one JSON file
per test in the requested directory. Filters and `--seed` work as usual.
Existing sidecars are never replaced. An incomplete or unsupported trace is a
tool failure (exit 3), even if the program itself succeeds.

Each event identifies its task, operation, source site, action, token and owner.
Token identities distinguish equal values and survive storage reuse. A final
summary lists resources still held and repeats each one's complete move chain.
For example, this program succeeds but retains its resource through `forget`:

```hirsel
resource type Token {}
drop(x) {}

pub fn main() -> Result[(), ()] {
  let first = Token()
  let held = take first
  forget(take held)
  Ok(())
}
```

Its trace has one creation, moves of that same token, no drop, and one live
resource held by `Forgotten` storage. The trace does not change the program's
exit status for a retained resource. JSON includes the program binding,
completeness flag and the same ordered events as the readable output.

Native programs can select either backend:

```sh
hirsel run app.hir --native --backend llvm --trace-ownership /tmp/llvm.json
hirsel run app.hir --native --backend cranelift --trace-ownership /tmp/clif.json
```

Native coverage includes statically nested resources, their finalizers and
failure cleanup, owned direct-call parameters and results, checked once captures,
named arena callbacks, and parallel SSA moves including swaps. Swaps capture all
sources before replacing any destination, so equal payloads retain their distinct
identities. Resources created and dropped inside a child task are supported
along with captured Resources transferred at the actual committed spawn. The
compiler binds the callback and capture paths from the complete checked code
graph. Resource, tuple and Option task results keep their original credits
through the actual child-to-parent result copy. A detached result stays with its child
until its cleanup drops it. Failed children create no parent result credit.
Unknown callbacks and unsupported dynamic result storage refuse before launch.
The runtime sidecar binds each created task's generation and canonical path,
so interleaved frames and reused scheduler slots remain distinct. Channel
payloads with statically nested Resources preserve FIFO item identity through
sends, receives and queued cleanup. Unsupported dynamic storage still refuses
before launch.
Native launches accept the validated grants, arguments and recording or replay
selected for the ordinary program. The diagnostic runtime must be available;
the default runtime archive contains no ownership hooks.

```sh
hirsel run app.hir --native --grants grants.json --record /tmp/app.tape --trace-ownership /tmp/live.json
hirsel run app.hir --native --replay /tmp/app.tape --trace-ownership /tmp/replay.json
```

Replay uses the recorded admission and leaves the tape unchanged. The ownership
sidecar remains separate from the tape. Native tracing cannot combine with
`--cores`, `--proofs` or `--trace-events`.
Interpreted test tracing runs one default schedule; native tests, benchmarks,
blessing, JSON test reports and custom schedules are not supported by this flag.
`run --simulation` also refuses the flag.

The current adapters track resources. General unique and owned-value coverage,
and an independent RPC admission witness are still in development. The compiler
checks each traced pass against the original marker inventory. Native and
interpreted storage moves may differ; logical resource creations and drops can
be compared across engines. Ownership JSON is
a diagnostic sidecar, separate from the capability recording used for replay.
