# First-contact friction

Minutes are approximate elapsed investigation time; overlapping waits are not a benchmark.
Copied diagnostics normalize checkout paths and strip trailing display whitespace.
The table preserves original output; current diagnostics and repairs follow below.

| Tried | Historical diagnostic | Current disposition / repair | Minutes lost | Class |
| --- | --- | --- | ---: | --- |
| Used closure parameter `n` under the Net local `n`. | [Exact output](repros/incident-1.txt) | Renamed the closure parameter `served`; LANG-4 forbids lexical shadowing even in nested functions. [Minimal program](repros/incident-1.hir). | 0.5 | language friction |
| Expected `io.dir` errors to fit the Grant arm. | [Exact output](repros/incident-2.txt) | Added `Files(FileError)`; `io.dir` returns FileError rather than GrantError. LANG-8; the diagnostic correctly lists injectable arms. [Minimal program](repros/incident-2.hir). | 0.5 | language friction |
| Returned AppError (with store.Corrupt(String)) from an asynchronous task. | [Exact output](repros/incident-3.txt) | Explicitly return `unique Result[(), AppError]` at the task boundary and use the same TaskSet type; nested counted errors prevent sendability. MODE-4. [Minimal program](repros/incident-3.hir). [Nested cause and checked expression repair shipped in 7874edcd](https://github.com/SamGalanakis/hirsel-lang/commit/7874edcd97e00508b3ff84d9abe7d65e7edb9d42); see the current recipe below. | 1 | diagnostic quality (fixed; duplicate reports remain) |
| Translated NetClient.from directly into a grants JSON field. | [Exact output](repros/incident-4.txt) | JSON clients use separate `address` and `port`; added the JSON schema example to building.md. | 0.5 | doc gap |
| Provided listeners without an empty peers array. | [Exact output](repros/incident-5.txt) | Add `"peers":[]` for `net.script@2`; documented that it is mandatory. | 0.5 | doc gap |
| Closed the scripted client before the signal could be delivered at idle. | [Exact output](repros/incident-6.txt) | End the client script with Stall rather than Close; the final recording serves four commands then SIGTERM shuts down successfully. Documented exhaustion and idle delivery. | 0.5 | doc gap |
| Replayed after canonical source formatting. | [Exact output](repros/format-replay.txt): `hirsel: replay refused: {"kind":"binding","field":"program"}` | Fixed by [semantic formatting identity, c8092dec](https://github.com/SamGalanakis/hirsel-lang/commit/c8092dece9dabb92dc28c40a24866d0c10a9d3ee). The unchanged [reproducer script](repros/format-replay.sh) now prints `ok` twice and exits 0. [Before](repros/format-before.txt) / [after](repros/format-after.hir) remain historical; see current replay guarantees below. | 1 | bug (fixed) |

## Current task result recipe

Task sendability examines every possible result arm, even when a particular call
returns `Ok(())`. The [minimal incident](repros/incident-3.hir) uses `E.Bad(String)`;
current MODE-4 names `Result.Error[0].E.Bad(String)` and the callback's sendable
requirement. The application's longer cause runs through `AppError.Store` and
`StoreError.Corrupt(String)`. That cause and the legal repair are shipped, not
outstanding requests. The minimal incident still emits two MODE-4 reports of the
same cause; diagnostic duplication remains, not a second ownership failure.

The [applied-repair tests](../../crates/hirsel-driver/tests/it/error_injection.rs)
insert `unique ` before `work()` and source-check the result. Merely strengthening
a callback's return annotation does not make an ordinary counted result unique.
This complete explicit repair supplies both the contract and the expression:

```hirsel
type E = Bad(String)

fn work() -> Result[(), E] {
  Ok(())
}

pub fn main(t: Test) {
  let tasks = test.tasks(t)
  task.scope(
    tasks,
    fn(s: Scope) {
      let h = task.async(
        s,
        fn() -> unique Result[(), E] {
          unique work()
        }
      )
      let _ = task.await(s, take h)
    }
  )
}
```

`unique work()` may allocate and costs O(size); it supplies independent counted
storage. The application retains `unique serve(...)`,
`TaskSet[unique Result[(), AppError]]` and matching callbacks; its signal callback
constructs `Ok(())` under that unique result context. Keep these ownership proofs;
a type annotation alone is insufficient. Error injection remains
explicit: `io.dir` needs `Files(FileError)`, and `?` unwraps success rather than
wrapping a returned Result. Closure parameters still obey LANG-4 no-shadowing.
Store lookups and parsed text lend views; the application's `unique` byte/string
copies supply owners for transaction storage and UTF-8 conversion.

## Current replay and script status

[Formatting replay controls](../../crates/hirsel-cli/tests/cli/replay_format.rs)
cover the original source across interpretation, LLVM and Cranelift: formatting,
comments, CRLF and declaration relocation preserve binding; semantic, dependency,
manifest and run-configuration changes still refuse before entry. No re-record
is needed solely because of canonical formatting. DET-1 retains current source
locations for diagnostics separately from semantic identity.

The script repairs remain: clients use separate `address` and `port`, listeners
need `"peers":[]` for `net.script@2`, and a final `Stall` permits idle signal delivery
where `Close` ends the connection first. See [the complete grant](grants.json) and
[scripted network documentation](../../docs/building.md). These are provider
encoding and event-order requirements, not unresolved compiler incidents.
