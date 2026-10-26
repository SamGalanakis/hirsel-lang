# Event-window ownership and qualification friction

The app uses shipped APIs only. It changes no std protocol, ownership rule,
scheduler, compiler construct or runtime layout.

The generic state is `Windows[A]`, with CountSum and MinMax as two real main-path
instantiations. `accumulate(take state: unique Windows[A], ..., step:
fn(mut A,i64)->Result[(),Problem] uses {}) -> Result[unique Windows[A],Problem]`
borrows the selected mutable aggregate through a scoped callback and returns an
explicit owned receipt. `A: plain + inert` is instantiated here with integer aggregate records. It does not claim arbitrary resource-bearing state support.
Keys are owned once in State; window cells use bounded integer registry indexes.
The durable format serializes key text, never those internal indexes.

Canonical presentation needs independent ordering without consuming live state.
The minimal copies are `window.ordered(unique totals, keys)` (at most 32 live or
128 emitted plain cells) and `let mut sorted = unique state.keys` (at most 16
strings/512 UTF-8 bytes plus list storage). CountSum cells have five 64-bit
fields, so live/history presentation copies carry at most 1280/5120 scalar payload
bytes (6400 combined).
`input.key` copies at most 32 bytes from its borrowed DOM text per decoded
event/registry/window row; `unique key` at first registry admission copies at
most another 32 bytes because Record does not promise a unique root. Checkpoint
DOM projections use bounded immutable counted copies (`Ok(copy fields)` and
`Ok(copy items)`); they retain the bounded DOM during adoption. No full batch
copy is required. Final staging is one at-most-65536-byte output. JSON quoting
uses one at-most-194-byte temporary per key. Scoped closure transport retains
the implementation's existing heap/counted-environment costs; this app makes
no allocation-elimination claim.

Keeping two aggregate instantiations duplicates the three scalar cell identity
fields: 24 extra payload bytes per retained window, at most 3840 across the
160 retained windows, plus the separate list roots.

Insertion ordering scans at most 160 cells overall; a live generic transition
scans at most 32 cells. Presentation joins the two aggregate instantiations by
bounded key/start scans. This avoids storing callbacks or a second mutable
string-key map and keeps topology independently checkable. It is a bounded
application choice, not a std API proposal or a measured optimization.

Signal cancellation requires two scoped children only when Signals is granted:
preparation and signal wait. `task.select` cancels and joins the loser. There is no
worker pool and no width claim. Publication starts only after full staging.

Frozen qualification limits: each full-main cell has 5,000,000 steps, 60 CPU
seconds/2 GiB per native process, and a 600-second action limit. The separately
named output/arithmetic control uses the same limits. Exact/one-over checkpoint
byte tests pad an otherwise legal snapshot with JSON whitespace. Output byte
admission is also checked directly because fixed semantic caps can make an
arbitrary output length unreachable. These controls supplement, and never
replace, the original full-main cells. Zero-start, Steps, timeout, unsupported
adapter and failed cells must be retained as incomplete in external receipts.

Qualification completed in separately named Buck actions. The final scanner
generation covers 138 full-main fixtures with 276 LLVM/Cranelift recordings
and 1104 replays (each recording on reference, checked H, LLVM and Cranelift).
Every native partition ran one nonzero Rust test body under the unchanged
600-second action limit; the invalid partition took 586.61 seconds. Ordinary
success and duplicate-error recordings additionally check binding, request,
missing and extra-event corruption on all four adapters. Original providers
are absent during replay; raw tape bytes and complete consumption are checked.
Pending+Interrupt tapes contain an uncommitted parked read before Interrupt.

Direct reference/checked-H fixtures and the four-engine arithmetic/output
control also passed. Failed input-byte and fault-schema cells were recovered
at their original limits; affected scanner and Beam cases have distinct new
receipts. The outer source/evaluated-build/config/tool captures for unions
13 through 17 each match before and after their readers; actual worker inputs
also have paired captures. Earlier partial inventories remain partial.
External evidence retains all attempts, exact inputs, observations, tapes and
compiler witnesses. The two original source/V mismatches below remain OPEN
with Sam / @agentlang-24; the app workarounds do not resolve those originals.

## Retained authoring and proof attempts

`result` and `before` are reserved (LANG-14). A parameter named `key` shadowed
`input.key`, and rebinding `scanner` shadowed the earlier pattern (LANG-4).
These are local renames, with no runtime cost. `None()` as the first match arm
needed `let watermark: Option[i64]` (LANG-2), also with no runtime cost.

`take owned.totals` while retaining `owned` refused OWN-8. A qualified record
pattern `let model.Windows(...)` refused LANG-5; unqualified `Windows` outside
its declaring module refused LANG-4. The final `model.parts` and
`model.window_parts` destructure whole owners in their declaring module and
return explicit unique roots. No counted payload is copied by these receipts.
A nested tuple receipt through `Result` lost the list uniqueness needed for
checkpoint adoption (MODE-2). `Result[unique Decoded,Problem]` plus whole-owner
destructuring retains that receipt, with no payload copy. The private-helper case is fixed by private summaries (b552ff23d839 / f71ba1242e00); the current exported decode contract stays declared. Explicit receiving
annotations on the constructed Windows roots fix their prefix contract.

