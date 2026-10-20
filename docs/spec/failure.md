# Failure and destruction

Expected failures are values: `Result` and `?` ([LANG-8](language.md#lang-8-errors)).
Bugs are **failures**: deterministic, fail-stop, and caught only at an explicit isolation
boundary. Resources are destroyed at strictly defined points on every exit path, and their
finalizers are the only user code that runs while a failure unwinds.

### FAIL-1: Kinds

`Result` is for expected failures. Bugs (a failed check or contract, `Depth`, `OutOfSpace`,
using a dead mutex) are **failures**: deterministic, fail-stop, and reported at their source
position in program order. Environmental failures (heap exhaustion, guard-page overflow, a
foreign crash) end the process and are outside the determinism contract. Global quiescence
is noncompletion under [CONC-8](concurrency.md#conc-8-global-quiescence), not a failure
kind.

A host timeout or forced abandonment is tooling or environmental termination and does not
imply that language cleanup ran. Environmental termination may interrupt an external effect
after any partial or complete effect and before its outcome is observed or recorded. Hirsel
promises neither rollback nor exactly-once execution across such termination; the absence
of a recorded completion is not evidence that the effect did not occur.

An ordinary function marked `@track_caller` attributes failures raised in its body or
contracts to its application call. Direct calls through other marked functions forward
that caller transitively. An unmarked function starts attribution at its own operations.
A call through a function value starts a new boundary at that invocation, including when
the target is statically known or specialized. Argument evaluation precedes entry and
retains the evaluating body's attribution. A marked entry with no language caller uses
the true operation site. Callback and task entry use their invocation site as the boundary.

The caller is an implicit constant source identity resolved at each call: forwarding it
requires no allocation, stack inspection or authority. The original failing operation is
retained separately. Human reports show it as a secondary `in` line when it differs from
the primary attribution; JSON reports retain the same pair. Repropagation and cleanup do
not replace an already attributed failure or its causes.

### FAIL-2: Boundary

`task.isolate(tasks, body) -> Result[R, Failure]` is the only place a failure is caught,
besides `arena.bounded` for its own `OutOfSpace` and a runner call (planned), whose callee
runs as a separate run: a failure that ends the callee run reaches the caller as the
outcome `Faulted(failure)` ([RUN-6](runners.md#run-6-outcomes)) and never unwinds the
caller. A failure in a store transaction's body discards its writes and propagates
([STORE-2](storage.md#store-2-transactions)). Elsewhere a failure propagates: a failed
child cancels its siblings, the scope joins them, and the failure continues outward; with
several, the first in spawn order is reported. `Failure` is `data`: its kind, rule ID,
source site, the path of spawn indices and an optional boxed cause
([core library](core-library.md#concurrency)). The site follows the failing operation
and `@track_caller` attribution above, rather than the isolation boundary.

### FAIL-3: What runs

A failure unwinds: frames are released as at scope exit, counted values are released,
regions and arenas opened inside are freed, and resource drops are the only user code that
runs. A failure inside a drop abandons that drop and still releases its fields; if the drop
belongs to a type declared in a package marked `unsafe ffi`, the process ends instead.

The primary failure stays primary; failures during later cleanup extend its ordered cause
chain. Cleanup continues across siblings after a failure in one of them. Shielding
suppresses cancellation checks, not deterministic bug failures. The unwinding mechanism is
the implementation's choice ([native code](implementation.md#failure-and-cleanup)).

### FAIL-4: Mutexes

A failure inside a `mutex.with` body kills that mutex: its payload is released unseen, and
every later `with` on it is a failure in the acquiring task whose cause is the original
failure. There is no recoverable poisoning: the payload of a dead mutex is never reachable
again.

### DROP-1: Values

Copyable values have no user-visible destruction; their memory is freed when no longer
held.

### DROP-2: Resources

A `resource type` declares `drop(x) { .. }`. Lifetimes are strict: a resource is dropped
when its owner replaces it, or at the end of the scope that owns it, in reverse declaration
order (fields in reverse order, sequences from high index to low), on every exit path:
normal exit, `?`, `return`, cancellation and failure. Taking a resource moves it; it does
not drop it.

A value whose destruction releases a `Shared` root, directly or through any component,
holds that root until its cleanup point (its scope's cleanup or its consumption, on normal
and failure paths). No implementation releases such a value earlier, so the holders of
every root at every synchronising operation are the same in every engine.

```hirsel
resource type Temp { dir: Dir, path: Path }
  drop(t) { let _ = file.remove(t.dir, t.path) }

fn work(d: Dir, p: Path) -> Result[(), FileError] {
  let t = Temp(dir: d, path: p)
  file.write_bytes(d, p, b"x")?      // on error, t is dropped here
  Ok(())                             // and here on success
}
```

A committed whole destructure in the defining module consumes the resource without running
its finalizer; bound fields become owners, and omitted fields are dropped immediately in
declaration order, including on failure paths ([LANG-13](language.md#lang-13-patterns)).

A drop declaration may be prefixed `@noalloc`, including on the following line; without it
the resource signature permits allocating destruction. The annotation promises both the
user body and the implicit cleanup of fields. It is checked, never inferred from the body.

### DROP-3: Drop rules

A drop is infallible, never a cancellation point and shielded; it cannot resurrect the
value and may use only the capabilities stored in the resource. Close errors are discarded.
Infallible means a unit result and no `?` propagation or nonunit return; deterministic bug
failures remain possible and follow [FAIL-3](#fail-3-what-runs). The finalizer cannot move,
copy or return the whole resource being finalized. Its allocation contract follows
[DROP-2](#drop-2-resources), independently of infallibility.

With or without a user finalizer, a resource's authority includes the implicit destruction
of its fields and is computed only from declared field and type summaries
([CAP-2](capabilities.md#cap-2-charged-at-use)). The finalizer has no further ambient
authority and none inferred from its body. A sealed resource's destruction is checked under
its stored implementation authority ceiling, while its externally charged authority remains
its capability term.

### DROP-4: Commit

Fallible finalisation is an explicit consuming call (`file.commit(dir, take f) -> Result`,
`close`, `flush`). Implicit drop is the abort path, so an early `?` needs no `errdefer`.

`forget` is allowed only in the type's defining module. Its intrinsic signature is
`fn forget[T](take x: T) -> ()`: it applies only to resource types, only in their defining
module, and suppresses both their finalizer and the destruction of their owned fields. It is
`@noalloc`. Cross-module legality is checked from definition paths, not from dotted source
declarations. Forgetting a sealed resource neither exposes its representation nor erases
its scoped-lifetime obligations.
