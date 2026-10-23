# First-contact friction

Minutes are approximate elapsed investigation time; overlapping waits are not a benchmark.
Copied diagnostics normalize checkout paths and strip trailing display whitespace.
The captured panic is historical; the resource trigger remains environmental.

| Tried | Historical diagnostic | Current disposition / repair | Minutes lost | Class |
| --- | --- | --- | ---: | --- |
| Compiled natively while the shared host could not create another worker thread. | [Exact panic](repros/thread-limit.txt): `job worker creation failed: Os { code: 11, kind: WouldBlock, message: "Resource temporarily unavailable" }` | `HIRSEL_JOBS=1` avoids the observed worker pressure. Compiler worker creation now falls back to the started workers (or the caller when none started), with one operational warning naming `HIRSEL_JOBS=1`; successful compilation keeps exit 0. [Control program](repros/thread-creation.hir); the required trigger is `pthread_create` failing, so the default-worker crash is environment-dependent. Covered by injected first-worker and partial-start failures in `hirsel-intern` and CLI tests. | 0.5 | bug (fixed; environmental trigger) |

[Worker-start recovery shipped in ca1a6f2e](https://github.com/SamGalanakis/hirsel-lang/commit/ca1a6f2e2fba236b944a0cb1addda8a487c21f4d).
The [CLI injection control](../../crates/hirsel-cli/tests/cli/compiler_workers.rs)
fails first-worker and partial-start creation without exhausting host threads;
it asserts one operational warning, preserved output and source-error status.
[Pool tests](../../crates/hirsel-intern/src/jobs/tests.rs) also cover queue draining
and joining. The linked minimal program alone does not reproduce EAGAIN.
`HIRSEL_JOBS=1` remains an operational choice under thread pressure; no fresh
host-exhaustion experiment is claimed here.

The language contracts remain explicit: empty lists need element context;
`unique pending` snapshots buffered bytes before reset; `unique row` copies a
borrowed counted loop item into owned output. `take matches` transfers the list.
A consuming loop transfers owned elements, but its declared element contract
controls their deep uniqueness: `List[String]` does not supply `unique String`.
A producer using `List[unique String]` can transfer strong rows from a currently
unshared source; the annotation alone cannot restore evidence after copying.
Logtail retains its borrowed-row copy; [wc2](../wc2/FRICTION.md#whole-value-handback-and-row-copies)
shows both contracts. Mutable
buffer/offset parameters return their state through handback, while `?` and
`Ok(...)` handle Result completion. These are intentional ownership and prefix
inference recipes, not worker-start workarounds.