The original `step(mut owned.live[i].aggregate,value)?` passes source checking
but strict V refuses `TypeMismatch(value modes)` (original full app Body0,
Block13, instruction11). The permanent [original_receipt.hir](../../crates/hirsel-cli/tests/support/event_window_39/original_receipt.hir) reduces it to a
concrete u64 list, with no generic, loop or Result inside the helper. Its source
check passes, while reachable TIR export refuses advance Body0/Block1/instruction8.
This original receipt is INCOMPLETE, owned by the compiler/V maintainer (Sam); it is not
made green by the workaround. Minimal source:

```hirsel
fn advance(take xs: unique List[u64], change: fn(mut u64) -> () uses {}) -> unique List[u64] uses {} {
  let mut owned = take xs
  change(mut owned[0])
  take owned
}
```

The shipped engine stages `let mut current = owned.live[i].aggregate`, calls
`step(mut current,value)?`, then assigns it back. Each reducer copies exactly
16 scalar bytes per hit; both reducers together copy 32 bytes into locals, plus
32 bytes on successful installation. No list or string copy is added. The
corresponding scalar-local control passes source and reachable V export.

A second source/V mismatch occurs for consuming `for item in take xs` under
`T: plain` and `uses {}`: V refuses `TypeMismatch(body uses)` on loop exits.
`T: plain + inert`, `uses {xs}`, and indexed traversal each export successfully.
The engine declares the intended `plain + inert` aggregate contract explicitly;
the original weaker source/V identity is separately retained with the same
owner. No compiler or V rule was changed.

```hirsel
fn keep[T: plain](take xs: unique List[T]) -> unique List[T] uses {} {
  let mut out: List[T] = []
  for item in take xs { list.push(mut out, item) }
  take out
}
```

String-valued fixed errors caused MODE-1 at related DOM projections and MODE-4
at the signal task result. A plain Reason enum transports the same fixed codes
without copying error strings. Output helpers initially lost uniqueness at
`bytes.from_list(take out)` (MODE-2); fixed by private summaries (b552ff23d839 / f71ba1242e00) for quoted/rows, with no final staging copy. Exported put retains its checked promise. Signal racing now ends
before publication, so cancellation cannot report a rejected batch after that
race has committed an output.

The original image-free source check hit its 60-second limit (EXIT124) during
std preparation. The first named semantic test had zero bodies due to setup
ENOSPC (EXIT32); its missing-only host retry also had zero bodies because the
launcher required exact4 affinity (EXIT32). Those are separate from the V
failures and from executed test cells. Owned evidence was losslessly relocated
to home storage, with a byte-verified original-path symlink; no shared cache,
other lane files, CPU reservation or affinity check was changed. Subsequent
qualification uses ordinary focus pool routing.


The first paired actual-tool helper treated optional `/usr/bin/systemd-run`
as mandatory. The worker lacks it; all four selected Rust bodies stopped in
setup with zero app-cell credit. The complete outer before/after source and
evaluated-build union matched. The helper now records availability/absence as
probes; configured tool roots and compiler executables remain required. The
normal capped path and its original limits are unchanged.

Output admission is tested directly at 65536/65537 bytes. A loose upper bound
for a semantically valid canonical document is 59680 bytes: 160 maximal rows,
16 maximally escaped keys, and 400 bytes for framing/scalars. Thus a valid
full-main document cannot reach the defensive output cap under the other
unchanged caps; the direct writer control is the applicable exact/one-over
witness. Input checkpoint byte caps still execute full main at 65536/65537.


Original input-byte vectors at 32768/32769 bytes exhausted the frozen 5M-step
budget on both reference and checked H at the per-byte `model.add` call. Their
exact sources, inputs and stopped observations are retained. The driver now
uses `bytes.find_any(data,b"\n",start)` (Go ScanLines' IndexByte structure) to
find the identical newline-terminated segment boundaries. It still checks raw
byte admission before feeding each segment, keeps 256-byte lookahead and does
not copy a whole batch. This removes per-byte Result handbacks; caps and original
fixture bytes remain unchanged. No compiler/app-steps repair or benchmark was
introduced.

The first byte-fault fixtures omitted `requested`; launch refused them with
`expected request count exactly for byte operations`, before an app observation.
The existing typed fault schema requires 4096 for the first checkpoint read
and the independent oracle's canonical empty-document byte length for the
partial write. Original malformed scripts remain setup failures, separate from
app error handling; only those failed cells are recovered.


The first 58 semantic and 106 invalid cells and first native ordinary/tape cells
retain their original scanner/source generations. Later scanner qualification
has distinct paired source/build/tool unions and covers CRLF across refills,
UTF-8 split inside a multibyte scalar, LF at a refill edge, blank lines, empty
EOF, final lines without LF, no EOF flush and input/line/record precedence.
Earlier one-sided source inventories are disclosed as partial and are never
upgraded retroactively by rerunning cells for bookkeeping. Setup and stopped
receipts remain distinct from completed app observations.
