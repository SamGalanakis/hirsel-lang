# localrpc authoring friction

The first full attempt was checked on frozen main
`f36e8f4b04d616e24f63e1b74fa4ba97bd46f466`. Its source, manifest and exported JSON
diagnostics were retained outside the repository before repair. No compiler, std,
provider, scheduler or language rule was changed. Times below are approximate
active investigation time. This enumerates this program's authoring scenarios;
it makes no claim about untested ergonomics.

## Shallow copies are still local (about 2 minutes; intentional MODE-1)

Attempted header conversion:

```hirsel
pub type Decoder { data: List[u8] }
fn header(mut state: Decoder) -> Bytes {
  bytes.from_list(copy state.data)
}
```

Actual first-attempt diagnostic: MODE-1, `model.hir:27:22–54`, byte span
1259–1291, “`state` is the caller's storage, but `bytes.from_list` requires a global
argument”. The note explicitly says that `copy state` shares blocks and stays local.
The repair uses `bytes.from_list(unique state.data)`. Only four header bytes are
copied. Extracting a completed payload uses `replace(mut state.data, [])`, which
moves the old owned builder out and resets it without a forbidden partial field
move. The replacement recipe checked on its first attempt.

The same first check rejected `string.from_bytes(copy payload)` in a borrowed
helper: MODE-1, `model.hir:67:18–49`, bytes 2637–2668, “`payload` is local to the
caller, but `string.from_bytes` requires a global argument”. Repair:
`string.from_bytes(unique payload)`. This copies at most 2048 bytes after the cap
check. It keeps `request`/`response` borrowing interfaces consistent and avoids
propagating a taken-buffer requirement through callers. No claim that shallow
`copy` can detach a borrow is made.

Related ownership recipes left in the final code: `field` uses `unique text` to
return an independently owned string from `json_access.member/as_text` shallow
views; `answer` copies borrowed ID/method strings with `unique id`/`unique method`.
These were applied with the locality repair and passed the next check; no separate
failed form was run for them. `copy text`/`copy method` in response validation share
owned local `expected` fields after validation. Socket resources use `take
connected` into mutable storage and branded `mut stream` helper parameters.

## Module function names cannot be parameter names (about 1 minute; intentional LANG-4)

Attempted forms:

```hirsel
pub type Request = Ping(String)
fn request() -> Request { Ping("a") }
fn answer(request: Request) -> Request { request }
```

And analogously `encode(response: Response)` next to `fn response(...)`.
The actual first check reports LANG-4 at the parameter names:
`model.hir:94:15–22`, bytes 3855–3862, and `102:15–23`, bytes 4116–4124,
“a local cannot shadow a top-level value in the same module”. Rename them to
`incoming` and `reply`, updating their uses. The check also reported dependent
unresolved match constructors (`Ping`, `Normalize`, `Unknown`, `Pong`,
`Normalized`, `UnknownMethod`); those disappeared with the renames. No independent
resolver/diagnostic defect was demonstrated or patched.

## Explicit types, error plumbing and bounded helpers (about 2 minutes; documented recipes)

The first attempt explicitly annotated empty `List[u8]`/`List[Bytes]` builders,
`candidate: Response`, and `length: u64` following the prefix law. These annotations
remain; an annotation-free alternative was not attempted. `int.widen` and
`int.narrow` consume the shipped fixed-width binary APIs; narrowing is preceded by
the 2048-byte length cap.

`AppError` wraps nominal input, grant, network and output errors for plain `?`
injection. Validation turns expected UTF-8/JSON/shape failures into
`model.InputError` with `let … else`; main renders stable error codes and returns
`process.ExitCode`. These are explicit policies, with no discarded native error
cast or checker workaround. `receive` and `configure` carry the same originating
`Net` before `Stream of n`. No brand erasure or fresh narrowed Net is needed.

A deterministic listening record was added after successful bind so tests and
users can wait for readiness without guessing a startup delay. Initial accept
has no timeout API. The documented external deadline and each test's hard wall
condition bound that wait; connected streams additionally use existing Net
inactivity timeouts. This avoids adding tasks/clock authority to a sequential
helper. It is an application launch policy, not evidence of a std defect.

## Verification disposition

The original first check had 13 diagnostics; the repaired source check had zero.
The first focused run executed both selected tests: two passed, zero failed or
ignored. It exercised checked H model vectors and actual CLI interpreter, LLVM
and Cranelift execution; a source check alone was not counted as V or native
acceptance. No failed engine cell was replaced by a smaller passing input.
No shared repair or owner-routed issue was needed in these scenarios.

The final focused run again executed two tests with zero failures/ignored tests.
Its independent-peer harness reported 243 runtime fixture executions (81 per
engine), in addition to checked H model execution, source check, reachable TIR
export, and four native build setups (application and raw-vector entry on each
backend). Added controls cover missing stdout/Net capabilities, escaped Unicode
IDs, empty replies, and a valid reply followed by invalid input. The call helper
now accepts the already validated request so it does not parse local input twice;
failed input-report output returns status 3. Both are application refinements
that passed this final run, without a new language workaround.

The first landing attempt stopped before build checks because a blank line added
to `crates/hirsel-cli/tests/cli/modules.rs` violated the independent-line registry
format: line 52, “expected one independent mod declaration”. Removing that blank
line leaves the same module registered. This was a tooling-format repair (under
one minute), with no application or test semantic change.
